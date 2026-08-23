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

/// Face name for Archivo Regular (the design's 400 weight). Unlike the three
/// below it is not a *named family*: it is the front of egui's own
/// [`FontFamily::Proportional`] stack, which is what plain text resolves to.
pub const REGULAR: &str = "Archivo-Regular";

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

/// The four Cyrillic faces, paired with the Archivo weight each stands
/// behind: `(Archivo family name, Noto face name, Noto bytes)`.
///
/// **All four bundled Archivo faces carry ZERO codepoints in U+0400–04FF.**
/// Without these, every Cyrillic string in the app — item names, usernames,
/// folder names, notes — fell through to egui's bundled proportional
/// fallback, which is one typeface at one weight: a Cyrillic name rendered
/// identically whether the design asked for 400, 600, 700 or 800, and
/// visibly lighter than the Latin sitting beside it.
///
/// These are Noto Sans' *Cyrillic subset* (Fontsource, SIL OFL 1.1; see
/// assets/fonts/OFL-NotoSans.txt, kept as its own file rather than folded
/// into Archivo's OFL.txt). The subset carries 100 Cyrillic codepoints and
/// **no Latin at all**, and that absence is the point: a face with no
/// `A`–`z` in its `cmap` cannot win a Latin lookup wherever it sits in a
/// family stack, so adding these cannot move one existing Latin
/// measurement. 64 KB for all four. Their `usWeightClass` values
/// (400/600/700/800) land exactly on the four Archivo cuts.
const CYRILLIC_FACES: [(&str, &str, &[u8]); 4] = [
    (
        REGULAR,
        "NotoSans-Cyrillic-Regular",
        include_bytes!("../assets/fonts/NotoSans-Cyrillic-Regular.ttf"),
    ),
    (
        SEMIBOLD,
        "NotoSans-Cyrillic-SemiBold",
        include_bytes!("../assets/fonts/NotoSans-Cyrillic-SemiBold.ttf"),
    ),
    (
        BOLD,
        "NotoSans-Cyrillic-Bold",
        include_bytes!("../assets/fonts/NotoSans-Cyrillic-Bold.ttf"),
    ),
    (
        EXTRABOLD,
        "NotoSans-Cyrillic-ExtraBold",
        include_bytes!("../assets/fonts/NotoSans-Cyrillic-ExtraBold.ttf"),
    ),
];

/// The four bundled Archivo cuts, as `(egui family name, GDI family name, GDI
/// weight, bytes)`.
///
/// **One `include_bytes!` per face for the whole crate.** [`font_definitions`]
/// registers these with egui by the first field; `unlock_prompt` registers the
/// same bytes with GDI through `AddFontMemResourceEx` and then asks for them
/// by the second and third. A Win32 surface that shipped its own
/// `include_bytes!` would put a second copy of every face in the binary and,
/// worse, let the two renderers drift onto different files.
///
/// **The GDI names are not the egui ones, and are read out of the files
/// rather than guessed.** GDI matches on the legacy `name` records (IDs 1 and
/// 2), which can hold only four styles per family, so Archivo's static cuts
/// spell themselves this way: Regular and Bold share the family `Archivo` and
/// are told apart by weight, while SemiBold and ExtraBold each carry their own
/// legacy family and are `Regular` *within* it. Asking GDI for
/// `("Archivo", 600)` therefore returns synthesised-looking Regular, not
/// SemiBold, which is exactly the kind of near-miss that made the last raw
/// Win32 surface in this project read as foreign.
pub const ARCHIVO_FACES: [(&str, &str, i32, &[u8]); 4] = [
    (REGULAR, "Archivo", 400, include_bytes!("../assets/fonts/Archivo-Regular.ttf")),
    (SEMIBOLD, "Archivo SemiBold", 400, include_bytes!("../assets/fonts/Archivo-SemiBold.ttf")),
    (BOLD, "Archivo", 700, include_bytes!("../assets/fonts/Archivo-Bold.ttf")),
    (EXTRABOLD, "Archivo ExtraBold", 400, include_bytes!("../assets/fonts/Archivo-ExtraBold.ttf")),
];

/// The `(GDI family, GDI weight)` a caller outside egui asks for to get the
/// cut `family` names, from [`ARCHIVO_FACES`].
///
/// Falls back to Regular's pair rather than panicking: a prompt set in the
/// wrong weight is a cosmetic defect, and this is the one surface in the app
/// whose whole reason for existing is that it must open when the heavier
/// machinery cannot.
pub fn gdi_face_for(family: &str) -> (&'static str, i32) {
    ARCHIVO_FACES
        .iter()
        .find(|(egui_family, ..)| *egui_family == family)
        .map(|(_, gdi, weight, _)| (*gdi, *weight))
        .unwrap_or(("Archivo", 400))
}

/// The Noto Cyrillic face standing behind `archivo`, from [`CYRILLIC_FACES`].
fn cyrillic_for(archivo: &str) -> &'static str {
    CYRILLIC_FACES
        .iter()
        .find(|(a, _, _)| *a == archivo)
        .map(|(_, noto, _)| *noto)
        .expect("every Archivo weight is paired with a Cyrillic face in CYRILLIC_FACES")
}

/// The bundled Archivo faces (the design's typeface, OFL-licensed; see
/// assets/fonts/OFL.txt), layered over egui's defaults.
///
/// egui has no weight axis — `RichText::strong()` only tints — so each
/// weight is registered as its own named family, with egui's default
/// proportional stack kept behind it for glyphs Archivo lacks (arrows,
/// emoji, CJK).
///
/// Cyrillic used to be on that "glyphs Archivo lacks" list, and reaching
/// egui's fallback for it meant losing the weight (see [`CYRILLIC_FACES`]).
/// Each weight's Noto face therefore goes in at **position 1: behind its own
/// Archivo cut, ahead of egui's defaults**. Behind Archivo so Latin keeps
/// resolving exactly as it does today; ahead of the defaults because the
/// defaults are precisely what Cyrillic was reaching, so a face appended
/// after them would never be consulted and nothing would change.
fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    for (family, _, _, bytes) in ARCHIVO_FACES {
        fonts
            .font_data
            .insert(family.to_owned(), Arc::new(egui::FontData::from_static(bytes)));
    }

    for (_, noto, bytes) in CYRILLIC_FACES {
        fonts
            .font_data
            .insert(noto.to_owned(), Arc::new(egui::FontData::from_static(bytes)));
    }

    let default_stack = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();

    if let Some(proportional) = fonts.families.get_mut(&FontFamily::Proportional) {
        proportional.insert(0, REGULAR.to_owned());
        proportional.insert(1, cyrillic_for(REGULAR).to_owned());
    }
    for weight in [SEMIBOLD, BOLD, EXTRABOLD] {
        let mut stack = vec![weight.to_owned(), cyrillic_for(weight).to_owned()];
        stack.extend(default_stack.iter().cloned());
        fonts
            .families
            .insert(FontFamily::Name(weight.into()), stack);
    }

    // Monospace gets NO Cyrillic face, deliberately. Consolas covers
    // U+0400-04FF itself, and so does the Hack egui bundles behind it, so
    // the family already renders Cyrillic from a real monospaced face at
    // both ends of the `system_monospace` branch below -- asserted in
    // `the_monospace_family_carries_cyrillic_without_a_noto_face`. Putting
    // the proportional Noto subset in front of them would be strictly
    // worse: it would take those codepoints away from a monospaced face and
    // hand them to one whose advances differ per glyph, breaking the one
    // property the monospace family exists for. Monospace is also a single
    // weight here; there is no lost-weight problem to fix.
    //
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
/// **Public because the mark is painted by two renderers, not one.**
/// `unlock_prompt` draws the same shield through GDI `Polygon` with no egui
/// anywhere in the process, and it reads the fills and the outlines from here
/// rather than restating them. Two copies of a brand mark that must agree is
/// the same defect shape this crate's palette constants exist to prevent, and
/// a cross-renderer copy is the version of it nobody would notice drifting.
pub const QUADRANT_FILLS: [Color32; 4] = [BLUE_DEEP, BLUE_BRIGHT, BLUE_SOFT, BLUE];

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
/// Public for [`QUADRANT_FILLS`]'s reason: `unlock_prompt` scales these same
/// points into GDI device space and fills them with `Polygon`, so the Win32
/// surface draws the design's shield rather than an approximation of it.
pub fn quadrant_outlines() -> &'static [Vec<Pos2>; 4] {
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

/// Lays `text` out into a single line no wider than `room`, ellipsised if it
/// does not fit.
///
/// This exists because `Painter::text` takes no width at all: it lays a
/// string out at its natural width and draws it wherever that reaches, which
/// is *outside* the tile it was meant for as soon as the string is longer
/// than the tile is wide. Any surface that paints a name it did not choose --
/// a vault item's name is the user's text, not ours -- has to lay it into a
/// galley against a measured width instead, and this is that step, spelled
/// once.
///
/// Two things it is careful about, which are the whole reason it is a
/// function and not four lines copied twice:
///
/// * **`room` is clamped to `1.0`, never `0.0`.** egui reads a zero wrap
///   width as "do not wrap", i.e. exactly the unbounded behaviour the caller
///   is trying to withdraw -- so a pane dragged narrower than its own padding
///   would spring back to overflowing.
/// * **`TextWrapMode::Truncate`, not `Wrap`.** These callers paint into
///   fixed-height tiles; a wrapped second row would be drawn over the row
///   below rather than growing anything.
///
/// `style` is only the fallback face -- a `RichText` carrying its own size,
/// family and colour (which is what every caller passes) overrides it.
///
/// Note that egui truncates at the END of a laid-out run. A caller that
/// paints a name *and* a trailing suffix must therefore lay them out
/// separately and take the suffix's width off `room` first, or the suffix is
/// what disappears; see `item_list::paint_title_with_suffix`.
pub fn truncated_galley(
    ui: &Ui,
    text: impl Into<egui::WidgetText>,
    room: f32,
    style: TextStyle,
) -> Arc<egui::Galley> {
    text.into().into_galley(ui, Some(egui::TextWrapMode::Truncate), room.max(1.0), style)
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
/// Where a laid-out run's INK is centred, measured down from the top of its
/// own box.
///
/// The vertical companion to [`ink_offset_x`], read off the same `uv_rect`.
/// A galley's box is ascent plus descent and a reader sees neither: two runs
/// whose boxes are centred on one another still print visibly apart when
/// their faces or their sizes differ, which is what "not on the same mid
/// line" looks like. `None` for a run with no glyphs, where there is no ink
/// to centre and the caller should leave the box alone.
pub fn ink_center_y(galley: &egui::Galley) -> Option<f32> {
    ink_band_y(galley).map(|(lo, hi)| (lo + hi) / 2.0)
}

/// The top and the bottom of a laid-out run's INK, measured down from the top
/// of its own box.
///
/// [`ink_center_y`] is the midpoint of this band and is expressed in terms of
/// it, so the two cannot come to disagree about where a run's ink is.
///
/// The BAND rather than only its centre is what a caller needs when it has to
/// match a run's optical *size* and not just its position:
/// [`crate::card_mark`] scales a brand logo so that the logo's ink stands
/// exactly as tall as the wordmark it replaced, which is what makes a row of
/// mixed marks -- some logos, some words, some logos on their own coloured
/// ground -- read as one set rather than as whatever files the user happened
/// to download.
///
/// `None` for a run with no glyphs, where there is no ink to measure.
pub fn ink_band_y(galley: &egui::Galley) -> Option<(f32, f32)> {
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for row in galley.rows.iter() {
        for glyph in row.glyphs.iter() {
            let top = row.pos.y + glyph.pos.y + glyph.uv_rect.offset.y;
            lo = lo.min(top);
            hi = hi.max(top + glyph.uv_rect.size.y);
        }
    }
    lo.is_finite().then_some((lo, hi))
}

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

/// Paints `texture` to FILL an [`avatar_tile`], clipped to the tile's own
/// corner radius, and re-draws the tile's border over it.
///
/// **Full bleed, and it took three passes to get here.** The history, because
/// the next person to look at this row will be holding one of these reports
/// and not the other two:
///
/// 1. The favicon was drawn at the full 32pt with `fit_to_exact_size`, and the
///    report was "the favicon fills its tile edge-to-edge and feels too big".
/// 2. So it was inset 4pt a side -- a 24pt image in a 32pt tile, set against
///    the MONOGRAM beside it, whose letters ([`avatar`], `size * 0.38`) cover
///    about a third of their tile. The report on THAT was "icon is not fully
///    taking the rounded rectangle".
/// 3. The design is the tiebreaker, and it says the image takes the tile. The
///    spec has no favicon rule of its own to read off -- it gives the tile
///    ("rows = 32px monogram") and the radius (the avatar radius is ~25% of
///    the tile size), which is what [`avatar_corner_radius`] already computes
///    -- so a favicon is that same 32pt tile with artwork in it, not a smaller
///    box floating inside one.
///
/// **The rounding is the whole risk, and it is why this is a function and not
/// a `0.0` constant.** The tile is a rounded rectangle. An image painted flush
/// to its bounds with square corners would poke four corners out past the
/// rounding, which is a worse result than any inset. The corner radius is
/// therefore given to the image itself: egui tessellates a textured
/// `RectShape` with a rounded, antialiased outline exactly as it does an
/// untextured one, so the artwork is clipped to the tile's curve rather than
/// merely placed inside its box.
///
/// **The fill and the border still earn their place, in that order.**
/// [`avatar_tile`] paints both first: the fill is what a favicon with
/// transparent margins (which is most of them) shows through, and it carries
/// the selected treatment's `BLUE_WASH`. The BORDER is then re-drawn ON TOP of
/// the artwork, because `StrokeKind::Middle` straddles the edge -- an image
/// painted over it would eat its inner half and leave a half-pixel ghost.
/// Drawn over, the tile keeps one visible edge against a pale favicon, which is
/// the same edge every monogram beside it has.
///
/// NOTE FOR WHOEVER CHANGES EITHER SIDE OF THIS: `favicon::decode_rgba`
/// resamples every icon to a 64px longest edge, a number chosen for a 32pt
/// draw at 200% scaling. Nothing in the code links that constant to this one.
/// It covers a full-bleed 32pt draw exactly, so a tile that ever grows past
/// 32pt needs `decode_rgba`'s constant raised with it.
pub fn avatar_image(ui: &Ui, tile: Rect, texture: &egui::TextureHandle, emphasized: bool) {
    let rounding = avatar_corner_radius(tile.width());
    egui::Image::new((texture.id(), texture.size_vec2()))
        .corner_radius(rounding)
        .paint_at(ui, tile);
    ui.painter()
        .rect_stroke(tile, rounding, avatar_tile_stroke(emphasized), StrokeKind::Middle);
}

/// The avatar tile's 1px border, in its two states.
///
/// One function rather than a colour picked at each of the two places that
/// draw it ([`avatar_tile`], which draws it under the content, and
/// [`avatar_image`], which draws it again over a full-bleed favicon), so the
/// selected treatment cannot end up blue in one and grey in the other.
pub fn avatar_tile_stroke(emphasized: bool) -> Stroke {
    Stroke::new(1.0, if emphasized { BLUE_EDGE } else { HAIRLINE })
}

/// The avatar tile's BOX -- allocated, filled, bordered and rounded -- with
/// nothing drawn in it, returning the rect so the caller can place its own
/// content inside.
///
/// Split out of [`avatar`] so a favicon can be drawn into the very same box
/// the monogram fallback draws, rather than replacing the box entirely:
/// the favicon and the monogram are the same tile, at the same size, with the
/// same edge, and only their contents differ. [`avatar_image`] is the favicon
/// half, and it paints over what this leaves.
pub fn avatar_tile(ui: &mut Ui, size: f32, emphasized: bool) -> Rect {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let bg = if emphasized { BLUE_WASH } else { CANVAS };
    let rounding = avatar_corner_radius(size);
    ui.painter().rect_filled(rect, rounding, bg);
    ui.painter()
        .rect_stroke(rect, rounding, avatar_tile_stroke(emphasized), StrokeKind::Middle);
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
pub const CHIP_HEIGHT: f32 = 18.0;

/// The chip's text size, its horizontal padding and its corner radius, as
/// [`kbd_chip`] paints them.
///
/// Public, and named rather than left as literals at [`kbd_chip`]'s call to
/// [`paint_chip`], because the GDI renderer draws the same chip and cannot
/// call into egui: `crate::win32_draw::draw_hint_chip` reads these four
/// numbers so the picker card's shortcut hints are the design's chip rather
/// than a second, nearly-identical one.
pub const CHIP_TEXT_PX: f32 = 10.0;
pub const CHIP_PAD_X: f32 = 6.0;
pub const CHIP_RADIUS: f32 = 4.0;

/// The GDI family name of the face [`system_monospace`] reads.
///
/// The same file (`%SystemRoot%\Fonts\consola.ttf`) by the name GDI knows it
/// under, for callers that ask the OS for a font rather than handing egui
/// bytes. It is here rather than in `crate::win32_draw` for the reason
/// [`TEXT_CLIP_INSET`] is: the GDI renderer takes every face, colour and
/// dimension from this module.
pub const GDI_MONO_FACE: &str = "Consolas";

/// Paints one keyboard-hint chip: `text` in 10px monospace, centered in a
/// rounded box of exactly [`CHIP_HEIGHT`] with `pad_x` either side.
fn paint_chip(ui: &mut Ui, text: &str, bg: Color32, fg: Color32, radius: u8, pad_x: f32) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::new(CHIP_TEXT_PX, FontFamily::Monospace), fg);
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
    paint_chip(ui, text, bg, fg, CHIP_RADIUS as u8, CHIP_PAD_X);
}

/// The Windows Hello panel's CTRL+H chip (design 3h: `font-size: 10px;
/// color: #1b3fa0; background: #ffffff; border-radius: 5px; padding: 3px
/// 7px`) — a white chip on the panel's blue wash, which is neither of
/// [`kbd_chip`]'s two treatments.
pub fn kbd_chip_on_card(ui: &mut Ui, text: &str) {
    paint_chip(ui, text, CARD, BLUE, 5, 7.0);
}

// ---------------------------------------------------------------------------
// The indeterminate progress indicator (design turn 7).
// ---------------------------------------------------------------------------

/// **The bar every waiting surface in this app draws, and the disc none of
/// them draw any more.**
///
/// Design turn 7 -- "TURN 7 · FIRST WINDOW", the last section of
/// `docs/design/Deskwarden.dc.html` -- draws the indicator for BOTH of its
/// waiting bodies (7a's load and 7b's slow) as a short bar sliding inside a
/// track:
///
/// ```text
/// track: height 3px, radius 2px, background #eae7e7, overflow hidden
/// knob:  width 32%, height 3px, radius 2px, background #1b3fa0,
///        animation: dw-bar 1.4s ease-in-out infinite
/// @keyframes dw-bar { 0% { translateX(-100%) } 100% { translateX(320%) } }
/// ```
///
/// The rotating disc (`egui::Spinner`, the file's `dw-spin` keyframe) is what
/// this app drew before, and it survives in that design only as a keyframe
/// turn 7 never references. The owner's report was that the loading and
/// locking screens "ha[ve the] old design with round spinner - wrong".
///
/// **Here rather than in `loading_ui`** because more than one module has a
/// wait to draw -- `loading_ui`'s three bodies, `login_ui`'s in-flight
/// sign-in, the vault's own first load -- and two hand-drawn copies of a
/// four-constant animation are two chances for the app's idea of "waiting" to
/// fork. `theme` is where this crate already keeps the widgets more than one
/// window draws.
///
/// `width` is the TRACK width: the design uses 260px in its full-frame body
/// and 200px in its half-width card, so the measure belongs to the surface and
/// the proportions belong here.
///
/// Repaints itself, exactly as `egui::Spinner` did, so a host that otherwise
/// only wakes on a channel poll still animates: an indicator that moves only
/// when something else happens is a still picture of a bar.
pub fn progress_bar(ui: &mut Ui, width: f32) {
    let (track, _) = ui.allocate_exact_size(Vec2::new(width, BAR_HEIGHT), Sense::hover());
    let phase = bar_phase(ui.input(|i| i.time));
    paint_progress_bar(ui.painter(), track, phase);
    ui.ctx().request_repaint();
}

/// [`progress_bar`]'s painting, at an explicit phase.
///
/// Split out so the preview example can render a still frame at a phase where
/// the knob is actually inside the track -- at phase 0 the design's own
/// keyframe has it entirely off the left edge, so a PNG taken there shows an
/// empty track and tells a reviewer nothing.
pub fn paint_progress_bar(painter: &egui::Painter, track: Rect, phase: f32) {
    let radius = CornerRadius::same(BAR_RADIUS);
    painter.rect_filled(track, radius, HAIRLINE);
    // `overflow: hidden` on the track. Without it the knob is drawn outside
    // the track for most of the cycle, which reads as a stray blue dash
    // crossing the window rather than as something moving inside a rail.
    painter.with_clip_rect(track).rect_filled(bar_knob(track, phase), radius, BLUE);
}

/// The track's height and the knob's -- design 7's own 3px.
///
/// Public because the bodies that draw the bar have to CENTRE it, and a
/// surface that restated "3" to do its own arithmetic would be a copy of this
/// number sitting in another file waiting to disagree with it.
pub const BAR_HEIGHT: f32 = 3.0;

/// The track's and knob's corner radius -- design 7's own 2px.
const BAR_RADIUS: u8 = 2;

/// The knob's share of the track -- design 7's own `width: 32%`.
const BAR_KNOB_FRACTION: f32 = 0.32;

/// One full cycle -- design 7's own `1.4s`.
pub const BAR_PERIOD: f32 = 1.4;

/// Where the knob starts, as a multiple of its OWN width: design 7's
/// `translateX(-100%)`, i.e. one knob-width left of the track's left edge, so
/// the cycle opens with the knob entirely out of sight.
const BAR_FROM: f32 = -1.0;

/// Where it ends: design 7's `translateX(320%)`. Together with [`BAR_FROM`]
/// that is 4.2 knob-widths of travel per cycle, which at a 32% knob is 1.344
/// track-widths -- the knob leaves the right edge completely before it
/// reappears at the left.
const BAR_TO: f32 = 3.2;

/// **How far through one cycle the animation is, eased.**
///
/// Pure and separate from the painting so the timing is something a test can
/// assert rather than an expression inside a paint call. `time` is
/// `egui::InputState::time`, seconds since the context started.
///
/// The easing is `ease-in-out`, which the design states and CSS defines as
/// `cubic-bezier(.42, 0, .58, 1)`. This is the sine form of the same shape --
/// slow at both ends, fastest in the middle, exactly symmetric -- rather than
/// a Bezier solver, because what the easing has to get right is that the knob
/// hesitates at the edges and hurries through the middle, and the two curves
/// differ by a couple of percent of the travel anywhere.
pub fn bar_phase(time: f64) -> f32 {
    let t = (time.rem_euclid(BAR_PERIOD as f64) / BAR_PERIOD as f64) as f32;
    0.5 - 0.5 * (std::f32::consts::PI * t).cos()
}

/// **Where the knob is at `phase`**, as a rect in the track's own space.
///
/// Pure, so "the knob starts off the left edge and ends off the right one" is
/// arithmetic a test runs rather than a claim about a CSS keyframe nothing in
/// this process reads.
pub fn bar_knob(track: Rect, phase: f32) -> Rect {
    let knob = track.width() * BAR_KNOB_FRACTION;
    let offset = (BAR_FROM + (BAR_TO - BAR_FROM) * phase) * knob;
    Rect::from_min_size(
        Pos2::new(track.left() + offset, track.top()),
        Vec2::new(knob, track.height()),
    )
}

/// Height of the design's action buttons (3h Continue, 2b/3f toolbar).
/// Named because things placed *beside* a button — the login window's
/// in-flight indicator — have to match it, and a second hardcoded `32.0`
/// could drift away from this one unnoticed.
pub const BUTTON_HEIGHT: f32 = 32.0;

/// The filled primary action button, optionally with a trailing keyboard
/// hint, per the design's "Save ↵" / "Fill in app CTRL+⇧+F" buttons.
///
/// A `kbd` of `"↵"` is painted as a vector return-arrow rather than typed:
/// neither Archivo nor egui's fallback fonts carry U+21B5, so as text it
/// renders as a tofu box.
pub fn primary_button(ui: &mut Ui, label: &str, kbd: Option<&str>) -> Response {
    primary_button_with_metrics(ui, label, kbd, BUTTON_HEIGHT, 7, true)
}

/// [`primary_button`], but able to say "not yet".
///
/// [`primary_button`] is `ui.add`, which has no way to express an unavailable
/// action, and that gap is why the item form's Save was a bare
/// `egui::Button` for so long: it needed `add_enabled`, so it skipped the
/// design system entirely and picked up egui's default fill and egui's
/// default font instead. The user's report was that the two footer buttons
/// looked like they came from different families -- they did, and this is the
/// missing half that lets Save come from this one.
///
/// `enabled == false` runs the button inside a disabled `Ui`, so egui fades
/// the whole control -- the explicit [`BLUE`] fill included -- toward the
/// window colour. That fade is the only signal that the action is off, which
/// is why `detail_edit`'s
/// `the_disabled_save_button_does_not_look_enabled` asserts on the painted
/// fill and not on anything structural.
pub fn primary_button_enabled(
    ui: &mut Ui,
    label: &str,
    kbd: Option<&str>,
    enabled: bool,
) -> Response {
    primary_button_with_metrics(ui, label, kbd, BUTTON_HEIGHT, 7, enabled)
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
    primary_button_with_metrics(ui, label, None, SEARCH_FIELD_HEIGHT, 8, true)
}

fn primary_button_with_metrics(
    ui: &mut Ui,
    label: &str,
    kbd: Option<&str>,
    height: f32,
    radius: u8,
    enabled: bool,
) -> Response {
    let paint_return = kbd == Some("↵");
    let text = match kbd {
        // Trailing spaces reserve room for the painted arrow and the gap
        // before it.
        Some("↵") => format!("{label}      "),
        Some(k) => format!("{label}  {k}"),
        None => label.to_string(),
    };
    // `add_enabled_ui` and not `add_enabled`, so the arrow below is drawn by
    // the SAME faded painter as the button it sits inside. `add_enabled`
    // returns to the parent `Ui` before this function paints the glyph, which
    // would leave a full-opacity ↵ on a greyed-out button.
    ui.add_enabled_ui(enabled, |ui| {
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
    })
    .inner
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

// The FILLED 34px header button (design 2b's "Fill in app") is gone with the
// control it drew. Commit `7da1bba` removed that button from the detail pane
// at the user's request -- two adjacent controls acted on one application,
// one launching it and one typing credentials into it, with nothing in the
// strip saying which was which -- and took `DetailAction::Fill`, the
// `CTRL+SHIFT+F` chord and the pane's own test for the pill with it.
//
// `header_primary_button` and `header_primary_button_width` were left `pub`
// and unused by that commit, together with their private galley/width
// helpers and the `HEADER_PRIMARY_*` numbers. Deleted here rather than kept:
// a lib crate raises no dead-code warning for a `pub` item, so they would
// have sat here indefinitely as a doc comment describing a control that no
// longer exists -- the same reason the outlined 34px button above was
// deleted rather than parked. `HEADER_BUTTON_HEIGHT` stays: `star_toggle`
// and `kebab_button` still lay themselves out with it.

/// The small outlined control at the right-hand end of a detail-pane row
/// (design 2b's "Copy" / "Reveal" / "Open": `height: 28px; padding: 0 10px;
/// border: 1px solid #d7d3d3; border-radius: 7px; font-size: 12px`).
///
/// Regular weight, not [`semibold`]: the design gives these no `font-weight`,
/// unlike the 600 it sets explicitly on the header pair.
pub fn row_button(ui: &mut Ui, label: &str) -> Response {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = ROW_BUTTON_PADDING;
        ui.add(
            egui::Button::new(RichText::new(label).size(ROW_BUTTON_TEXT_SIZE).color(INK))
                .fill(CARD)
                .stroke(Stroke::new(1.0, BORDER_STRONG))
                .corner_radius(CornerRadius::same(7))
                .min_size(Vec2::new(0.0, 28.0)),
        )
    })
    .inner
}

/// [`row_button`]'s own `padding: 0 10px` and `font-size: 12px`, named rather
/// than written twice, so [`row_button_width`] measures the button that will
/// really be drawn instead of a second copy of its numbers.
const ROW_BUTTON_PADDING: Vec2 = Vec2::new(10.0, 4.0);
const ROW_BUTTON_TEXT_SIZE: f32 = 12.0;

/// How wide [`row_button`] will be for `label`, without drawing it.
///
/// For a caller that has to choose a LAYOUT before it draws -- the detail
/// pane's MATCHED APP footer, which puts its controls beside the notes when
/// they fit on that line and on a line of their own when they do not. Laying
/// the same galley the button will lay is the whole point: a caller that
/// estimated would reserve room the button then overflows -- the drift the
/// header strip's own deleted `header_primary_button_width` existed to
/// prevent, before the button it measured was removed.
pub fn row_button_width(ui: &Ui, label: &str) -> f32 {
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::new(ROW_BUTTON_TEXT_SIZE, FontFamily::Proportional),
        INK,
    );
    galley.size().x + ROW_BUTTON_PADDING.x * 2.0
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
    link_widget(ui, egui::Label::new(RichText::new(text).size(size).color(BLUE)))
}

/// The same link, from text the caller has **already laid out**.
///
/// A `Galley` rather than a `&str` because the one other link on this app's
/// detail pane -- the matched app's name -- has to wrap inside a fixed column
/// on a 298pt pane, and `Label` given anything but a finished galley re-lays
/// it with the surrounding layout's own wrap width (`f32::INFINITY` in a
/// horizontal row), which is how an unwrapped run inflated that very card to
/// 467.8pt once already. The caller lays the job, this paints it as a link.
///
/// **One link widget, two ways to feed it**: the blue, the pointing hand and
/// the un-selectable click sense are [`link_widget`]'s and are not written out
/// a second time, so the two links on this pane cannot start behaving
/// differently. The colour is the caller's here -- a galley carries its own --
/// and [`BLUE`] is what the caller must lay it in.
pub fn link_galley(ui: &mut Ui, galley: std::sync::Arc<egui::Galley>) -> Response {
    link_widget(ui, egui::Label::new(galley))
}

fn link_widget(ui: &mut Ui, label: egui::Label) -> Response {
    let response = ui.add(label.selectable(false).sense(Sense::click()));
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
pub fn close_glyph(ui: &mut Ui) -> Response {
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

/// Corners in the star's skeleton: five points and five valleys.
///
/// The skeleton is not the outline. Every one of these ten corners is
/// replaced by a rounding arc in [`star_outline`], so the path that reaches
/// the screen has [`STAR_VERTICES`] points and not ten.
const STAR_CORNERS: usize = 10;

/// Samples along each corner's rounding arc, endpoints included, so a corner
/// contributes `STAR_ROUND_SEGMENTS + 1` points.
///
/// Three. The arc it draws is about 1.5px long at the shipped size, and a
/// quadratic Bézier across 1.5px is already smooth at four samples; more of
/// them buy nothing a person can see and cost vertices in every frame the
/// detail pane is drawn in.
const STAR_ROUND_SEGMENTS: usize = 3;

/// Vertices in the star's outline: one rounding arc per [`STAR_CORNERS`].
///
/// **It was 10** -- one point per corner, back when the corners were sharp.
/// The number itself is this file's business; what is not is that it stays
/// distinct from every other closed path this crate strokes, because
/// [`icon_probe`] tells the marks apart by point count and nothing else.
/// [`no_two_drawn_icons_share_a_vertex_count`] is the live guard, which is
/// why the arithmetic is written out here rather than the answer.
pub const STAR_VERTICES: usize = STAR_CORNERS * (STAR_ROUND_SEGMENTS + 1);

/// Samples along the eye's upper lid.
const EYE_LID_SEGMENTS: usize = 12;

/// Vertices in the eye's almond outline: the upper lid sampled
/// `EYE_LID_SEGMENTS + 1` times, and the lower lid the `EYE_LID_SEGMENTS - 1`
/// times strictly between the two corners it shares with it.
pub const EYE_VERTICES: usize = EYE_LID_SEGMENTS * 2;

/// The kebab's dot radius. Also what tells its three circles apart from the
/// eye's pupil, which is deliberately a different size.
pub const KEBAB_DOT_RADIUS: f32 = 1.7;

/// Vertices in the envelope's body: a plain rectangle, stroked as a closed
/// path rather than emitted as an `egui::Shape::Rect` **so that
/// [`icon_probe`] can find it the way it finds every other mark in this
/// family** -- by point count. A `Rect` shape would be indistinguishable
/// from the strip's own white fill and every card behind it.
pub const ENVELOPE_VERTICES: usize = 4;

/// Vertices in the envelope's flap: the two top corners and the point the
/// fold meets in the middle.
///
/// Three, which is also the point count `icon_probe::stars` walks for a
/// star's fill triangles -- and it does not collide, because that probe
/// additionally requires a non-transparent `fill` and this path is stroked
/// with no fill at all. [`the_envelope_flap_is_not_findable_as_a_star_fill`]
/// is the live guard on that, not this comment.
pub const ENVELOPE_FLAP_VERTICES: usize = 3;

/// Vertices in the folder mark's single closed outline: the tab's two top
/// corners, the shoulder where the tab steps down onto the body, and the
/// body's three remaining corners.
///
/// **Six, and nothing else in this crate strokes a six-point closed path** --
/// the star is [`STAR_VERTICES`], the eye [`EYE_VERTICES`], the envelope's
/// two paths [`ENVELOPE_VERTICES`] and [`ENVELOPE_FLAP_VERTICES`], and no
/// module outside this file emits an `egui::Shape::Path` at all. That is what
/// lets [`icon_probe::folder_marks`] find this mark by point count the way
/// every other probe in that module finds its own, and `detail.rs`'s
/// `the_folder_mark_is_the_only_six_point_path_in_the_header` is the live
/// guard on it rather than this sentence.
pub const FOLDER_VERTICES: usize = 6;

/// Half the folder mark's width: the outline spans 9px, sized against a 12px
/// subtitle rather than against the 34px header controls -- this is the only
/// mark in this file that sits INSIDE a run of text.
const FOLDER_HALF_WIDTH: f32 = 4.5;

/// Half the folder mark's height, giving a 9x7 outline. Wider than tall, the
/// proportions of a manila folder; a square would read as a plain box.
const FOLDER_HALF_HEIGHT: f32 = 3.5;

/// How much of the mark's top edge the tab claims. 4.0 of 9.0 is a little
/// under half, which is what keeps the tab reading as a tab rather than as a
/// lid over the whole width.
const FOLDER_TAB_WIDTH: f32 = 4.0;

/// How far the body's top edge sits below the tab's -- 2.0 of the 7px height,
/// so the body still has 5px of its own to be a body.
const FOLDER_TAB_RISE: f32 = 2.0;

/// The horizontal run of the shoulder: the short slant from the tab's
/// trailing corner down onto the body's top edge. A vertical step here (0.0)
/// reads as a bite taken out of the corner at this size.
const FOLDER_TAB_SLANT: f32 = 1.2;

/// Lighter than [`ICON_STROKE`], deliberately rather than by oversight: the
/// drawn-icon family's 1.3 is measured for marks 17-18px across, and at 9px
/// the same weight closes the tab's notch up. This mark introduces a run of
/// [`TEXT_FAINT`] secondary text and must not be louder than the words it
/// introduces.
const FOLDER_STROKE: f32 = 1.0;

/// The box [`folder_mark`] allocates for itself. 14 tall is the 12pt
/// subtitle's own line height, so the mark centres on the text it sits in
/// instead of making the line taller; 15 wide is the 9px outline plus 3px of
/// air on each side, which is the mark's ONLY separation from the folder
/// name -- the caller sets `item_spacing.x` to zero so that the separator run
/// and the mark do not drift apart, and at 2px the mark and the `W` of `Work`
/// touched in the rendered header.
pub const FOLDER_MARK_SIZE: Vec2 = Vec2::new(15.0, 14.0);

/// How far below its box's centre the outline is drawn.
///
/// The box is the subtitle's whole line, descender space included, so a mark
/// centred in it sits visibly high against a run of mostly-x-height letters --
/// the same optical correction `account_switcher_button` makes for its chevron
/// and for the same reason. One pixel puts the body's underside on the text's
/// baseline.
const FOLDER_MARK_DROP: f32 = 1.0;

/// Half the envelope's width, so the body spans 18px inside the 34px header
/// button -- the same optical extent as the star's 18px and the kebab's 15.
const ENVELOPE_HALF_WIDTH: f32 = 9.0;

/// Half the envelope's height. Not half its width: a square envelope reads
/// as a picture frame. 6.5 puts the body at 18x13, close to a real DL
/// envelope's proportions.
const ENVELOPE_HALF_HEIGHT: f32 = 6.5;

/// Half the diagonal extent of the detail pane's close ✕, so its two arms
/// span 14.1x14.1 and the mark's ink covers 15.4x15.4 once
/// [`ICON_STROKE`] is on it.
///
/// **It was 5.5 -- an 11x11 cross, 12.3x12.3 of ink -- and that was the
/// strip's odd one out.** Reported as "also those icons are not same size
/// feels like", and measured rather than left at that: the other four marks
/// cover 15.4 to 19.3 on their long side, and this one covered 12.3. All five
/// allocate the same 34pt square, so nothing about the boxes or the hit
/// targets was wrong and no rect assertion could ever have seen it; the
/// difference was entirely in what got drawn inside them.
///
/// 7.05 is not a chosen number. It is what puts this mark's ink at exactly
/// the kebab's 15.4 -- the nearest neighbour in the set and the smallest mark
/// that was NOT reported -- so the target is read off the strip rather than
/// invented. It deliberately does not go all the way to the clock's 18.3: a ✕
/// reaches its corners diagonally, so a cross whose box matches a circle's
/// diameter reads as the bigger mark, and every icon set draws diagonal marks
/// inside the nominal for that reason. [`every_header_mark_is_drawn_at_the_
/// same_optical_size`] is the live band check.
///
/// **Deliberately none of the other three ✕ sizes in this app.** The vault
/// titlebar strokes a 9x9 close and [`close_glyph`] a 7x7 one, and
/// [`icon_probe::pane_close_marks`] tells them apart by extent alone -- so a
/// ✕ that matched either would report the titlebar's window-close as the
/// detail pane's, in every frame the whole window is painted in. Growing this
/// one moves it further from both, not nearer.
/// [`the_drawn_close_marks_do_not_share_an_extent`] is the live guard.
const PANE_CLOSE_ARM: f32 = 7.05;

/// The eye's pupil radius -- not [`KEBAB_DOT_RADIUS`], see there, and not
/// [`CLOCK_RADIUS`] for the same reason.
///
/// **It was 2.4, against an almond 10.0 tall**: the pupil filled 48% of the
/// eye's height and left barely a pixel of white above and below it, which
/// is the cramped middle the old mark had. The almond is 12.8 tall now
/// ([`eye_toggle`]'s `HALF_H`), and a pupil left at 2.4 would have rattled
/// around inside it -- so this grows with it, to 2.9, which holds the same
/// share of a taller eye and keeps a clear 2.85pt of white between the
/// pupil's edge and the lid's inner face.
const EYE_PUPIL_RADIUS: f32 = 2.9;

/// The clock face of [`add_totp_button`], as a radius.
///
/// **Deliberately none of the other ring radii in this app.**
/// [`icon_probe::kebab_dots`] matches [`KEBAB_DOT_RADIUS`] and the eye's
/// pupil is [`EYE_PUPIL_RADIUS`]; both of those are walked out of the same
/// shape tree this mark is painted into, and a ring that shared a radius
/// with either would be counted as one of that family in every frame the
/// detail header is drawn in. [`the_drawn_circles_do_not_share_a_radius`] is
/// the live guard, and it is the reason this is 8.5 rather than a rounder
/// number that happened to collide.
///
/// The Preferences mark used to be a fourth radius in this list. It is a
/// mixer now and its handles are filled blocks, so it contributes no circle
/// at all -- see that test for why the shorter list is not a weaker one.
///
/// 8.5 also puts the face at 17px across, inside the 34px header button and
/// at the same optical extent as the star's 18 and the envelope's 18.
const CLOCK_RADIUS: f32 = 8.5;

/// The clock's two hands, as lengths from its centre. The short one points
/// straight up and the long one to the right -- three o'clock, the reading
/// that gives the two hands the largest angle a clock face can show, so
/// neither is hidden under the other at 17px.
///
/// **Axis-aligned deliberately**, and not merely for legibility:
/// [`icon_probe::pane_close_marks`] finds a ✕ by walking every
/// [`egui::Shape::LineSegment`] whose bounding box is square at
/// `PANE_CLOSE_ARM * 2`, and this mark is painted into the same strip as that
/// ✕. A hand on a diagonal has a square bounding box of its own and would be
/// reported as half a close mark -- which that probe treats as a probe that
/// has stopped matching, and panics over. A vertical hand's box is zero wide
/// and a horizontal one's is zero tall, so neither can ever be square.
const CLOCK_HOUR_HAND: f32 = 4.5;
const CLOCK_MINUTE_HAND: f32 = 6.0;

/// Channel faders in the Preferences mark. **Three, and the mark is
/// vertical.**
///
/// It was two horizontal slider rows, on a density argument: the icons it
/// sits with are one- and two-mark shapes, and a third row would have made
/// this the busiest thing in the strip again. That argument was sound and it
/// has been overruled by the person it was made for, who asked for "vertical
/// with just cross bars like a DJ mixer" and, asked how many, answered
/// three.
///
/// Three is also what the metaphor needs once the mark turns vertical. Two
/// faders at different heights read as a comparison; three is the fewest
/// that reads as a *bank* of them, which is the whole of what a mixing desk
/// looks like. The density cost is paid back by the handles: a fader block
/// is a solid rectangle rather than a stroked ring, so three tracks and
/// three blocks are still only two kinds of mark, which is what
/// [`the_tune_icon_repeats_no_more_marks_than_the_kebab_beside_it`] actually
/// bounds.
pub const TUNE_FADERS: usize = 3;

/// Half the length of each fader's track, so each spans 16.7px inside the
/// 28px button and the mark stands 18.0px tall with its stroke -- the extent
/// the drawn-icon family shares.
const TUNE_TRACK_HALF_HEIGHT: f32 = 8.35;

/// Horizontal distance between adjacent tracks.
///
/// It has to clear the fader block's own width or the blocks of neighbouring
/// channels touch and the bank reads as one bar. A first pass at 6.4 against
/// a 4.4-wide block was rendered and looked at: the blocks cleared each other
/// by 2.0 and still read as cramped, because what the eye judges is the white
/// between them against the ink of them and 2.0 against 4.4 is not enough of
/// it. 7.0 against a 4.0-wide block leaves 3.0 of white -- more gap than
/// block -- and puts the whole mark 18.0px across, the extent the family
/// shares.
const TUNE_TRACK_PITCH: f32 = 7.0;

/// A fader block: the cap that rides the track, as a full width and height.
///
/// **Blocks, not bars crossing the track.** The report said "cross bars",
/// and a line crossing the track is the literal reading -- but the owner's
/// reference image settled it as a filled cap, and at this size that is also
/// the reading that survives: a crossing stroke at [`ICON_STROKE`] is one
/// pixel of ink laid across another pixel of ink, which at 18pt merges into
/// a thickened section of line rather than reading as a separate handle.
///
/// Taller than it is wide, at 1.65:1, because that is what a fader cap is
/// and because a block as wide as it is tall reads as a knot in the track.
/// 4.0 wide leaves 1.35 of block proud of the 1.3 track on each side, which
/// is what makes it a cap rather than a bulge.
const TUNE_FADER_WIDTH: f32 = 4.0;
const TUNE_FADER_HEIGHT: f32 = 6.6;

/// The corner radius on a fader block.
///
/// 1.2 on a 4.4 x 6.6 block: enough to take the hardness off the corners at
/// 18pt, not so much that the cap turns into a lozenge. Below about 0.8 it
/// stops being visible at this size at all, which would make it a number
/// that costs a reader's attention and buys nothing.
const TUNE_FADER_ROUNDING: f32 = 1.2;

/// Where each channel's block sits along its track, as an offset from the
/// mark's centre. Positive is DOWN, as everywhere else in egui.
///
/// **The stagger is the mark.** Three blocks level with each other read as a
/// fence or a grid; three at visibly different heights read as a mixing
/// desk, and nothing else in the shape carries that. The owner's reference
/// puts the left channel high, the middle one low and the right one near the
/// middle, and these are those three positions.
///
/// They are not merely different, they are far apart: -3.2 and +3.4 are on
/// opposite sides of the centre and 6.6 apart on a track only 16.7 long --
/// two fifths of its travel -- and -0.4 is off the centre line rather than
/// on it, so no two blocks and no axis of symmetry line up.
///
/// Each satisfies `|offset| + TUNE_FADER_HEIGHT / 2 <=
/// TUNE_TRACK_HALF_HEIGHT`, which is what keeps a block *on* its track with
/// a run of track still showing past both of its ends -- a block flush with
/// the end reads as a cap on a post, not as a fader that could travel. A
/// first pass at ±3.8/4.0 was rendered and pulled back for exactly that:
/// 1.25pt of track above the highest block is under a pixel once it is
/// anti-aliased, and the mark looked as though its travel had run out.
const TUNE_FADER_OFFSETS: [f32; TUNE_FADERS] = [-3.2, 3.4, -0.4];

/// The weight every drawn icon in this titlebar/header family is stroked at:
/// the tune icon's lines and knobs, the eye's almond, the switcher's chevron
/// ([`SWITCHER_CHEVRON_STROKE`]).
///
/// At file scope because "the same styling as its neighbours" is the whole
/// of what was asked for this control, and a number written out separately
/// in each of them is a number that drifts apart.
///
/// Named for the family rather than for one member since the gear it was
/// first written for became a tune icon; the number, and the rule it
/// carries, are unchanged.
pub const ICON_STROKE: f32 = 1.3;

/// How far in from the tip the star's outline leaves the skeleton, as a
/// fraction of the edge it leaves along.
///
/// **This is what rounds the points, and it is geometry rather than a stroke
/// setting.** The previous version got its blunting from a fat
/// [`STAR_STROKE`], because egui's `Stroke` has no join style and a heavy
/// width makes the miter clamp visible. That worked and it cost the mark its
/// weight: the star was the only thing on the header strip not drawn at
/// [`ICON_STROKE`], which is exactly what "the star looks too bold compared
/// to the other glyphs" is a report of. Cutting the corner in the PATH
/// separates the two -- the tip is as blunt as this number says, at whatever
/// weight the family is drawn at.
const STAR_TIP_ROUND: f32 = 0.32;

/// The same, for the five valleys between the points.
///
/// **Smaller than [`STAR_TIP_ROUND`] on purpose.** The tips are the acute
/// corners and the ones a person calls spiky; the valleys are already
/// obtuse, and rounding them as hard as the tips shallows them until the
/// five points stop separating and the mark drifts towards a blob -- which
/// is the failure the old 0.382-ratio comment was reaching for when it said
/// "flower".
///
/// The pair also has an invariant: `STAR_TIP_ROUND + STAR_VALLEY_ROUND < 1`,
/// or the trims taken from the two ends of a single edge overlap and the
/// outline crosses itself. 0.44 leaves plenty of room, and
/// [`the_stars_rounded_corners_do_not_eat_their_own_edges`] holds it.
///
/// 0.12 rather than something nearer the tip's number is a MEASURED
/// retreat. A first pass at 0.24 was rendered and looked at, and the five
/// points stopped separating -- the mark read as a rounded pentagon with
/// bumps, not as a star. The valleys are what make a star a star.
const STAR_VALLEY_ROUND: f32 = 0.12;

/// The star's valley radius as a fraction of its point radius -- how FAT the
/// five points are.
///
/// Deliberately NOT 1/φ² (0.382), the regular pentagram's ratio: that is the
/// geometrically pure star, and at 18px it reads as thin and dated, the
/// points long spikes with very little body. It was raised to 0.50 once
/// already for that reason. **It has come back DOWN to 0.46, and that is the
/// finding.** The report asked for "more rounded (wider) edges so it looks
/// bit more modern", and the obvious reading -- push the ratio further up,
/// to 0.56, so the points fatten -- was tried, rendered and rejected by
/// looking at it: combined with the corner fillets it left a mark whose
/// valleys were too shallow to separate the points, so it read as a rounded
/// pentagon rather than as a star. Width and roundness are what was asked
/// for; a ratio that high buys them by spending the shape.
///
/// So the roundness comes from [`STAR_TIP_ROUND`], which is a fillet and
/// costs the valleys nothing, and the ratio moves the other way to give the
/// valleys back their depth. 0.46 puts the point at 46.6°, sharper than the
/// 52.5° it had, but the tip a person actually sees is the 0.32 fillet
/// across it and not that angle.
///
/// The old comment here claimed anything past 0.382 "reads as a flower".
/// Measured, the flower turns up well before it was expected to once the
/// corners are rounded as well -- so the claim was right about the failure
/// and wrong about where it starts. Recorded because it is the reason the
/// ratio went unquestioned for so long, and because the next person to reach
/// for a fatter star should know the ceiling is real.
const STAR_INNER_RATIO: f32 = 0.46;

/// The five-pointed star's outline, starting just short of the top point.
///
/// Built around the origin and then translated so its own BOUNDING BOX --
/// not the circle its points lie on -- is centred on `center`, exactly as
/// [`pencil_glyph_at`] does and for the same reason: a pentagram has one
/// point above and two below, so its extent is taller above than below, and
/// anchoring by the circle's centre leaves it sitting visibly high in a
/// square hit target.
///
/// **Every corner is an arc, not a point.** The ten skeleton corners are
/// laid out as before and then each is replaced by a quadratic Bézier that
/// leaves the incoming edge at [`STAR_TIP_ROUND`]/[`STAR_VALLEY_ROUND`] of
/// its length, passes the corner as its control point, and rejoins the
/// outgoing edge the same distance along. A quadratic with the corner as
/// control is tangent to both edges at its ends, so the result is a true
/// fillet with no crease where it meets the straights -- the round join
/// egui's `Stroke` cannot be asked for, drawn into the path where it also
/// applies to the FILLED state.
fn star_outline(center: Pos2, outer: f32) -> Vec<Pos2> {
    let inner = outer * STAR_INNER_RATIO;
    let corner = |i: usize| {
        let radius = if i % 2 == 0 { outer } else { inner };
        // -90° so a POINT is at the top, not a valley.
        let angle = -std::f32::consts::FRAC_PI_2
            + i as f32 * std::f32::consts::TAU / STAR_CORNERS as f32;
        Vec2::new(radius * angle.cos(), radius * angle.sin())
    };
    let mut local: Vec<Vec2> = Vec::with_capacity(STAR_VERTICES);
    for i in 0..STAR_CORNERS {
        let here = corner(i);
        let before = corner((i + STAR_CORNERS - 1) % STAR_CORNERS);
        let after = corner((i + 1) % STAR_CORNERS);
        // A star polygon's ten edges are all the same length, so a fraction
        // of the vector to the neighbour IS a fraction of the edge and no
        // normalise-then-scale is needed.
        let cut = if i % 2 == 0 { STAR_TIP_ROUND } else { STAR_VALLEY_ROUND };
        let from = here + (before - here) * cut;
        let to = here + (after - here) * cut;
        for step in 0..=STAR_ROUND_SEGMENTS {
            let t = step as f32 / STAR_ROUND_SEGMENTS as f32;
            let u = 1.0 - t;
            local.push(from * (u * u) + here * (2.0 * u * t) + to * (t * t));
        }
    }
    debug_assert_eq!(local.len(), STAR_VERTICES);
    let top = local.iter().fold(f32::INFINITY, |a, p| a.min(p.y));
    let bottom = local.iter().fold(f32::NEG_INFINITY, |a, p| a.max(p.y));
    let offset = Vec2::new(0.0, -(top + bottom) / 2.0);
    local.into_iter().map(|p| center + p + offset).collect()
}

/// Paints the star at `center`, filled or outlined, in one colour.
/// Stroke width for the favourite star, in both states.
///
/// **It was 2.2, and that is what the report was about.** "Star (fav) glyph
/// looks too bold now compared to the other glyphs" -- and measured, it was
/// the only mark on the header strip not drawn at [`ICON_STROKE`]: the
/// envelope, the clock, the kebab's dots and the eye beside it are all 1.3,
/// and 2.2 is 69% more ink along every millimetre of the same outline. The
/// star's own doc for that width said so outright, calling it "deliberately
/// heavy" -- it was carrying the corner rounding, because egui's `Stroke`
/// offers no join style and a wide line makes the miter clamp visible.
///
/// [`STAR_TIP_ROUND`] carries the rounding now, in the path, so the width no
/// longer has a second job and can simply be the family's. Measured on the
/// rendered strip that nearly halves the outlined star's ink (123.6 -> 70.8
/// square px of stroke) and takes 12% off the filled one, without touching
/// what the mark is.
const STAR_STROKE: f32 = ICON_STROKE;

/// The favourite star's outer radius, as [`star_outline`] takes it.
///
/// **It was a bare `9.0` at the call site, and it was the largest mark on the
/// header strip.** Measured: at 9.0 the star's ink covered 19.32x18.48
/// against the clock's 18.30x18.30 and the envelope's 19.30 wide, which is
/// the wrong way round twice over -- the star is the one mark that FILLS when
/// it is on, and a solid shape already reads heavier than an outline at the
/// same extent, so it should be the smallest of the set and not the biggest.
///
/// 8.46 solved `outline_width + STAR_STROKE = 18.30` for the clock's own
/// extent back when [`STAR_STROKE`] was 2.2, and landed the mark at
/// 18.29x17.50 -- level with the clock across. **That answer was measured
/// against the wrong quantity.** Squaring a FILLED mark to an outlined one's
/// bounding box equalises the boxes and not the ink, and the report that
/// followed ("looks too bold") was about the ink: at 18.29 across, solid, the
/// star painted 167 square px against the envelope's 118 and the clock's 90,
/// and no bounding box was going to show that.
///
/// 9.20 is measured against the ink instead. With the corners rounded
/// ([`STAR_TIP_ROUND`]) and the weight back at [`ICON_STROKE`], it paints
/// 17.01x16.24 and 147 square px filled, 71 outlined -- and 17.01 is
/// deliberately BETWEEN the strip's two tiers rather than in either. The
/// edge marks (✉ 19.30x14.30, ⏱ 18.30x18.30) reach their extremes at a few
/// points and need the box; the sparse marks (⋮ 3.40x15.40, ✕ 15.40x15.40)
/// would read as the biggest thing on the strip at that size. A solid star
/// is neither: it fills its box the way neither tier does, so it earns its
/// own position just above the sparse tier and well below the edge one. The
/// tiers are not collapsed by this -- all three sizes stay distinct and
/// [`the_favourite_star_is_no_larger_than_the_outlined_marks_beside_it`]
/// still holds them apart.
///
/// Named rather than left inline for the reason the rest of this family is
/// named: a number written at one call site is a number nobody can find when
/// the next report arrives.
const STAR_OUTER: f32 = 9.20;

fn paint_star(ui: &Ui, center: Pos2, outer: f32, filled: bool, color: Color32) {
    let points = star_outline(center, outer);
    let painter = ui.painter();
    if filled {
        // A pentagram is CONCAVE, so `convex_polygon` over its outline
        // would tessellate to garbage. It is star-shaped about its own
        // centre, though, so a triangle fan from there is exact -- and every
        // triangle in it is convex. The apex is the mean of the outline's
        // own vertices, NOT `center`, which `star_outline` has offset away
        // from the star's geometric middle.
        //
        // **One MESH, not one filled shape per triangle**, and the
        // difference is visible rather than a tidiness. Each
        // `Shape::convex_polygon` is tessellated on its own WITH its own
        // anti-aliased feather, so a fan of them lays a soft edge down the
        // inside of every spoke: adjacent triangles double up where their
        // feathers overlap and fall short where they do not, which is the
        // mottling the old fan showed through its fill. Rounding the corners
        // made it worse by multiplying the fan four-fold, and at
        // [`STAR_ROUND_SEGMENTS`] the arc triangles are slivers narrow
        // enough that one of them tessellated to a detached speck outside
        // the mark. A mesh shares its vertices, so there are no interior
        // edges to feather at all and no sliver has an edge of its own; the
        // outline stroked over it is what gives the silhouette its
        // anti-aliasing, which is the one place it belongs.
        let apex = points
            .iter()
            .fold(Vec2::ZERO, |a, p| a + p.to_vec2())
            .to_pos2()
            / points.len() as f32;
        let mut fan = egui::epaint::Mesh::default();
        fan.colored_vertex(apex, color);
        for point in &points {
            fan.colored_vertex(*point, color);
        }
        for i in 0..points.len() as u32 {
            fan.add_triangle(0, 1 + i, 1 + (i + 1) % points.len() as u32);
        }
        painter.add(egui::Shape::mesh(fan));
    }
    // Always, in both states: outlined it IS the star, and filled it covers
    // the hairline seams anti-aliasing leaves between adjacent fan triangles.
    // It is also what [`icon_probe::stars`] finds, so both states are equally
    // visible to a test.
    //
    // **The width no longer rounds anything** -- [`star_outline`] does, in
    // the path. That is the whole of the change: egui's `Stroke` exposes no
    // join style, so a wide line's miter clamp used to be the only blunting
    // available here, and paying for it in weight is what made this the one
    // mark on the strip heavier than [`ICON_STROKE`]. A fillet in the path
    // is free of that, and it rounds the FILLED silhouette too, which a
    // stroke-side trick never could.
    //
    // The width still applies to both states, and for the unchanged reason:
    // a thinner stroke under the fill would leave the "on" star a different
    // size from the "off" one, which reads as two icons rather than two
    // states of one.
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
    paint_star(ui, rect.center(), STAR_OUTER, on, color);
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

/// The detail header's "Send a record" control: an envelope, stroked at the
/// weight its neighbours are.
///
/// **An envelope because the user asked for one** -- "those are global things
/// to the details like email icon after Fav" -- and because what this opens
/// composes a link to hand somebody, which is the one thing a mail mark says
/// without a word. It replaces the titlebar's worded `Send a record` pill:
/// that pill acted on the SELECTED ITEM from a strip of global controls, and
/// had to grey itself out with nothing selected to say so. Here there is
/// always an item, because this strip is only drawn when there is one -- so
/// the control has no disabled state at all, and needs none.
///
/// Square at [`HEADER_BUTTON_HEIGHT`] like the star and the kebab, so all
/// four controls have the same hit target rather than each being as big as
/// its own mark.
///
/// DRAWN, not typed, for this family's standing reason: ✉ (U+2709) would
/// come out of egui's bundled fallback with its own weight and optical size
/// beside three marks measured from the design, if it resolved at all.
pub fn send_record_button(ui: &mut Ui) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::splat(HEADER_BUTTON_HEIGHT), Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // The kebab's resting grey, not the star's: this is an action, and the
    // star's fainter TEXT_FAINT is the colour of a toggle that is OFF.
    let color = if response.hovered() { INK } else { TEXT_SECONDARY };
    let stroke = Stroke::new(ICON_STROKE, color);
    let c = rect.center();
    let (hw, hh) = (ENVELOPE_HALF_WIDTH, ENVELOPE_HALF_HEIGHT);
    let corner = |x: f32, y: f32| c + Vec2::new(x, y);
    // The body, as a CLOSED PATH of exactly `ENVELOPE_VERTICES` points --
    // see that constant for why this is not `rect_stroke`.
    ui.painter().add(egui::Shape::closed_line(
        vec![
            corner(-hw, -hh),
            corner(hw, -hh),
            corner(hw, hh),
            corner(-hw, hh),
        ],
        stroke,
    ));
    // The flap: down from both top corners to the fold in the middle. Closed
    // over the top edge it shares with the body, so it is one countable path
    // rather than two loose segments -- `icon_probe::line_segments` already
    // finds the eye's strike and the titlebar's ✕, and two more entries
    // there would be two more things every such test has to exclude.
    //
    // `fill: TRANSPARENT` is load-bearing and not a default being restated:
    // `icon_probe::stars` walks three-point paths WITH a fill, so a filled
    // flap would be reported as a star's fill triangle and every "the header
    // painted exactly one star" assertion in `detail.rs` would start reading
    // this mark instead.
    ui.painter().add(egui::Shape::Path(egui::epaint::PathShape {
        points: vec![corner(-hw, -hh), corner(0.0, hh * 0.35), corner(hw, -hh)],
        closed: true,
        fill: Color32::TRANSPARENT,
        stroke: stroke.into(),
    }));
    response
}

/// The detail header's close ✕: it clears the selection, so the item list
/// takes the whole window and the pane is gone until a row is clicked.
///
/// **Never [`ERROR`], and that is the whole of the safety rule.** It sits
/// immediately to the right of the kebab, and inside that kebab is a Delete
/// that arms on its first click and is permanent on its second -- so these
/// two controls are one misclick apart and one of them cannot be undone. A
/// red ✕, or one drawn at the weight of a primary, would be inviting exactly
/// that mistake.
///
/// # It used to rest at `TEXT_GHOST`, and that was overshooting
///
/// Reported as "close button feels too gray/thin compared to the rest on
/// details screen", and the report is simply right: the palette's faintest
/// clickable ink, on a strip where the ✉ and the ⋮ beside it rest at
/// [`TEXT_SECONDARY`], made the ✕ read as disabled rather than as quiet.
///
/// The old reasoning ran "not mistakable for the armed Delete, therefore as
/// faint as possible", and the second half does not follow from the first.
/// **Not red is the property.** [`ERROR`] is `#b42318`; [`TEXT_SECONDARY`] is
/// `#444141`, a neutral dark grey with no hue in it at all. Nothing about
/// resting there makes a ✕ readable as a delete, and the distance to
/// [`TEXT_GHOST`] bought no safety the neutral grey did not already have --
/// it only cost legibility on the one control that is on every detail pane.
///
/// So it rests where its siblings rest, and darkens to [`INK`] on hover as
/// they do. The favourite star's [`TEXT_FAINT`] is not the reference: that is
/// the colour of a TOGGLE THAT IS OFF, and this is an action, which is the
/// same distinction [`send_record_button`] makes in its own body.
///
/// It still has no armed state, because closing a pane is undone by clicking
/// the row again.
///
/// `the_close_mark_is_never_dressed_as_the_delete_beside_it` is the live
/// guard, and it asserts the invariant -- never `ERROR`, and the same resting
/// ink as the ✉ -- rather than either constant's value.
pub fn close_pane_button(ui: &mut Ui) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::splat(HEADER_BUTTON_HEIGHT), Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let color = if response.hovered() { INK } else { TEXT_SECONDARY };
    let stroke = Stroke::new(ICON_STROKE, color);
    let arm = PANE_CLOSE_ARM;
    let c = rect.center();
    let painter = ui.painter();
    painter.line_segment([c + Vec2::new(-arm, -arm), c + Vec2::new(arm, arm)], stroke);
    painter.line_segment([c + Vec2::new(arm, -arm), c + Vec2::new(-arm, arm)], stroke);
    response
}

/// The detail header's **"Add a one-time code"** control: a clock face,
/// showing three o'clock.
///
/// It sits between the ✉ and the kebab, and it is in this strip for exactly
/// the reason the ✉ is -- see [`send_record_button`], where the user's
/// rejection of a titlebar pill *for acting on the selected item* is quoted.
/// Adding a code acts on the selected item too, so the same argument puts it
/// in the same strip. The kebab is what it is drawn against: what it opens is
/// a form that writes a field onto this item, which is the kebab's Edit's
/// neighbourhood, and it is the one control here that is not drawn for every
/// item (see `detail::header_controls`).
///
/// Square at [`HEADER_BUTTON_HEIGHT`] like the rest of the strip, so all five
/// controls have the same hit target rather than each being as big as its own
/// mark.
///
/// **A CLOCK, and drawn rather than typed -- with the measurement taken
/// rather than assumed.** The four obvious codepoints were put to `has_glyph`
/// against the app's real font stack, and they split exactly the way the
/// folder's four did: U+23F2 ⏲, U+231A ⌚, U+1F550 🕐 and U+1F551 🕑 resolve
/// nowhere at all, while **U+23F1 ⏱ DOES resolve -- at ★'s own advance, out
/// of egui's bundled emoji fallback.** That second answer is the dangerous
/// one, because it is the one that would have shipped: a `true` from
/// `has_glyph` is not a licence to type a mark, since the face answering may
/// be one nobody here chose, at a weight nobody here set. See
/// [`the_clock_codepoints_are_not_carried_by_this_apps_own_typeface`], which
/// records both halves, and [`folder_mark`], where the same trap was found.
///
/// The hover ink is [`send_record_button`]'s and not [`star_toggle`]'s: this
/// is an action, and the star's fainter resting grey is the colour of a
/// toggle that is OFF.
pub fn add_totp_button(ui: &mut Ui) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::splat(HEADER_BUTTON_HEIGHT), Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let color = if response.hovered() { INK } else { TEXT_SECONDARY };
    let stroke = Stroke::new(ICON_STROKE, color);
    let c = rect.center();
    let painter = ui.painter();
    // The face. A stroked ring at [`CLOCK_RADIUS`] -- a radius nothing else
    // in this crate strokes, which is what keeps `icon_probe`'s three other
    // ring probes from counting it.
    painter.circle_stroke(c, CLOCK_RADIUS, stroke);
    // The hands, both on an axis -- see [`CLOCK_HOUR_HAND`] for why that is
    // load-bearing and not a drawing preference.
    painter.line_segment([c, c + Vec2::new(0.0, -CLOCK_HOUR_HAND)], stroke);
    painter.line_segment([c, c + Vec2::new(CLOCK_MINUTE_HAND, 0.0)], stroke);
    response
}

/// One slider row of the tune icon: the horizontal line it is drawn on, and
/// the knob sitting on that line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TuneRow {
    /// Top and bottom ends of the track, in that order. Same `x` in both, by
    /// construction -- a mixer's tracks are vertical, and
    /// [`icon_probe::tune_icons`] finds the mark by exactly that.
    pub line: [Pos2; 2],
    /// The fader block riding this track. Always on `line`'s `x`, and always
    /// far enough inside the ends that a run of track shows above and below.
    pub knob: Rect,
}

/// The mixer mark's geometry: [`TUNE_FADERS`] vertical tracks of equal
/// length, spaced about `center`, each carrying one block at its own height.
///
/// A pure function, and public to this module's tests, for the reason
/// `gear_outline` was before it: nothing computed inside an eframe closure
/// can be asserted about. Everything that decides how this mark reads -- the
/// tracks being vertical and equal, the blocks sitting on them, the three
/// heights differing -- is decided here and measured directly.
fn tune_rows(center: Pos2) -> Vec<TuneRow> {
    // Channels centred on `center.x` as a group: for three that is one in
    // the middle and one either side. Written for any `TUNE_FADERS` rather
    // than for three, so a fourth could be added without the mark drifting
    // off its own hit target -- which is the mistake the horizontal version
    // of this function was written to avoid, and it was right about it.
    let first = -(TUNE_FADERS as f32 - 1.0) / 2.0;
    (0..TUNE_FADERS)
        .map(|channel| {
            let x = center.x + (first + channel as f32) * TUNE_TRACK_PITCH;
            let y = center.y + TUNE_FADER_OFFSETS[channel];
            TuneRow {
                line: [
                    Pos2::new(x, center.y - TUNE_TRACK_HALF_HEIGHT),
                    Pos2::new(x, center.y + TUNE_TRACK_HALF_HEIGHT),
                ],
                knob: Rect::from_center_size(
                    Pos2::new(x, y),
                    Vec2::new(TUNE_FADER_WIDTH, TUNE_FADER_HEIGHT),
                ),
            }
        })
        .collect()
}

/// The vault titlebar's Preferences control: a **mixer** mark -- three
/// vertical tracks, each carrying a fader block at its own height -- drawn
/// rather than typed.
///
/// **It was two horizontal slider rows**, on the report "also settings glyph
/// I prefer to have vertical with just cross bars like a DJ mixer, maybe".
/// The same mark rotated, in other words, which is what the vocabulary
/// below still is: a straight stroke and a handle, repeated per channel.
/// The handle became a filled block rather than a stroked ring for the
/// reason [`TUNE_FADER_WIDTH`] records -- a ring reads as a slider knob and
/// a block reads as a fader cap, and the cap is what a mixing desk has.
///
/// **The trailing "maybe" was taken seriously and the mark was rendered
/// before it was shipped.** A horizontal tune icon is the more conventional
/// signifier for *settings*, and there was a real chance the vertical
/// version would not read as one at 18pt. Looked at against its neighbours
/// it does: the bank of three staggered blocks is unmistakably a control
/// surface, and it is a good deal more legible than the two-row version it
/// replaces, whose ring knobs were small enough to blur into their own
/// lines.
///
/// **It was a gear before that**, and the gear's whole design record is
/// still worth reading (see [`TUNE_FADERS`], which inherits it): the tooth count
/// had already been cut twice on the user's note that the mark looked
/// outdated beside its neighbours. A cog is a dense shape at 18px no matter
/// how few teeth it has, and the same note is what asked for this. The
/// drawn-control mark answers it structurally rather than by tuning:
/// straight lines and three small blocks, nothing radial.
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
/// **It sits in the drawn-icon family the same way the gear was made to.**
/// Every track is stroked at [`ICON_STROKE`], which is what [`eye_toggle`]
/// and [`account_switcher_button`] are stroked at, and
/// [`the_tune_icon_is_stroked_at_the_weight_the_eye_and_the_switcher_are`]
/// reads that off the painted shapes rather than off the constant. Its
/// vocabulary is two kinds of mark -- a straight stroke and a small block --
/// repeated [`TUNE_FADERS`] times, which is exactly the kebab's own idiom
/// (one dot, three times). The hit target and the two-state colouring are
/// unchanged; neither was ever the complaint.
///
/// The blocks are **filled and opaque, and they cover their track**: a fader
/// cap is a solid thing that hides the run of track behind it, and that is
/// the reading the reference image asked for. It is also what makes the
/// handle findable by eye at 18pt, which a crossing stroke at
/// [`ICON_STROKE`] would not be -- one pixel of ink over another pixel of
/// ink is a thickened line, not a handle.
///
/// Carries a hover label because, unlike Lock, it has no word on it --
/// [`close_glyph`]'s "Dismiss" is the precedent for an unlabelled drawn
/// control naming itself on hover.
pub fn tune_button(ui: &mut Ui) -> Response {
    const SIZE: f32 = 28.0;

    let (rect, response) = ui.allocate_exact_size(Vec2::splat(SIZE), Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // The same two-state treatment `kebab_button` uses: this is a neutral
    // navigation control with no "on" state, so it never takes BLUE (which
    // `star_toggle` reserves for an actual toggle being on) and never takes
    // ERROR (reserved for failures).
    let color = if response.hovered() { INK } else { TEXT_SECONDARY };
    let stroke = Stroke::new(ICON_STROKE, color);
    let painter = ui.painter();
    for row in tune_rows(rect.center()) {
        // The track first, the block over it. It matters here in a way it
        // did not when the handle was a ring: the block is OPAQUE, and
        // painting it second is what hides the run of track behind it. The
        // other order would leave a hairline of track showing down the
        // middle of every cap, which at this size reads as a seam in the
        // block rather than as a track passing behind it.
        painter.line_segment(row.line, stroke);
        painter.rect_filled(row.knob, TUNE_FADER_ROUNDING, color);
    }
    response.on_hover_text("Preferences")
}

/// Half the switcher chevron's width. Deliberately smaller than the gear's
/// 9px tip radius: this is a subordinate mark beside the avatar, not a control
/// competing with it.
///
/// At file scope rather than inside [`account_switcher_button`] so
/// [`icon_probe::chevrons`] can find the mark by the same two numbers that
/// draw it -- the vault titlebar strokes ✕ and — as line segments too, and a
/// probe that spelled these out again would go on finding the close glyph
/// after this shape was retuned out from under it.
const SWITCHER_CHEVRON_ARM: f32 = 4.0;
/// How far the switcher chevron's point drops below its two arms' ends.
const SWITCHER_CHEVRON_DROP: f32 = 2.6;
/// The chevron's stroke width, at file scope for the same reason as the two
/// above: `Shape::visual_bounding_rect` expands a line segment by half the
/// stroke at each end, so a probe matching on the raw arm and drop finds
/// nothing at all.
///
/// [`ICON_STROKE`] by definition rather than by coincidence: these two
/// controls sit next to each other in the same 28px strip.
const SWITCHER_CHEVRON_STROKE: f32 = ICON_STROKE;

/// The vault titlebar's account switcher: a downward chevron, 28px square,
/// sized and coloured exactly like [`tune_button`] beside it.
///
/// **Two strokes, not U+25BE**, and measured before it was decided.
/// `the_switcher_chevron_is_not_carried_by_this_apps_own_typeface` asks the
/// resolved stack the same way
/// `the_icon_codepoints_are_not_carried_by_this_apps_own_typeface` asks it
/// about ★/☆/👁 -- and gets the *worse* answer. ▾ is not in their position
/// (a real glyph out of a fallback face nobody chose); it is in ⋮'s and ✕'s:
/// `has_glyph` says no, and ▾ ▼ ▸ ✓ all measure to one identical width that
/// is the replacement box. Typed, this control would be a tofu square beside
/// a 28px gear, a 28px avatar and a 28px Lock pill.
///
/// Named for what it opens rather than for its shape, and carrying a hover
/// label, for [`close_glyph`]'s reason: it has no word on it.
pub fn account_switcher_button(ui: &mut Ui) -> Response {
    const SIZE: f32 = 28.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(SIZE), Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // `tune_button`'s two-state treatment, and for its reason: a navigation
    // control with no "on" state never takes BLUE, and never takes ERROR.
    let color = if response.hovered() { INK } else { TEXT_SECONDARY };
    let stroke = Stroke::new(SWITCHER_CHEVRON_STROKE, color);
    // Centred on the chevron's own bounding box rather than on `rect`, so
    // the mark reads as vertically centred: a "V" hangs low if its widest
    // edge is put on the centre line.
    let c = rect.center() - Vec2::new(0.0, SWITCHER_CHEVRON_DROP / 2.0);
    let painter = ui.painter();
    // Two segments rather than one three-point path, so `icon_probe::chevrons`
    // can find them the way `line_segments` finds the eye's strike.
    painter.line_segment(
        [
            c + Vec2::new(-SWITCHER_CHEVRON_ARM, 0.0),
            c + Vec2::new(0.0, SWITCHER_CHEVRON_DROP),
        ],
        stroke,
    );
    painter.line_segment(
        [
            c + Vec2::new(0.0, SWITCHER_CHEVRON_DROP),
            c + Vec2::new(SWITCHER_CHEVRON_ARM, 0.0),
        ],
        stroke,
    );
    response.on_hover_text("Switch account")
}

/// How full each of the eye's lids is, as the exponent on `1 - t²`.
///
/// **It was 1.0 -- a plain parabola -- and that is what "eye glyphs, bit
/// taller / more rounded" was a report of.** A parabolic lid is flat across
/// the middle and then dives at the ends: at 90% of the way to the corner it
/// has already given up 81% of its height, so the almond's belly is thin
/// everywhere except dead centre and the mark reads as a squashed oval
/// rather than as an eye.
///
/// The exponent is the direct control on that, and it runs the useful way
/// round: 1.0 is the parabola, 0.5 is an exact ellipse, and anything between
/// is fuller than the one and less mechanical than the other. 0.72 keeps
/// 74% of the height at that same 90% mark -- an almond with a body -- while
/// staying off the ellipse, which at this size reads as a circle squashed by
/// a layout rather than as a drawn shape.
///
/// Below 1.0 the lids also meet the corners with a vertical tangent instead
/// of a shallow one, which is what gives the almond its two points. That is
/// the shape's own doing and not a separate treatment.
const EYE_LID_FULLNESS: f32 = 0.72;

/// The eye's almond outline: two lids meeting at the corners.
///
/// **Sampled by ANGLE, not by x.** The lids are walked as `t = sin(u)` with
/// `u` sweeping corner to corner, so the samples bunch where the curve turns
/// hardest and spread where it runs straight. Stepping `t` uniformly -- what
/// this did while the lids were parabolas, when it hardly mattered -- puts
/// the same number of points along the flat middle as along the corner, and
/// [`EYE_LID_FULLNESS`] below 1.0 turns the corner sharply enough that the
/// faceting shows at 28px. The point count is unchanged; only where they sit
/// is.
fn eye_outline(center: Pos2, half_w: f32, half_h: f32) -> Vec<Pos2> {
    let lid = |i: usize, sign: f32| {
        let sweep = std::f32::consts::PI * i as f32 / EYE_LID_SEGMENTS as f32;
        let t = (sweep - std::f32::consts::FRAC_PI_2).sin();
        let height = (1.0 - t * t).max(0.0).powf(EYE_LID_FULLNESS);
        center + Vec2::new(t * half_w, sign * half_h * height)
    };
    let mut points: Vec<Pos2> = (0..=EYE_LID_SEGMENTS).map(|i| lid(i, -1.0)).collect();
    // The lower lid, back from just inside the right corner to just inside
    // the left one -- the corners themselves are already in the list.
    points.extend((1..EYE_LID_SEGMENTS).rev().map(|i| lid(i, 1.0)));
    debug_assert_eq!(points.len(), EYE_VERTICES);
    points
}

/// The square [`eye_toggle`] allocates for itself.
///
/// Public because the width of the reveal control is what decides whether a
/// masked row's label, value and eye fit on one line -- see `detail.rs`'s
/// `masked_row`. A caller that guessed 28 here and a control retuned to 32
/// there is a row that overflows its card again, silently.
pub const EYE_TOGGLE_SIZE: f32 = 28.0;

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
    /// Half the almond's width. Unchanged: the eye's width is what
    /// `detail.rs`'s `masked_row` budgets a row's controls against, and
    /// nothing about it was reported.
    const HALF_W: f32 = 8.5;
    /// Half the almond's height. **It was 5.0**, which with a parabolic lid
    /// put the mark at 17.0 x 10.0 -- a 1.7:1 letterbox, and the "bit
    /// taller" half of the report. 6.4 makes it 17.0 x 12.8, a 1.33:1
    /// almond, which is where an eye stops reading as an oval somebody sat
    /// on. The 28px hit target ([`EYE_TOGGLE_SIZE`]) is nowhere near
    /// troubled by it, and the strike below still clears the lids.
    const HALF_H: f32 = 6.4;

    let (rect, response) = ui.allocate_exact_size(Vec2::splat(EYE_TOGGLE_SIZE), Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let color = if response.hovered() { INK } else { TEXT_FAINT };
    let center = rect.center();
    let painter = ui.painter();
    painter.add(egui::Shape::closed_line(
        eye_outline(center, HALF_W, HALF_H),
        // [`ICON_STROKE`]: this is the weight the drawn-icon family shares,
        // and the gear was retuned to sit with this eye rather than the
        // other way round.
        Stroke::new(ICON_STROKE, color),
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

/// A small folder outline, painted inline in a run of text: a tab on the
/// left, a shoulder, and the body under it.
///
/// The detail header's subtitle reads `Card · Work`, and the user's report is
/// that this is "two words what they mean" -- nothing on the line says which
/// half is the item's TYPE and which is where it lives. This mark goes
/// immediately before the folder's name and answers that, in the one way a
/// second line of text could not without saying more than the design's one
/// line has room for.
///
/// **DRAWN, not typed, and the measurement was taken rather than assumed.**
/// The four obvious codepoints were put to `has_glyph` against the app's real
/// font stack before this function existed, and they split in two: 📁 U+1F4C1
/// and 📂 U+1F4C2 resolve nowhere at all -- tofu boxes in the header of every
/// foldered item, exactly what design 4d's ⇥ and ⏎ turned out to be -- while
/// 🗀 U+1F5C0 and 🗁 U+1F5C1 DO resolve, out of egui's bundled emoji fallback,
/// at ★'s own advance. The second pair is the one that would have shipped, and
/// it is refused for the reason [`star_toggle`] and [`send_record_button`]
/// already give: a mark from a face nobody here chose has a weight and an
/// optical size nobody here set. See
/// [`the_folder_codepoints_are_not_carried_by_this_apps_own_typeface`], which
/// holds both halves.
///
/// The colour is the caller's, and the caller passes the subtitle's own
/// [`TEXT_FAINT`]: the mark introduces secondary text and is not a control,
/// so it has no hover state and no ink of its own to assert over the words.
///
/// Sized against the text rather than against the header controls -- see
/// [`FOLDER_MARK_SIZE`] -- and stroked lighter than the rest of the family
/// for the same reason ([`FOLDER_STROKE`]).
pub fn folder_mark(ui: &mut Ui, color: Color32) {
    // `Sense::hover()`, not `click()`: this is punctuation, not a control.
    // The subtitle around it is a plain label, and a mark that took the
    // pointing hand would promise a navigation this header does not have.
    let (rect, _) = ui.allocate_exact_size(FOLDER_MARK_SIZE, Sense::hover());
    let c = rect.center() + Vec2::new(0.0, FOLDER_MARK_DROP);
    let (hw, hh) = (FOLDER_HALF_WIDTH, FOLDER_HALF_HEIGHT);
    let p = |x: f32, y: f32| c + Vec2::new(x, y);
    // ONE closed path of exactly [`FOLDER_VERTICES`] points, stroked with no
    // fill -- the envelope's body states the general form of this rule: a
    // `Shape::Rect` plus a separate tab would be indistinguishable from every
    // card fill behind it to `icon_probe`, and two loose paths would be two
    // more things every "exactly one X" assertion in `detail.rs` has to
    // exclude.
    ui.painter().add(egui::Shape::Path(egui::epaint::PathShape {
        points: vec![
            p(-hw, -hh),                                          // tab, top left
            p(-hw + FOLDER_TAB_WIDTH, -hh),                       // tab, top right
            p(-hw + FOLDER_TAB_WIDTH + FOLDER_TAB_SLANT, -hh + FOLDER_TAB_RISE), // shoulder
            p(hw, -hh + FOLDER_TAB_RISE),                         // body, top right
            p(hw, hh),                                            // body, bottom right
            p(-hw, hh),                                           // body, bottom left
        ],
        closed: true,
        // Load-bearing and not a default restated, exactly as on the
        // envelope's flap: `icon_probe::folder_marks` walks UNFILLED closed
        // paths, and a fill here would take this mark out of its own probe.
        fill: Color32::TRANSPARENT,
        stroke: Stroke::new(FOLDER_STROKE, color).into(),
    }));
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

/// How far short of its rect's right edge a single line of clipped text stops.
///
/// Used by [`crate::win32_draw::draw_row`], whose `DrawTextW` calls carry
/// `DT_END_ELLIPSIS`: without an inset the "..." Windows substitutes sits hard
/// against the card's edge and reads as a cut rather than as a truncation.
/// Three device pixels is the smallest gap that separates the glyph from the
/// edge at 100% scaling; it is here rather than in `win32_draw` so the GDI
/// renderer keeps taking every dimension from this module.
pub const TEXT_CLIP_INSET: f32 = 3.0;

/// Configures `ui` so that an [`egui::ScrollArea`] shown inside it reserves a
/// `gutter`-wide lane down its right-hand edge and draws its bar in the
/// OUTERMOST [`SCROLLBAR_WIDTH`] of that lane, instead of over the content's
/// own right edge.
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
/// The placement itself is `bar_outer_margin`: egui pins a floating bar's
/// RIGHT edge at `outer_rect.right() - bar_outer_margin`, and the outer rect
/// now ends at the container's own right edge because of the reserved lane.
/// Zero therefore puts the bar flush to the lane's OUTER edge -- where the
/// platform's own scroll bars sit -- and leaves every point of the lane's
/// slack on the inner side, between the bar and the content.
/// `floating_width` is raised to the full `bar_width` so the bar does not
/// GROW leftward over the content when hovered -- a bar that only stayed put
/// while dormant would not have fixed the report.
///
/// # Why the outer edge and not the centre
///
/// The report: "the right padding feels smaller", on a list that DOES
/// scroll, so the bar is genuinely needed. Measured on the item list's 10pt
/// lane it was 10pt of clear space left of the tiles against 2pt right of
/// them. There is a floor on that asymmetry and it is not zero. Every caller
/// of this function pins two things by test: the content ends at
/// `pane_right - gutter` (so the lane is exactly `gutter` wide and cannot be
/// widened without moving the content), and the content keeps ONE width
/// whether or not the bar is showing. The same strip of pane is therefore
/// clear space when the bar is hidden and ink when it is not, so the hidden
/// state and the shown state CANNOT both be symmetric: showing the bar costs
/// the right side at least [`SCROLLBAR_WIDTH`]. Equal is unreachable; the
/// floor -- `gutter - SCROLLBAR_WIDTH` of clear space -- is reachable, and
/// `bar_outer_margin = 0` is what reaches it.
///
/// Centring missed that floor by the OUTER half of the leftover lane, spent
/// on a gap between the bar and the pane's own edge that the reader is not
/// comparing to anything: 2pt of the item list's and the edit form's 10pt
/// lanes, and 9pt of the read pane's 24pt one.
///
/// **Stated as "the outermost `SCROLLBAR_WIDTH` of the lane", not as a
/// margin.** The rejected alternative was to keep centring for wide lanes
/// and go flush only for narrow ones -- some threshold above which a gap
/// behind the bar reads as deliberate rather than as lost padding. There is
/// no such threshold in the design to read off, and it would make the three
/// panes disagree about where a scroll bar lives for no reason a reader
/// could see. The wide lane's numbers are better under the flush rule
/// anyway: the read pane's right-hand clear space goes from 9pt to 18pt
/// against 24pt on the left, i.e. from a quarter of the left side's to
/// three quarters of it.
///
/// The other rejected framing was "the bar is flush to the CONTENT", which
/// is the placement the very first report complained about.
pub fn scrollbar_in_gutter(ui: &mut Ui, gutter: f32) {
    let scroll = &mut ui.spacing_mut().scroll;
    scroll.floating_allocated_width = gutter;
    scroll.bar_width = SCROLLBAR_WIDTH;
    scroll.floating_width = SCROLLBAR_WIDTH;
    scroll.bar_outer_margin = 0.0;
}

/// Makes the floating bar of an [`egui::ScrollArea`] shown inside `ui` paint
/// NOTHING, while leaving every measurement [`scrollbar_in_gutter`] set alone.
///
/// This is for the list that fits: `AlwaysVisible` is what keeps the reserved
/// lane -- and therefore the content's width -- from changing as items are
/// added and removed, but it also paints a full-height bar for a list with
/// nothing to scroll. A 6pt line running the whole height of the 10pt gutter
/// leaves only 2pt of clear space between it and the tiles, against 10pt on
/// the left, which is what a reader sees as "the right padding is smaller".
/// The tiles are symmetric; the bar is what is not.
///
/// It works by zeroing the six opacities egui multiplies a FLOATING bar's
/// track and handle colours by, rather than by changing the visibility mode
/// or any width. Nothing about the layout moves, so the bar can be turned
/// back on the moment the content overflows without the tiles resizing --
/// which is the whole reason [`scrollbar_in_gutter`] demands `AlwaysVisible`.
/// All SIX, and not just the pair that happens to matter today. egui picks
/// one of the three pairs per frame from how close the pointer is (dormant /
/// pointer-in-the-area / pointer-on-the-bar), and its floating defaults
/// already leave the dormant pair at 0 -- so the bar a reader of this list
/// actually sees is the `active_*` one, which is what the item-list test
/// kills a mutation of. Setting only that pair would leave the bar to
/// reappear the moment the pointer crossed into the gutter itself, and
/// "hidden unless you point at where it would be" is not a state worth
/// having.
pub fn hide_scrollbar(ui: &mut Ui) {
    let scroll = &mut ui.spacing_mut().scroll;
    scroll.dormant_background_opacity = 0.0;
    scroll.active_background_opacity = 0.0;
    scroll.interact_background_opacity = 0.0;
    scroll.dormant_handle_opacity = 0.0;
    scroll.active_handle_opacity = 0.0;
    scroll.interact_handle_opacity = 0.0;
}

/// A muted field label ("User name", "Master password").
pub fn field_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(TEXT_MUTED));
}

/// [`field_label`], greyed, for a field that cannot be typed into right now
/// -- see [`disabled_text_field`]. A label left at [`TEXT_MUTED`] over a
/// greyed box reads as a live field whose box happens to be pale.
pub fn disabled_field_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(TEXT_GHOST));
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
///
/// Public so a test can find these boxes in a painted frame by the one
/// measurement every one of them shares, live or greyed, rather than by a
/// number written out again beside the assertion.
pub const FIELD_HEIGHT: f32 = 38.0;

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

/// How many bullets a masked readout shows, **regardless of how long the real
/// value is -- or whether there is one at all**.
///
/// `vault_window::detail` declares its own copy of this number for exactly
/// this reason, and the two are deliberately separate: that one masks a
/// stored password in the detail pane, this one masks a password field while
/// a sign-in is in flight, and neither wants to move because the other did.
/// What they must not do is either of the things a length-tracking mask does:
/// tell a shoulder-surfer how many characters to expect, or -- the case the
/// login window actually hits -- collapse to nothing and announce that the
/// buffer behind it has already been emptied.
pub const MASKED_BULLETS: usize = 10;

/// The mask itself: [`MASKED_BULLETS`] bullets, always the same string.
pub fn masked_readout() -> String {
    "\u{2022}".repeat(MASKED_BULLETS)
}

/// [`text_field`]'s box with **no `TextEdit` in it at all**: the 38px box
/// painted in the greyed treatment, and `text` painted as a galley.
///
/// Returns the box, so a caller can put an (equally inert) in-field
/// affordance on it -- which is what [`disabled_password_field`] does.
///
/// **A galley, not `TextEdit::interactive(false)`**, and the difference is
/// visible: egui's read-only `TextEdit` still takes focus, still draws a
/// caret, and still eats the click that lands on it. A field that greys out
/// and then blinks a cursor at you is not disabled, it is broken -- the same
/// conclusion the minutes stepper in `prefs_ui` reached, and it is solved the
/// same way here. Nothing is allocated with a `Sense::click`, so the pointer
/// passes over this box as if it were background.
pub fn disabled_text_field(ui: &mut Ui, text: &str) -> Rect {
    disabled_field_box(ui, text, 10.0)
}

/// [`password_field`]'s box while an attempt is in flight: [`masked_readout`]
/// in the greyed treatment, with the reveal toggle painted greyed and inert
/// beside it.
///
/// **It takes no value.** The mask does not depend on one -- that is
/// [`MASKED_BULLETS`]'s whole point -- and not taking one means this cannot
/// later be "improved" into something that leaks the length.
///
/// The toggle reads "Show" rather than whatever the field was on: what is on
/// screen underneath it IS masked, whichever way the user had it set before
/// they submitted, so "Hide" would be offering to hide something already
/// hidden.
pub fn disabled_password_field(ui: &mut Ui) -> Rect {
    let box_rect = disabled_field_box(ui, &masked_readout(), 52.0);
    let label = ui.painter().layout_no_wrap(
        "Show".to_string(),
        FontId::new(11.0, FontFamily::Name(SEMIBOLD.into())),
        TEXT_GHOST,
    );
    ui.painter().galley(
        Pos2::new(
            box_rect.right() - 50.0,
            box_rect.center().y - label.size().y / 2.0,
        ),
        label,
        TEXT_GHOST,
    );
    box_rect
}

/// The shared body of [`disabled_text_field`] and [`disabled_password_field`]:
/// the same 38px box [`field_box`] allocates, painted greyed, with `text` sat
/// on the same baseline a live field's text would be.
///
/// Greyed means all three of fill, border and ink move together --
/// [`CANVAS`] instead of [`CARD`], [`BORDER`] instead of [`BORDER_STRONG`],
/// [`TEXT_GHOST`] instead of the ambient body colour. Any one of them alone
/// reads as a styling accident rather than as a control that is switched off.
fn disabled_field_box(ui: &mut Ui, text: &str, right_pad: f32) -> Rect {
    let (outer, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), FIELD_HEIGHT),
        // Hover, NOT click: this box is not a control, and giving it a click
        // sense is how a disabled field starts swallowing the clicks meant
        // for whatever is behind or beside it.
        Sense::hover(),
    );
    ui.painter().rect(
        outer,
        CornerRadius::same(8),
        CANVAS,
        Stroke::new(1.0, BORDER),
        StrokeKind::Middle,
    );
    // Laid out at [`field_box`]'s own font and inside its own text width, and
    // TRUNCATED there rather than wrapped: this box is one line tall, and an
    // email long enough to wrap would otherwise paint its second line
    // straight through the bottom border and on down the card.
    let mut job = egui::text::LayoutJob::single_section(
        text.to_string(),
        egui::TextFormat::simple(FontId::new(14.0, FontFamily::Proportional), TEXT_GHOST),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(
        (outer.width() - 10.0 - right_pad).max(0.0),
    );
    let galley = ui.painter().layout_job(job);
    // `field_box`'s own optical nudge, for its reason: a row box is
    // ascent+descent tall and typical field text fills only the upper part,
    // so geometric centring reads as text sitting high.
    ui.painter().galley(
        Pos2::new(
            outer.min.x + 10.0,
            outer.center().y - galley.size().y / 2.0 + galley.size().y * 0.09,
        ),
        galley,
        TEXT_GHOST,
    );
    outer
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

/// [`toggle_pill`]'s greyed twin, for a switch a master switch has turned
/// off.
///
/// **Still shows its own state**, knob and all: the row is disabled, not
/// meaningless, and the value is the one that comes straight back when the
/// master switch is turned on again. A pill that flattened to "off" while
/// disabled would be displaying a value that is not the stored one, which is
/// the same lie as a control whose number the clamp silently overrides.
///
/// Built from the design's own two lighter greys rather than a new colour, the
/// way `prefs_ui::minutes_stepper`'s disabled box is: [`HAIRLINE`] for the
/// track, and a knob in [`CANVAS`] instead of white so it stays visible
/// against it. The design has no disabled variant of this control, so this is
/// assembled from its parts.
pub fn toggle_pill_disabled(ui: &mut Ui, on: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(40.0, 22.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(11), HAIRLINE);
    let knob_x = if on {
        rect.max.x - 11.0
    } else {
        rect.min.x + 11.0
    };
    ui.painter()
        .circle_filled(Pos2::new(knob_x, rect.center().y), 9.0, CANVAS);
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

    /// **The same measurement for the account switcher's chevron**, taken
    /// before it was drawn rather than assumed from the star's answer -- and
    /// it is not the star's answer.
    ///
    /// U+25BE BLACK DOWN-POINTING SMALL TRIANGLE is the codepoint a switcher
    /// beside an avatar would reach for first. It is in the *worse* of the
    /// two positions this app's icons can be in: not "resolves out of a
    /// fallback face nobody chose" like ★ and 👁, but the U+22EE/U+2715
    /// position -- nothing in the resolved stack carries it at all, so as
    /// text it is a tofu box.
    ///
    /// The advances below are the second half of that. ▾, ▼, ▸ and ✓ are
    /// four unrelated marks that all lay out to one identical width, and it
    /// is not ★'s: that is the replacement box being measured four times,
    /// not four glyphs. So `account_switcher_button` strokes its chevron,
    /// and this is the evidence rather than an argument by analogy.
    #[test]
    fn the_switcher_chevron_is_not_carried_by_this_apps_own_typeface() {
        let ctx = ctx_with_fonts();
        let font = FontId::new(13.0, FontFamily::Proportional);
        let width = |s: &str| {
            ctx.fonts_mut(|f| f.layout_no_wrap(s.to_string(), font.clone(), INK))
                .size()
                .x
        };

        assert!(
            !ctx.fonts_mut(|f| f.has_glyph(&font, '\u{25BE}')),
            "U+25BE now resolves; the switcher's chevron is two drawn strokes because it \
             did not"
        );
        // The positive control for that, the same one the kebab's assertion
        // above carries: `has_glyph` is not simply answering "no" to
        // everything, and the fonts really did load.
        assert!(
            ctx.fonts_mut(|f| f.has_glyph(&font, 'A')),
            "the font set resolves no 'A' either, so the assertion above proves nothing"
        );

        let chevron = width("\u{25BE}");
        for missing in ["\u{25BC}", "\u{25B8}", "\u{2713}"] {
            assert_eq!(
                chevron,
                width(missing),
                "▾ and {missing} no longer share one advance, so at least one of them is \
                 now a real glyph rather than the replacement box"
            );
        }
        assert_ne!(
            chevron,
            width("\u{2605}"),
            "▾ now advances like ★, which DOES resolve -- out of egui's bundled icon \
             fallback. Re-measure: this test's whole claim is that ▾ is in the worse \
             position of the two"
        );
        // The positive control for the equal-advance argument, the same one
        // the test above uses: this stack really does measure a proportional
        // face proportionally, so four equal advances mean something.
        assert_ne!(
            width("A"),
            width("W"),
            "'A' and 'W' advance identically, so the equal-advance argument above is \
             about the measurement, not about the face"
        );
    }

    /// **The same measurement for the subtitle's folder mark**, taken before
    /// [`folder_mark`] was written rather than assumed from the star's or the
    /// chevron's answer -- **and the answer is not one answer.**
    ///
    /// The crate had already been bitten once by a codepoint nobody measured:
    /// design 4d's ⇥ and ⏎ resolved in neither shipped face and would have
    /// rendered as empty rectangles. So the codepoints a folder mark would
    /// reach for were asked directly, and the four split into two groups:
    ///
    /// * **U+1F4C1 FILE FOLDER and U+1F4C2 OPEN FILE FOLDER do not resolve at
    ///   all** -- the U+22EE/U+25BE position, the worse of the two. As text
    ///   they are tofu boxes beside every foldered item's name. Their advance
    ///   is the replacement box's, which is neither the app's own letters' nor
    ///   the emoji face's.
    /// * **U+1F5C0 FOLDER and U+1F5C1 OPEN FOLDER DO resolve**, out of egui's
    ///   bundled emoji fallback -- the ★/👁 position. They advance at exactly
    ///   ★'s width, which is the tell: a face nobody here chose, laying out
    ///   two folder pictographs and an unrelated star to one identical em.
    ///
    /// Neither group is usable, and the second is the interesting one because
    /// it would have shipped. A 12px subtitle set in [`TEXT_FAINT`] would get
    /// a mark at that fallback's own weight and optical size, beside a header
    /// whose every other measurement comes from the design -- the argument
    /// [`send_record_button`] and [`star_toggle`] already make, now with a
    /// measurement behind it for this mark rather than by analogy. So
    /// [`folder_mark`] strokes an outline.
    #[test]
    fn the_folder_codepoints_are_not_carried_by_this_apps_own_typeface() {
        let ctx = ctx_with_fonts();
        // The subtitle's own 12pt, not the 13pt the two tests above use: a
        // measurement taken at a size this mark is never drawn at would be a
        // measurement of something else.
        let font = FontId::new(12.0, FontFamily::Proportional);
        let width = |s: &str| {
            ctx.fonts_mut(|f| f.layout_no_wrap(s.to_string(), font.clone(), INK))
                .size()
                .x
        };

        for absent in ['\u{1F4C1}', '\u{1F4C2}'] {
            assert!(
                !ctx.fonts_mut(|f| f.has_glyph(&font, absent)),
                "U+{:04X} now resolves; it was recorded as a tofu box in this app's own \
                 stack, which is half the case for drawing the folder mark",
                absent as u32
            );
        }
        // The positive control for those two: `has_glyph` is not simply
        // answering "no" to everything, and the fonts really did load. The
        // same control the kebab's and the chevron's assertions carry.
        assert!(
            ctx.fonts_mut(|f| f.has_glyph(&font, 'A')),
            "the font set resolves no 'A' either, so the assertions above prove nothing"
        );
        // And the two that DO resolve, recorded rather than glossed: the
        // reason this mark is drawn is not "no folder codepoint exists", it is
        // that the ones that exist come from a face this app never chose.
        for present in ['\u{1F5C0}', '\u{1F5C1}'] {
            assert!(
                ctx.fonts_mut(|f| f.has_glyph(&font, present)),
                "U+{:04X} no longer resolves; it did when the folder mark was drawn, and \
                 that -- not its absence -- is what this test records",
                present as u32
            );
            // Out of the EMOJI fallback, which is the whole objection. ★ is
            // already known to come from there (see the star's own test), and
            // a proportional text face does not give a star and a folder one
            // identical advance.
            assert_eq!(
                width(&present.to_string()),
                width("\u{2605}"),
                "U+{:04X} no longer advances like ★, so it may now come from a real text \
                 face -- re-measure before trusting the drawn mark's justification",
                present as u32
            );
        }
        // The two that do not resolve share the REPLACEMENT box's advance, and
        // it is not the emoji face's: this is the measurement agreeing with
        // `has_glyph` rather than merely being asked alongside it.
        assert_eq!(
            width("\u{1F4C1}"),
            width("\u{1F4C2}"),
            "📁 and 📂 no longer share one advance, so at least one of them is now a real \
             glyph rather than the replacement box"
        );
        assert_ne!(
            width("\u{1F4C1}"),
            width("\u{2605}"),
            "📁 now advances like ★, which DOES resolve -- so the two groups this test \
             separates have collapsed into one"
        );
        // The positive control for those advance arguments, the same one the
        // two tests above use: this stack really does measure a proportional
        // face proportionally, so equal advances mean something.
        assert_ne!(
            width("A"),
            width("W"),
            "'A' and 'W' advance identically, so the equal-advance argument above is \
             about the measurement, not about the face"
        );
    }

    /// **The same measurement, for the clock**, and it came back split the
    /// same way the folder's did -- which is why [`add_totp_button`] strokes
    /// an outline instead of setting a codepoint.
    ///
    /// * **U+23F2 ⏲, U+231A ⌚, U+1F550 🕐 and U+1F551 🕑 do not resolve at
    ///   all.** As text they are tofu boxes in the detail header of every
    ///   login, and their advance is the replacement box's -- neither the
    ///   app's own letters' nor the emoji face's.
    /// * **U+23F1 ⏱ DOES resolve**, out of egui's bundled emoji fallback, at
    ///   exactly ★'s advance. That is the tell, and it is the same tell
    ///   [`the_folder_codepoints_are_not_carried_by_this_apps_own_typeface`]
    ///   caught: a face nobody here chose, laying out a stopwatch and an
    ///   unrelated star to one identical em.
    ///
    /// **The second answer is the whole point of this test.** `has_glyph`
    /// returning `true` is not a licence to type a mark -- it says only that
    /// *something* in the stack will draw it -- and ⏱ is what would have
    /// shipped had the question stopped there: a 34px header control set at
    /// the emoji fallback's own weight and optical size, beside four marks
    /// measured from the design.
    #[test]
    fn the_clock_codepoints_are_not_carried_by_this_apps_own_typeface() {
        let ctx = ctx_with_fonts();
        // 13pt, the size a header control's glyph would have been set at --
        // a measurement taken at a size this mark is never drawn at would be
        // a measurement of something else.
        let font = FontId::new(13.0, FontFamily::Proportional);
        let width = |s: &str| {
            ctx.fonts_mut(|f| f.layout_no_wrap(s.to_string(), font.clone(), INK))
                .size()
                .x
        };

        for absent in ['\u{23F2}', '\u{231A}', '\u{1F550}', '\u{1F551}'] {
            assert!(
                !ctx.fonts_mut(|f| f.has_glyph(&font, absent)),
                "U+{:04X} now resolves; it was recorded as a tofu box in this app's own \
                 stack, which is half the case for drawing the clock",
                absent as u32
            );
        }
        // The positive control for those four: `has_glyph` is not simply
        // answering "no" to everything, and the fonts really did load. The
        // same control every sibling measurement in this file carries.
        assert!(
            ctx.fonts_mut(|f| f.has_glyph(&font, 'A')),
            "the font set resolves no 'A' either, so the assertions above prove nothing"
        );
        // And the one that DOES resolve, recorded rather than glossed -- the
        // reason this mark is drawn is not "no clock codepoint exists".
        assert!(
            ctx.fonts_mut(|f| f.has_glyph(&font, '\u{23F1}')),
            "U+23F1 no longer resolves; it did when the clock was drawn, and that -- not \
             its absence -- is what this test records"
        );
        // Out of the EMOJI fallback, which is the whole objection. ★ is
        // already known to come from there (see the star's own test), and a
        // proportional text face does not give a star and a stopwatch one
        // identical advance.
        assert_eq!(
            width("\u{23F1}"),
            width("\u{2605}"),
            "⏱ no longer advances like ★, so it may now come from a real text face -- \
             re-measure before trusting the drawn mark's justification"
        );
        // The four that do not resolve share the REPLACEMENT box's advance,
        // and it is not the emoji face's: this is the measurement agreeing
        // with `has_glyph` rather than merely being asked alongside it.
        assert_eq!(
            width("\u{23F2}"),
            width("\u{1F550}"),
            "⏲ and 🕐 no longer share one advance, so at least one of them is now a real \
             glyph rather than the replacement box"
        );
        assert_ne!(
            width("\u{23F2}"),
            width("\u{2605}"),
            "⏲ now advances like ★, which DOES resolve -- so the two groups this test \
             separates have collapsed into one"
        );
        // The positive control for those advance arguments, the same one the
        // sibling tests use: this stack really does measure a proportional
        // face proportionally, so equal advances mean something.
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

    // -----------------------------------------------------------------
    // Cyrillic coverage (see `CYRILLIC_FACES`).
    // -----------------------------------------------------------------

    /// A word in Cyrillic ("Passwords"), all of it inside U+0400-04FF and
    /// none of it in Archivo.
    const CYRILLIC_WORD: &str = "Пароли";

    /// Every family that carries one of the four weights, paired with the
    /// Archivo face and the Noto face that belong in it. Regular's home is
    /// egui's own `Proportional`; the other three are named families.
    fn weighted_families() -> Vec<(FontFamily, &'static str, &'static str)> {
        CYRILLIC_FACES
            .iter()
            .map(|(archivo, noto, _)| {
                let family = if *archivo == REGULAR {
                    FontFamily::Proportional
                } else {
                    FontFamily::Name((*archivo).into())
                };
                (family, *archivo, *noto)
            })
            .collect()
    }

    /// A context whose font set is exactly `fonts`, two frames in (see
    /// [`ctx_with_fonts`] for why two).
    fn ctx_with(fonts: egui::FontDefinitions) -> egui::Context {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 400.0))),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        ctx.set_fonts(fonts);
        let _ = ctx.run_ui(input(), |_ui| {});
        ctx
    }

    /// **Position, not presence.** A Noto face appended to the *end* of a
    /// family list is registered, is contained, and changes absolutely
    /// nothing: egui's own fallbacks are still in these lists and are what
    /// Cyrillic was reaching. So this pins both edges -- each Noto face sits
    /// after its Archivo cut (Latin must never be taken away from Archivo)
    /// and before every entry egui shipped (or the fallback still wins).
    #[test]
    fn each_cyrillic_face_sits_behind_its_archivo_cut_and_ahead_of_eguis_fallbacks() {
        let fonts = font_definitions();
        let egui_defaults = egui::FontDefinitions::default()
            .families
            .get(&FontFamily::Proportional)
            .cloned()
            .expect("egui ships a Proportional family");
        assert!(
            !egui_defaults.is_empty(),
            "egui's Proportional family is empty, so \"Noto comes before the fallbacks\" \
             below would be vacuously true"
        );

        for (family, archivo, noto) in weighted_families() {
            let stack = fonts
                .families
                .get(&family)
                .unwrap_or_else(|| panic!("no family {family:?} in the font set"));
            let at = |face: &str| {
                stack
                    .iter()
                    .position(|f| f == face)
                    .unwrap_or_else(|| panic!("{face} is not in {family:?}: {stack:?}"))
            };
            let (archivo_at, noto_at) = (at(archivo), at(noto));

            assert!(
                archivo_at < noto_at,
                "{noto} sits at {noto_at} in {family:?}, ahead of {archivo} at {archivo_at} \
                 -- Latin would resolve to a Cyrillic-only subset first. Stack: {stack:?}"
            );

            for fallback in &egui_defaults {
                let fallback_at = at(fallback);
                assert!(
                    noto_at < fallback_at,
                    "{noto} sits at {noto_at} in {family:?}, BEHIND egui's own {fallback} at \
                     {fallback_at}. Registered, contained -- and never consulted, because the \
                     fallback is exactly what Cyrillic already resolved to. Stack: {stack:?}"
                );
            }
        }
    }

    /// **Rendering, not registration.** The reported symptom was one
    /// typeface at one weight for every Cyrillic string, whatever the design
    /// asked for; the four weights collapsing onto a single measurement is
    /// precisely that symptom, and it is what this refuses.
    ///
    /// Also checks the run has real ink: a `uv_rect` of zero size is a glyph
    /// that rasterised to nothing, which is how a missing codepoint gets
    /// through a width comparison unnoticed.
    #[test]
    fn a_cyrillic_run_renders_real_glyphs_at_the_weight_it_was_asked_for() {
        let ctx = ctx_with_fonts();
        let measure = |family: &FontFamily| {
            ctx.fonts_mut(|f| {
                let galley = f.layout_no_wrap(
                    CYRILLIC_WORD.to_owned(),
                    FontId::new(25.0, family.clone()),
                    INK,
                );
                let glyphs: Vec<_> = galley.rows.iter().flat_map(|r| r.glyphs.iter()).collect();
                assert_eq!(
                    glyphs.iter().map(|g| g.chr).collect::<String>(),
                    CYRILLIC_WORD,
                    "{family:?} laid out something other than the text it was given"
                );
                for g in &glyphs {
                    assert!(
                        g.uv_rect.size.x > 0.0 && g.uv_rect.size.y > 0.0,
                        "{:?} rasterised to nothing in {family:?} -- the run is blank, not \
                         merely the wrong weight",
                        g.chr
                    );
                }
                galley.size().x
            })
        };

        let widths: Vec<(String, f32)> = weighted_families()
            .iter()
            .map(|(family, archivo, _)| ((*archivo).to_owned(), measure(family)))
            .collect();

        for (i, (a_name, a)) in widths.iter().enumerate() {
            for (b_name, b) in widths.iter().skip(i + 1) {
                assert!(
                    (a - b).abs() > 0.5,
                    "{CYRILLIC_WORD:?} measures the same at {a_name} ({a}px) and {b_name} \
                     ({b}px), so Cyrillic is resolving to ONE face for every weight -- the \
                     bug itself. All four: {widths:?}"
                );
            }
        }

        // ...and that one face is not the one egui ships: an unstyled
        // context is what the app rendered Cyrillic with before.
        let bare = ctx_with(egui::FontDefinitions::default());
        let fallback = bare.fonts_mut(|f| {
            f.layout_no_wrap(
                CYRILLIC_WORD.to_owned(),
                FontId::new(25.0, FontFamily::Proportional),
                INK,
            )
            .size()
            .x
        });
        for (name, w) in &widths {
            assert!(
                (w - fallback).abs() > 0.5,
                "{name} still measures {w}px for {CYRILLIC_WORD:?}, the same as egui's \
                 untouched default ({fallback}px) -- nothing was actually substituted"
            );
        }
    }

    /// **The negative half, and the whole promise of a Latin-free subset.**
    ///
    /// The Noto faces carry no `A`-`z` at all, so no Latin lookup can reach
    /// them however the stacks are ordered -- meaning not one existing
    /// measurement in the app is allowed to move. Proving that needs the
    /// "before" to exist, so this builds it: the very same font set with the
    /// four faces stripped back out, and compares whole laid-out galleys
    /// glyph field by glyph field (position, advance, ascent, line height,
    /// and the `uv_rect` that identifies the rasterised glyph itself, so a
    /// substituted face of coincidentally equal width could not pass).
    #[test]
    fn latin_layout_is_identical_with_and_without_the_cyrillic_faces() {
        let mut stripped = font_definitions();
        for (_, noto, _) in CYRILLIC_FACES {
            assert!(
                stripped.font_data.remove(noto).is_some(),
                "{noto} was never registered, so stripping it proves nothing"
            );
            let mut removed = false;
            for stack in stripped.families.values_mut() {
                let before = stack.len();
                stack.retain(|f| f != noto);
                removed |= stack.len() != before;
            }
            assert!(removed, "{noto} was in no family, so stripping it proves nothing");
        }

        let with = ctx_with(font_definitions());
        let without = ctx_with(stripped);

        // A Latin run that exercises both cases and the digits and
        // punctuation between them, plus the two strings the design's own
        // measurements are taken from.
        let texts = [
            "The quick brown fox jumps over the lazy dog, 0123456789 (@/+-.)",
            "Deskwarden",
            "CTRL+L",
        ];
        let dump = |ctx: &egui::Context, text: &str, font: FontId| {
            ctx.fonts_mut(|f| {
                let galley = f.layout_no_wrap(text.to_owned(), font, INK);
                let mut out = format!("{:?}\n", galley.size());
                for row in &galley.rows {
                    for glyph in &row.glyphs {
                        out.push_str(&format!("{glyph:?}\n"));
                    }
                }
                out
            })
        };

        for (family, archivo, _) in weighted_families() {
            for text in texts {
                for size in [11.0, 13.0, 14.0, 22.0, 25.0] {
                    let font = FontId::new(size, family.clone());
                    assert_eq!(
                        dump(&with, text, font.clone()),
                        dump(&without, text, font),
                        "{text:?} at {size}px in {archivo} lays out differently once the \
                         Cyrillic subset faces are added. They carry no Latin codepoints, \
                         so this cannot happen unless one of them is being consulted for \
                         Latin -- which means the stack order is wrong"
                    );
                }
            }
        }
    }

    /// The monospace family deliberately gets no Noto face (see the comment
    /// in `font_definitions`), which is only defensible while it renders
    /// Cyrillic itself. Consolas covers U+0400-04FF, and so does the Hack
    /// egui bundles when `system_monospace` finds nothing -- so this holds
    /// on both branches, and reds if the branch it runs on stops covering
    /// Cyrillic, which is the point at which the decision would need
    /// revisiting.
    #[test]
    fn the_monospace_family_carries_cyrillic_without_a_noto_face() {
        let fonts = font_definitions();
        let monospace = fonts
            .families
            .get(&FontFamily::Monospace)
            .expect("egui ships a Monospace family");
        for (_, noto, _) in CYRILLIC_FACES {
            assert!(
                !monospace.contains(&noto.to_owned()),
                "{noto} is proportional; in the Monospace family it would take Cyrillic \
                 away from a monospaced face and hand it to one with per-glyph advances"
            );
        }

        let ctx = ctx_with_fonts();
        let font = FontId::new(13.0, FontFamily::Monospace);
        ctx.fonts_mut(|f| {
            let galley = f.layout_no_wrap(CYRILLIC_WORD.to_owned(), font.clone(), INK);
            for row in &galley.rows {
                for glyph in &row.glyphs {
                    assert!(
                        glyph.uv_rect.size.x > 0.0 && glyph.uv_rect.size.y > 0.0,
                        "{:?} rasterises to nothing in the monospace family, so a Cyrillic \
                         run in a monospace context IS blank and the family needs a \
                         Cyrillic face after all",
                        glyph.chr
                    );
                }
            }

            // Still monospaced for it: the reason the Noto subset was kept out.
            let narrow = f.layout_no_wrap("шшшшшш".to_owned(), font.clone(), INK).size().x;
            let wide = f.layout_no_wrap("ііііії".to_owned(), font, INK).size().x;
            assert!(
                (narrow - wide).abs() < 0.5,
                "the monospace family renders Cyrillic with unequal advances \
                 ({narrow}px vs {wide}px), so it is resolving them from a proportional face"
            );
        });
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

/// **The drawn-icon family, measured against each other rather than each
/// against itself.**
///
/// The user's note on the settings control was comparative -- "looks
/// outdated, use the same styling (more minimalistic)" -- so every assertion
/// here is comparative too: the Preferences mark's stroke weight, its hit
/// target and its mark count are checked against [`eye_toggle`],
/// [`account_switcher_button`] and [`kebab_button`] beside it, not against
/// numbers copied out of [`tune_button`]. A test that restated the control's
/// own constants would pass against any mark at all, including the gear that
/// prompted the note and that this control no longer draws.
#[cfg(test)]
mod drawn_icon_family_tests {
    use super::*;

    /// One frame at a size big enough for a row of 28px controls, with this
    /// app's real font set installed -- `apply` only takes effect at the
    /// start of the *next* frame, which is why the styling frame is run and
    /// discarded first (the same dance `tests::ctx_with_fonts` does).
    fn frame(mut build: impl FnMut(&mut Ui)) -> Vec<egui::Shape> {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0))),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});
        let output = ctx.run_ui(input(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| build(ui));
        });
        output.shapes.into_iter().map(|c| c.shape).collect()
    }

    /// Every shape in `shapes`, flattened out of the `Shape::Vec` nesting
    /// egui builds, that paints inside `within`.
    fn marks_in(shapes: &[egui::Shape], within: Rect) -> Vec<egui::Shape> {
        fn walk(shape: &egui::Shape, within: Rect, out: &mut Vec<egui::Shape>) {
            match shape {
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, within, out);
                    }
                }
                egui::Shape::Noop => {}
                other => {
                    let rect = other.visual_bounding_rect();
                    // `is_finite` rejects the sentinel `Rect::NOTHING` an
                    // empty shape reports, which `contains_rect` would
                    // otherwise answer "yes" to for every box on screen.
                    if rect.is_finite() && within.expand(1.0).contains_rect(rect) {
                        out.push(other.clone());
                    }
                }
            }
        }
        let mut out = Vec::new();
        for shape in shapes {
            walk(shape, within, &mut out);
        }
        out
    }

    /// The stroke width of one painted mark, whichever of the three stroked
    /// shape kinds it is. `None` for a glyph, and `None` for a purely FILLED
    /// mark -- the eye's pupil is a `circle_filled`, which reports width 0
    /// and is not a stroke weight this family shares or should be compared
    /// against. A mark that quietly became a fill therefore disappears from
    /// the weight comparison rather than passing it; what catches that is
    /// `the_tune_icon_repeats_no_more_marks_than_the_kebab_beside_it`, which counts
    /// marks of every kind.
    fn stroke_width(shape: &egui::Shape) -> Option<f32> {
        let width = match shape {
            egui::Shape::Path(p) => p.stroke.width,
            egui::Shape::Circle(c) => c.stroke.width,
            egui::Shape::LineSegment { stroke, .. } => stroke.width,
            _ => return None,
        };
        (width > 0.0).then_some(width)
    }

    /// Lays out one control, alone, and returns its allocated rect together
    /// with everything painted inside it.
    fn control(mut draw: impl FnMut(&mut Ui) -> Response) -> (Rect, Vec<egui::Shape>) {
        let rect = std::cell::Cell::new(Rect::NOTHING);
        let shapes = frame(|ui| {
            rect.set(draw(ui).rect);
        });
        let rect = rect.get();
        let marks = marks_in(&shapes, rect);
        (rect, marks)
    }

    /// [`control`], **with the pointer resting on the mark** -- the only way
    /// a control whose entire visible state is its stroke colour can be seen
    /// in its hovered state at all.
    ///
    /// The control is laid out first with no pointer, so its rect is known,
    /// and only then is the pointer moved onto that rect: egui resolves
    /// hovering against the widget rects of the frame BEFORE, so a pointer
    /// supplied in the same frame the widget first appears in hovers nothing.
    /// Two frames are then run with the pointer held, and the second is the
    /// one measured -- an arrangement whose failure mode is a test that
    /// cannot see the hover and therefore FAILS, rather than one that quietly
    /// measures the resting state and passes.
    fn hovered_control(mut draw: impl FnMut(&mut Ui) -> Response) -> (Rect, Vec<egui::Shape>) {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0));
        let rect = std::cell::Cell::new(Rect::NOTHING);
        let mut run = |pointer: Option<Pos2>| -> Vec<egui::Shape> {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events: pointer.into_iter().map(egui::Event::PointerMoved).collect(),
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| rect.set(draw(ui).rect));
            });
            output.shapes.into_iter().map(|c| c.shape).collect()
        };
        // The same styling dance [`frame`] does, and for the same reason.
        let _ = run(None);
        apply(&ctx);
        let _ = run(None);
        let at = rect.get().center();
        let _ = run(Some(at));
        let shapes = run(Some(at));
        let rect = rect.get();
        let marks = marks_in(&shapes, rect);
        (rect, marks)
    }

    /// **The clock is one findable mark, and its hands cannot be mistaken for
    /// a ✕.**
    ///
    /// Both halves are measured off what the control actually paints. The
    /// first is what every `detail.rs` guard over this control depends on;
    /// the second is the reason [`CLOCK_HOUR_HAND`]'s doc calls the axis
    /// alignment load-bearing -- [`icon_probe::pane_close_marks`] *panics* on
    /// an odd number of arms, so a diagonal hand would not merely miscount,
    /// it would take down every test that renders the detail header.
    #[test]
    fn the_one_time_code_clock_is_one_ring_and_no_close_arms() {
        let (_, marks) = control(add_totp_button);
        let tree = egui::Shape::Vec(marks);
        assert_eq!(
            icon_probe::clocks(&tree).len(),
            1,
            "`add_totp_button` painted no clock face its own probe can find"
        );
        // The positive control for the ✕ assertion below: the hands really
        // were painted, so "no close arms" is a statement about their shape
        // and not about an empty frame.
        assert_eq!(
            icon_probe::line_segments(&tree).len(),
            2,
            "the clock painted no two hands, so the assertion below proves nothing"
        );
        assert!(
            icon_probe::pane_close_marks(&tree).is_empty(),
            "a clock hand is being reported as an arm of the detail pane's close ✕"
        );
        // And it is none of the other three ring families either -- the
        // radius guard says the numbers differ, this says the probes agree.
        assert!(
            icon_probe::kebab_dots(&tree).is_empty()
                && icon_probe::tune_icons(&tree).is_empty()
                && icon_probe::eyes(&tree).is_empty(),
            "the clock's face is being counted as a kebab dot, a tune knob or an eye"
        );
    }

    /// **It darkens on hover, like the ✉ beside it.** Read off the PAINTED
    /// stroke colour: a constant comparison would only restate that the
    /// source says so, and this control's entire visible state is that
    /// colour -- it paints no fill and no string.
    #[test]
    fn the_one_time_code_clock_darkens_on_hover() {
        let (_, resting) = control(add_totp_button);
        let resting = icon_probe::clocks(&egui::Shape::Vec(resting));
        assert_eq!(resting.len(), 1, "no clock was painted at rest");
        assert_eq!(
            resting[0].1, TEXT_SECONDARY,
            "the clock rests at {:?}, not the kebab's own resting grey",
            resting[0].1
        );
        let (_, hovered) = hovered_control(add_totp_button);
        let hovered = icon_probe::clocks(&egui::Shape::Vec(hovered));
        assert_eq!(hovered.len(), 1, "no clock was painted while hovered");
        assert_eq!(
            hovered[0].1, INK,
            "the clock did not darken to INK on hover, so the control looks inert"
        );
    }

    /// **The constraint that decides these outlines' vertex counts**, and it
    /// is not decorative: [`icon_probe`] identifies them by point count
    /// ALONE, so two of them sharing a count makes each findable as the
    /// other, and every probe over them would then be reporting about
    /// whichever mark happened to be found first.
    ///
    /// It was three shapes when the Preferences control was a gear -- and it
    /// really did decide that gear's tooth count, twice. The tune icon that
    /// replaced it closes no path at all, so it is not in this comparison;
    /// it is found by circle radius instead, which is what
    /// [`the_drawn_circles_do_not_share_a_radius`] guards.
    #[test]
    fn no_two_drawn_icons_share_a_vertex_count() {
        for (a, a_name, b, b_name) in [
            (EYE_VERTICES, "the eye", STAR_VERTICES, "the star"),
            (ENVELOPE_VERTICES, "the envelope's body", EYE_VERTICES, "the eye"),
            (ENVELOPE_VERTICES, "the envelope's body", STAR_VERTICES, "the star"),
            (
                ENVELOPE_VERTICES,
                "the envelope's body",
                ENVELOPE_FLAP_VERTICES,
                "the envelope's own flap",
            ),
        ] {
            assert_ne!(
                a, b,
                "{a_name} and {b_name} both close over {a} points, and `icon_probe` tells \
                 these outlines apart by point count alone -- so each is now findable as \
                 the other and every probe over them is reporting the wrong mark"
            );
        }
    }

    /// **The envelope's flap shares the star's fill triangles' point count,
    /// and must not be findable as one.**
    ///
    /// [`no_two_drawn_icons_share_a_vertex_count`] cannot express this pair:
    /// three IS three, deliberately, and what separates them is the fill.
    /// So this drives both real controls and asserts each probe finds only
    /// its own mark -- which is the assertion, not the count.
    ///
    /// Paired in both directions on purpose. "The envelope is not a star"
    /// alone would pass against a `stars` probe that had stopped finding
    /// anything at all, so the same frames are asserted to still contain the
    /// star they should.
    #[test]
    fn the_envelope_flap_is_not_findable_as_a_star_fill() {
        let (_, envelope_marks) = control(send_record_button);
        let envelope_tree = egui::Shape::Vec(envelope_marks);
        assert_eq!(
            icon_probe::envelopes(&envelope_tree).len(),
            1,
            "`send_record_button` painted no envelope its own probe can find"
        );
        assert!(
            icon_probe::stars(&envelope_tree).is_empty(),
            "the envelope is being reported as a star, so every \"the header painted \
             exactly one star\" assertion in `detail.rs` is now reading this mark"
        );

        for on in [false, true] {
            let (_, star_marks) = control(|ui| star_toggle(ui, on));
            let star_tree = egui::Shape::Vec(star_marks);
            assert_eq!(
                icon_probe::stars(&star_tree).len(),
                1,
                "`star_toggle({on})` painted no star, so the negative above proves nothing"
            );
            assert!(
                icon_probe::envelopes(&star_tree).is_empty(),
                "the {on} star is being reported as an envelope"
            );
        }
    }

    /// The detail header's five controls, each drawn alone, in the order the
    /// strip reads left to right, with the box its painted ink really covers.
    ///
    /// The ★ is asked for in its OFF state and the ⋮ unarmed, which is what
    /// an ordinary item shows -- and neither state changes the geometry
    /// anyway, only the colour.
    ///
    /// `visual_bounding_rect` and not the control's own rect: every one of
    /// the five allocates `Vec2::splat(HEADER_BUTTON_HEIGHT)`, so the rects
    /// are identical by construction and say nothing about what is drawn
    /// inside them. The INK is the thing a reader compares.
    fn header_control_ink() -> Vec<(&'static str, Rect)> {
        header_control_marks()
            .into_iter()
            .map(|(name, marks)| {
                let extent = marks
                    .iter()
                    .map(|m| m.visual_bounding_rect())
                    .filter(|r| r.is_finite())
                    .reduce(|a, b| a.union(b))
                    .unwrap_or_else(|| panic!("the header's {name} painted nothing at all"));
                (name, extent)
            })
            .collect()
    }

    /// The shapes each of the five header controls paints, named.
    ///
    /// The tier below [`header_control_ink`], and separate from it because
    /// [`no_header_control_paints_a_glyph`] asks about the shape KINDS rather
    /// than about their extents -- and because a list of five controls
    /// written out twice is a list that drifts.
    fn header_control_marks() -> Vec<(&'static str, Vec<egui::Shape>)> {
        vec![
            ("★", control(|ui| star_toggle(ui, false)).1),
            ("✉", control(send_record_button).1),
            ("⏱", control(add_totp_button).1),
            ("⋮", control(|ui| kebab_button(ui, false)).1),
            ("✕", control(close_pane_button).1),
        ]
    }

    /// **The five header marks are drawn, not typed -- so their sizes are
    /// this file's to answer for.**
    ///
    /// The premise everything below rests on. Reported as "also those icons
    /// are not same size feels like", and the first question such a report
    /// raises here is the glyph trap this project has hit three times: a mark
    /// that renders out of egui's bundled emoji fallback rather than the
    /// app's own face has different metrics AND is a different typeface, and
    /// no amount of tuning a size fixes that. [`add_totp_button`]'s own doc
    /// records U+23F1 ⏱ resolving exactly that way, which is why it is drawn.
    ///
    /// It cannot be the cause on this strip, and this is what says so: not
    /// one of the five emits a `Shape::Text`, so no face is involved in any
    /// of them and there is no font metric to blame. All five are paths,
    /// circles and line segments this module draws itself, and their extents
    /// are a consequence of this file's own constants and of nothing else.
    #[test]
    fn no_header_control_paints_a_glyph() {
        for (name, marks) in header_control_marks() {
            assert!(
                !marks.iter().any(|m| matches!(m, egui::Shape::Text(_))),
                "the header's {name} paints a text glyph, so its size is a font's decision \
                 and not this file's -- and the face answering may not be the app's own"
            );
        }
    }

    /// **The strip reads as one set of marks, measured rather than felt.**
    ///
    /// Reported as "also those icons are not same size feels like". All five
    /// allocate the same 34pt square, so the boxes and the hit targets were
    /// already identical to the pixel and no rect assertion could see the
    /// defect. What differs is the painted ink, and here it is -- the extents
    /// this file drew before the report and after it:
    ///
    /// ```text
    ///        before            after
    /// *  19.32 x 18.48   18.29 x 17.50
    /// M  19.30 x 14.30   19.30 x 14.30
    /// O  18.30 x 18.30   18.30 x 18.30
    /// :   3.40 x 15.40    3.40 x 15.40
    /// X  12.30 x 12.30   15.40 x 15.40
    /// ```
    ///
    /// **Two tiers, and that is a finding rather than an excuse.** The naive
    /// rule -- one nominal extent for all five -- is wrong here, and the
    /// measurements are what say so. Three of these marks put their ink on
    /// the edges of their box (a star's points, an envelope's rectangle, a
    /// clock's rim); two do not (a column of three dots, and a cross whose
    /// only ink is two diagonals). Sparse ink reads smaller at equal extent,
    /// and diagonal ink reads LARGER because it reaches the corners -- two
    /// pulls in opposite directions that land the cross and the kebab in the
    /// same place, below the round marks. Squaring all five to one number
    /// would put a ✕ on this strip that read as the biggest thing on it.
    ///
    /// So the tiers are asserted by NAMING a member of each rather than by
    /// writing either number down:
    ///
    /// * the edge marks -- envelope, clock -- against the clock, the round
    ///   mark none of this touched;
    /// * the sparse marks -- kebab, close -- against the kebab, which nobody
    ///   reported and which is therefore the evidence for where that tier
    ///   sits.
    ///
    /// **The star was in the first list and is not any more**, on the report
    /// that followed this one ("Star (fav) glyph looks too bold now compared
    /// to the other glyphs"). It is the one mark here that is SOLID when it
    /// is on, so its box and its ink are the same thing, and squaring that
    /// box to an outlined mark's was equalising the wrong quantity -- at
    /// 18.29 across it painted 167 square px against the envelope's 118.
    /// It now sits strictly between the two tiers, which the assert at the
    /// end of this test pins by order rather than by number.  [`STAR_OUTER`]
    /// carries the full measurement.
    ///
    /// The second tier is the whole of the repair: the ✕ was in it by kind
    /// and nowhere near it by size, 12.30 against 15.40, which is what the
    /// report felt. The tolerance is 1.0pt -- wide enough for the envelope's
    /// 19.30 against the clock's 18.30, tight enough that the old 12.30
    /// misses by triple it.
    ///
    /// The short dimension of the envelope and the kebab is not asserted at
    /// all, and deliberately: an envelope as tall as it is wide is a picture
    /// frame ([`ENVELOPE_HALF_HEIGHT`] argues that already) and a kebab is one
    /// dot across by definition. That is silhouette, not size.
    #[test]
    fn every_header_mark_is_drawn_at_the_same_optical_size() {
        let ink = header_control_ink();
        let long = |name: &str| {
            let rect = ink
                .iter()
                .find(|(n, _)| *n == name)
                .unwrap_or_else(|| panic!("{name} is not on the strip"))
                .1;
            rect.width().max(rect.height())
        };
        let all = || {
            ink.iter()
                .map(|(n, r)| (*n, r.width(), r.height()))
                .collect::<Vec<_>>()
        };
        for (tier, reference, members) in [
            ("the edge marks", "⏱", ["✉", "⏱"].as_slice()),
            ("the sparse marks", "⋮", ["⋮", "✕"].as_slice()),
        ] {
            let nominal = long(reference);
            for name in members {
                assert!(
                    (long(name) - nominal).abs() <= 1.0,
                    "the header's {name} is one of {tier} and reaches {:.2}pt against \
                     {reference}'s {nominal:.2}pt, so the strip no longer reads as one set: \
                     {:?}",
                    long(name),
                    all()
                );
            }
        }
        // **The star is a third tier of its own, and it is asserted by
        // POSITION rather than by a nominal.** It was in the edge tier and
        // squared to the clock, which equalised the bounding boxes and not
        // the ink -- see [`STAR_OUTER`] for the measurement that refuted it.
        // A solid mark fills its box the way neither outlined tier does, so
        // it belongs strictly between them: bigger than the sparse marks,
        // which are mostly whitespace, and smaller than the edge marks,
        // whose ink is only their rim.
        //
        // Naming no number here is the same discipline the two loops above
        // follow. What is pinned is the ORDER, which is the design claim;
        // the sizes themselves stay in the constants that draw them.
        assert!(
            long("★") > long("⋮") && long("★") < long("⏱"),
            "the solid star reaches {:.2}pt, which is not between the sparse marks' \
             {:.2}pt and the edge marks' {:.2}pt -- a filled mark sits between the two \
             tiers precisely because a bounding box does not measure its ink: {:?}",
            long("★"),
            long("⋮"),
            long("⏱"),
            all()
        );
        // The control. "Each mark is near its own tier" is only worth
        // asserting while the tiers are really apart; if they ever converge
        // this test has stopped saying anything and the simpler one-nominal
        // rule should replace it rather than being quietly satisfied. The
        // margin has to clear the star's own position between them, or the
        // assert above would be satisfiable by three marks all but touching.
        assert!(
            long("⏱") - long("⋮") > 2.0,
            "the tiers have collapsed together, so this test is no longer checking \
             anything: {:?}",
            all()
        );
    }

    /// **And the star is not the loudest thing on the strip.**
    ///
    /// Split from the band check above because it is a different claim. The
    /// star is the one FILLED mark here when an item is a favourite, and a
    /// filled mark reads heavier than an outlined one at the same extent --
    /// so "inside the band" is not enough for it; it has to be no bigger than
    /// the outlined marks it sits among. It used to be the biggest of the
    /// five in both dimensions, 19.32 x 18.48 against the clock's 18.30,
    /// which is the wrong way round twice over.
    #[test]
    fn the_favourite_star_is_no_larger_than_the_outlined_marks_beside_it() {
        let ink = header_control_ink();
        let of = |want: &str| {
            ink.iter()
                .find(|(name, _)| *name == want)
                .unwrap_or_else(|| panic!("{want} is not on the strip"))
                .1
        };
        let star = of("★");
        for outlined in ["✉", "⏱"] {
            let other = of(outlined);
            let reach = other.width().max(other.height());
            assert!(
                star.width() <= reach + 0.01 && star.height() <= reach + 0.01,
                "the filled star paints {:.2}x{:.2}, larger than the outlined {outlined}'s \
                 {:.2}x{:.2} -- a solid mark already reads heavier than an outline at the \
                 same size",
                star.width(),
                star.height(),
                other.width(),
                other.height()
            );
        }
    }

    /// **The star's corner fillets have to fit on the edges they are cut
    /// from**, and nothing about the drawn result says when they stop.
    ///
    /// [`star_outline`] trims [`STAR_TIP_ROUND`] of an edge at the point end
    /// and [`STAR_VALLEY_ROUND`] of the same edge at the valley end. Past a
    /// sum of 1.0 those two trims cross, the Bézier control points swap
    /// order along the edge, and the outline self-intersects -- which
    /// tessellates to a mark that is still recognisably a star and is
    /// subtly, permanently wrong in its fill. There is no assertion in the
    /// painter that could catch it, because both states still paint.
    ///
    /// So it is checked here, and on the OUTLINE rather than on the two
    /// constants. Comparing the constants would be a truth about two
    /// literals -- something the compiler can fold and clippy says so -- and
    /// it would not survive [`star_outline`] being rewritten around them.
    /// Measuring the straight run left between one corner's fillet and the
    /// next one's is a fact about the shape that reaches the screen, at the
    /// size it is shipped at.
    #[test]
    fn the_stars_rounded_corners_do_not_eat_their_own_edges() {
        let points = star_outline(Pos2::ZERO, STAR_OUTER);
        let arc = STAR_ROUND_SEGMENTS + 1;
        for corner in 0..STAR_CORNERS {
            // Where this corner's fillet lets go of the edge, to where the
            // next one takes hold of it.
            let run = points[corner * arc + arc - 1]
                .distance(points[((corner + 1) % STAR_CORNERS) * arc]);
            assert!(
                run > 1.0,
                "the star's corner {corner} leaves only {run:.2}pt of straight edge before \
                 the next corner's fillet starts -- at zero the two fillets meet, past it \
                 they cross and the outline self-intersects, and either way the shape has \
                 stopped having edges to read as a star by"
            );
        }
        // **And the five points still project.** This is the failure that
        // was actually rendered and rejected on the way here: at
        // STAR_INNER_RATIO 0.56 with the valleys filleted at 0.24, the star
        // read as a rounded pentagon with bumps on it -- every assertion in
        // this file passed and the mark had stopped being a star.
        //
        // Measured as how deep the valleys come in against how far the
        // points reach out, both from the outline's own centroid, so it
        // holds whatever combination of ratio and fillet produces them.
        // Shipped it is 0.54; the pentagon that was rejected measured 0.66.
        let middle = points
            .iter()
            .fold(Vec2::ZERO, |a, p| a + p.to_vec2())
            .to_pos2()
            / points.len() as f32;
        let reach = points.iter().fold(0.0_f32, |a, p| a.max(p.distance(middle)));
        let valley = points
            .iter()
            .fold(f32::INFINITY, |a, p| a.min(p.distance(middle)));
        assert!(
            valley / reach < 0.60,
            "the star's valleys reach {valley:.2}pt against its points' {reach:.2}pt, a \
             ratio of {:.3} -- past 0.60 the points stop separating and the mark reads as \
             a rounded pentagon rather than as a star",
            valley / reach
        );
    }

    /// **This app strokes three different ✕ marks, and `icon_probe` tells
    /// the detail pane's from the other two by EXTENT alone.**
    ///
    /// The vault titlebar's window-close is painted in every frame the whole
    /// window is, and [`card_header_with_close`]'s dismiss in the overlay's
    /// -- so a pane close that matched either extent would have
    /// [`icon_probe::pane_close_marks`] reporting a control that closes the
    /// WINDOW as the one that closes the pane.
    ///
    /// Measured off the painted shapes rather than compared as constants:
    /// the titlebar's ✕ is not drawn by this module at all, so a constant
    /// comparison could only restate what this file already says.
    #[test]
    fn the_drawn_close_marks_do_not_share_an_extent() {
        let (_, pane_marks) = control(close_pane_button);
        let pane_tree = egui::Shape::Vec(pane_marks);
        let found = icon_probe::pane_close_marks(&pane_tree);
        assert_eq!(
            found.len(),
            1,
            "`close_pane_button` painted no ✕ its own probe can find"
        );

        let (_, dismiss_marks) = control(close_glyph);
        let dismiss_tree = egui::Shape::Vec(dismiss_marks);
        // The positive control: the dismiss ✕ really is two segments, so
        // "the probe found none of them" is a statement about the extent and
        // not about an empty frame.
        assert_eq!(
            icon_probe::line_segments(&dismiss_tree).len(),
            2,
            "`close_glyph` painted no two-armed ✕, so the assertion below proves nothing"
        );
        assert!(
            icon_probe::pane_close_marks(&dismiss_tree).is_empty(),
            "the card header's dismiss ✕ is being reported as the detail pane's close, \
             so `PANE_CLOSE_ARM` has collided with `close_glyph`'s own arm"
        );
    }

    /// **The circle counterpart of the test above**, and a live one: several
    /// controls in this family paint circles, and `icon_probe` tells them
    /// apart by RADIUS alone -- [`icon_probe::kebab_dots`] matches
    /// [`KEBAB_DOT_RADIUS`], `one_time_code_clocks` matches [`CLOCK_RADIUS`],
    /// and the eye's pupil must be neither. Two of them sharing a radius
    /// would report one mark's circles as another's in every frame the vault
    /// titlebar is painted in.
    ///
    /// The Preferences mark's knob used to be in this list. It is not any
    /// more, and not because the constraint relaxed: the mark became a mixer
    /// and its handles became filled blocks, so it paints no circle to
    /// collide with anything. Recorded rather than silently dropped, because
    /// a shorter list here looks like a weakened test.
    ///
    /// **Every pair, generated rather than written out**: the list used to be
    /// three hand-written rows over three radii, and adding
    /// [`CLOCK_RADIUS`] to it would have meant three more rows written by
    /// hand -- with the one that was forgotten being exactly the collision
    /// nobody would notice. The pairs come off the list of radii now, so a
    /// fifth ring adds one entry and is compared against all four.
    #[test]
    fn the_drawn_circles_do_not_share_a_radius() {
        let radii = [
            (KEBAB_DOT_RADIUS, "the kebab dot"),
            (EYE_PUPIL_RADIUS, "the eye's pupil"),
            (CLOCK_RADIUS, "the one-time code clock's face"),
        ];
        // The premise, stated so this cannot pass by comparing an empty set:
        // every radius above is really in the list, and the loop below really
        // does run over all of their pairs.
        let mut compared = 0;
        for (i, (a, a_name)) in radii.iter().enumerate() {
            for (b, b_name) in &radii[i + 1..] {
                compared += 1;
                assert!(
                    (a - b).abs() > 0.01,
                    "{a_name} and {b_name} are both radius {a}, and `icon_probe` tells these \
                     circles apart by radius alone -- so each is now findable as the other and \
                     every probe over them is reporting the wrong mark"
                );
            }
        }
        assert_eq!(
            compared,
            radii.len() * (radii.len() - 1) / 2,
            "the loop did not compare every pair of radii, so a collision could hide in the \
             pair it skipped"
        );
    }

    /// **The tune icon is stroked at the weight its neighbours are.** Read
    /// off the painted shapes rather than off [`ICON_STROKE`]: the constant
    /// being shared is only evidence that the source says so, and the eye,
    /// the chevron and this icon hand their stroke to different egui shape
    /// kinds (`Path`, `LineSegment`, `Circle`), any of which could stop
    /// honouring it.
    #[test]
    fn the_tune_icon_is_stroked_at_the_weight_the_eye_and_the_switcher_are() {
        let (_, tune) = control(tune_button);
        let (_, eye) = control(|ui| eye_toggle(ui, false));
        let (_, switcher) = control(account_switcher_button);

        let widths = |marks: &[egui::Shape]| -> Vec<f32> {
            marks.iter().filter_map(stroke_width).collect()
        };
        let tune_widths = widths(&tune);
        let eye_widths = widths(&eye);
        let switcher_widths = widths(&switcher);

        // Positive controls: all three really did paint stroked marks. Without
        // these, a control that painted nothing at all would satisfy every
        // comparison below by having no widths to disagree about.
        for (found, what) in [
            (&tune_widths, "the tune icon"),
            (&eye_widths, "the eye"),
            (&switcher_widths, "the switcher"),
        ] {
            assert!(
                !found.is_empty(),
                "{what} painted no stroked mark at all, so the weight comparison below \
                 compares nothing"
            );
        }

        let reference = eye_widths[0];
        for width in tune_widths.iter().chain(&switcher_widths).chain(&eye_widths) {
            assert!(
                (width - reference).abs() < 0.01,
                "this family is stroked at {reference} but a mark here is stroked at \
                 {width}: tune {tune_widths:?}, eye {eye_widths:?}, switcher \
                 {switcher_widths:?}"
            );
        }
    }

    /// **The tune icon is two kinds of mark, repeated no more often than the
    /// kebab repeats its dot.**
    ///
    /// The gear this replaced could be held to the eye's two marks exactly
    /// (an outline and a hub against an almond and a pupil). A tune mark
    /// cannot: it is a line and a knob *per row*, so counting raw shapes
    /// would say four and fail a test that meant "not busy". What the family
    /// actually bounds is how much the eye has to read, and the kebab
    /// already settles the idiom -- one mark, repeated three times, is not
    /// three icons. So this asserts the two things that keep the icon in
    /// that company: its vocabulary is no larger than the eye's two kinds of
    /// mark, and it repeats them fewer times than the kebab repeats its dot.
    ///
    /// **The ceiling was strict and is now inclusive**, and that is a
    /// decision rather than a slip. It read `rows < kebab.len()`, which is
    /// what kept the horizontal tune mark to two slider rows where
    /// Material's own `tune` draws three. The mark became a three-channel
    /// mixer on the owner's own instruction, so the strict form would now be
    /// this file forbidding what it was asked for. The claim that still has
    /// force is the one the kebab settles: one mark repeated three times is
    /// not three icons, and three is where that stops being true. A fourth
    /// channel still fails here.
    #[test]
    fn the_tune_icon_repeats_no_more_marks_than_the_kebab_beside_it() {
        let (_, tune) = control(tune_button);
        let (_, eye) = control(|ui| eye_toggle(ui, false));
        let (_, kebab) = control(|ui| kebab_button(ui, false));

        // Positive control: `marks_in` really does find marks, so the counts
        // below are counts of something.
        assert_eq!(
            kebab.len(),
            3,
            "the kebab is three dots and `marks_in` found {} shapes in it -- this helper \
             has stopped seeing what these controls paint, so the tune icon's count means \
             nothing either",
            kebab.len()
        );
        assert_eq!(
            eye.len(),
            2,
            "the eye is an almond and a pupil and `marks_in` found {} shapes in it",
            eye.len()
        );
        // The vocabulary: how many KINDS of shape the mark is spelled with.
        // The eye is two (a path and a circle); the tune icon is two (a
        // segment and a circle). A gradient, a fill or a glyph added to it
        // would be a third.
        let kinds = |marks: &[egui::Shape]| -> usize {
            let mut seen: Vec<std::mem::Discriminant<egui::Shape>> = Vec::new();
            for mark in marks {
                let d = std::mem::discriminant(mark);
                if !seen.contains(&d) {
                    seen.push(d);
                }
            }
            seen.len()
        };
        assert!(
            kinds(&tune) <= kinds(&eye),
            "the tune icon is spelled with {} kinds of mark against the eye's {} beside \
             it, so it carries detail no neighbour does",
            kinds(&tune),
            kinds(&eye)
        );

        // The repeats: one channel is one track plus one block, so the
        // channel count is the mark count over the vocabulary. Measured off
        // the painted frame rather than read back out of `TUNE_FADERS`,
        // which would be the constant asserting it equals itself.
        assert_eq!(
            tune.len() % kinds(&tune),
            0,
            "the tune icon painted {} marks in {} kinds, which is not a whole number of \
             slider rows -- this count has stopped matching what `tune_button` draws",
            tune.len(),
            kinds(&tune)
        );
        let rows = tune.len() / kinds(&tune);
        assert!(
            rows <= kebab.len(),
            "the mixer mark repeats its marks {rows} times against the kebab's {}, which \
             is this family's ceiling -- past it, it is again the busiest icon in a strip \
             of one- and two-mark shapes",
            kebab.len()
        );
    }

    /// **It is a mixing desk and not a fence**, measured off `tune_rows`
    /// rather than read back out of [`TUNE_FADER_OFFSETS`] -- a test that
    /// asserted the constants equal themselves would pass against any block
    /// placement at all, including every block at the same height.
    ///
    /// Two blocks at the same height is precisely the failure worth pinning:
    /// the tracks would still be tracks, but the mark would read as a fence
    /// or a grid, which is a different icon meaning a different thing.
    #[test]
    fn the_tune_blocks_sit_at_different_heights_along_their_tracks() {
        let center = Pos2::new(50.0, 50.0);
        let rows = tune_rows(center);

        // Positive control: there are rows at all, and more than one -- with
        // a single row every pairwise comparison below is vacuous.
        assert!(
            rows.len() >= 2,
            "`tune_rows` produced {} rows, so there is nothing for the knobs to differ \
             across and this icon cannot express a slider position at all",
            rows.len()
        );

        for (i, a) in rows.iter().enumerate() {
            for b in &rows[i + 1..] {
                assert!(
                    (a.knob.center().y - b.knob.center().y).abs() > 1.0,
                    "two fader blocks sit at y {} and {}, within a block's own height of \
                     each other -- level with one another this mark reads as a fence or a \
                     grid rather than as a mixing desk",
                    a.knob.center().y,
                    b.knob.center().y
                );
            }
        }

        // **And the stagger is pronounced, not merely non-zero.** The
        // reference the owner sent has one channel high, one low and one near
        // the middle; three blocks a hair apart would satisfy the loop above
        // and would still read as a fence. So the spread across all of them
        // is held to a real fraction of the track they ride.
        let highest = rows.iter().fold(f32::INFINITY, |a, r| a.min(r.knob.center().y));
        let lowest = rows.iter().fold(f32::NEG_INFINITY, |a, r| a.max(r.knob.center().y));
        let travel = rows[0].line[1].y - rows[0].line[0].y;
        assert!(
            lowest - highest > travel / 3.0,
            "the fader blocks are spread over {:.2}pt of a {travel:.2}pt track, under a \
             third of its travel -- the stagger is what makes this a mixer rather than a \
             row of posts",
            lowest - highest
        );
    }

    /// **Vertical tracks, with the blocks ON them**, and every track the same
    /// length -- the three facts that make this shape a bank of faders rather
    /// than a scatter of strokes and chips.
    ///
    /// It read "horizontal lines with the knobs on them" until the mark was
    /// turned on its side. The claim is the same one rotated, and it is
    /// asserted off `tune_rows` rather than off the constants for the reason
    /// it always was: a test over the constants would pass against any
    /// arrangement at all.
    #[test]
    fn every_tune_row_is_a_vertical_track_with_its_block_on_it() {
        let center = Pos2::new(50.0, 50.0);
        let rows = tune_rows(center);
        assert!(!rows.is_empty(), "`tune_rows` produced no tracks to measure");

        let first_length = (rows[0].line[1].y - rows[0].line[0].y).abs();
        assert!(first_length > 0.0, "the first track has no length");

        for row in &rows {
            let [top, bottom] = row.line;
            assert!(
                (top.x - bottom.x).abs() < 0.01,
                "a track runs from x {} to x {}, so it is not vertical",
                top.x,
                bottom.x
            );
            assert!(
                ((bottom.y - top.y).abs() - first_length).abs() < 0.01,
                "a track is {} long against the first channel's {first_length} -- tracks \
                 of unequal length read as a ragged list, not as one control",
                (bottom.y - top.y).abs()
            );
            assert!(
                (row.knob.center().x - top.x).abs() < 0.01,
                "a fader block sits at x {} while its track is at x {}, so it is floating \
                 beside the track rather than riding on it",
                row.knob.center().x,
                top.x
            );
            // The whole block, not just its centre, has to be on the track,
            // and with track still showing past both of its ends -- a block
            // flush with an end reads as a cap on a post rather than as a
            // fader that could travel.
            assert!(
                row.knob.top() > top.y.min(bottom.y) + 0.01
                    && row.knob.bottom() < top.y.max(bottom.y) - 0.01,
                "a fader block spanning y {}..{} reaches the end of its track, which spans \
                 {}..{}",
                row.knob.top(),
                row.knob.bottom(),
                top.y,
                bottom.y
            );
            // A cap, not a knot: taller than it is wide, and proud of the
            // track it covers on both sides.
            assert!(
                row.knob.height() > row.knob.width()
                    && row.knob.width() > ICON_STROKE * 2.0,
                "a fader block is {:.2} wide by {:.2} tall against a {ICON_STROKE} track -- \
                 a block no taller than it is wide, or no wider than the track, reads as a \
                 thickened section of line rather than as a handle",
                row.knob.width(),
                row.knob.height()
            );
        }

        // The tracks are centred on the icon's own centre, so the mark sits
        // in the middle of its 28px target rather than off to one side.
        let left = rows.iter().fold(f32::INFINITY, |a, r| a.min(r.line[0].x));
        let right = rows.iter().fold(f32::NEG_INFINITY, |a, r| a.max(r.line[0].x));
        assert!(
            ((left + right) / 2.0 - center.x).abs() < 0.01,
            "the tracks span x {left}..{right}, whose middle is not the icon's centre {}",
            center.x
        );
        // And they do not touch: adjacent blocks with no white between them
        // are one bar, not a bank of channels.
        let mut xs: Vec<f32> = rows.iter().map(|r| r.knob.center().x).collect();
        xs.sort_by(f32::total_cmp);
        for pair in xs.windows(2) {
            assert!(
                pair[1] - pair[0] > rows[0].knob.width() + 1.0,
                "two channels are {:.2}pt apart against a {:.2}pt block, so their caps all \
                 but touch and the bank reads as one bar",
                pair[1] - pair[0],
                rows[0].knob.width()
            );
        }
    }

    /// **The regression this change exists to prevent: it must not be a cog
    /// again.**
    ///
    /// A gear at this size is a closed path -- that is how every version of
    /// it was drawn here, and how `icon_probe` used to find it -- and it is
    /// the only shape kind whose corner count can read as teeth. The tune
    /// mark closes no path, so the absence is exact rather than a proxy: any
    /// return to a cog, a star, a shield or any other polygonal outline
    /// fails here.
    #[test]
    fn the_preferences_control_paints_no_closed_outline_at_all() {
        let (rect, tune) = control(tune_button);
        assert!(
            !tune.is_empty(),
            "the Preferences control painted nothing, so the absence below is not a \
             fact about its shape"
        );
        for mark in &tune {
            assert!(
                !matches!(mark, egui::Shape::Path(_)),
                "the Preferences control painted a closed outline again -- a cog, a \
                 flower or some other polygon is back where the tune mark should be"
            );
        }

        // The positive control: this harness DOES see a closed path when one
        // is painted into the same rect, so the loop above is measuring the
        // control and not a blind walker.
        let with_path = frame(|ui| {
            ui.painter().add(egui::Shape::closed_line(
                vec![rect.left_top(), rect.right_top(), rect.center()],
                Stroke::new(ICON_STROKE, INK),
            ));
        });
        assert!(
            marks_in(&with_path, rect)
                .iter()
                .any(|s| matches!(s, egui::Shape::Path(_))),
            "a closed path drawn into the control's own rect was not found either, so \
             the assertion above proves nothing"
        );
    }

    /// **The frame really paints what `tune_rows` computes.** The two tests
    /// above measure the pure function; without this one `tune_button` could
    /// stop calling it entirely and they would all still pass.
    #[test]
    fn the_painted_control_is_the_rows_that_tune_rows_computes() {
        let (rect, tune) = control(tune_button);
        let expected = tune_rows(rect.center());

        let blocks: Vec<&egui::epaint::RectShape> = tune
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Rect(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(
            blocks.len(),
            expected.len(),
            "the control painted {} fader blocks where `tune_rows` computes {}",
            blocks.len(),
            expected.len()
        );
        for (block, row) in blocks.iter().zip(&expected) {
            assert!(
                (block.rect.center() - row.knob.center()).length() < 0.01,
                "a fader block was painted at {:?} where `tune_rows` puts it at {:?}",
                block.rect.center(),
                row.knob.center()
            );
            assert!(
                (block.rect.size() - row.knob.size()).length() < 0.01,
                "a fader block was painted {:?} where `tune_rows` sizes it {:?}",
                block.rect.size(),
                row.knob.size()
            );
            assert_ne!(
                block.fill,
                Color32::TRANSPARENT,
                "a fader block was painted unfilled -- a cap that lets its track show \
                 through reads as a bulge in the line rather than as a handle"
            );
        }

        let lines = tune
            .iter()
            .filter(|s| matches!(s, egui::Shape::LineSegment { .. }))
            .count();
        assert_eq!(
            lines,
            expected.len(),
            "the control painted {lines} slider lines where `tune_rows` computes {}",
            expected.len()
        );
    }

    /// **28px, the same target the controls beside it have.** The mark got
    /// smaller and lighter; the thing the user has to hit did not.
    #[test]
    fn the_tune_icon_keeps_its_neighbours_hit_target() {
        let (gear, _) = control(tune_button);
        let (eye, _) = control(|ui| eye_toggle(ui, false));
        let (switcher, _) = control(account_switcher_button);

        assert!(
            gear.width() > 0.0 && gear.height() > 0.0,
            "the tune icon allocated nothing at all, so the comparisons below are between \
             empty rects"
        );
        assert_eq!(
            gear.size(),
            eye.size(),
            "the tune icon's hit target is {:?} against the eye's {:?}",
            gear.size(),
            eye.size()
        );
        assert_eq!(
            gear.size(),
            switcher.size(),
            "the tune icon's hit target is {:?} against the switcher's {:?}",
            gear.size(),
            switcher.size()
        );
    }

    /// **Still a stroked shape, not a codepoint.**
    /// `the_icon_codepoints_are_not_carried_by_this_apps_own_typeface`
    /// records why: the gear codepoint resolves here only out of egui's
    /// bundled icon fallback, at a weight and baseline nobody in this app
    /// chose. "Make it more minimalistic" is exactly the note that gets
    /// answered by reaching for a glyph, so the absence is pinned.
    #[test]
    fn the_preferences_control_paints_no_text() {
        let (rect, gear) = control(tune_button);
        assert!(
            !gear.iter().any(|s| matches!(s, egui::Shape::Text(_))),
            "the Preferences control painted a text shape -- it is a typed glyph again"
        );
        // The positive control: a text shape drawn in this same harness IS
        // found, so the absence above is the control's and not the walker's.
        let with_text = frame(|ui| {
            ui.put(rect, egui::Label::new("\u{2699}"));
        });
        assert!(
            marks_in(&with_text, rect)
                .iter()
                .any(|s| matches!(s, egui::Shape::Text(_))),
            "a label painted into the control's own rect produced no text shape either, so \
             the assertion above proves nothing about the control"
        );
    }
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
    ///
    /// **The fan is a `Shape::Mesh` and this used to look for three-point
    /// filled paths.** It changed with the mark: a fan of separately
    /// tessellated triangles laid a visible seam down every spoke, so
    /// `paint_star` emits one mesh instead. A mesh is a *better* anchor than
    /// the old triangles were -- nothing else in this crate paints one
    /// (images do, but through `Shape::Image`), where a filled triangle was
    /// close enough to the envelope's flap that
    /// [`the_envelope_flap_is_not_findable_as_a_star_fill`] had to exist to
    /// hold them apart.
    pub fn stars(shape: &egui::Shape) -> Vec<Star> {
        let mut fills = Vec::new();
        walk(shape, &mut fills, &|s| matches!(s, egui::Shape::Mesh(_)));
        let mut strokes = Vec::new();
        walk_paths(shape, STAR_VERTICES, &mut strokes);
        strokes
            .into_iter()
            .map(|(rect, stroke)| Star {
                rect,
                stroke,
                filled: fills.iter().any(|t| rect.expand(1.0).contains_rect(*t)),
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

    /// The "Send a record" envelopes this shape tree paints, each as the
    /// union of its body and its flap, with the colour it was stroked in --
    /// which is the only way `send_record_button`'s hover state is visible to
    /// a test, since it paints no fill and no string.
    ///
    /// **Found by its FLAP and not by its body.** A closed four-point path is
    /// the least distinctive shape in this module -- anything that ever
    /// strokes a quadrilateral becomes an envelope to a probe phrased over
    /// [`ENVELOPE_VERTICES`] alone. The flap's three points plus a
    /// transparent fill is the pair nothing else in this crate paints, so
    /// that is the anchor; the body is then the one
    /// [`ENVELOPE_VERTICES`]-point path enclosing it, and a flap with no body
    /// around it is a probe that has stopped matching what
    /// `send_record_button` draws, so it panics rather than reporting half a
    /// mark -- exactly as [`chevrons`] and [`tune_icons`] do.
    pub fn envelopes(shape: &egui::Shape) -> Vec<(Rect, Color32)> {
        let mut flaps = Vec::new();
        walk_open_paths(shape, ENVELOPE_FLAP_VERTICES, &mut flaps);
        let mut bodies = Vec::new();
        walk_paths(shape, ENVELOPE_VERTICES, &mut bodies);
        flaps
            .into_iter()
            .map(|(flap, color)| {
                let body = bodies
                    .iter()
                    .find(|(b, _)| b.expand(1.0).contains_rect(flap))
                    .unwrap_or_else(|| {
                        panic!(
                            "found an envelope flap at {flap:?} with no {ENVELOPE_VERTICES}\
                             -point body around it -- this probe has stopped matching what \
                             `send_record_button` draws"
                        )
                    });
                (body.0.union(flap), color)
            })
            .collect()
    }

    /// The subtitle's folder mark, as its outline's bounding box and the
    /// colour it was stroked in.
    ///
    /// Matched on [`FOLDER_VERTICES`] alone, which is enough here in a way it
    /// would not be for the envelope's four-point body: six is a point count
    /// nothing else in this crate paints, and `detail.rs` guards that over a
    /// whole rendered header rather than trusting the claim.
    ///
    /// The colour is reported because it is the only way a test can see that
    /// this mark rests at [`TEXT_FAINT`] -- it paints no fill and no string,
    /// the same blindness [`envelopes`] was written around.
    pub fn folder_marks(shape: &egui::Shape) -> Vec<(Rect, Color32)> {
        let mut out = Vec::new();
        walk_open_paths(shape, FOLDER_VERTICES, &mut out);
        out
    }

    /// Like [`walk_paths`], but only closed paths that are **not filled** --
    /// which is what keeps the envelope's flap clear of the star's fill
    /// triangles, since those share its point count. See [`envelopes`].
    fn walk_open_paths(shape: &egui::Shape, vertices: usize, out: &mut Vec<(Rect, Color32)>) {
        match shape {
            egui::Shape::Path(p)
                if p.closed
                    && p.points.len() == vertices
                    && p.fill == Color32::TRANSPARENT =>
            {
                let color = match p.stroke.color {
                    egui::epaint::ColorMode::Solid(color) => color,
                    egui::epaint::ColorMode::UV(_) => Color32::TRANSPARENT,
                };
                out.push((shape.visual_bounding_rect(), color));
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk_open_paths(shape, vertices, out);
                }
            }
            _ => {}
        }
    }

    /// The detail pane's close ✕, as the union of its two arms, with the
    /// colour it was stroked in.
    ///
    /// **Matched on [`PANE_CLOSE_ARM`], and that is why that number is not
    /// shared with the app's other two ✕ marks.** The vault titlebar strokes
    /// a window-close in every frame this pane is painted in, and
    /// [`line_segments`] finds it, the eye's strike and this identically. So
    /// this probe measures the extent: only a segment whose bounding box is
    /// the pane close's own diagonal is one of these.
    ///
    /// The pairing is positional -- `close_pane_button` emits its two
    /// segments back to back -- and an odd count is half a ✕, which is a
    /// probe that has stopped matching rather than a mark worth reporting.
    pub fn pane_close_marks(shape: &egui::Shape) -> Vec<(Rect, Color32)> {
        let span = PANE_CLOSE_ARM * 2.0;
        let mut arms = Vec::new();
        walk_segments(shape, span, &mut arms);
        assert!(
            arms.len() % 2 == 0,
            "found {} pane-close arms, which is not a whole number of two-armed ✕ marks \
             -- this probe has stopped matching what `close_pane_button` draws",
            arms.len()
        );
        arms.chunks(2)
            .map(|pair| (pair[0].0.union(pair[1].0), pair[0].1))
            .collect()
    }

    fn walk_segments(shape: &egui::Shape, span: f32, out: &mut Vec<(Rect, Color32)>) {
        match shape {
            egui::Shape::LineSegment { points, stroke } => {
                let rect = Rect::from_two_pos(points[0], points[1]);
                if (rect.width() - span).abs() < 0.01 && (rect.height() - span).abs() < 0.01 {
                    out.push((rect, stroke.color));
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk_segments(shape, span, out);
                }
            }
            _ => {}
        }
    }

    /// The "Add a one-time code" clock faces this shape tree paints, each as
    /// its ring's bounding box and the colour it was stroked in -- which is
    /// the only way `add_totp_button`'s hover state is visible to a test,
    /// since it paints no fill and no string.
    ///
    /// **Found by its FACE alone**, matched on [`CLOCK_RADIUS`], and that is
    /// why that number is not shared with the app's other three ring radii:
    /// this walks the same shape tree [`kebab_dots`], [`tune_icons`] and
    /// [`eyes`] walk, and the detail header paints a kebab in every frame
    /// this mark appears in. The hands are deliberately NOT part of the
    /// identification -- they are axis-aligned segments, which the tune
    /// icon's own lines and the titlebar's — also draw -- so retuning a hand
    /// cannot make the control invisible here.
    pub fn clocks(shape: &egui::Shape) -> Vec<(Rect, Color32)> {
        let mut out = Vec::new();
        walk_rings(shape, CLOCK_RADIUS, &mut out);
        out
    }

    /// The Preferences mixer marks this shape tree paints, each as the union
    /// of its tracks, with the colour they were stroked in -- which is the
    /// only way `tune_button`'s hover state is visible to a test, since the
    /// mark paints no string.
    ///
    /// **Found by its TRACKS**, which is the reverse of how the two-slider
    /// version of this mark was found: that one was identified by its ring
    /// knobs, on the same principle [`kebab_dots`] works by, and its lines
    /// were deliberately excluded because a plain horizontal segment is a
    /// shape the titlebar's own — close glyph also draws.
    ///
    /// Turning the mark vertical inverts both halves of that. The handles
    /// became filled rounded rectangles, which is the least distinctive
    /// shape in this app -- every card, pill and badge is one -- while the
    /// tracks became vertical segments of one exact length, which nothing
    /// else here draws. [`walk_tracks`] carries the full argument.
    /// [`TUNE_FADERS`] consecutive tracks are one mark.
    ///
    /// An odd remainder means half an icon was found, which is a probe that
    /// has stopped matching rather than a shape worth reporting -- so it
    /// panics, exactly as [`chevrons`] does.
    pub fn tune_icons(shape: &egui::Shape) -> Vec<(Rect, Color32)> {
        let mut tracks = Vec::new();
        walk_tracks(shape, &mut tracks);
        assert!(
            tracks.len() % TUNE_FADERS == 0,
            "found {} mixer tracks, which is not a whole number of {TUNE_FADERS}-channel \
             marks -- this probe has stopped matching what `tune_button` draws",
            tracks.len()
        );
        tracks
            .chunks(TUNE_FADERS)
            .map(|channels| {
                let rect = channels.iter().fold(Rect::NOTHING, |a, (r, _)| a.union(*r));
                (rect, channels[0].1)
            })
            .collect()
    }

    /// One mixer channel's track: a VERTICAL line segment of exactly the
    /// length [`tune_rows`] gives it.
    ///
    /// **The track is the anchor and not the block**, which is the opposite
    /// of how the knob-ringed version of this mark was found. A filled
    /// rounded rectangle is the least distinctive shape this crate paints --
    /// every card, pill and badge in the app is one -- so anchoring on the
    /// blocks would have made this probe report a control surface wherever
    /// three small rects happened to line up. A line segment whose bounding
    /// box is exactly [`ICON_STROKE`] wide and the full track long is a
    /// shape nothing else here draws: the app's other short segments are the
    /// clock's hands (much shorter), the eye's strike, the switcher's
    /// chevron and the ✕ marks (all diagonal, so none has a box this narrow).
    fn walk_tracks(shape: &egui::Shape, out: &mut Vec<(Rect, Color32)>) {
        match shape {
            egui::Shape::LineSegment { stroke, .. } => {
                let rect = shape.visual_bounding_rect();
                let long = TUNE_TRACK_HALF_HEIGHT * 2.0 + stroke.width;
                if (rect.width() - stroke.width).abs() < 0.01
                    && (rect.height() - long).abs() < 0.01
                {
                    out.push((rect, stroke.color));
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk_tracks(shape, out);
                }
            }
            _ => {}
        }
    }

    /// Stroked circles at one radius. The mixer mark no longer paints any --
    /// its handles are filled blocks -- but [`one_time_code_clocks`] still
    /// finds its face this way.
    fn walk_rings(shape: &egui::Shape, radius: f32, out: &mut Vec<(Rect, Color32)>) {
        match shape {
            egui::Shape::Circle(c)
                if (c.radius - radius).abs() < 0.01 && c.stroke.width > 0.0 =>
            {
                out.push((shape.visual_bounding_rect(), c.stroke.color));
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk_rings(shape, radius, out);
                }
            }
            _ => {}
        }
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

    /// Every account-switcher chevron in a frame, each as the union of its
    /// two strokes.
    ///
    /// In this module, and matched on [`SWITCHER_CHEVRON_ARM`] and
    /// [`SWITCHER_CHEVRON_DROP`] rather than on numbers written out again,
    /// for the reason this module exists at all: the vault titlebar this
    /// chevron lives in also strokes ✕ (two 9x9 arms) and — (one 9x0 bar) as
    /// line segments, so a test over there that used
    /// [`line_segments`] directly would find three marks and could not say
    /// which was the switcher -- and one that spelled out 4.0 x 2.6 itself
    /// would go on finding the close glyph after this shape was retuned.
    ///
    /// The pairing is positional: `account_switcher_button` emits its two
    /// segments back to back, so consecutive matching pairs are one chevron
    /// each. An odd count means half a chevron was found, which is a probe
    /// that has stopped matching rather than a shape worth reporting -- so it
    /// panics rather than silently dropping it.
    pub fn chevrons(shape: &egui::Shape) -> Vec<Rect> {
        let arms: Vec<Rect> = line_segments(shape)
            .into_iter()
            .filter(|r| {
                // `visual_bounding_rect` expands a segment by half the stroke
                // at each end, so the arm's box is one whole stroke wider and
                // taller than the arm.
                (r.width() - (SWITCHER_CHEVRON_ARM + SWITCHER_CHEVRON_STROKE)).abs() < 0.01
                    && (r.height() - (SWITCHER_CHEVRON_DROP + SWITCHER_CHEVRON_STROKE)).abs()
                        < 0.01
            })
            .collect();
        assert!(
            arms.len() % 2 == 0,
            "found {} chevron arms, which is not a whole number of chevrons -- this probe \
             has stopped matching what `account_switcher_button` draws",
            arms.len()
        );
        arms.chunks(2).map(|pair| pair[0].union(pair[1])).collect()
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

/// **The sliding bar's arithmetic** -- design turn 7's `dw-bar`.
///
/// The painting itself needs a `Painter`, which needs a `Context`; what is
/// worth pinning is not the two `rect_filled` calls but the two pure
/// functions under them, because those are where the design's numbers live.
/// A bar that drew perfectly and moved on the wrong curve, or never left the
/// track, would look exactly like a bar to every test that only checked a
/// blue rect was painted.
#[cfg(test)]
mod sliding_bar_tests {
    use super::*;

    fn track() -> Rect {
        Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(260.0, BAR_HEIGHT))
    }

    /// The design's `0% { translateX(-100%) }`: the knob opens the cycle one
    /// whole knob-width to the LEFT of the track, so its right edge is exactly
    /// the track's left edge and nothing of it is visible.
    #[test]
    fn the_knob_starts_entirely_off_the_left_edge_of_the_track() {
        let knob = bar_knob(track(), 0.0);
        assert!(
            (knob.right() - track().left()).abs() < 0.01,
            "at phase 0 the knob's right edge is at {}, not at the track's left edge {} -- so \
             the cycle starts with a stub of blue already showing instead of with an empty \
             track",
            knob.right(),
            track().left()
        );
    }

    /// And `100% { translateX(320%) }`: at the end of the cycle the knob's
    /// LEFT edge is past the track's right one, so the clip in
    /// `paint_progress_bar` hides it completely. A knob that only reached the
    /// right edge would appear to stall there once per cycle.
    #[test]
    fn the_knob_ends_entirely_off_the_right_edge_of_the_track() {
        let knob = bar_knob(track(), 1.0);
        assert!(
            knob.left() >= track().right() - 0.01,
            "at phase 1 the knob's left edge is at {}, still inside a track that ends at {} -- \
             the design's 320% carries it clear of the track before the cycle restarts",
            knob.left(),
            track().right()
        );
    }

    /// `width: 32%` of the track, at both of the design's two track widths.
    #[test]
    fn the_knob_is_the_designs_thirty_two_percent_of_whatever_track_it_is_given() {
        for width in [260.0_f32, 200.0] {
            let track = Rect::from_min_size(Pos2::ZERO, Vec2::new(width, BAR_HEIGHT));
            let knob = bar_knob(track, 0.5);
            assert!(
                (knob.width() - width * 0.32).abs() < 0.01,
                "a {width}px track got a {}px knob, not the design's 32%",
                knob.width()
            );
            assert!(
                (knob.height() - BAR_HEIGHT).abs() < f32::EPSILON,
                "the knob is {}px tall in a {BAR_HEIGHT}px track",
                knob.height()
            );
        }
    }

    /// **The cycle is `BAR_PERIOD` long and repeats.** A phase that ran off
    /// with the clock instead of wrapping would slide the knob out of the
    /// track once and never bring it back -- an indicator that stops
    /// indicating a few seconds into exactly the waits it exists for.
    #[test]
    fn the_phase_wraps_once_per_period_and_covers_the_whole_travel() {
        assert!(bar_phase(0.0).abs() < 1e-5, "the cycle does not start at 0");
        for cycle in 0..4 {
            let base = f64::from(cycle) * f64::from(BAR_PERIOD);
            assert!(
                (bar_phase(base) - bar_phase(0.0)).abs() < 1e-4,
                "second {base} is a whole number of periods in and is not back at the start of \
                 the cycle"
            );
            assert!(
                (bar_phase(base + f64::from(BAR_PERIOD) / 2.0) - 0.5).abs() < 1e-4,
                "half a period in, the eased phase is not half way -- the curve has lost the \
                 symmetry `ease-in-out` is"
            );
            assert!(
                bar_phase(base + f64::from(BAR_PERIOD) * 0.999) > 0.99,
                "at the very end of the cycle the knob has not reached the far end of its \
                 travel, so the design's 320% is never actually spent"
            );
        }
        // ...and never leaves [0, 1], which is what keeps `bar_knob`'s
        // interpolation between the design's two keyframes rather than
        // extrapolating past them.
        for step in 0..280 {
            let phase = bar_phase(f64::from(step) * 0.01);
            assert!(
                (0.0..=1.0).contains(&phase),
                "phase {phase} at t={} is outside the keyframes it interpolates",
                f64::from(step) * 0.01
            );
        }
    }

    /// **`ease-in-out`, not linear** -- the design says so, and it is the
    /// difference between a bar that reads as motion and one that reads as a
    /// marquee. Asserted as the property the easing exists for: the middle
    /// quarter of the cycle covers more travel than the first quarter.
    #[test]
    fn the_travel_is_eased_rather_than_linear() {
        let quarter = f64::from(BAR_PERIOD) / 4.0;
        let first = bar_phase(quarter) - bar_phase(0.0);
        let second = bar_phase(2.0 * quarter) - bar_phase(quarter);
        assert!(
            second > first * 1.5,
            "the first quarter of the cycle covers {first:.3} of the travel and the second \
             {second:.3}; that is a linear slide, not the design's ease-in-out"
        );
    }
}

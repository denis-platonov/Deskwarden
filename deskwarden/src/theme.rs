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
    self, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind,
    TextStyle, Ui, Vec2,
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

/// **The gap egui leaves between two stacked widgets**, set once in
/// [`apply`] and named here because a surface that has to take it AWAY --
/// bands that butt against one another, with their rule as the join -- has
/// to be able to hand it back to whatever it wraps. See
/// `sequence_builder::steps_column`.
pub const ITEM_SPACING: Vec2 = Vec2::new(8.0, 8.0);
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

/// The danger wash and its two companions: the ground, edge and ink of any
/// tinted red surface the design draws (5a's password warning band, 5b's
/// `Password` chip, 5c's `Revoked` pill, 4b's secret step).
///
/// **These three were already in the crate, three times over** --
/// `preflight_card::SECRET_FILL`/`SECRET_INK`, `scratch_window`'s
/// `BAND_FILL`/`BAND_EDGE`, `detail_edit`'s `SECRET_STEP_*` -- each a private
/// copy of the same hex written where it was first needed. They are here now
/// because [`state_pill`] is a design-system widget and a widget in this file
/// may not reach into a window's private constants. The three older copies
/// are deliberately left alone: this pass has no business editing four
/// unrelated screens, and a fourth private copy would have been the actual
/// mistake.
///
/// **Since then, one of the three has come home.** `detail_edit`'s
/// `SECRET_STEP_*` are gone and its keystroke list draws [`secret_band`]. The
/// other two are still out there and still need doing, and one of them has
/// drifted in the meantime -- see [`SECRET_INK`] for the two spellings and
/// [`secret_band`] for which caller gets which half.
pub const DANGER_WASH: Color32 = Color32::from_rgb(0xfd, 0xf3, 0xf2);

/// The caution wash and its two inks: design 5c's warning band, 4c's caution
/// note, and the first window's "this is taking a while" strip.
///
/// **The same story as [`DANGER_WASH`], one shade over.** These three were
/// already in the crate twice -- `loading_ui`'s `WARN_FILL`/`WARN_INK` and
/// `totp_add`'s `CAUTION_FILL`/`CAUTION_MARK_INK`/`CAUTION_TEXT_INK` -- each
/// a private copy of the same hex written where it was first needed. They are
/// here because design 5c's band is now drawn on a third screen, and a third
/// private copy is the point at which the pattern stops being an accident.
/// The two older copies are deliberately left alone for `DANGER_WASH`'s
/// stated reason: this pass has no business editing two unrelated screens.
///
/// Two inks and not one, because the design uses two: the glyph is drawn in
/// the darker [`CAUTION_MARK`] and the prose in [`CAUTION_INK`], which is
/// what keeps a 12px sentence on a pale amber ground readable without the
/// icon shouting.
pub const CAUTION_WASH: Color32 = Color32::from_rgb(0xfe, 0xf6, 0xe7);
/// Text on a [`CAUTION_WASH`] surface -- 5c's `color: #7a4f05`.
pub const CAUTION_INK: Color32 = Color32::from_rgb(0x7a, 0x4f, 0x05);

/// The design's one green, in the same three values.
///
/// **Green is as rationed as red.** The design uses it in exactly two places
/// -- 4d's rehearsal tick and 5b/5c's `Used` pill -- both of which mean "this
/// finished", never "this is good". `scratch_window::CHECK_GREEN` and
/// `totp_add::VALID_MARK_INK` are the two existing private copies of
/// [`DONE_MARK`], left where they are for [`DANGER_WASH`]'s reason.
pub const DONE_WASH: Color32 = Color32::from_rgb(0xe8, 0xf3, 0xec);
/// Text on a [`DONE_WASH`] surface.
pub const DONE_INK: Color32 = Color32::from_rgb(0x17, 0x67, 0x3a);
/// The tick itself, one step brighter than [`DONE_INK`] because a 2.4px
/// stroke at 10px reads lighter than type does at the same value.
pub const DONE_MARK: Color32 = Color32::from_rgb(0x1b, 0x7a, 0x3f);

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

/// The monospace family's BOLD cut -- the design's `font-weight: 700` on the
/// live one-time code, and the only place this app asks for a heavy
/// monospace. See [`system_monospace_bold`].
pub const MONO_BOLD: &str = "Consolas-Bold";
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
    system_font("consola.ttf")
}

/// **The BOLD cut of the same face**, for the one place the design asks for
/// a heavy monospace: the live one-time code, which §6c and §6d both declare
/// at `font-weight: 700`.
///
/// This family used to be a single weight -- Consolas Regular in front of
/// egui's Hack -- and a comment two functions up said so as a statement of
/// fact: "Monospace is also a single weight here; there is no lost-weight
/// problem to fix". There was: a six-digit code asking for 700 got 400, and
/// the owner read it as "6 digit code font feels samller not that bold as
/// per design".
///
/// Registered as a NAMED family rather than pushed into `Monospace`, because
/// everything else in that family -- the keycaps, the chips, the seed field,
/// the masked rows -- is correctly regular and must not move.
fn system_monospace_bold() -> Option<Vec<u8>> {
    system_font("consolab.ttf")
}

/// One face out of the system's font directory, or `None` with a line
/// saying which and why.
///
/// `None` is never a reason to fail startup: this is a cosmetic match, and
/// each caller has a face to fall back to.
fn system_font(file: &str) -> Option<Vec<u8>> {
    let system_root = std::env::var_os("SystemRoot")?;
    let path = std::path::Path::new(&system_root).join("Fonts").join(file);
    match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            log::debug!(
                "could not read {} ({e}); keeping the face already in that family",
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
    font_definitions_for(&[crate::i18n::current().fonts])
}

/// Where a language's own script comes from: the **system** faces in
/// `C:\Windows\Fonts`, as `(file, index inside the file)` candidates tried in
/// order. `regular` stands behind Archivo Regular, `semibold` behind SemiBold
/// and `bold` behind Bold and ExtraBold. Chinese, Japanese and Korean have no
/// semibold of their own on Windows, so theirs is the bold face -- a label
/// that asks for 600 still reads as a label.
///
/// **Nothing here is bundled, and nothing here is read unless the language
/// is active.** A CJK face is 14-20 MB; carrying four of them would triple
/// the installer for text most users never see, and loading them for an
/// English session would hold ~40 MB for nothing. Greek gets Segoe UI
/// because the bundled Archivo has no Greek (4 of 75 codepoints) and the
/// bundled Noto subset is Cyrillic only.
struct SystemFaces {
    regular: &'static [(&'static str, u32)],
    semibold: &'static [(&'static str, u32)],
    bold: &'static [(&'static str, u32)],
}

fn system_faces_for(need: crate::i18n::Fonts) -> Option<SystemFaces> {
    use crate::i18n::Fonts;
    Some(match need {
        Fonts::Bundled => return None,
        Fonts::Greek => SystemFaces {
            regular: &[("segoeui.ttf", 0)],
            semibold: &[("seguisb.ttf", 0), ("segoeuib.ttf", 0)],
            bold: &[("segoeuib.ttf", 0)],
        },
        Fonts::ChineseSimplified => SystemFaces {
            regular: &[("msyh.ttc", 0), ("msyh.ttf", 0), ("simsun.ttc", 0)],
            semibold: &[("msyhbd.ttc", 0), ("msyhbd.ttf", 0), ("msyh.ttc", 0)],
            bold: &[("msyhbd.ttc", 0), ("msyhbd.ttf", 0), ("msyh.ttc", 0)],
        },
        Fonts::ChineseTraditional => SystemFaces {
            regular: &[("msjh.ttc", 0), ("msjh.ttf", 0), ("mingliu.ttc", 0)],
            semibold: &[("msjhbd.ttc", 0), ("msjhbd.ttf", 0), ("msjh.ttc", 0)],
            bold: &[("msjhbd.ttc", 0), ("msjhbd.ttf", 0), ("msjh.ttc", 0)],
        },
        Fonts::Japanese => SystemFaces {
            // Medium, not Regular: Yu Gothic Regular is hairline at 13 px.
            regular: &[("YuGothM.ttc", 0), ("YuGothR.ttc", 0), ("meiryo.ttc", 0), ("msgothic.ttc", 0)],
            semibold: &[("YuGothB.ttc", 0), ("meiryob.ttc", 0), ("YuGothM.ttc", 0)],
            bold: &[("YuGothB.ttc", 0), ("meiryob.ttc", 0), ("YuGothM.ttc", 0)],
        },
        Fonts::Korean => SystemFaces {
            regular: &[("malgun.ttf", 0), ("gulim.ttc", 0)],
            semibold: &[("malgunbd.ttf", 0), ("malgun.ttf", 0)],
            bold: &[("malgunbd.ttf", 0), ("malgun.ttf", 0)],
        },
    })
}

/// A face's vertical metrics in em, from its `hhea` table: `(ascent, descent,
/// line gap)`, descent as a positive distance. `index` picks the member of a
/// `.ttc` collection. Read by hand because the only consumer is
/// [`baseline_shift_factor`] and a parser dependency for three integers is
/// more than the job.
fn sfnt_vertical_em(bytes: &[u8], index: u32) -> Option<(f32, f32, f32)> {
    let u16_at = |o: usize| bytes.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let i16_at = |o: usize| u16_at(o).map(|v| v as i16);
    let u32_at = |o: usize| bytes.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let base = if bytes.get(0..4)? == b"ttcf" { u32_at(12 + 4 * index as usize)? as usize } else { 0 };
    let tables = u16_at(base + 4)? as usize;
    let (mut head, mut hhea) = (None, None);
    for i in 0..tables {
        let record = base + 12 + 16 * i;
        let offset = u32_at(record + 8)? as usize;
        match bytes.get(record..record + 4)? {
            b"head" => head = Some(offset),
            b"hhea" => hhea = Some(offset),
            _ => {}
        }
    }
    let (head, hhea) = (head?, hhea?);
    let per_em = u16_at(head + 18)? as f32;
    if per_em <= 0.0 {
        return None;
    }
    Some((
        i16_at(hhea + 4)? as f32 / per_em,
        -(i16_at(hhea + 6)? as f32) / per_em,
        i16_at(hhea + 8)? as f32 / per_em,
    ))
}

/// **How far, as a fraction of the font size, a fallback face's glyphs must be
/// drawn down to sit on Archivo's baseline.** Zero or negative means up.
///
/// egui puts a glyph's baseline at `face ascent + (row height of the FIRST
/// face in the family - the face's own row height) / 2`: it centres the
/// difference between the two faces' line boxes, which lines the baselines up
/// only when both faces split their line box the same way. Yu Gothic reserves
/// 0.88 above, 0.22 below and a 0.5 em gap, so its glyphs came out 0.255 em --
/// 3-4 px at body size -- above the Latin beside them; Segoe UI and Malgun
/// Gothic sit 0.08 em low. Rather than hard-code a number per face, this
/// solves the same equation egui does for whichever face was found, so the
/// Meiryo and MS Gothic fallbacks are right too.
fn baseline_shift_factor(face: &[u8], index: u32) -> f32 {
    let (Some(archivo), Some(other)) = (sfnt_vertical_em(ARCHIVO_FACES[0].3, 0), sfnt_vertical_em(face, index)) else {
        return 0.0;
    };
    let height = |m: (f32, f32, f32)| m.0 + m.1 + m.2;
    archivo.0 - (other.0 + 0.5 * (height(archivo) - height(other)))
}

/// The Greek codepoints Archivo maps (U+0394 Delta, U+03A9 Omega, U+03BC mu,
/// U+03C0 pi) -- four of 75, so in a Greek session those four letters came
/// from Archivo and the rest from Segoe UI: two typefaces, two sizes and two
/// baselines in one word.
const ARCHIVO_GREEK: [u16; 4] = [0x394, 0x3a9, 0x3bc, 0x3c0];

/// `font` with the BMP codepoints in `unmap` mapped to `.notdef` in every
/// format-4 `cmap` subtable. egui treats a `.notdef` answer as "this face does
/// not have it" and asks the next face in the family, which is the point: the
/// letter is then drawn by whoever draws the rest of the script. Only
/// single-codepoint segments with no glyph-id array are rewritten (that is how
/// Archivo stores these four); anything else is left alone.
fn unmap_codepoints(font: &[u8], unmap: &[u16]) -> Vec<u8> {
    let mut out = font.to_vec();
    let u16_at = |b: &[u8], o: usize| b.get(o..o + 2).map(|v| u16::from_be_bytes([v[0], v[1]]));
    let u32_at = |b: &[u8], o: usize| b.get(o..o + 4).map(|v| u32::from_be_bytes([v[0], v[1], v[2], v[3]]));
    let Some(tables) = u16_at(font, 4) else { return out };
    let cmap = (0..tables as usize).find_map(|i| {
        let record = 12 + 16 * i;
        (font.get(record..record + 4)? == b"cmap").then(|| u32_at(font, record + 8)).flatten()
    });
    let Some(cmap) = cmap.map(|o| o as usize) else { return out };
    let subtables = u16_at(font, cmap + 2).unwrap_or(0) as usize;
    for i in 0..subtables {
        let Some(offset) = u32_at(font, cmap + 4 + 8 * i + 4) else { continue };
        let table = cmap + offset as usize;
        if u16_at(font, table) != Some(4) {
            continue;
        }
        let Some(segments) = u16_at(font, table + 6).map(|v| v as usize / 2) else { continue };
        let ends = table + 14;
        let starts = ends + 2 * segments + 2;
        let deltas = starts + 2 * segments;
        let ranges = deltas + 2 * segments;
        for seg in 0..segments {
            let (Some(end), Some(start), Some(range)) =
                (u16_at(font, ends + 2 * seg), u16_at(font, starts + 2 * seg), u16_at(font, ranges + 2 * seg))
            else {
                continue;
            };
            if start == end && range == 0 && unmap.contains(&start) {
                // (code + delta) mod 65536 == 0
                let delta = 0u16.wrapping_sub(start).to_be_bytes();
                out[deltas + 2 * seg..deltas + 2 * seg + 2].copy_from_slice(&delta);
            }
        }
    }
    out
}

/// The first of `candidates` that is a readable file, as egui font data.
fn first_system_face(candidates: &[(&str, u32)]) -> Option<egui::FontData> {
    candidates.iter().find_map(|(file, index)| {
        system_font(file).map(|bytes| {
            let shift = baseline_shift_factor(&bytes, *index);
            let mut data = egui::FontData::from_owned(bytes);
            data.index = *index;
            data.tweak.y_offset_factor = shift;
            data
        })
    })
}

/// Adds the system faces for `needs` to `fonts`, behind each Archivo cut and
/// its Noto Cyrillic face and ahead of egui's defaults -- the same slot the
/// Cyrillic faces took, for the same reason: Latin keeps resolving to Archivo
/// exactly as it does, and the script this language needs stops falling
/// through to egui's one light face (or to nothing at all, for CJK).
///
/// A face that cannot be read is skipped with a debug line; the language then
/// draws what egui's fallbacks can, which for CJK is empty boxes -- visible,
/// and not a reason to refuse to start.
fn add_system_script_faces(fonts: &mut egui::FontDefinitions, needs: &[crate::i18n::Fonts]) {
    let mut slot = 2;
    let mut loaded: Vec<crate::i18n::Fonts> = Vec::new();
    for need in needs {
        if loaded.contains(need) {
            continue;
        }
        loaded.push(*need);
        let Some(faces) = system_faces_for(*need) else { continue };
        let tag = format!("{need:?}");
        let mut register = |suffix: &str, candidates: &[(&str, u32)]| -> Option<String> {
            let data = first_system_face(candidates)?;
            let name = format!("System-{tag}-{suffix}");
            fonts.font_data.insert(name.clone(), Arc::new(data));
            Some(name)
        };
        let regular = register("Regular", faces.regular);
        let semibold = register("SemiBold", faces.semibold);
        // Bold and SemiBold often resolve to the same file; registering it
        // twice would hold it twice, so the second asks for the first's entry.
        let bold = if faces.bold == faces.semibold { semibold.clone() } else { register("Bold", faces.bold) };
        for (family, face) in [
            (REGULAR, &regular),
            (SEMIBOLD, &semibold),
            (BOLD, &bold),
            (EXTRABOLD, &bold),
        ] {
            let Some(face) = face else { continue };
            let key = if family == REGULAR {
                FontFamily::Proportional
            } else {
                FontFamily::Name(family.into())
            };
            if let Some(stack) = fonts.families.get_mut(&key) {
                let at = slot.min(stack.len());
                stack.insert(at, face.clone());
            }
        }
        slot += 1;
        // Text in the monospace family -- a username or a note in Japanese --
        // would otherwise draw boxes: the faces go on the END, behind Consolas
        // and Hack, so nothing Latin or numeric moves.
        for (family, face) in [(FontFamily::Monospace, &regular), (FontFamily::Name(MONO_BOLD.into()), &bold)] {
            if let (Some(face), Some(stack)) = (face, fonts.families.get_mut(&family)) {
                stack.push(face.clone());
            }
        }
    }
}

/// [`font_definitions`] with the faces of `needs` in place of the active
/// language's alone. The picker uses it so a language's own name -- Japanese
/// 日本語, Greek Ελληνικά -- is drawn in its own script before that language
/// is the one in force.
pub(crate) fn font_definitions_for(needs: &[crate::i18n::Fonts]) -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    let greek = needs.contains(&crate::i18n::Fonts::Greek);
    for (family, _, _, bytes) in ARCHIVO_FACES {
        let data = if greek {
            // Greek is drawn from ONE face: Archivo's four stray Greek letters
            // are handed over to the system face behind it. See ARCHIVO_GREEK.
            egui::FontData::from_owned(unmap_codepoints(bytes, &ARCHIVO_GREEK))
        } else {
            egui::FontData::from_static(bytes)
        };
        fonts.font_data.insert(family.to_owned(), Arc::new(data));
    }

    for (_, noto, bytes) in CYRILLIC_FACES {
        let mut data = egui::FontData::from_static(bytes);
        data.tweak.y_offset_factor = baseline_shift_factor(bytes, 0);
        fonts.font_data.insert(noto.to_owned(), Arc::new(data));
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

    // **A named family of its own**, and the regular monospace behind it: a
    // machine without `consolab.ttf` gets the code at the weight it has
    // always had rather than a tofu column. See [`system_monospace_bold`].
    let mut bold_mono = vec![];
    if let Some(bytes) = system_monospace_bold() {
        fonts.font_data.insert(
            MONO_BOLD.to_owned(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
        bold_mono.push(MONO_BOLD.to_owned());
    }
    bold_mono.extend(
        fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default(),
    );
    fonts.families.insert(FontFamily::Name(MONO_BOLD.into()), bold_mono);

    add_system_script_faces(&mut fonts, needs);

    fonts
}

/// Re-installs the fonts for the language in force **plus** the scripts in
/// `extra` (`Fonts::Japanese`, ...), on a context that has already had
/// [`apply`]. The Settings language picker calls it with every language it
/// lists so each name is drawn in its own script, and again with an empty
/// slice when it closes. Like any `set_fonts`, it goes live at the start of
/// the next pass.
pub fn set_fonts_for(ctx: &egui::Context, extra: &[crate::i18n::Fonts]) {
    let mut needs = vec![crate::i18n::current().fonts];
    needs.extend_from_slice(extra);
    ctx.set_fonts(font_definitions_for(&needs));
}

/// `RichText` in Archivo SemiBold — the design's 600 weight.
/// `RichText` in Archivo Regular — the design's 400 weight, and the one every
/// sentence in this app is meant to be set in.
///
/// **Spelled out even though it is the default.** [`FontFamily::Proportional`]
/// already resolves to [`REGULAR`], so `RichText::new(..).size(..)` produces
/// exactly this and the function adds nothing the compiler can see.
///
/// What it adds is to the reader. Body weight was previously an ABSENCE --
/// the one call in a run of `semibold(..)` that did not say a family -- and an
/// absence is not something a reviewer notices missing. `loading_ui` set every
/// string it owned in SemiBold, headings and body copy alike, and it took two
/// reports about one sentence ("wrong font", then "bold for some reason") to
/// find it. Written down, the odd one out is the one that says `semibold`, and
/// `the_body_copy_on_every_loading_screen_is_body_weight` can hold the rule.
pub fn regular(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text.into()).size(size).family(FontFamily::Proportional)
}

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
    letterspaced_in(text, FontId::new(size, FontFamily::Monospace), tracking, color, None)
}

/// [`letterspaced_mono`] in a line box of the caller's choosing.
///
/// For the one thing a `LayoutJob` cannot be given afterwards the way a
/// `RichText` can. Every caller here passes [`ascent_of`]'s answer, which is
/// the rule this app follows for a single line that has to sit centred
/// against something beside it -- see that function for why.
pub fn letterspaced_mono_in(
    text: &str,
    size: f32,
    tracking: f32,
    color: Color32,
    line_height: f32,
) -> egui::text::LayoutJob {
    letterspaced_in(
        text,
        FontId::new(size, FontFamily::Monospace),
        tracking,
        color,
        Some(line_height),
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
///
/// # And on every frame, for the two windows that can host the region overlay
///
/// The reasoning above is about the FIRST frame, and it was written when
/// leaving later frames unpainted was harmless: `eframe` cleared them to
/// `epi::App::clear_color`'s near-black default, and the two windows in
/// question paint over essentially all of it anyway. That is no longer true of
/// the vault window (`vault_window::run`) or of the single startup window it
/// grows out of (`app_window::run_the_one_window`). Both now open
/// `with_transparent(true)` and clear to nothing at all -- see
/// [`crate::window_host`] for why design 6b's region overlay needs that -- so
/// on those two windows an unpainted pixel is a SEE-THROUGH pixel.
///
/// The reasoning is unchanged and extended rather than replaced: the first
/// frame still paints this because the fonts are not live yet, and every later
/// frame paints it because the window's opacity is now the window's own
/// responsibility rather than a side effect of what `eframe` happened to clear
/// to. Hoisted to the top of both closures so it covers the first frame and the
/// rest with one call -- one `rect_filled` per frame, which is nothing beside
/// the item list drawn on top of it.
///
/// **Since the 18 px corners, `window_host` calls this on every frame for every
/// frameless window**, before the window's own closure runs; the calls in the
/// window modules are now redundant for the first frame and harmless. The
/// corners follow [`window_corner_radius`].
pub fn paint_window_background(ui: &Ui) {
    ui.painter().rect_filled(
        ui.max_rect(),
        CornerRadius::same(window_corner_radius(ui.ctx())),
        crate::kit::tokens::palette(ui.ctx()).window_bg,
    );
}

/// **The radius of a main window's outer corners, in points: 18, the same
/// value the daemon's fill-time cards use** (`panel_draw::RADIUS` there).
///
/// Windows 11's own `DWMWCP_ROUND` is a fixed ~8 px that no API lets a window
/// change, so these windows ask DWM for none (`DWMWCP_DONOTROUND`), open
/// transparent, and cut their own corners: [`window_corner_radius`] says
/// where, and `window_host::CornerMask` does the cutting, antialiased, on the
/// GPU.
pub const WINDOW_RADIUS: u8 = 18;

/// The radius to draw this window's outer corners at right now:
/// [`WINDOW_RADIUS`], or **0 while the window is maximized or fullscreen** --
/// a window that fills the screen has its corners at the screen's, and a
/// rounded corner there is a hole in the edge of the display. Snapped windows
/// are not "maximized" to the OS and keep their corners, as Windows 11's own
/// do.
pub fn window_corner_radius(ctx: &egui::Context) -> u8 {
    let (maximized, fullscreen) = ctx.input(|i| {
        let viewport = i.viewport();
        (
            viewport.maximized.unwrap_or(false),
            viewport.fullscreen.unwrap_or(false),
        )
    });
    if maximized || fullscreen {
        0
    } else {
        WINDOW_RADIUS
    }
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

    style.spacing.item_spacing = ITEM_SPACING;
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
    // `ui.indent` is used to inset text, never to show nesting, and egui's
    // default draws a grey guide line down its left edge, which hung beside
    // the Sends pane's empty, loading and failed notices as a stray bar.
    v.indent_has_left_vline = false;
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
    // The one state that kept egui's own 2-point corner: a drop-down's button
    // while its list is open drew square beside the rounded boxes either side
    // of it. The owner, with the folder list open: "when selected - it is not
    // rounded".
    v.widgets.open.corner_radius = CornerRadius::same(7);

    // Anything that opens an egui menu without going through `kit::menu` still
    // wears the design's popover: the same corner, padding, shadow and scroll
    // bar `kit::menu::MENU` applies to its own.
    v.menu_corner_radius = CornerRadius::same(crate::kit::menu::MENU.radius);
    v.popup_shadow = crate::kit::menu::MENU.shadow(&crate::kit::tokens::Palette::LIGHT);
    style.spacing.menu_margin = egui::Margin::same(crate::kit::menu::MENU.pad);
    style.spacing.scroll = scroll_style();

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
///
/// **Kept on the v2 shape (owner decision, 2026-11-18).** The v2 brand artboard
/// lays the four values out in palette order again (Deep, Brand / Bright,
/// Light). The owner chose the v2 geometry with these checkerboard fills, for
/// the reason above, which holds just as much for the wider v2 body.
/// **Public because the mark is painted by two renderers, not one.**
/// `unlock_prompt` draws the same shield through GDI `Polygon` with no egui
/// anywhere in the process, and it reads the fills and the outlines from here
/// rather than restating them. Two copies of a brand mark that must agree is
/// the same defect shape this crate's palette constants exist to prevent, and
/// a cross-renderer copy is the version of it nobody would notice drifting.
pub const QUADRANT_FILLS: [Color32; 4] = [BLUE_DEEP, BLUE_BRIGHT, BLUE_SOFT, BLUE];

/// The mark reversed onto a navy or brand-blue ground (design 1c's app-icon
/// tiles, and the sign-in, lock and splash screens): white, then the three
/// lighter blues. Reading order, as [`QUADRANT_FILLS`].
pub const MARK_REVERSED: [Color32; 4] = [Color32::WHITE, BLUE_EDGE, BLUE_SOFT, BLUE_BRIGHT];

/// The mark in the v2 sidebar's lockup: `q1` (white on Navy, deep blue on
/// Light) then bright, soft and edge blue, as design 1a draws it.
pub const fn mark_on_sidebar(q1: Color32) -> [Color32; 4] {
    [q1, BLUE_BRIGHT, BLUE_SOFT, BLUE_EDGE]
}

/// The v2 shield's top-left quadrant (design 1c), exactly as the design's
/// SVG draws it. The v2 shape is wider than v1's (x 0.5..23.5 against 2..22),
/// rounds its top corners at 5 rather than 2.4, and reaches a deeper tip
/// (y 27.2 against 26).
pub const MARK_TL: &str = "M12 1H5.5A5 5 0 0 0 .5 6v8H12Z";
/// The v2 shield's top-right quadrant.
pub const MARK_TR: &str = "M12 1h6.5a5 5 0 0 1 5 5v8H12Z";
/// The v2 shield's bottom-left quadrant.
pub const MARK_BL: &str = "M.5 14H12v13.2C5 25.4.9 21 .5 14Z";
/// The v2 shield's bottom-right quadrant.
pub const MARK_BR: &str = "M12 14h11.5c-.4 7-4.5 11.4-11.5 13.2Z";

/// The four quadrant outlines of the mark, in the design's 24×28 SVG
/// coordinate space: [`MARK_TL`], [`MARK_TR`], [`MARK_BL`] and [`MARK_BR`],
/// read by [`crate::kit::icon::parse`]. Each is convex (a rectangle with one
/// rounded or one curved corner), which is what lets `paint_mark` use
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
    [MARK_TL, MARK_TR, MARK_BL, MARK_BR].map(|d| {
        let mut paths = crate::kit::icon::parse(d).expect("the mark's path data is the design's own");
        assert_eq!(paths.len(), 1, "each quadrant is one closed shape");
        paths.remove(0).points
    })
}

/// Paints the full-color quartered-shield mark filling `rect` (preserving the
/// 24:28 aspect ratio, centered).
pub fn paint_mark(painter: &egui::Painter, rect: Rect) {
    paint_mark_with(painter, rect, None)
}

fn paint_mark_with(painter: &egui::Painter, rect: Rect, tint: Option<Color32>) {
    paint_mark_colours(painter, rect, tint.map_or(QUADRANT_FILLS, |t| [t; 4]));
}

/// Paints the mark into `rect` with each quadrant in its own colour, in
/// reading order: [`QUADRANT_FILLS`], [`MARK_REVERSED`] or
/// [`mark_on_sidebar`].
pub fn paint_mark_colours(painter: &egui::Painter, rect: Rect, fills: [Color32; 4]) {
    let scale = (rect.width() / MARK_ARTBOARD.0).min(rect.height() / MARK_ARTBOARD.1);
    let origin = rect.center() - Vec2::new(12.0 * scale, 14.0 * scale);
    for (outline, fill) in quadrant_outlines().iter().zip(fills) {
        let points: Vec<Pos2> = outline
            .iter()
            .map(|p| origin + Vec2::new(p.x * scale, p.y * scale))
            .collect();
        painter.add(egui::Shape::convex_polygon(points, fill, Stroke::NONE));
    }
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

/// The top and the bottom of a laid-out run's INK, measured down from the top
/// of its own box.
///
/// [`ink_center_y`] is the midpoint of this band and is expressed in terms of
/// it, so the two cannot come to disagree about where a run's ink is.
///
/// The BAND rather than only its centre is what [`crate::card_mark`] reads:
/// it places a brand logo so that the logo's ink centre lands on the ink
/// centre of the wordmark it replaced, which is what keeps a row of mixed
/// marks -- some logos, some words -- on one line. (It once also scaled the
/// logo to this band's HEIGHT; that made every logo the cap height of small
/// type, and a logo now fills its box instead.)
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
/// design's window mocks (3h shows the mark in the titlebar). 64px with 4×
/// supersampling, same approach as `assets/generate-icon.py` — this one is
/// generated at runtime because eframe windows take an `egui::IconData`, not
/// the .ico resource `build.rs` embeds for the exe itself.
///
/// **64, not 32**: the taskbar wants 40-48 px at 125-150% scaling, and a 32
/// px icon is stretched up to that, soft beside the exe's own sharp frames.
/// Scaling down from 64 stays crisp.
pub fn window_icon() -> egui::IconData {
    const SIZE: usize = 64;
    const SS: usize = 4;
    let outlines = quadrant_outlines();

    let mut rgba = Vec::with_capacity(SIZE * SIZE * 4);
    for py in 0..SIZE {
        for px in 0..SIZE {
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    // The 24×28 viewbox spread over the whole square, as
                    // `assets/generate-icon.py` draws the exe's icon: the
                    // shield fills the icon's width rather than keeping the
                    // viewbox's proportions inside it. Kept to the exact
                    // aspect it read slim beside the exe's own icon on the
                    // taskbar, and the full one is the one chosen.
                    let nx = (px * SS + sx) as f32 + 0.5;
                    let ny = (py * SS + sy) as f32 + 0.5;
                    let p = Pos2::new(
                        nx / (SIZE * SS) as f32 * 24.0,
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

/// The monogram's cap height as a fraction of its tile: **16pt in a 40pt
/// tile**, set by the owner alongside the tile and the artwork box in one
/// breath -- "let's make those - 40, 24, 24, 16".
///
/// A fraction rather than a bare 16, for the same reason [`ARTWORK_BOX`] is:
/// the tile has been resized three times, and a letter size that stayed put
/// while the tile moved would change how the monogram sits in it without
/// anyone editing this line.
pub(crate) const MONOGRAM: f32 = 0.4;

/// The square every favicon is fitted into, centred in its [`avatar_tile`]:
/// **24pt**, which is three fifths of the 40pt tile.
///
/// Expressed as a fraction of the tile rather than as a bare 16.0 so the two
/// cannot drift apart -- the tile has been resized once already, and an
/// artwork box that stayed at 16 while the tile moved would change the
/// padding the owner asked for without anyone editing this line.
///
/// The box was tried at 16, then 24, and settled at 20. It was tried at 16 -- the size most sites actually serve, so
/// the common favicon would be drawn at its own pixels with nothing resampled
/// at all. Seen running, the owner asked for 24. The trade is deliberate: a
/// 16x16 source is now magnified 1.5x and gives up some sharpness, and what
/// it buys is that every icon in the column reads at a usable size. A source
/// SMALLER than the box is still never magnified past its own size (see
/// [`avatar_artwork`], which fits the box in both directions), so the column
/// is ONE icon size rather than every size the web happens to serve.
pub(crate) const ARTWORK_BOX: f32 = 0.6;

fn artwork_box(tile: Rect) -> f32 {
    tile.width().min(tile.height()) * ARTWORK_BOX
}

/// Where the artwork goes inside an [`avatar_tile`], and what corner radius it
/// is clipped with: [`avatar_image`]'s geometry, split out as a pure function
/// so it can be asserted on directly rather than only inferred from a paint.
///
/// `source` is the texture's own size in pixels, which is read as POINTS. That
/// is the whole meaning of "a 16x16 favicon draws at 16x16": the number of
/// pixels the artwork has is the number of points it is given, so the tile's
/// look does not change between a 100% and a 200% monitor. `favicon::
/// decode_rgba` is what makes that number trustworthy -- it never magnifies a
/// source, and it letterboxes a non-square one onto a transparent square
/// canvas, so the size handed here is the artwork's real size and not a
/// stretched one.
///
/// **Two rules, and the second is the one with a history.**
///
/// **One rule: fit [`ARTWORK_BOX`], centred -- not the tile, and not the
/// source's own size.** Every favicon lands in the same 24pt square at the
/// tile's centre whatever its source is: a 64x64 is reduced into it, a 16x16
/// is magnified into it.
///
/// **The magnifying half is deliberate and was arrived at last.** Two earlier
/// rules capped the scale at 1.0 so a small source kept its own pixels, and
/// both produced the same defect: the artwork's size became the SOURCE's
/// size, so a column of favicons drew at every width the web happens to serve
/// and the big ones reached the tile's edge. The owner, on that: "icon
/// touches the edges but should be in the middle with paddings", then "make
/// icon 16px", then -- seeing the column at one size and liking it -- "make
/// it 24px". A uniform size is what makes the tiles read as a column, and it
/// costs a 1.5x magnification on the 16x16 sources that are still the most
/// common thing a site serves. That cost is now a decision, not an oversight.
///
/// **The corner radius follows the tile's curve, concentrically.** The tile is
/// a rounded rectangle, and the original reason the artwork was ever given a
/// radius stands: an image reaching the tile's bounds with square corners
/// pokes four corners out past the rounding. But that reason only applies
/// where the artwork actually reaches the curve. An inner rect inset by `g`
/// from a rounded outer rect of radius `R` follows the same curve at radius
/// `R - g`, so that is what is used: it is exactly `R` at full bleed (the old
/// behaviour, unchanged), it tapers as the artwork pulls in, and it reaches 0
/// once the artwork is further inside than the corner arc -- at which point
/// square corners are correct and rounding them would be inventing a shape
/// the artwork does not have. The inset is taken as the SMALLER of the two
/// axes' insets, because the axis where the artwork comes closest to the edge
/// is the one that can overhang.
///
/// A degenerate source (either axis at or below zero) falls back to the whole
/// tile, which is what this drew before and cannot divide by zero.
pub fn avatar_artwork(tile: Rect, source: Vec2, pixels_per_point: f32) -> (Rect, CornerRadius) {
    let full = avatar_corner_radius(tile.width());
    if source.x <= 0.0 || source.y <= 0.0 {
        return (tile, full);
    }
    // Fit ARTWORK_BOX rather than the tile, in BOTH directions: uncapped, so
    // a source smaller than the box is magnified into it. See the docs.
    let box_side = artwork_box(tile);
    let scale = (box_side / source.x).min(box_side / source.y);
    let art = centre_on_pixel_grid(tile, source * scale, pixels_per_point);
    let inset = (tile.width() - art.width()).min(tile.height() - art.height()) / 2.0;
    let radius = (f32::from(full.nw) - inset).max(0.0).round() as u8;
    (art, CornerRadius::same(radius))
}

/// Centres a `size`-sized rect in `tile` **counting in whole device pixels**,
/// so the gap either side is the same whole number of them.
///
/// **The report this exists for, twice: "Amazon has 5 pixels to the left and
/// 6 to the right", and the same top to bottom.** Snapping the tile to the
/// pixel grid was the first answer and it was not enough, which is the useful
/// part of the story: the tile can start on an exact pixel and the gap still
/// be fractional, because the gap is `(32 - 20) / 2 = 6` POINTS, and six
/// points is 7.5 pixels at 125% scaling and 9 at 150%. A gap of seven and a
/// half pixels cannot be equal on both sides -- one of them gets the half.
///
/// So the arithmetic is done in pixels from the start: the tile's edge, its
/// span and the artwork's span are each rounded to whole pixels, and then --
/// the step that actually matters -- the LEFTOVER is forced even by giving
/// the artwork one more pixel when it is odd. An even leftover splits
/// exactly. The result is converted back to points for `egui`, which is the
/// only place a fraction is allowed to reappear, and it reappears identically
/// on both sides.
///
/// Growing the artwork rather than shrinking it is arbitrary in the same way
/// either choice would be; it is a single pixel, and taking one is as visible
/// as adding one. What is not arbitrary is doing it at all, because a
/// symmetric layout of an odd leftover does not exist.
///
/// A non-positive `pixels_per_point` has no grid to speak of, and the plain
/// centred rect is returned.
fn centre_on_pixel_grid(tile: Rect, size: Vec2, pixels_per_point: f32) -> Rect {
    if pixels_per_point <= 0.0 {
        return Rect::from_center_size(tile.center(), size);
    }
    let axis = |min: f32, tile_span: f32, art_span: f32| {
        let min_px = (min * pixels_per_point).round();
        let tile_px = (tile_span * pixels_per_point).round();
        let mut art_px = (art_span * pixels_per_point).round().min(tile_px);
        if (tile_px - art_px) % 2.0 != 0.0 {
            art_px = (art_px + 1.0).min(tile_px);
        }
        let start = min_px + (tile_px - art_px) / 2.0;
        (start / pixels_per_point, art_px / pixels_per_point)
    };
    let (x, w) = axis(tile.left(), tile.width(), size.x);
    let (y, h) = axis(tile.top(), tile.height(), size.y);
    Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
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
    let bg = if emphasized { BLUE_WASH } else { CANVAS };
    avatar_box(ui, size, emphasized, Some(bg))
}

/// The same box drawn for a FAVICON: allocated, bordered and rounded, with
/// **no fill at all**.
///
/// **Why the fill goes when there is artwork in the box.** [`avatar_tile`]'s
/// ground exists for the monogram -- a letter needs something to sit on, and
/// `CANVAS` is what makes the tile read as a tile. A favicon does not: it is
/// its own artwork, most of them carry their own background, and the ones that
/// do not are transparent on purpose. Filling behind them put a grey square
/// inside the tile that the icon then sat in the middle of, which is what the
/// owner was looking at -- "no backgound inside of tile". Left unfilled, the
/// icon sits on the pane's own white with only the tile's edge around it.
///
/// The border still comes from [`avatar_tile_stroke`], so a selected row's
/// tile is still edged in blue; it is only the WASH that goes. The selection
/// is carried by the row behind it either way.
pub fn avatar_artwork_tile(ui: &mut Ui, size: f32, emphasized: bool) -> Rect {
    avatar_box(ui, size, emphasized, None)
}

/// The shared body of [`avatar_tile`] and [`avatar_artwork_tile`]: one
/// allocation, one rounding and one border, so the two cannot drift into
/// different geometry while claiming to be the same tile.
/// **`StrokeKind::Inside`, and that word is the whole fix for "Amazon has 5
/// pixels to the left and 6 to the right".**
///
/// The artwork was centred correctly the entire time. A 20pt icon in a 32pt
/// tile leaves 6pt a side, and at this display's 100% scaling that is 6 whole
/// pixels a side. What was not centred was the EDGE the owner was measuring
/// from. `StrokeKind::Middle` straddles the rectangle: a 1px border on a tile
/// starting at x = 79 covers 78.5 to 79.5, so it renders as two
/// half-covered pixels rather than one solid one, and the clear space between
/// that smear and the artwork is 5.5px -- which resolves to 5 down one side
/// and 6 down the other. The report was exact, and it was about the border.
///
/// Drawn inside, the border occupies 79 to 80 exactly, one crisp pixel, and
/// the clear space is 5 on both sides. Nothing else moved: the tile is still
/// 32, the artwork is still 20pt at the same coordinates, and the two earlier
/// attempts at this -- snapping the tile, then centring in whole pixels --
/// both stand, because they fix the same defect at the scale factors where it
/// is real (125%, 150%) and this display is not one of them.
fn avatar_box(ui: &mut Ui, size: f32, emphasized: bool, fill: Option<Color32>) -> Rect {
    let (allocated, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let rect = snap_to_pixels(ui, allocated);
    let rounding = avatar_corner_radius(size);
    if let Some(bg) = fill {
        ui.painter().rect_filled(rect, rounding, bg);
    }
    ui.painter()
        .rect_stroke(rect, rounding, avatar_tile_stroke(emphasized), StrokeKind::Inside);
    rect
}

/// Moves `rect` so its top-left sits on a whole PHYSICAL pixel, keeping its
/// size exactly.
///
/// **The report: "Amazon has 5 pixels to the left and 6 to the right", and
/// the same 5/6 top to bottom.** That is a half-pixel, not a centring bug.
/// The artwork is centred in the tile to within a hundredth of a point, but
/// the tile itself is placed by the surrounding layout, which has no reason to
/// land on a whole pixel -- a row's available width divided among a rail, a
/// gap and a text column is fractional far more often than not. With the tile
/// at x = 79.33, a symmetric 6pt gap becomes 5.67 on the left and 6.33 on the
/// right, and the rasteriser resolves those to 5 and 6. The icon really is
/// centred; the pixel grid is what is off.
///
/// So the tile is snapped before anything is drawn into it. Snapping the
/// TILE rather than the artwork is what makes both gaps whole: move the
/// artwork alone and the gaps stay unequal, only differently. The tile's
/// side and the artwork's are both even numbers of points, so once the tile
/// starts on a pixel the artwork's edges land on pixels too.
///
/// Rounded in PHYSICAL pixels, not points: at 125% or 150% scaling a whole
/// point is not a whole pixel, and rounding to points there would leave
/// exactly the fringe this removes. The allocation is deliberately left
/// alone -- layout keeps its own fractional geometry, and only the paint is
/// snapped, so nothing downstream shifts by up to half a pixel per row and
/// accumulates.
fn snap_to_pixels(ui: &Ui, rect: Rect) -> Rect {
    Rect::from_min_size(snapped_min(rect.min, ui.ctx().pixels_per_point()), rect.size())
}

/// The arithmetic of [`snap_to_pixels`], split out so it can be asserted on
/// directly: a `Ui` in a test harness reports whatever scaling the harness
/// happens to use and lays its rows out wherever it likes, so a test driven
/// through one could pass by never meeting a fractional coordinate at all --
/// which is the shape of test this crate treats as a defect.
///
/// A non-positive `pixels_per_point` cannot be divided back out; the position
/// is returned untouched, which is the same no-op the old code was.
///
/// `pub(crate)` for `card_mark`, which draws a brand logo one texel to one
/// device pixel and needs its corner on this same grid for that to hold.
pub(crate) fn snapped_min(min: Pos2, pixels_per_point: f32) -> Pos2 {
    if pixels_per_point <= 0.0 {
        return min;
    }
    let snap = |v: f32| (v * pixels_per_point).round() / pixels_per_point;
    Pos2::new(snap(min.x), snap(min.y))
}

/// The avatar tile's `border-radius: 8px` at the design's 32px size, as a
/// ratio so it stays right at any size the tile is drawn at.
pub fn avatar_corner_radius(size: f32) -> CornerRadius {
    CornerRadius::same((size * 0.25) as u8)
}

/// A rounded initials tile, `size` square. `emphasized` renders the selected
/// treatment (blue on a blue wash) versus the neutral grey one.
pub fn avatar(ui: &mut Ui, text: &str, size: f32, emphasized: bool) {
    // Unfilled, like every other avatar tile now. The fill was kept here one
    // pass longer than the others on the argument that a letter is type and
    // type needs a ground; the owner's next screenshot was a monogram tile
    // with the same grey square in it. One tile, one treatment -- a column
    // where the letters sit on grey and the icons beside them do not is two
    // designs, which is what the fill was supposed to prevent.
    let rect = avatar_artwork_tile(ui, size, emphasized);
    let fg = if emphasized { BLUE } else { TEXT_MUTED };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        FontId::new((size * MONOGRAM).round(), FontFamily::Name(SEMIBOLD.into())),
        fg,
    );
}

// ---------------------------------------------------------------------------
// The state pill (design 5b/5c)
// ---------------------------------------------------------------------------

/// The leading mark inside a [`state_pill`], or the absence of one.
///
/// **The absence is a variant rather than an `Option`**, because the design
/// uses all three and each means something: a dot for a state that is still
/// running, a tick for one that completed, and nothing at all for one that
/// simply ended. A pill drawn with a dot it does not mean is a pill that
/// claims to be live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PillMark {
    /// No mark. The label carries the whole meaning.
    None,
    /// A filled 5px dot.
    Dot(Color32),
    /// The design's tick: `M20 6 9 17l-5-5` at 10px, 2.4px stroke.
    Check(Color32),
}

/// The three colours one [`state_pill`] is drawn in, plus its mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PillTone {
    pub fill: Color32,
    pub edge: Color32,
    pub ink: Color32,
    pub mark: PillMark,
}

/// Total height of a [`state_pill`].
///
/// **20, not 18, and the two are one border apart.** The design declares
/// `padding: 3px 8px` on a `1px` border, and the page is content-box: the
/// 10px line box is ~12 tall (the same measurement [`CHIP_HEIGHT`] is built
/// on), so 12 + 3 + 3 = 18 of content-plus-padding and the border adds one
/// pixel either side. Writing 18 here would have drawn the design's box minus
/// its border, which is the exact arithmetic slip this screen has been
/// re-measured for before.
pub const PILL_HEIGHT: f32 = 20.0;
/// Text size inside a [`state_pill`] (5b's list and detail pills;
/// 5c's legend draws the same pill one point larger and is not a second
/// component).
pub const PILL_TEXT_PX: f32 = 10.0;
/// Horizontal padding **inside the border**, per the design's `padding: 3px
/// 8px`.
pub const PILL_PAD_X: f32 = 8.0;

/// Height of the design's keyboard-hint chips: a 10px monospace line
/// (~12px line box) inside 3px of vertical padding.
///
/// Fixed rather than derived from the galley's own height, which is ascent
/// + descent — it reserves room for descenders that strings like "CTRL+H",
///   "CTRL+N" and "Enter" never contain, so a galley-sized chip always came
///   out taller than the design's. That gap widened when the monospace face
///   became Consolas (see `system_monospace`), whose descent is deeper than
///   the previously-used bundled face.
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

// --- design 6a's ↵ keycap (added by the TOTP picker's design pass) ---------

pub const RETURN_KEYCAP_PAD_X: f32 = 7.0;

// --- end of design 6a's keycap --------------------------------------------

// ---------------------------------------------------------------------------
// The indeterminate progress indicator (design turn 7).
// ---------------------------------------------------------------------------

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

/// §8a's `padding: 0 14px` on both footer buttons -- two more than the
/// style's `button_padding`, and set on the buttons themselves so the width
/// [`action_button_width`] measures for the footer's line-fitting is the
/// width the buttons then take.
pub const SECTION_FOOTER_BUTTON_PAD_X: f32 = 14.0;
/// §8a's `gap: 10px` between the footer's two answers.
pub const SECTION_FOOTER_GAP: f32 = 10.0;

/// 8a's `CTRL+S` chip on the Save button: `font-size: 10px` in
/// `ui-monospace`, `opacity: 0.8`, `gap: 9px` after the label.
///
/// Its own three constants rather than the keyboard chip `theme` already has
/// ([`CHIP_TEXT_PX`] and friends): that one is a bordered pill on a card, and
/// this is a bare run inside a filled button. Same idea, different object.
pub const SECTION_FOOTER_CHIP_PX: f32 = 10.0;
pub const SECTION_FOOTER_CHIP_GAP: f32 = 9.0;
pub const SECTION_FOOTER_CHIP_OPACITY: f32 = 0.8;

/// §8a's `font-size: 13px` on both footer buttons. The WEIGHTS differ (700
/// on Save, 600 on Cancel); the size does not.
pub const SECTION_FOOTER_TEXT_PX: f32 = 13.0;

/// Extent of the drawn ↵ glyph. The design sets it in 10px monospace beside
/// a 13px label (`opacity: 0.8`, which is the 204 alpha above); a real 10px
/// ↵ glyph's ink is roughly half its em box, so the arrow is drawn at 5px
/// rather than the 6.5 it used before, which read as heavier than the
/// design's at the same nominal size.
///
/// Public because the GDI renderer draws the same glyph at the same extent --
/// see [`return_arrow_segments`].
pub const RETURN_GLYPH_SIZE: f32 = 5.0;

/// **The four strokes the ↵ glyph is made of**, as segments around `center`:
/// down the right side, along the bottom, then the arrowhead's two barbs.
///
/// Pure geometry with no painter in it, and `pub`, for the reason
/// [`quadrant_outlines`] is both: design 3d's `Fill & save to vault ↵` is
/// drawn by `crate::win32_draw::draw_button_with_return` in GDI, which cannot
/// call into egui, and a second copy of this shape is a second ↵ to keep in
/// step. It is a vector on both sides rather than a character because **no face
/// this app ships carries U+21B5** -- neither Archivo nor egui's fallbacks, and
/// the GDI side would have to bet on the OS monospace having it.
///
/// Every part scales with `size` -- the arrowhead barbs used to be a fixed
/// 3.2px, so shrinking the glyph left them oversized.
pub fn return_arrow_segments(center: Pos2, size: f32) -> [[Pos2; 2]; 4] {
    let half = size / 2.0;
    let barb = size * 0.5;
    let right_top = Pos2::new(center.x + half, center.y - half);
    let corner = Pos2::new(center.x + half, center.y + half * 0.7);
    let left = Pos2::new(center.x - half, center.y + half * 0.7);
    [
        [right_top, corner],
        [corner, left],
        [left, Pos2::new(left.x + barb, left.y - barb)],
        [left, Pos2::new(left.x + barb, left.y + barb)],
    ]
}

/// The outlined secondary button ("Not now", "Cancel", "Copy").
/// §8a's `gap: 8px` between a row's field and the buttons after it.
///
/// The BUTTON is `row_button` further down -- the read pane's own 2b control,
/// not a second one at 8a's numbers. The owner asked for "default Copy button
/// for the rest of items", and the default is the one that pane already
/// draws: the two panes are one click apart and a Copy that changed size
/// between them is the difference this whole pass has been removing.
pub const ROW_BUTTON_GAP: f32 = 8.0;

// ---------------------------------------------------------------------------
// Design 5b's action row: design-system buttons placed into a MEASURED RECT
// ---------------------------------------------------------------------------

/// The label: 5b's `font-size: 13px; font-weight: 600`, which is this
/// module's [`semibold`] at 13 -- the same face and size every other button
/// here wears.
pub const ACTION_BUTTON_TEXT_PX: f32 = 13.0;
/// A floor, so a one-word control is still a target rather than a sliver.
pub const ACTION_BUTTON_MIN_WIDTH: f32 = 64.0;

// ---------------------------------------------------------------------------
// Design 5b's receded row
// ---------------------------------------------------------------------------

/// `colour` at `opacity`, for [`ENDED_ROW_OPACITY`]'s use.
///
/// Composited against nothing -- the alpha is simply scaled -- because every
/// caller paints over a ground it has just filled, so blending here would
/// mean knowing the ground twice.
pub fn faded(colour: Color32, opacity: f32) -> Color32 {
    let a = (colour.a() as f32 * opacity.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), a)
}

/// The height every cell in a run shares, and the height of the run.
///
/// The same 28 the Preferences window's steppers, key buttons and form fields
/// are, because a multiple-choice row sits in cards beside all three and a
/// control that were a couple of points taller would make the card's rows
/// look mismeasured rather than deliberate.
pub const SEGMENT_HEIGHT: f32 = 28.0;

/// The cell label's size: 12px, the same as the Preferences window's other
/// in-card controls -- and design 3d's own `font-size: 12px` on the
/// `Words / Letters / PIN` run.
///
/// Public for the reason [`SEGMENT_HEIGHT`] and [`SEGMENT_SEAM`] are: the GDI
/// renderer draws the same run and cannot call into egui, so
/// `crate::win32_draw::draw_segment_cell` reads this rather than restating it.
pub const SEGMENT_TEXT_SIZE: f32 = 12.0;

// ---------------------------------------------------------------------------
// The dropdown: the multiple-choice control for a set a run cannot hold
// ---------------------------------------------------------------------------

/// One row of a [`dropdown`]. The kit's: see [`crate::kit::menu::Choice`].
pub use crate::kit::menu::Choice;

// ---------------------------------------------------------------------------
// The date picker: one month, and only the days that are answers
// ---------------------------------------------------------------------------

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

/// [`row_button`]'s own height, named so a caller laying a row of mixed
/// widgets can put them all on it. See `detail_edit`'s `row_with_buttons`.
pub const ROW_BUTTON_HEIGHT_2B: f32 = 28.0;

/// **The wordmark, in the card header's caps setting.** One constant, because
/// the egui header below and the four Win32 cards' GDI header both set it, and
/// a second literal would be a second brand free to drift from this one.
pub const WORDMARK_CAPS: &str = "DESKWARDEN";

/// The card header's mark height, the size its wordmark is set at, and the
/// tracking on it (0.1em at 11px). Public for the Win32 cards, which lay the
/// same lockup out in whole pixels rather than through egui.
pub const CARD_HEADER_MARK_H: f32 = 16.0;
pub const CARD_HEADER_WORD_PX: f32 = 11.0;
pub const CARD_HEADER_TRACKING: f32 = 1.1;

/// **The mark's design artboard**, `(width, height)`. [`quadrant_outlines`] is
/// drawn in these coordinates, so anything fitting the mark into a box of its
/// own -- egui's [`paint_mark`] and `win32_draw::draw_mark` alike -- scales
/// against this pair rather than against two copies of 24 and 28.
pub const MARK_ARTBOARD: (f32, f32) = (24.0, 28.0);

// ---------------------------------------------------------------------------
// The field marks: one drawn icon per kind of thing the picker can type.
//
// STROKES, NOT GLYPHS, for the reason the detail pane's star, kebab, eye and
// clock are strokes: a mark out of a fallback face nobody here chose brings
// its own weight, optical size and baseline next to controls measured from
// the design. These are drawn a second time over: the account picker is bare
// Win32 with no egui anywhere on its path, so an `egui::Shape` would not
// reach it at all -- what crosses that line is geometry, exactly as
// [`quadrant_outlines`] already does for the shield.
//
// Points only, in one square artboard, with a kind saying how each path is to
// be laid down. `win32_draw::draw_field_mark` scales them into device pixels
// and strokes them with GDI; nothing here knows what a renderer is.
// ---------------------------------------------------------------------------

/// **The field marks' artboard**, a square. Every path below is drawn in
/// `0.0..FIELD_MARK_ARTBOARD` on both axes, so a caller fitting a mark into a
/// box scales against this one number.
pub const FIELD_MARK_ARTBOARD: f32 = 20.0;

/// The drawn side of a field mark inside a row's square gutter.
///
/// Smaller than the 24px a favicon is blended at, deliberately: a favicon is
/// a picture with its own padding baked in, and a stroked mark drawn to the
/// same 24 reads as the louder of the two. 20 puts this family's ink at the
/// optical weight of the artwork it sits beside in the step before.
pub const FIELD_MARK_SIDE: f32 = 20.0;

/// The field marks' stroke, in artboard units.
///
/// Heavier than [`ICON_STROKE`]'s 1.3 because these are drawn at 20px rather
/// than the detail strip's 34, and a 1.3 stroke on a 20px mark beside 13px
/// Archivo Medium reads as a hairline sketch next to the row's own name.
pub const FIELD_MARK_STROKE: f32 = 1.4;

/// Which mark a row carries. One per kind of thing the picker can type, plus
/// the sequence that runs several of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldMark {
    /// A bust: the username.
    Person,
    /// A key: the password.
    Key,
    /// A clock face: the one-time code, which is the only field that expires.
    Clock,
    /// An arrow into a stop: *username, Tab, password*, which is the only
    /// offer that types a key rather than a value.
    TabArrow,
    /// A run of steps: the item's own saved sequence.
    Steps,
    /// A luggage tag: a custom field, which is the only one whose name the
    /// user chose.
    Tag,
}

/// How one path of a mark is laid down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkPathKind {
    /// Stroked from the first point to the last and no further.
    Open,
    /// Stroked and closed back onto its first point.
    Closed,
    /// Filled, and closed.
    Filled,
}

/// One piece of one mark, in artboard coordinates.
///
/// **A circle is its own variant rather than a sampled path**, and that is
/// not a nicety: a ring flattened to twenty-four points and then rounded to
/// whole device pixels at a 3-unit radius comes out an octagon, which is
/// exactly what the first render of these marks showed. A renderer with a
/// real ellipse primitive -- GDI has one, egui has one -- draws the circle it
/// was asked for at any size.
#[derive(Clone, Debug, PartialEq)]
pub enum MarkShape {
    /// A run of points, stroked or filled per [`MarkPathKind`].
    Path { points: Vec<Pos2>, kind: MarkPathKind },
    /// A circle, stroked when `filled` is false and filled when it is true.
    Circle { centre: Pos2, radius: f32, filled: bool },
}

/// A stroked ring.
fn mark_ring(cx: f32, cy: f32, r: f32) -> MarkShape {
    MarkShape::Circle { centre: Pos2::new(cx, cy), radius: r, filled: false }
}

/// A filled dot.
fn mark_dot(cx: f32, cy: f32, r: f32) -> MarkShape {
    MarkShape::Circle { centre: Pos2::new(cx, cy), radius: r, filled: true }
}

/// An arc, open, from `from` to `to` radians. Sampled rather than a
/// [`MarkShape::Circle`], because it is not one: no renderer here draws a
/// partial ellipse the same way twice.
fn mark_arc(cx: f32, cy: f32, r: f32, from: f32, to: f32, steps: usize) -> MarkShape {
    let points = (0..=steps)
        .map(|i| {
            let t = from + (to - from) * i as f32 / steps as f32;
            Pos2::new(cx + r * t.cos(), cy + r * t.sin())
        })
        .collect();
    MarkShape::Path { points, kind: MarkPathKind::Open }
}

fn mark_line(x0: f32, y0: f32, x1: f32, y1: f32) -> MarkShape {
    MarkShape::Path {
        points: vec![Pos2::new(x0, y0), Pos2::new(x1, y1)],
        kind: MarkPathKind::Open,
    }
}

/// **Every piece of one mark**, in [`FIELD_MARK_ARTBOARD`] coordinates.
///
/// Built once and kept, for [`quadrant_outlines`]'s reason: the geometry is a
/// compile-time constant in all but name -- it only needs float arithmetic a
/// `const` cannot do -- and the caller is a repaint path that runs on every
/// hover.
pub fn field_mark_shapes(mark: FieldMark) -> &'static [MarkShape] {
    static SHAPES: OnceLock<[Vec<MarkShape>; 6]> = OnceLock::new();
    let all = SHAPES.get_or_init(build_field_marks);
    &all[match mark {
        FieldMark::Person => 0,
        FieldMark::Key => 1,
        FieldMark::Clock => 2,
        FieldMark::TabArrow => 3,
        FieldMark::Steps => 4,
        FieldMark::Tag => 5,
    }]
}

/// One field mark, laid down by egui into `frame`.
///
/// **The same geometry [`crate::win32_draw::draw_field_mark`] strokes with
/// GDI**, scaled the same way -- [`FIELD_MARK_ARTBOARD`] into the frame's
/// shorter side, the stroke [`FIELD_MARK_STROKE`] in artboard units and never
/// under a point, a real circle for a ring -- so the mark in the sequence
/// builder's field pill IS the mark in the picker's gutter, and the two
/// renderers cannot be told apart by the shape they drew. Written the day 4a's
/// step rows got their pill, which wants a 12-point bust beside `Username`
/// and a key beside `Password`; before that every egui surface that needed
/// one of these had drawn it as a favicon or as nothing.
pub fn paint_field_mark(painter: &egui::Painter, frame: Rect, mark: FieldMark, color: Color32) {
    let side = frame.width().min(frame.height());
    let unit = side / FIELD_MARK_ARTBOARD;
    let origin = frame.center() - Vec2::splat(side / 2.0);
    let at = |q: Pos2| origin + q.to_vec2() * unit;
    let stroke = Stroke::new((FIELD_MARK_STROKE * unit).max(1.0), color);
    for shape in field_mark_shapes(mark) {
        match shape {
            MarkShape::Circle { centre, radius, filled } => {
                if *filled {
                    painter.circle_filled(at(*centre), radius * unit, color);
                } else {
                    painter.circle_stroke(at(*centre), radius * unit, stroke);
                }
            }
            MarkShape::Path { points, kind } => {
                let points: Vec<Pos2> = points.iter().map(|q| at(*q)).collect();
                if points.len() < 2 {
                    continue;
                }
                match kind {
                    MarkPathKind::Filled => {
                        painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
                    }
                    MarkPathKind::Closed => {
                        painter.add(egui::Shape::closed_line(points, stroke));
                    }
                    MarkPathKind::Open => {
                        painter.add(egui::Shape::line(points, stroke));
                    }
                }
            }
        }
    }
}

fn build_field_marks() -> [Vec<MarkShape>; 6] {
    use std::f32::consts::PI;
    let p = Pos2::new;

    // A bust. The head is a ring rather than a disc so that the mark holds
    // the same amount of white as the key and the clock beside it, and the
    // shoulders are a wide, shallow arc: a deeper one reads as a bowl the
    // head is sitting in.
    let person = vec![
        mark_ring(10.0, 7.0, 3.8),
        mark_arc(10.0, 19.4, 7.0, PI * 1.22, PI * 1.78, 16),
    ];

    // A key, bow left. Two teeth, because one reads as a lollipop.
    let key = vec![
        mark_ring(6.2, 10.0, 3.9),
        mark_line(10.1, 10.0, 17.2, 10.0),
        mark_line(13.2, 10.0, 13.2, 13.8),
        mark_line(16.4, 10.0, 16.4, 12.9),
    ];

    // A clock at three o'clock, the reading that gives the two hands the
    // largest angle a face can show -- `CLOCK_HOUR_HAND`'s argument, applied
    // to a mark drawn by a different renderer.
    let clock = vec![
        mark_ring(10.0, 10.0, 6.5),
        mark_line(10.0, 10.0, 10.0, 5.2),
        mark_line(10.0, 10.0, 14.1, 10.0),
    ];

    // Tab: an arrow that runs into a stop. The head is FILLED, which is what
    // tells it apart from the outlined marks either side of it at 20px.
    let tab_arrow = vec![
        mark_line(2.6, 10.0, 11.6, 10.0),
        MarkShape::Path {
            points: vec![p(11.0, 6.0), p(11.0, 14.0), p(15.6, 10.0)],
            kind: MarkPathKind::Filled,
        },
        mark_line(17.4, 4.8, 17.4, 15.2),
    ];

    // A run of steps: three lines, each led by a filled dot. A saved sequence
    // is the one offer that is a list of actions rather than a single value,
    // and this is the shape of a list.
    let mut steps = Vec::new();
    for row in 0..3 {
        let y = 5.2 + row as f32 * 4.8;
        steps.push(mark_dot(4.2, y, 1.4));
        steps.push(mark_line(8.0, y, 16.4, y));
    }

    // A luggage tag with its eyelet: the custom field, whose name is the
    // user's own and whose contents this card knows nothing about.
    let tag = vec![
        MarkShape::Path {
            points: vec![p(10.4, 2.6), p(17.4, 2.6), p(17.4, 9.6), p(9.6, 17.4), p(2.6, 10.4)],
            kind: MarkPathKind::Closed,
        },
        mark_dot(14.2, 5.8, 1.35),
    ];

    [person, key, clock, tab_arrow, steps, tag]
}

// ---------------------------------------------------------------------------
// The dismiss ✕, and the ONE definition of it
//
// Every card in this app that can be got rid of carries this mark, and the
// point of the family below is that there is nothing for a call site to
// choose: the glyph, its size, its hit target, its hover treatment, its
// cursor, its tooltip and how far it is held off the card's edge are all
// settled here. A card asks for the mark and says which surface it is going
// on; it does not say how big, how grey, or how far in.
//
// That is not tidiness for its own sake. The report this family answers was
// "some modals have an ✕ and some don't", and the way that state is reached is
// one mark per card, each written by hand at the moment that card was built --
// which is also how you end up with a 16pt target on one card and a 20pt one
// on the next, ghost-grey here and muted there. Two of them had already been
// written by hand before this, and they had already drifted four points apart
// (see [`MODAL_CLOSE_INSET`]).
//
// There are two entry points because there are two kinds of header in this
// crate and no third:
//
// * [`close_glyph`] ALLOCATES, for a header assembled out of widgets in a
//   layout -- `card_header_with_close`'s overlay strip.
// * [`modal_dismiss_mark`] does NOT allocate: it is handed the rectangle of
//   the line the mark belongs on and interacts at an absolute rect inside it.
//   Every modal in this app takes this one, and not allocating is the half
//   that matters -- it means the mark can be added to a card that already
//   exists without moving one pixel of what that card already paints, which is
//   why the paint tests on all seven of them still measure the same layout.
// ---------------------------------------------------------------------------

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

/// Width of a scroll bar drawn by [`scrollbar_in_gutter`].
///
/// egui's own `ScrollStyle::solid()` uses 6 for the same thing; there is no
/// scroll bar anywhere in the design document to read a value off, so this
/// follows egui rather than inventing a number.
pub const SCROLLBAR_WIDTH: f32 = 6.0;

/// **The scroll bar every `ScrollArea` gets** unless a surface asks for its own
/// lane ([`scrollbar_in_gutter`]). Set once in [`apply`] as
/// `Style::spacing.scroll`.
///
/// The design draws no scroll bar (its panes are `overflow: auto` under a
/// browser's own), so this is the app's rule, recorded as a Ruling in
/// `docs/superpowers/plans/2026-11-20-menus-scrollbars.md`: a thin floating
/// bar, [`SCROLL_IDLE_WIDTH`] wide and [`SCROLL_HOVER_WIDTH`] under the
/// pointer, rounded, in the ink colour at low alpha, **with no track**, set
/// [`SCROLL_EDGE_MARGIN`] in from the edge. It takes no room from the content
/// (`floating_allocated_width` is 0), is hidden until the pointer is over the
/// area, and is never white-on-white: the handle is the ink at
/// [`SCROLL_HANDLE_ACTIVE`] / [`SCROLL_HANDLE_GRABBED`] alpha.
pub fn scroll_style() -> egui::style::ScrollStyle {
    egui::style::ScrollStyle {
        floating: true,
        content_margin: egui::Margin::ZERO,
        bar_width: SCROLL_HOVER_WIDTH,
        handle_min_length: 28.0,
        bar_inner_margin: 4.0,
        bar_outer_margin: SCROLL_EDGE_MARGIN,
        floating_width: SCROLL_IDLE_WIDTH,
        floating_allocated_width: 0.0,
        foreground_color: true,
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.0,
        interact_background_opacity: 0.0,
        dormant_handle_opacity: 0.0,
        active_handle_opacity: SCROLL_HANDLE_ACTIVE,
        interact_handle_opacity: SCROLL_HANDLE_GRABBED,
        fade: Default::default(),
    }
}

/// The bar's width while the pointer is in the area but not on the bar.
pub const SCROLL_IDLE_WIDTH: f32 = 6.0;
/// The bar's width under the pointer.
pub const SCROLL_HOVER_WIDTH: f32 = 10.0;
/// How far the bar stands in from the area's edge.
pub const SCROLL_EDGE_MARGIN: f32 = 3.0;
/// The handle's alpha while the pointer is in the area.
pub const SCROLL_HANDLE_ACTIVE: f32 = 0.4;
/// The handle's alpha while the pointer is on the bar.
pub const SCROLL_HANDLE_GRABBED: f32 = 0.7;

/// How far short of its rect's right edge a single line of clipped text stops.
///
/// Used by [`crate::win32_draw::draw_row`], whose `DrawTextW` calls carry
/// `DT_END_ELLIPSIS`: without an inset the "..." Windows substitutes sits hard
/// against the card's edge and reads as a cut rather than as a truncation.
/// Three device pixels is the smallest gap that separates the glyph from the
/// edge at 100% scaling; it is here rather than in `win32_draw` so the GDI
/// renderer keeps taking every dimension from this module.
pub const TEXT_CLIP_INSET: f32 = 3.0;

/// [`eyebrow`]'s type size. `font-size: 11px` in the design, everywhere it
/// appears.
pub const EYEBROW_PX: f32 = 11.0;

/// **Where a face's ink sits vertically in a line box of `line_height`**, as a
/// distance from the row's top -- the number a caller places a box by if it
/// wants that face optically centred rather than box-centred.
///
/// Measured off the same probe as [`ink_depth_of`] and for the same reason: a
/// row given a custom `line_height` does not put its baseline where the face's
/// default row does, so nothing here can be reasoned to from the metrics.
pub fn line_ink_middle(ctx: &egui::Context, font: &FontId, line_height: f32) -> f32 {
    let (top, bottom) = probe_ink_at(ctx, font, line_height, ASCENT_PROBE);
    (top + bottom) / 2.0
}

/// [`face_ink_at`] over a probe of the caller's.
///
/// Two probes, because the two questions this file asks of a line box are
/// different ones. **How DEEP is the box** has to be answered over
/// [`INK_PROBE`], descenders and all, or a `g` is clipped at its foot.
/// **Where is the run's MIDDLE** is answered over [`ASCENT_PROBE`], the cap
/// band alone: a box whose descender band is counted into the middle pushes
/// every run in it about a point up, which is the defect
/// `detail_edit::sequence_chip` settled in three attempts and the owner then
/// found again in a field -- "1 in field is not centered", of a digit that
/// has no descender to be centred with.
fn probe_ink_at(
    ctx: &egui::Context,
    font: &FontId,
    line_height: f32,
    probe: &str,
) -> (f32, f32) {
    ctx.fonts_mut(|f| {
        let mut job = egui::text::LayoutJob {
            wrap: egui::text::TextWrapping::no_max_width(),
            ..egui::text::LayoutJob::default()
        };
        job.append(
            probe,
            0.0,
            egui::TextFormat {
                line_height: Some(line_height),
                font_id: font.clone(),
                color: Color32::BLACK,
                ..Default::default()
            },
        );
        let galley = f.layout_job(job);
        let mut top = f32::INFINITY;
        let mut bottom = f32::NEG_INFINITY;
        for row in &galley.rows {
            for glyph in &row.glyphs {
                if glyph.uv_rect.is_nothing() {
                    continue;
                }
                let at = row.pos.y + glyph.pos.y + glyph.uv_rect.offset.y;
                top = top.min(at);
                bottom = bottom.max(at + glyph.uv_rect.size.y);
            }
        }
        if top.is_finite() { (top, bottom) } else { (0.0, line_height) }
    })
}

/// **A galley's top-left, for a galley that is to sit optically centred in
/// `band`.**
///
/// Box-centring a galley is what almost every caller reaches for and it reads
/// high, because the face inks only the upper part of its row box -- the
/// measurement [`ink_drop`] carries. This is that correction applied, so a
/// caller centring one line in a pill, a chip or a band does not have to know
/// the rule, only to use this instead of the arithmetic.
///
/// `x` is the caller's: nothing here knows whether the line is left-aligned
/// against a column or centred across the band.
pub fn centred_galley_top(
    ctx: &egui::Context,
    band: Rect,
    galley: &egui::Galley,
    font: &FontId,
    x: f32,
) -> Pos2 {
    Pos2::new(
        x,
        band.center().y - galley.size().y / 2.0 + ink_drop(ctx, font, None),
    )
}

/// **How far a galley of `font` has to move DOWN to sit optically centred**,
/// for the callers that cannot set a line height.
///
/// [`ascent_of`]'s rule -- make the line box the ascent -- is the right one
/// wherever a single line is being laid out, and it is wrong for a WRAPPED
/// paragraph, where `line_height` is also the leading between its rows. A
/// caution band's sentence is such a paragraph, so its rows keep their
/// natural height and the whole block is offset instead.
///
/// The offset is half the difference between the slack under the ink and the
/// slack over it: the face reserves a descender band the text mostly does not
/// use, so a block top-aligned in its padding reads high by about that much.
/// On the monospace cut at 14 points it is 2.7 of a 16.4-point row.
/// `line_height` is the one the caller's `TextFormat` sets, or `None` for the
/// face's own -- it is the ROW the ink has to be centred in, and overriding it
/// moves only the row's bottom (egui leaves the baseline where the face puts
/// it), so a generous one makes this bigger rather than smaller.
pub fn ink_drop(ctx: &egui::Context, font: &FontId, line_height: Option<f32>) -> f32 {
    ctx.fonts_mut(|f| {
        let galley = f.layout_no_wrap(ASCENT_PROBE.to_string(), font.clone(), Color32::BLACK);
        let Some(glyph) = galley.rows.first().and_then(|row| row.glyphs.first()) else {
            return 0.0;
        };
        // The ink's top, as a distance from the row's top: the baseline plus
        // the glyph's own (negative) offset from it.
        let above = glyph.pos.y + glyph.uv_rect.offset.y;
        // ...and the slack under it. `X` has no descender, so its ink ends at
        // the baseline and everything below is the band being measured.
        let below = line_height.unwrap_or(glyph.font_height) - glyph.pos.y;
        ((below - above) / 2.0).max(0.0)
    })
}

// **`cap_height_of` was here.** It measured the ink of a capital so the
// name box could lay its line at that height and give the caret the size of
// the letters beside it -- the owner's "text cursor in header seems wrong
// size", asked twice.
//
// It does not work, and the reason is worth keeping. In egui the caret IS the
// galley's row: `TextEdit` draws it from the row's top to its bottom, and the
// BASELINE stays where the face puts it whatever the row height says. Cut the
// row to the cap height and the ink no longer fits in it -- the glyphs render
// from `ascent - cap` to `ascent`, most of it below a row that now ends at
// `cap`, and `ui.put`'s clip rect takes the difference off the top and the
// bottom of every letter. That is exactly what the owner screenshotted next:
// "cut off".
//
// Making the clip rect taller fixes the clipping and breaks the thing it was
// for: the caret then runs from the row's top to `cap` below it, which no
// longer straddles the text at all.
//
// **And the clip rect is not the caller's anyway**, which is the correction
// this note needed later. `TextEdit` clips to the rect its own GALLEY occupies
// -- `text_clip_rect = inner_rect`, expanded by a point for the caret -- and
// not to the rect `ui.put` hands it, so the line box is the only dial there
// is. A line box of the bare ascent ends AT the baseline and slices the tail
// off every `g`, `p`, `y` and `j`; the owner, with `Apple` in the 20-point
// name box, "title cut off from the bottom".
//
// So the line is the ascent plus whatever the face really inks below it,
// measured rather than reasoned to -- see `field_box` and `ink_depth_of`. The
// caret straddles the text and reaches a descender's foot, which is what a
// caret is; it is nothing like the full default row in a 38-point box that
// "cursor is huge" was about.

/// The character [`ascent_of`] measures.
///
/// A capital with no descender and no accent, so the glyph's own ink box is
/// the band this is about. The value read is `font_ascent`, which is the
/// FACE's metric and not this glyph's, so the choice only has to be a
/// character the face certainly carries.
const ASCENT_PROBE: &str = "X";

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

/// The shared body of [`disabled_text_field`] and [`disabled_password_field`]:
/// the same 38px box [`field_box`] allocates, painted greyed, with `text` sat
/// on the same baseline a live field's text would be.
///
/// Greyed means all three of fill, border and ink move together --
/// [`CANVAS`] instead of [`CARD`], [`BORDER`] instead of [`BORDER_STRONG`],
/// [`TEXT_GHOST`] instead of the ambient body colour. Any one of them alone
/// reads as a styling accident rather than as a control that is switched off.
/// **How far a field's text sits below the geometric middle of its box**, as
/// a fraction of the run's own height.
///
/// A row box is ascent-plus-descent tall and typical field text fills only the
/// upper part of that, so a geometrically centred run reads as sitting high.
/// Every box on this form nudges it down by this much.
///
/// Named because a SECOND painter now has to agree with it: the edit form's
/// `Native apps` box holds the app's name, painted here, and its executable in
/// a chip painted by the caller over the same box. Centring the chip on the box
/// while the name sits `FIELD_TEXT_NUDGE` below it put the two runs a point and
/// a half apart on one line -- the owner, with that row screenshotted: "Claude
/// and claude.exe - not on the same line".
pub const FIELD_TEXT_NUDGE: f32 = 0.09;

/// **The middle of a laid run's INK, measured from the galley's own top.**
///
/// What "on the same line" means for two runs set in different faces at
/// different sizes. It took three goes to get here, and the two wrong answers
/// are worth keeping because each is the obvious one:
///
/// * **Centre the BOXES.** A galley's box is the font's ascent plus its
///   descent, and two faces divide that differently -- the edit form's
///   `Native apps` row sets the app's name in 14px Archivo and its executable
///   in 11px Consolas. With the boxes centred the mono ink sat visibly above
///   the name's: "*.exe is still positioned higher".
/// * **Share a BASELINE.** Typographically correct for one size, and wrong
///   here: hung from the name's baseline, the smaller run has less ink above
///   the line and the same little below it, so its visual mass falls. "now it
///   is lower".
///
/// The answer between them is the ink itself. `Glyph::pos` is the baseline
/// (documented as "baseline position, relative to the row") and
/// `uv_rect.offset` / `uv_rect.size` are the drawn glyph's own box in points,
/// so the union over a run's glyphs is exactly what a reader sees. Glyphs that
/// draw nothing -- the space between two words -- are skipped, or every run
/// with a space in it would be measured against a blank.
///
/// A run with no ink at all has no middle; half its box is returned, which is
/// where a caller centring on it would have put things anyway.
/// **The middle of a FACE's ink on one line, measured from a galley's top and
/// independent of what the run says.**
///
/// [`ink_middle_of`] answers about a STRING, which is right for asking where
/// something was painted and wrong for deciding where to paint it: `ledgerline`
/// has a descender and `Firefox` has none, so hanging a neighbour off the
/// former's ink middle puts it one and a half points lower than off the
/// latter's. A line that moves with the letters in it is not a line. The owner,
/// with the pill hung off a name that had no descender and then one that did:
/// "still croocked".
///
/// [`ASCENT_PROBE`] is a capital `X` -- no descender, full cap height -- so
/// this is the face's cap-height middle, and two faces aligned on it read as
/// being on one line whatever each of them happens to be spelling.
pub fn face_ink_middle(ui: &Ui, font: &FontId) -> f32 {
    let probe =
        ui.painter().layout_no_wrap(ASCENT_PROBE.to_string(), font.clone(), Color32::BLACK);
    ink_middle_of(&probe)
}

/// **How tall a face's ink band is** -- [`ASCENT_PROBE`]'s cap height, which
/// is the band a reader sees and not the line box round it.
///
/// For a chip drawn round a run: a box taken from the row height is a box
/// with the face's leading in it, and at 13 points that is three points of
/// nothing over the letters and three under. `detail_edit`'s template chips
/// are drawn from this, which is what lets two rows of them sit on two lines
/// without touching.
pub fn face_ink_height(ui: &Ui, font: &FontId) -> f32 {
    let probe =
        ui.painter().layout_no_wrap(ASCENT_PROBE.to_string(), font.clone(), Color32::BLACK);
    let mut top = f32::INFINITY;
    let mut bottom = f32::NEG_INFINITY;
    for row in &probe.rows {
        for glyph in &row.glyphs {
            if glyph.uv_rect.is_nothing() {
                continue;
            }
            let at = row.pos.y + glyph.pos.y + glyph.uv_rect.offset.y;
            top = top.min(at);
            bottom = bottom.max(at + glyph.uv_rect.size.y);
        }
    }
    if top.is_finite() { bottom - top } else { probe.size().y }
}

pub fn ink_middle_of(galley: &egui::Galley) -> f32 {
    let mut top = f32::INFINITY;
    let mut bottom = f32::NEG_INFINITY;
    for row in &galley.rows {
        for glyph in &row.glyphs {
            if glyph.uv_rect.is_nothing() {
                continue;
            }
            let at = row.pos.y + glyph.pos.y + glyph.uv_rect.offset.y;
            top = top.min(at);
            bottom = bottom.max(at + glyph.uv_rect.size.y);
        }
    }
    if top.is_finite() { (top + bottom) / 2.0 } else { galley.size().y / 2.0 }
}

/// The knob's radius. Two points inside the track's own half-height, which is
/// what leaves the design's one-point rim of track showing above and below it.
pub const TOGGLE_KNOB_R: f32 = 9.0;

// ---------------------------------------------------------------------------
// The form card -- design §5a
// ---------------------------------------------------------------------------
//
// **§5a is a card in three bands, and until this existed neither of the two
// composers that draw it had any of the three.** The design's composer is
// `background #ffffff; border: 1px solid #d7d3d3; border-radius: 12px;
// box-shadow: 0 14px 34px rgba(45,43,43,0.18); overflow: hidden`, and inside
// that a header strip closed by a `#eae7e7` rule, a padded body, and a
// `#fbfaf9` footer opened by the same rule and carrying the two answers with
// a standing note pushed to the right.
//
// What the app drew instead, on BOTH composers, was `Frame::new().fill(CARD)
// .corner_radius(8).inner_margin(12)` -- white on white, no edge, no shadow,
// no bands -- with the buttons and a floating sentence simply the last things
// in the body. On the Sends screen that card sits on `theme::CANVAS` inside
// the detail column, so the only thing separating "the form" from "the pane"
// was a two-value difference in grey. The owner's word for it was that the
// composer "sits flat".
//
// It is one set of functions and not a card each, because the two composers
// differing from each other is the same defect one level up: the Sends
// screen's text composer and `record_ui`'s record composer are the same card
// with different middles, and a user who has seen one must not be able to
// tell they were drawn by different hands. That is the standing rule this
// module exists for and it has already been paid for twice on this screen --
// once for the footer buttons, once for the Access block.

/// The card's horizontal padding **in a narrow column**. See
/// [`form_card_pad_x`], which is what the bands actually ask.
///
/// §5a and §6c both say 18. This card lives in two places, and in one of them
/// 18 does not fit: the Sends screen's DETAIL COLUMN is 298 points at
/// `settings::MIN_VAULT_WINDOW_SIZE` -- 250 of card once the column's own
/// 24-point margins are off it -- where 18 a side is 14% of the card, and it
/// comes out of the one block that cannot spare it. §5a's Access rows are a
/// 96-point label column plus a 14-point gap plus a control, and every point
/// of padding is a point that block has to give back.
pub const FORM_CARD_PAD_X: i8 = 12;

/// The gap between an [`eyebrow`] and the block it names. §5a's `gap: 8px`,
/// drawn at the 6 both composers were already using -- the eyebrow's own line
/// box carries a couple of points of air that the design's `div` does not.
pub const EYEBROW_GAP: f32 = 6.0;

/// The gap between one eyebrowed block and the next. §5a's `gap: 16px` on the
/// card body, at the 12 both composers were already using.
///
/// Named here rather than left as a literal at six call sites because it is
/// the rhythm that makes two forms look like one form: the composers each
/// spelled it out in their own file, and the moment one of them wanted a
/// little more room somewhere the two would have stopped matching with
/// nothing failing to compile.
pub const BLOCK_GAP: f32 = 12.0;

/// §5a's send glyph: `M4 4l16 8-16 8 3-8z` in a 24-unit box, stroked.
///
/// The same path §8a hangs off its `Send` button, so the mark that opens the
/// composer and the mark that reaches it are one drawing.
pub fn paint_send_plane(painter: &egui::Painter, rect: Rect, color: Color32) {
    let unit = rect.width() / 24.0;
    let at = |ux: f32, uy: f32| rect.min + Vec2::new(ux * unit, uy * unit);
    // The outline, closed: tail, nose, tail again, and the notch that makes
    // it a plane rather than a triangle.
    painter.add(egui::Shape::closed_line(
        vec![at(4.0, 4.0), at(20.0, 12.0), at(4.0, 20.0), at(7.0, 12.0)],
        Stroke::new(2.2 * unit, color),
    ));
}

// ---- personal sharing's marks (design 6, tab 6e) ------------------------------
//
// A Send is a link: a square tile, a paper plane, a stadium pill. A share is a
// person: a round avatar, a person mark, a pill with 5-point corners. The two
// must not be confused, so every mark here differs from its Send sibling in
// shape and not only in colour.

/// A cubic from the mock's SVG, flattened into the points of one open path.
///
/// **The real curve rather than a hand-picked polyline through it.** The
/// first cut of these marks eyeballed six or nine points along each path, and
/// on the Share control's shoulder that put the line a third of a point off
/// the mock's curve -- at 21 points, on a strip whose owner compares it to the
/// mock side by side. epaint's own flattening walks the Bezier to `0.05` of a
/// point, which is well under what a 1.3-point stroke can show. Still one
/// `Shape::line` per stroke, so the marks stay open paths: nothing here is a
/// closed or filled path a probe in `icon_probe` counts.
fn flattened_cubic(points: [Pos2; 4]) -> Vec<Pos2> {
    egui::epaint::CubicBezierShape::from_points_stroke(points, false, Color32::TRANSPARENT, Stroke::NONE)
        .flatten(Some(0.05))
}

/// The person mark: a head and shoulders, in a 24-unit box scaled to `rect`
/// -- the mock's `<circle cx=12 cy=8 r=4/><path d="M4.5 21c.6-4 ...">`,
/// stroked, never filled (a filled path would read as a star to
/// `icon_probe::stars`).
///
/// The shoulders are the mock's two cubics -- `c.6-4 3.6-6.4 7.5-6.4` and its
/// mirror `s6.9 2.4 7.5 6.4` -- flattened by [`flattened_cubic`] and drawn as
/// ONE open path, so the crown at `(12, 14.6)` is a joint in a line and not
/// two butt ends meeting.
///
/// `stroke-width: 2.4` in the same 24-unit box, the mock's weight for this
/// mark at the size the list's subtitles draw it; the pill and chip draw it
/// at 2.6 there, a twelfth of a point apart at 10 points, and one weight for
/// the mark keeps it one drawing wherever it is.
pub fn paint_person_mark(painter: &egui::Painter, rect: Rect, color: Color32) {
    let unit = rect.width() / 24.0;
    let at = |ux: f32, uy: f32| rect.min + Vec2::new(ux * unit, uy * unit);
    let stroke = Stroke::new(2.4 * unit, color);
    painter.circle_stroke(at(12.0, 8.0), 4.0 * unit, stroke);
    let mut shoulders = flattened_cubic([at(4.5, 21.0), at(5.1, 17.0), at(8.1, 14.6), at(12.0, 14.6)]);
    // `s6.9 2.4 7.5 6.4`: the first control point is the reflection of the
    // previous curve's second, `(15.9, 14.6)`. `skip(1)` because the crown is
    // already the last point of the left half.
    shoulders.extend(
        flattened_cubic([at(12.0, 14.6), at(15.9, 14.6), at(18.9, 17.0), at(19.5, 21.0)])
            .into_iter()
            .skip(1),
    );
    painter.add(egui::Shape::line(shoulders, stroke));
}

/// **The info mark on 6a's no-account band** -- the mock's
/// `ic('info', 15, '#8a5a06', 2.2)`: a ring, a dot and the stem of an `i`,
/// in a 24-unit box scaled to `rect`.
///
/// An information mark and not [`paint_warning_glyph`]'s triangle, which is
/// the design's own choice and the right one: nothing has gone wrong. The
/// share went through; it is waiting for somebody to sign up.
pub fn paint_info_mark(painter: &egui::Painter, rect: Rect, color: Color32) {
    let unit = rect.width() / 24.0;
    let at = |ux: f32, uy: f32| rect.min + Vec2::new(ux * unit, uy * unit);
    let stroke = Stroke::new(2.2 * unit, color);
    painter.circle_stroke(at(12.0, 12.0), 9.0 * unit, stroke);
    // `M12 8h.01`: a round cap on a zero-length line is a dot of the
    // stroke's width -- drawn filled, since egui caps nothing.
    painter.circle_filled(at(12.0, 8.0), 1.1 * unit, color);
    // `M11 12h1v5h1`: the stem with its two serifs, one open path.
    painter.add(egui::Shape::line(
        vec![at(11.0, 12.0), at(12.0, 12.0), at(12.0, 17.0), at(13.0, 17.0)],
        stroke,
    ));
}

/// Where one person stands, as a pill says it. Short words; the status line
/// beside the pill carries the long form (mock 6e, SHARE PILLS).
///
/// Two of the four are the mock's. [`Self::KeyChanged`] and [`Self::Gone`]
/// are the engine's own states (`rest::sharing::PersonState`) that the mock's
/// strip does not draw: a changed key is the danger wash, because nothing is
/// sent to that person until it is looked at, and a person no membership
/// reaches any more is the mock's expired grey, because it means "not
/// receiving this" and asks for nothing.
///
/// The mock's "No account yet" and "Invite expired" are gone (DW2 fix round
/// 1): no server tells a client whether an invited address has an account,
/// and no expiry is claimed (official-path spec §3.4), so a pending
/// invitation is [`Self::Invited`] whatever it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharePillKind {
    Shared,
    Invited,
    KeyChanged,
    Gone,
}

impl SharePillKind {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Shared => crate::i18n::t("share_pill.shared"),
            Self::Invited => crate::i18n::t("share_pill.invited"),
            Self::KeyChanged => crate::i18n::t("share_pill.key_changed"),
            Self::Gone => crate::i18n::t("share_pill.gone"),
        }
    }
}

// ---------------------------------------------------------------------------
// The section card -- design §8a
// ---------------------------------------------------------------------------
//
// **§8a's grid card, which is not §5a's composer card**, and the distinction
// is worth a paragraph because the two are four values apart and it would be
// easy to read them as one control drawn twice.
//
// §5a is ONE card that IS the form: `border: 1px solid #d7d3d3`, `radius:
// 12px`, a `0 14px 34px` shadow, a 14px/800 heading. It is a thing laid ON the
// pane, and the shadow is what says so.
//
// §8a is NINE cards laid out in a grid on `#f7f6f5`: `border: 1px solid
// #eae7e7`, `radius: 10px`, **no shadow at all**, and a 12px/700 uppercase
// title tracked at 0.06em in `#605d5d`. They are regions OF the page, not
// objects on it, and nine shadowed cards stacked twelve points apart would
// read as nine floating panels -- the visual noise the design avoids by
// spending one grey on a border and nothing on elevation.
//
// So this is a second card, sharing §5a's padding constants (a card inside
// this app's detail pane has the same width problem whichever design drew it)
// and differing in exactly the four values above. The band helpers below are
// the same three-band idiom for the same reason `form_card_header` gives.
//
// Both families live in this file rather than in the two screens that draw
// them, because a user who has seen the Send composer must not be able to
// tell that the item form was drawn by a different hand -- the standing rule
// the §5a block's own comment states, applied one design turn later.

/// §8a's `border-radius: 10px`. Two less than [`FORM_CARD_RADIUS`]; see the
/// block comment for why the two cards are not one.
/// Spec B 0.6: the editor's cards are radius 18.
pub const SECTION_CARD_RADIUS: u8 = 18;

/// §8a's `padding: 11px 16px` header and `12px 16px` rows, in the horizontal
/// axis, **for a card too narrow to draw at 16**. See [`section_card_pad_x`],
/// which is what the edit form's bands actually ask.
///
/// §8a is drawn at 1240 points with a 1028-point card column. This card is
/// drawn in the vault window's DETAIL PANE, which is 298 points at
/// `settings::MIN_VAULT_WINDOW_SIZE` -- around 224 of card body once the
/// pane's own margins, the scroll gutter and this padding are off it. At that
/// width 16 a side is 14% of the card, taken from the one thing that cannot
/// spare it: the box the user is typing into.
///
/// It is [`FORM_CARD_PAD_X`] itself rather than a same-valued constant beside
/// it, because two cards in one app padded to two different numbers is the
/// drift this module exists to prevent, and neither design's number is more
/// right than the other's at 298 points.
///
/// **This used to be the card's ONLY padding**, and its doc used to end on
/// that last sentence as if it settled the wide case too. It did not: the
/// shipped detail pane at the 1240-point window is 638 points, the card in it
/// 628, and §8a's 16 fits there with room to spare -- so the form was drawing
/// §8a's cards at a padding argued entirely from a width it is almost never
/// at. That is the same history [`form_card_pad_x`] records for §5a's card,
/// and it is resolved the same way. The sequence builder's rail cards still
/// read this constant directly, through [`section_card_header`] and
/// [`section_card_body`], because they are 252 points wide and the wide arm
/// is not theirs.
pub const SECTION_CARD_PAD_X: i8 = FORM_CARD_PAD_X;

/// §8a's own horizontal padding, `padding: 11px 16px` on the header band and
/// `12px 16px` on every row, **at the width the design draws it**.
pub const SECTION_CARD_PAD_X_WIDE: i8 = 16;

/// The width at which a section card takes §8a's 16.
///
/// [`FORM_CARD_WIDE_AT`]'s rule applied to this card's own design rather than
/// its number borrowed: the narrowest card §8a draws at 16 is one half of its
/// `grid-template-columns: 1fr 1fr; gap: 14px` pair -- `Custom fields` beside
/// `Notes`, `Sharing` beside `History` -- inside a 1028-point column, which is
/// (1028 - 14) / 2 = 507. §5a's 470 was considered and rejected: it is the
/// narrowest card a DIFFERENT design draws at a DIFFERENT padding, and a
/// threshold is only a measurement if it is measured off the thing it
/// thresholds.
///
/// The read pane's cards, one click away, draw 2b's `padding: 11px 16px` at
/// every width; above this line the two panes agree to the point, which is
/// where the eye that just clicked Edit is.
pub const SECTION_CARD_WIDE_AT: f32 = 507.0;

/// §8a's header band padding: `padding: 11px 16px`.
///
/// Kept at the design's 11 rather than folded into [`FORM_CARD_PAD_Y`]'s 12,
/// because a section card's header is a NAME and §5a's is a heading: one
/// point of air either side is the difference between a strip that labels the
/// card and a strip that competes with it, and nine of these are stacked down
/// one column where §5a draws one.
pub const SECTION_CARD_HEADER_PAD_Y: i8 = 11;

/// §8a's row band padding: `padding: 12px 16px` -- drawn at the READ pane's
/// 13.
///
/// One point, and it is the owner's: "blocks within the module should be
/// same size\paddings\headers as details". 2b gives its rows `padding: 13px
/// 16px` and §8a gives its rows 12, and the two panes are one click apart
/// showing the same record -- so a row that changes height by a point when
/// Edit is pressed is a row that moves for no reason the reader can name.
/// `detail::ROW_PAD_Y` is the number, read across rather than copied.
pub const SECTION_CARD_PAD_Y: i8 = 13;

/// The gap between one section card and the next **in the sequence
/// builder**, which is [`BLOCK_GAP`] -- the 12 every other stacked block in
/// this app sits at.
///
/// **The edit form no longer reads this.** It used to, on the argument that a
/// form whose cards sit 14 apart and whose blocks inside them sit 12 apart is
/// a form with two rhythms. §8a draws exactly those two rhythms -- `gap: 14px`
/// between its cards, `padding: 12px` inside its rows -- and the owner asked
/// for §8a to the point, so the edit form's column is
/// [`SECTION_COLUMN_GAP`] now. This constant is kept for the builder because
/// its design is 4a, whose `gap: 14px` is the figure's own wrapper and not a
/// card column; moving the builder's cards on §8a's authority would be moving
/// another screen's pixels for a number its design never declared.
pub const SECTION_GAP: f32 = BLOCK_GAP;

/// The gap between one of §8a's cards and the next: its card column's
/// `gap: 14px`, exactly -- and exactly is the point.
///
/// A constant beside [`SECTION_GAP`] rather than a change to it, for the
/// reason that constant's doc now gives. It is not [`BLOCK_GAP`] + 2 either:
/// the two are different numbers because the design draws them as different
/// numbers, not because one is derived from the other.
///
/// **What the column was really drawn at before this existed was 20, not
/// 12.** `SECTION_GAP` is spent as a bare `add_space` after a `Frame`, and
/// egui spaces after every allocated widget, so the card column's gaps were
/// `item_spacing.y` + 12. The edit form spends this constant net of that
/// spacing (see `detail_edit::section`), so §8a's 14 is a six-point
/// tightening of what shipped and not the two-point loosening the two
/// constants' values suggest side by side. The builder's 20 is left as it is,
/// for the reason [`SECTION_GAP`] gives.
pub const SECTION_COLUMN_GAP: f32 = 14.0;

/// The width of a section row's label column: §8a's `width: 130px`.
pub const SECTION_LABEL_WIDTH: f32 = 130.0;

/// The gap between a section row's label and its control: §8a's `gap: 16px`.
///
/// **16, and it was 14.** The 14 was borrowed from §8a's `Item` grid
/// (`gap: 14px` between its three columns) and from this app's other rows,
/// on the ground that two points would not be seen. §8a draws every row on
/// every card at 16 -- `display: flex; align-items: center; gap: 16px` --
/// and the read pane's rows one click away are 2b's `gap: 16px` too, so the
/// 14 was the one row gap in the window that was not 16. It also moves
/// [`section_rows_fit_at`]'s floor by the same two points, which is the
/// arithmetic following the design rather than a second decision.
pub const SECTION_ROW_GAP: f32 = 16.0;

/// The gap between a section card's title and the note beside it: §8a's
/// `gap: 10px` on the header band.
///
/// Set on the band rather than left to `item_spacing`, whose 8 is what the
/// band used to draw the note at.
pub const SECTION_HEADER_GAP: f32 = 10.0;

/// §8a's row field: `height: 34px` on every box a card's rows hold, against
/// [`FIELD_HEIGHT`]'s 38.
///
/// **Two field heights on one screen, and both are the design's.** §8a's
/// title bar puts the record's name in a `height: 38px` box -- the same box
/// 2a, 3a and 3h draw, which is [`FIELD_HEIGHT`] and is what [`title_field`]
/// stays at -- and then draws every box on every card four points shorter.
/// The form used to draw all of them at 38, so a name box and a user-name box
/// were the same height on a screen whose design makes the name the taller
/// of the two by exactly this much. Read as the box, not the box plus a
/// border, because that is how this module read 2a's `height: 38px` into
/// [`FIELD_HEIGHT`]; the two constants are the same convention four points
/// apart.
///
/// Only the section-row variants below draw at this height. [`text_field`],
/// [`password_field`] and [`disabled_text_field`] are the login window's and
/// the overlay's, and their designs say 38.
pub const SECTION_FIELD_HEIGHT: f32 = 50.0;

/// The type size inside a row field.
///
/// **14, which is the READ pane's `ROW_VALUE_SIZE`, not §8a's 13.** The two
/// panes show the same record one click apart, and a value that changed size
/// when Edit was pressed is the kind of difference a reader notices before
/// they notice either size. The owner: "all text fields should have same text
/// size but like this border around that makes it editable".
///
/// So the BOX is what says a field is editable, and nothing else does: same
/// type, same size, a border around it.
pub const SECTION_FIELD_PX: f32 = 16.0;

/// §8a's `gap: 14px` between the cells of a card's COLUMN grid -- the
/// `ITEM` card's `grid-template-columns: 1fr 1fr 1fr`.
///
/// Its own constant and not [`SECTION_ROW_GAP`]'s 16: that one is the gap
/// between a row's label and its control, this is the gap between two
/// side-by-side cells, and §8a gives them different numbers.
pub const SECTION_GRID_GAP: f32 = 14.0;

/// The narrowest a cell of that grid may be before the grid stops being a
/// grid.
///
/// **§8a never had to have this number either.** Its `ITEM` card is about 996
/// points of body, so each of the three cells gets 322 and a collection name
/// has room to spell itself out. This pane's card body is about 472 at the
/// window the app actually opens at, which is 148 a cell -- tight, and still
/// a field. Below that the three go back to being stacked rows, which is the
/// shape this card already shipped and which works.
///
/// The same trade, and the same reasoning, as [`SECTION_ROW_CONTROL_FLOOR`]
/// one constant down: the narrow pane loses §8a's grid and nothing else.
pub const SECTION_GRID_CELL_FLOOR: f32 = 140.0;

/// The narrowest a section row's CONTROL may be before the row stops being a
/// row.
///
/// **The measurement behind [`section_rows_fit`], and the one number on this
/// card that §8a never had to have.** §8a's rows are a 130-point label beside
/// a control with 850 points to spend. In the detail pane the whole card body
/// is about 224 points at `settings::MIN_VAULT_WINDOW_SIZE`; a 130-point
/// label and a 14-point gap leave **80**, which is not a text field, it is a
/// slot. `https://app.ledgerline.com` in an 80-point box is six characters and
/// an ellipsis.
///
/// 180 is the floor a single-line URL, an email address or a program path
/// stays readable in, measured against the strings this form actually holds.
/// Below `130 + 14 + 180` the label goes back above its control, which is the
/// shape this form already ships and which works -- so the narrow pane loses
/// §8a's label column and nothing else.
pub const SECTION_ROW_CONTROL_FLOOR: f32 = 180.0;

// ---------------------------------------------------------------------------
// The modal card
//
// **One shape for every question this app puts in front of the window it is
// asking about.** It is the send preflight's refusal card, in egui: three
// bands rounded as a single piece and dropped on a dimmed scrim -- a coloured
// header saying what KIND of question this is, a white body saying which
// object it is about, and a footer carrying the two answers side by side.
//
// It lives here, beside the buttons and the avatar tile, for the reason every
// other composite widget in this file does. `vault_window::delete_modal` and
// `vault_window::icon_modal` had each grown a copy of the same hand-assembled
// card, and a third copy was what this replaced: a layout written out again
// at every call site is a design that drifts one modal at a time, and nothing
// about that fails to compile.
//
// The window's other overlays -- `folder_modal`, the launch confirmation, the
// discard prompt, the Send and import cards -- still paint their own plain
// cards, and are deliberately left where they are. Converting a modal changes
// what the user sees, and that was asked for on two of them.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Where a modal SITS, and how the user moves it
//
// Every card in this app used to be pinned, and not by a decision anybody
// argued: each one was written as an `Area` carrying
// `.anchor(Align2::CENTER_CENTER, Vec2::ZERO)`, and an anchored `Area` is
// pinned BY DEFINITION -- egui recomputes its position from the anchor on
// every frame, so a `.movable(true)` on the same builder is read and then
// thrown away. Eleven sites inherited the line from the one written before
// them, which is how a property nobody chose comes to hold everywhere.
//
// The answer lives HERE rather than at those eleven sites, for the reason the
// card itself does: "how does a modal behave" is one answer, and a modal
// written next month is movable by construction if the only way to get an
// `Area` for it is to ask this file for one.
// ---------------------------------------------------------------------------

/// **Throws away the pass on which a modal changed SHAPE, so the card is never
/// painted anchored by a size it no longer has.** Call it after the area's
/// `show`, handing it whatever decides the card's shape -- `totp_add` hands
/// it the stage.
///
/// # The report
///
/// > Scan same two small windows "Scanning your screen", first and then
/// > second on diff place
///
/// and, earlier, *"White small popup shows up. Same popup moves position."*
/// That is `totp_add`'s card going from the picker (470 wide, tall) to the
/// scanning stage (380 wide, four lines) between two frames, and it is egui
/// doing exactly what an anchored `Area` does. `Area::begin` places an
/// anchored area by the size stored from the **previous** pass:
///
/// ```text
/// let size = *state.size.get_or_insert_with(|| { sizing_pass = true; .. });
/// ..
/// if let Some((anchor, offset)) = anchor {
///     state.set_left_top_pos(anchor.align_size_within_rect(size, constrain_rect).left_top() + offset);
/// }
/// ```
///
/// A stage change is not a sizing pass -- `state.size` is `Some`, it is just
/// the wrong `Some` -- so the new, smaller card is laid out with its top-left
/// where the old card's top-left was: `centre - old_size / 2`, which for a
/// card that shrank in both directions is up and to the left of centre. Only
/// `Prepared::end` stores the new size, and it asks for nothing; the next
/// frame anchors by the new size and the card lands in the middle. The user
/// sees the card in the picker's corner, then sees it move.
///
/// # Why a discard and not a fixed position
///
/// `prefs_ui` avoids all of this by computing its rectangle and placing the
/// area with `fixed_pos` -- see [`movable_modal_at`] -- and that is the right
/// answer for a card whose size is known before it is laid out. This card's
/// is not: it is four cards, each sized by its type, and measuring them ahead
/// of layout would be a second layout kept in step with the first by hand.
///
/// `Context::request_discard` is egui's own answer to "this pass laid
/// something out in the wrong place": the pass is thrown away, not painted,
/// and run again with the memory the first one wrote -- which now holds the
/// new size, so the second pass anchors correctly. egui caps the passes it
/// will run per frame at `Options::max_passes`, two by default, so a shape
/// that somehow changed on every pass would be painted after two rather than
/// spinning.
///
/// # Keyed on the shape, and NOT on the measured size -- which was tried
///
/// The first draft compared the area's size before and after the show and
/// discarded on any difference. It fired on the **second frame of every
/// modal**, and it cost a keystroke. egui's own first pass of a never-sized
/// area is a sizing pass, laid out invisible with no width constraint, and
/// the size it stores is an estimate that the first real layout then
/// corrects -- a difference, so a discard. And a discarded pass is re-run on
/// `RawInput::take()`, which leaves the persistent fields and **empties the
/// events**: an Escape or a click that arrived on that frame is consumed by
/// the pass that was thrown away and never seen by the one that is kept.
/// `each_card_wears_its_own_footer` caught it -- an Escape on a card's second
/// frame answered nothing.
///
/// So the trigger is the thing that actually changes the card, remembered
/// from the last pass in the context's temporary data under the area's id:
/// a card that is on a different stage than it was drawn on last pass is
/// re-anchored, and a card on the same stage never is, whatever egui
/// measured. That includes the two frames after a first open, where the
/// stage has not changed, and it includes a REOPEN on a different stage than
/// the one the card closed on -- egui keeps `AreaState` across a close, so
/// that reopen would otherwise anchor the new card by the old one's size.
///
/// The one frame this still spends a pass on is the frame after a stage
/// changes, and the events on that frame belong to the discarded pass. That
/// is the frame after the press that changed the stage was answered; a
/// second input arriving within it is a sixteen-millisecond window, and the
/// alternative is the card the owner watched jump.
pub fn settle_reshaped_modal<K>(ctx: &egui::Context, area_id: egui::Id, shape: K)
where
    K: Copy + PartialEq + Send + Sync + 'static,
{
    let key = area_id.with("modal-shape");
    let last: Option<K> = ctx.data(|d| d.get_temp(key));
    ctx.data_mut(|d| d.insert_temp(key, shape));
    if last.is_some_and(|last| last != shape) {
        ctx.request_discard("a modal changed shape, so this pass anchored it by the old one");
    }
}

/// The dimmed, click-eating scrim a modal sits on.
///
/// **The `Area` is the CALLER's, and that is not an accident of style.**
/// `item_list::MODAL_SCRIM_AREAS` is the list that gates this window's arrow
/// keys behind every open modal, and the test that keeps it honest walks
/// `src/` for the literal `Area::new(egui::Id::new("..."))` declaration of
/// every id ending in `-scrim`. An id passed to this function as a bare
/// string would be invisible to that walk, and the gate would silently stop
/// covering the modal that moved. So the declaration stays at the call site
/// and only the drawing moves here.
///
/// **`content_rect().min`, not `Pos2::ZERO`.** An `Area`'s stored rect is
/// `fixed_pos + what it allocated`, and *that* is what `Memory::layer_id_at`
/// hit-tests -- the painted rectangle below is a separate thing. Anchored at
/// the origin the two agree only while `content_rect()` starts there, and
/// where it does not the scrim looks whole while blocking a box that starts
/// at the wrong corner. `prefs_ui::draw_prefs_modal` records the measurement;
/// this is the same fix, applied once for every caller.
/// **The order a scrim sits on, and it is BELOW the card it dims.**
///
/// # The freeze this fixes
///
/// The owner: "if open Send popup and then click outside of the parent app -
/// it gets dimmed and not responsive, so only restart app".
///
/// Every scrim in this app was on `Order::Foreground`, which is the order its
/// CARD is on. Within one order egui keeps a list and promotes an area to the
/// top of it when the pointer is pressed on that area, when it is dragged or
/// clicked, or when it was not visible on the previous frame
/// (`containers/area.rs:546-551`). All three happen here: the first click
/// anywhere outside the card lands on the scrim, and coming back to an
/// occluded window trips the third.
///
/// Once promoted, the scrim is a full-screen `Sense::click` rectangle ON TOP
/// of the card. It swallows every press from then on, the card can never be
/// reached again, and `record_ui`'s two modals bind no Escape by design -- so
/// the window is dimmed, inert, and only killable. Exactly the report.
///
/// # Why an order and not a re-promotion of the card
///
/// Moving the CARD back to the top each frame would work and is a race: it
/// fixes the symptom one frame after the press that caused it, and it needs
/// every modal to remember to do it. Two different orders cannot interleave
/// at all -- egui sorts by order first and by the within-order list second --
/// so a scrim on `Middle` is below a card on `Foreground` no matter what the
/// pointer does.
///
/// `Middle` and not `Background`: the scrim still has to cover the sidebar,
/// the list and the detail pane, which are ordinary panels on `Background`.
pub const SCRIM_ORDER: egui::Order = egui::Order::Middle;

#[cfg(test)]
mod tests {
    use super::*;

    /// **Every field mark stays inside its own artboard**, ink included.
    ///
    /// The marks are scaled into a row's square gutter by the number
    /// [`FIELD_MARK_ARTBOARD`] gives, so a point outside it is ink outside
    /// the box -- on a card with no scrolling and a hard edge one row away,
    /// that is a mark drawn over its neighbour's text. The stroke counts:
    /// what is checked is the outline plus half of [`FIELD_MARK_STROKE`],
    /// which is where a stroked path's ink actually lands.
    #[test]
    fn no_field_mark_paints_outside_its_artboard() {
        let half = FIELD_MARK_STROKE / 2.0;
        let mut seen = 0;
        for mark in [
            FieldMark::Person,
            FieldMark::Key,
            FieldMark::Clock,
            FieldMark::TabArrow,
            FieldMark::Steps,
            FieldMark::Tag,
        ] {
            let shapes = field_mark_shapes(mark);
            assert!(!shapes.is_empty(), "{mark:?} has no geometry at all, so its gutter is blank");
            for shape in shapes {
                let (lo, hi) = match shape {
                    MarkShape::Circle { centre, radius, .. } => (
                        Pos2::new(centre.x - radius - half, centre.y - radius - half),
                        Pos2::new(centre.x + radius + half, centre.y + radius + half),
                    ),
                    MarkShape::Path { points, .. } => {
                        assert!(points.len() >= 2, "{mark:?} carries a path of fewer than two points");
                        let mut lo = Pos2::new(f32::MAX, f32::MAX);
                        let mut hi = Pos2::new(f32::MIN, f32::MIN);
                        for q in points {
                            lo = Pos2::new(lo.x.min(q.x - half), lo.y.min(q.y - half));
                            hi = Pos2::new(hi.x.max(q.x + half), hi.y.max(q.y + half));
                        }
                        (lo, hi)
                    }
                };
                assert!(
                    lo.x >= 0.0 && lo.y >= 0.0,
                    "{mark:?} paints ink at ({}, {}), left of or above its artboard",
                    lo.x,
                    lo.y
                );
                assert!(
                    hi.x <= FIELD_MARK_ARTBOARD && hi.y <= FIELD_MARK_ARTBOARD,
                    "{mark:?} paints ink at ({}, {}), past its {FIELD_MARK_ARTBOARD}-unit \
                     artboard -- in a row's gutter that is ink over the row beside it",
                    hi.x,
                    hi.y
                );
                seen += 1;
            }
        }
        // CONTROL: the walk actually visited geometry, so the bounds above
        // are not a loop over nothing.
        assert!(seen >= 6, "control: only {seen} shapes were measured across all six marks");
    }

    /// **The six field marks are six different pictures.**
    ///
    /// They sit one under another in the same list, and two rows drawn with
    /// the same mark would say the two offers are the same thing -- which is
    /// exactly the confusion the marks were added to remove.
    #[test]
    fn no_two_field_marks_are_the_same_geometry() {
        let all = [
            FieldMark::Person,
            FieldMark::Key,
            FieldMark::Clock,
            FieldMark::TabArrow,
            FieldMark::Steps,
            FieldMark::Tag,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(
                    field_mark_shapes(*a),
                    field_mark_shapes(*b),
                    "{a:?} and {b:?} draw the same picture"
                );
            }
        }
        // CONTROL: a mark equals itself, so the comparison above is a real
        // one and not `PartialEq` refusing to match anything.
        assert_eq!(field_mark_shapes(FieldMark::Key), field_mark_shapes(FieldMark::Key));
    }

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

    /// A sentence of interface text in every language that needs a script the
    /// bundled Archivo lacks -- or, for Vietnamese, one it must keep -- as
    /// `(tag, text)`. Each is "Enter your master password" or near it.
    const SCRIPT_SAMPLES: [(&str, &str); 7] = [
        ("el", "\u{39a}\u{3c9}\u{3b4}\u{3b9}\u{3ba}\u{3cc}\u{3c2} \u{3c0}\u{3c1}\u{3cc}\u{3c3}\u{3b2}\u{3b1}\u{3c3}\u{3b7}\u{3c2}"),
        ("ru", "\u{413}\u{43b}\u{430}\u{432}\u{43d}\u{44b}\u{439} \u{43f}\u{430}\u{440}\u{43e}\u{43b}\u{44c}"),
        ("vi", "M\u{1ead}t kh\u{1ea9}u ch\u{ed}nh c\u{1ee7}a b\u{1ea1}n \u{111}\u{1ebf}n \u{1ee9}ng d\u{1ee5}ng"),
        ("ja", "\u{30de}\u{30b9}\u{30bf}\u{30fc}\u{30d1}\u{30b9}\u{30ef}\u{30fc}\u{30c9}\u{3092}\u{5165}\u{529b}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}"),
        ("zh-Hans", "\u{8f93}\u{5165}\u{60a8}\u{7684}\u{4e3b}\u{5bc6}\u{7801}"),
        ("zh-Hant", "\u{8f38}\u{5165}\u{60a8}\u{7684}\u{4e3b}\u{5bc6}\u{78bc}"),
        ("ko", "\u{b9c8}\u{c2a4}\u{d130} \u{bc88}\u{d638}\u{b97c} \u{c785}\u{b825}\u{d558}\u{c138}\u{c694}"),
    ];

    /// Whether the machine running the suite has the system faces `tag`
    /// needs. They are Windows' own and present on every desktop edition; a
    /// stripped server image may lack the CJK ones, and a test about drawing
    /// them has nothing to measure there -- said out loud, not skipped quietly.
    fn has_system_faces(tag: &str) -> bool {
        let Some(locale) = crate::i18n::locale(tag) else { return false };
        match system_faces_for(locale.fonts) {
            None => true,
            Some(faces) => {
                let ok = first_system_face(faces.regular).is_some()
                    && first_system_face(faces.bold).is_some();
                if !ok {
                    eprintln!("note: this machine has no system face for {tag}; its drawing test is not run");
                }
                ok
            }
        }
    }

    /// **A language's own script is drawn, in every weight the app asks for.**
    /// Every glyph of the sample must rasterise to something: a missing face
    /// answers egui's replacement box, which has a size, so the check is on
    /// the glyph's own bitmap -- and on the characters, which a replaced one
    /// no longer is.
    #[test]
    fn every_shipped_script_renders_in_every_weight() {
        for (tag, text) in SCRIPT_SAMPLES {
            if !has_system_faces(tag) {
                continue;
            }
            crate::i18n::with_language(tag, || {
                let ctx = ctx_with(font_definitions());
                for family in [
                    FontFamily::Proportional,
                    FontFamily::Name(SEMIBOLD.into()),
                    FontFamily::Name(BOLD.into()),
                    FontFamily::Name(EXTRABOLD.into()),
                ] {
                    let font = FontId::new(15.0, family.clone());
                    ctx.fonts_mut(|f| {
                        let galley = f.layout_no_wrap(text.to_owned(), font, INK);
                        let mut seen = 0;
                        for row in &galley.rows {
                            for glyph in &row.glyphs {
                                if glyph.chr.is_whitespace() {
                                    continue;
                                }
                                seen += 1;
                                assert!(
                                    glyph.uv_rect.size.x > 0.0 && glyph.uv_rect.size.y > 0.0,
                                    "{tag}: {:?} rasterises to nothing in {family:?}",
                                    glyph.chr
                                );
                            }
                        }
                        assert!(seen >= 4, "{tag}: only {seen} glyphs laid out in {family:?}");
                    });
                }
            });
        }
    }

    /// **Only the active language's system faces are loaded.** English holds
    /// none of them -- a CJK face is 14-20 MB and an English session has no use
    /// for it -- and Japanese holds Japanese's, not Korean's.
    #[test]
    fn only_the_active_languages_system_faces_are_loaded() {
        let system = |fonts: &egui::FontDefinitions| {
            let mut names: Vec<String> =
                fonts.font_data.keys().filter(|k| k.starts_with("System-")).cloned().collect();
            names.sort();
            names
        };
        assert!(system(&font_definitions()).is_empty(), "English loads a system script face");
        for tag in ["ru", "vi", "de", "tr", "pl"] {
            crate::i18n::with_language(tag, || {
                assert!(system(&font_definitions()).is_empty(), "{tag} loads a system script face");
            });
        }
        if has_system_faces("ja") {
            crate::i18n::with_language("ja", || {
                let names = system(&font_definitions());
                assert!(!names.is_empty(), "Japanese loads no face");
                assert!(names.iter().all(|n| n.contains("Japanese")), "{names:?}");
            });
        }
        if has_system_faces("el") {
            crate::i18n::with_language("el", || {
                let names = system(&font_definitions());
                assert!(names.iter().all(|n| n.contains("Greek")), "{names:?}");
                assert!(!names.is_empty());
            });
        }
        // The picker's way to draw every language's name: extras on top of
        // the active one.
        let both = font_definitions_for(&[crate::i18n::Fonts::Korean, crate::i18n::Fonts::Japanese]);
        if has_system_faces("ko") && has_system_faces("ja") {
            let names = system(&both);
            assert!(names.iter().any(|n| n.contains("Korean")) && names.iter().any(|n| n.contains("Japanese")), "{names:?}");
        }
    }

    // -----------------------------------------------------------------
    // The Win32/GDI surfaces ui_preview cannot draw.
    // -----------------------------------------------------------------

    /// One string a GDI surface paints: the catalog key, the Archivo family
    /// and logical px size it is drawn in, the width in logical px of the
    /// rect it is drawn into (the arithmetic is in the comment on each group,
    /// read off the painter's own constants), and the extra px a tracked run
    /// adds per character.
    struct Budget {
        key: &'static str,
        family: &'static str,
        px: i32,
        width: i32,
        tracking: i32,
    }

    const fn b(key: &'static str, family: &'static str, px: i32, width: i32) -> Budget {
        Budget { key, family, px, width, tracking: 0 }
    }

    /// Every single-line catalog string the cards, prompts and the tray
    /// popup paint, with the width it has. `DT_END_ELLIPSIS` rows would
    /// truncate and the rest clip; either way text that does not fit is
    /// text the user cannot read, so both are held to the same bound.
    fn gdi_budgets() -> Vec<Budget> {
        let mut v = vec![
            // tray_popup: inner = 390 - 2*16 = 358; footer 350 - 7 - 8 = 335;
            // tiles (358 - 16) / 3 = 114; search edit 374 - 14 - 56 = 304.
            b("tray_popup.locked_title", "Archivo-ExtraBold", 19, 358),
            b("tray_popup.locked_body", "Archivo-Regular", 15, 358),
            b("tray_popup.unlock", "Archivo-Bold", 16, 358),
            b("tray_popup.search", "Archivo-Regular", 15, 304),
            Budget { key: "tray_popup.label_recent", family: "Archivo-ExtraBold", px: 12, width: 350, tracking: 1 },
            Budget { key: "tray_popup.label_results", family: "Archivo-ExtraBold", px: 12, width: 350, tracking: 1 },
            Budget { key: "tray_popup.label_top", family: "Archivo-ExtraBold", px: 12, width: 350, tracking: 1 },
            b("tray_popup.empty_title", "Archivo-Bold", 15, 358),
            b("tray_popup.no_matches_title", "Archivo-Bold", 15, 358),
            b("tray_popup.empty_body", "Archivo-Regular", 13, 358),
            b("tray_popup.no_matches_body", "Archivo-Regular", 13, 358),
            b("tray_popup.tile_generate", "Archivo-Bold", 13, 114),
            b("tray_popup.tile_new_item", "Archivo-Bold", 13, 114),
            b("tray_popup.tile_share", "Archivo-Bold", 13, 114),
            b("tray_popup.footer_locked", "Archivo-Regular", 13, 335),
            b("tray_popup.footer_offline", "Archivo-Regular", 13, 335),
            b("tray_popup.footer_syncing", "Archivo-Regular", 13, 335),
            b("tray_popup.footer_synced", "Archivo-Regular", 13, 335),
            b("tray_popup.footer_synced_one_minute", "Archivo-Regular", 13, 335),
            b("tray_popup.footer_synced_minutes", "Archivo-Regular", 13, 335),
            b("tray_popup.toast_username_copied", "Archivo-Regular", 13, 335),
            b("tray_popup.toast_password_copied", "Archivo-Regular", 13, 335),
            b("tray_popup.toast_no_username", "Archivo-Regular", 13, 335),
            b("tray_popup.toast_no_password", "Archivo-Regular", 13, 335),
            b("tray_popup.toast_cancelled", "Archivo-Regular", 13, 335),
            b("tray_popup.toast_needs_hello", "Archivo-Regular", 13, 335),
            b("tray_popup.toast_gone", "Archivo-Regular", 13, 335),
            // panel_draw: the key tooltip is 230 - 2*14 = 202 wide, one line
            // per hint; the Enter badge costs the row's name lane its width.
            b("panel.hint_choose_account", "Archivo-Regular", 14, 202),
            b("panel.hint_enter_fill", "Archivo-Regular", 14, 202),
            b("panel.hint_esc_close", "Archivo-Regular", 14, 202),
            b("panel.enter_label", "Archivo-Bold", 13, 60),
            // locked_card keys (the unlock prompt's pill and lines): 410 - 2*18
            // = 374; the pill is 64.
            b("locked_card.pill", "Archivo-Bold", 12, 64),
            b("locked_card.title", "Archivo-ExtraBold", 19, 374),
            b("locked_card.body", "Archivo-Regular", 15, 374),
            // prompt_card: a row's text lane is 384 - 3 - 65 = 316 (246 once
            // the Enter badge is on it).
            b("prompt_card.saved_credentials", "Archivo-Regular", 14, 246),
            // notice_card: 374 - 24 = 350 titles; the done card is
            // 320 - 2*28 = 264; OK is 140 minus a chip lane.
            b("notice_card.nothing_typed", "Archivo-ExtraBold", 19, 350),
            b("notice_card.stopped", "Archivo-ExtraBold", 19, 350),
            b("notice_card.ok", "Archivo-Bold", 15, 90),
            b("notice_card.saved_title", "Archivo-ExtraBold", 24, 344),
            b("notice_card.updated_title", "Archivo-ExtraBold", 24, 344),
            b("notice_card.signed_in_title", "Archivo-ExtraBold", 24, 344),
            b("notice_card.saved_sub", "Archivo-Regular", 15, 344),
            b("notice_card.updated_sub", "Archivo-Regular", 15, 344),
            b("notice_card.signed_in_sub", "Archivo-Regular", 15, 344),
            // save_login_card: 370 - 36 = 334; title 272; folder row 256;
            // password caption 250; hints 306; Save loses a chip lane; the
            // pair of buttons is 163 each; Generate is 78 less focus.
            b("save_login.title", "Archivo-ExtraBold", 17, 272),
            b("save_login.folder_row", "Archivo-Regular", 14, 256),
            b("save_login.caption_username", "Archivo-SemiBold", 13, 334),
            b("save_login.caption_password", "Archivo-SemiBold", 13, 250),
            b("save_login.username_hint", "Archivo-Regular", 15, 306),
            b("save_login.password_hint", "Archivo-Regular", 15, 306),
            b("save_login.save", "Archivo-Bold", 15, 113),
            b("save_login.not_now", "Archivo-Bold", 15, 159),
            b("save_login.generate", "Archivo-SemiBold", 13, 74),
            b("save_login.never", "Archivo-SemiBold", 14, 334),
            // picker_prompt: 400 - 32 = 368; a palette row's lane is
            // 368 - 3 - 65 - 24 = 276 (206 selected); the search rows 285;
            // the footer buttons 180 less their chip lane; hints 202.
            b("picker.search_title", "Archivo-ExtraBold", 19, 368),
            // The No-match card (D 1.5) is 410 wide with pad 18: 374; its two
            // buttons are (374 - 8) / 2 = 183 less their 20 px of padding.
            b("picker.nomatch_title", "Archivo-ExtraBold", 19, 374),
            b("picker.search_sub", "Archivo-Regular", 15, 368),
            b("picker.nomatch_body", "Archivo-Regular", 15, 374),
            b("picker.send_all", "Archivo-Bold", 16, 206),
            b("picker.send_sequence", "Archivo-Bold", 16, 206),
            b("picker.send_all_sub", "Archivo-Regular", 14, 206),
            b("picker.send_sequence_sub", "Archivo-Regular", 14, 206),
            b("picker.send_username_sub", "Archivo-Regular", 14, 206),
            b("picker.send_password_sub", "Archivo-Regular", 14, 206),
            b("picker.send_totp_sub", "Archivo-Regular", 14, 206),
            b("picker.send_custom_sub", "Archivo-Regular", 14, 206),
            b("picker.overflow_title", "Archivo-Bold", 16, 285),
            b("picker.overflow_sub", "Archivo-Regular", 14, 285),
            b("picker.nothing_title", "Archivo-Bold", 16, 285),
            b("picker.nothing_sub", "Archivo-Regular", 14, 285),
            b("picker.save_new_login", "Archivo-Bold", 15, 163),
            b("picker.search_vault", "Archivo-Bold", 15, 163),
            // generate_prompt: 410 - 36 = 374; see the painter for each cell.
            b("generate.suggested", "Archivo-Regular", 15, 286),
            b("generate.failed", "Archivo-Regular", 12, 374),
            b("generate.length", "Archivo-Regular", 12, 187),
            b("generate.words", "Archivo-Regular", 12, 187),
            b("generate.characters", "Archivo-Regular", 12, 374),
            b("generate.min_digits", "Archivo-Regular", 12, 278),
            b("generate.avoid_lookalikes", "Archivo-Regular", 12, 324),
            b("generate.kind_password", "Archivo-SemiBold", 12, 78),
            b("generate.kind_passphrase", "Archivo-SemiBold", 12, 72),
            b("generate.regenerate", "Archivo-SemiBold", 12, 87),
            b("generate.use_password", "Archivo-Bold", 15, 246),
            b("generate.copy", "Archivo-Bold", 15, 94),
            // unlock_prompt: 470 - 52 = 418; label 386; buttons 108 / 88.
            Budget { key: "unlock_prompt.tagline", family: "Archivo-Bold", px: 10, width: 367, tracking: 2 },
            b("unlock_prompt.title", "Archivo-Bold", 19, 418),
            b("unlock_prompt.subtitle", "Archivo-Regular", 13, 418),
            b("unlock_prompt.label", "Archivo-Regular", 12, 386),
            b("unlock_prompt.unlock", "Archivo-SemiBold", 13, 120),
            b("unlock_prompt.cancel", "Archivo-SemiBold", 13, 84),
        ];
        v.shrink_to_fit();
        v
    }

    /// The width, in device px at 100 %, GDI gives `text` in `family` at
    /// `px` -- font linking to the system's CJK face included, which is the
    /// whole reason this is measured by GDI and not by egui.
    fn gdi_width(family: &str, px: i32, text: &str) -> i32 {
        use windows::Win32::Graphics::Gdi::{
            CreateCompatibleDC, CreateFontIndirectW, DeleteDC, DeleteObject, CLEARTYPE_QUALITY, FW_BOLD, FW_NORMAL,
            LOGFONTW,
        };
        crate::win32_draw::register_fonts();
        let (face, weight) = crate::win32_draw::gdi_face_for_text(family, text);
        let mut lf = LOGFONTW {
            lfHeight: -px,
            lfWeight: if weight >= 700 { FW_BOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
            lfQuality: CLEARTYPE_QUALITY,
            ..Default::default()
        };
        for (i, ch) in face.encode_utf16().take(31).enumerate() {
            lf.lfFaceName[i] = ch;
        }
        let units: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let hdc = CreateCompatibleDC(None);
            let font = CreateFontIndirectW(&lf);
            let width = crate::win32_draw::text_width_utf16(hdc, font, &units);
            let _ = DeleteObject(font);
            let _ = DeleteDC(hdc);
            width
        }
    }

    /// The catalog text for `key` as the surface would paint it, with the
    /// widest plausible placeholder values: a plural key takes its widest
    /// form.
    fn painted_text(key: &'static str) -> Vec<String> {
        let args: [(&str, &str); 6] = [
            ("minutes", "10080"),
            ("shown", "4"),
            ("total", "9999"),
            ("app_name", "Ledgerline Desktop"),
            ("folder", "Personal"),
            ("n", "9999"),
        ];
        let one = crate::i18n::tf(key, &args);
        if one != key && !one.contains("missing") {
            return vec![one];
        }
        [1u64, 2, 5, 22, 100].iter().map(|n| crate::i18n::tn(key, *n, &args)).collect()
    }

    /// **Every string a GDI card, prompt or the tray popup paints fits the
    /// rect it is drawn into, in every language.** ui_preview cannot draw
    /// these surfaces, so each string is measured by GDI itself with the face
    /// and size the surface creates and checked against the width its layout
    /// constants leave. The failure lists every key and language that does
    /// not fit, with the measured and available widths.
    #[test]
    fn every_gdi_surface_string_fits_its_rect_in_every_language() {
        let mut misfits = Vec::new();
        let mut truncated = 0;
        for locale in crate::i18n::available() {
            crate::i18n::with_language(locale.tag, || {
                for budget in gdi_budgets() {
                    for text in painted_text(budget.key) {
                        let chars = text.chars().count() as i32;
                        let width = gdi_width(budget.family, budget.px, &text) + budget.tracking * chars;
                        if width > budget.width && is_prose(budget.key) {
                            truncated += 1;
                        } else if width > budget.width {
                            misfits.push(format!(
                                "{} {}: {width}px > {}px ({} {}px) {text:?}",
                                locale.tag, budget.key, budget.width, budget.family, budget.px
                            ));
                        }
                    }
                }
            });
        }
        eprintln!("{truncated} sentence lines end in an ellipsis in some language (see is_prose)");
        assert!(misfits.is_empty(), "{} labels overflow:\n{}", misfits.len(), misfits.join("\n"));
    }

    /// **Sentences versus labels.** A sentence line (a subtitle, a hint, a
    /// caption, a toast, a row's secondary text) that is too long for its one
    /// line is ellipsised by [`crate::win32_draw::draw_text`] and the reader
    /// still gets the start of it; the surfaces are fixed-size Win32 windows
    /// and cannot wrap. A LABEL -- a button, a pill, a segment cell, a title --
    /// cut to "Regener..." is a control nobody can read, so labels must fit.
    fn is_prose(key: &str) -> bool {
        const PROSE: [&str; 23] = [
            "_sub", "_body", "_hint", "hint_", "folder_row", "empty_title", "toast_", "footer_", "suggested",
            "failed", "saved_credentials", "tray_popup.label_", "caption_", "generate.length", "generate.words",
            "generate.characters", "generate.min_digits", "avoid_lookalikes", "unlock_prompt.label",
            "unlock_prompt.tagline", "picker.send_", "locked_card.body", "unlock_prompt.subtitle",
        ];
        PROSE.iter().any(|needle| key.contains(needle))
            || matches!(key, "picker.overflow_title" | "picker.nothing_title" | "picker.search_sub" | "picker.nomatch_body")
    }

    /// The `(top, bottom)` of the ink of every drawn glyph in a galley, one
    /// pair per row, in the galley's own coordinates.
    fn ink_rows(galley: &egui::Galley) -> Vec<(f32, f32)> {
        galley
            .rows
            .iter()
            .map(|row| {
                let mut top = f32::INFINITY;
                let mut bottom = f32::NEG_INFINITY;
                for glyph in &row.glyphs {
                    if glyph.uv_rect.is_nothing() || glyph.chr.is_whitespace() {
                        continue;
                    }
                    let at = row.pos.y + glyph.pos.y + glyph.uv_rect.offset.y;
                    top = top.min(at);
                    bottom = bottom.max(at + glyph.uv_rect.size.y);
                }
                (top, bottom)
            })
            .collect()
    }

    /// The bottom of the ink of `text` set on one line in `family`.
    fn ink_bottom(ctx: &egui::Context, family: &FontFamily, size: f32, text: &str) -> f32 {
        ctx.fonts_mut(|f| {
            let galley = f.layout_no_wrap(text.to_owned(), FontId::new(size, family.clone()), INK);
            ink_rows(&galley)[0].1
        })
    }

    /// **A fallback script sits on Archivo's baseline.** egui places a
    /// fallback glyph at `its own ascent + half the difference of the two
    /// faces' line boxes`, which is level only when both faces split their box
    /// alike; Yu Gothic did not (0.88 above, 0.22 + 0.5 em gap below), and every
    /// Japanese glyph rode 0.255 em -- 4 px at body size -- above the Latin
    /// beside it. Flat-bottomed capitals (H / Eta / En) must end on the same
    /// line, and a CJK glyph's frame must end just under it.
    #[test]
    fn a_fallback_script_sits_on_archivos_baseline() {
        let weights = [
            FontFamily::Proportional,
            FontFamily::Name(SEMIBOLD.into()),
            FontFamily::Name(BOLD.into()),
            FontFamily::Name(EXTRABOLD.into()),
        ];
        // Flat-bottomed capital letters of each alphabet.
        for (tag, capital) in [("el", "\u{397}"), ("ru", "\u{41d}")] {
            if !has_system_faces(tag) {
                continue;
            }
            crate::i18n::with_language(tag, || {
                let ctx = ctx_with(font_definitions());
                for family in &weights {
                    for size in [11.0_f32, 13.0, 15.0, 22.0, 32.0] {
                        let latin = ink_bottom(&ctx, family, size, "H");
                        let other = ink_bottom(&ctx, family, size, capital);
                        assert!(
                            (latin - other).abs() <= 0.08 * size + 0.6,
                            "{tag} {family:?} {size}px: the capital ends at {other}, Latin H at {latin}"
                        );
                    }
                }
            });
        }
        // CJK: the glyph frame ends a little UNDER the baseline (the design
        // box has its own descent), never above it by an em fraction.
        for (tag, glyph) in [("ja", "\u{56fd}"), ("zh-Hans", "\u{56fd}"), ("zh-Hant", "\u{570b}"), ("ko", "\u{d55c}")] {
            if !has_system_faces(tag) {
                continue;
            }
            crate::i18n::with_language(tag, || {
                let ctx = ctx_with(font_definitions());
                for family in &weights {
                    for size in [11.0_f32, 13.0, 15.0, 22.0, 32.0] {
                        let latin = ink_bottom(&ctx, family, size, "H");
                        let cjk = ink_bottom(&ctx, family, size, glyph);
                        let below = (cjk - latin) / size;
                        assert!(
                            (-0.10..=0.22).contains(&below) || (cjk - latin).abs() <= 0.6,
                            "{tag} {family:?} {size}px: the glyph frame ends {below:.3} em below the Latin baseline"
                        );
                    }
                }
            });
        }
    }

    /// **A wrapped CJK paragraph's lines do not touch.** The row pitch is
    /// Archivo's (the Latin tests pin it), so what must hold is that the ink of
    /// one row ends above the ink of the next and that the pitch is at least
    /// the glyphs' own height.
    #[test]
    fn a_wrapped_cjk_paragraph_never_overlaps_its_own_lines() {
        let text = "\u{4e00}\u{5ea6}\u{30ed}\u{30b0}\u{30a4}\u{30f3}\u{3059}\u{308b}\u{3060}\u{3051}\u{3002}Bitwarden \u{3084} SAP \u{306a}\u{3069}\u{306e}\u{30ed}\u{30b0}\u{30a4}\u{30f3}\u{3082}Deskwarden \u{304c}\u{5165}\u{529b}\u{3057}\u{307e}\u{3059}\u{3002}";
        let hans = "\u{5e94}\u{7528}\u{7a0b}\u{5e8f}\u{4f1a}\u{81ea}\u{52a8}\u{586b}\u{5199}\u{60a8}\u{7684}\u{767b}\u{5f55}\u{4fe1}\u{606f}\u{ff0c}Bitwarden \u{5bc6}\u{7801}\u{5e93}\u{5728}\u{89e3}\u{9501}\u{540e}\u{53ef}\u{7528}\u{3002}";
        let hant = "\u{61c9}\u{7528}\u{7a0b}\u{5f0f}\u{6703}\u{81ea}\u{52d5}\u{586b}\u{5beb}\u{60a8}\u{7684}\u{767b}\u{5165}\u{8cc7}\u{8a0a}\u{ff0c}Bitwarden \u{5bc6}\u{78bc}\u{5eab}\u{89e3}\u{9396}\u{5f8c}\u{53ef}\u{7528}\u{3002}";
        let hangul = "\u{c571}\u{c774} \u{b85c}\u{adf8}\u{c778} \u{c815}\u{bcf4}\u{b97c} \u{c790}\u{b3d9}\u{c73c}\u{b85c} \u{c785}\u{b825}\u{d569}\u{b2c8}\u{b2e4}. Bitwarden \u{bcf4}\u{ad00}\u{d568}\u{c744} \u{c5f4}\u{c5b4} \u{c8fc}\u{c138}\u{c694}.";
        for (tag, text) in [("ja", text), ("zh-Hans", hans), ("zh-Hant", hant), ("ko", hangul)] {
            if !has_system_faces(tag) {
                continue;
            }
            crate::i18n::with_language(tag, || {
                let ctx = ctx_with(font_definitions());
                for family in [FontFamily::Proportional, FontFamily::Name(BOLD.into())] {
                    for size in [11.0_f32, 12.0, 13.0, 15.0, 22.0] {
                        ctx.fonts_mut(|f| {
                            let mut job = egui::text::LayoutJob::simple(
                                text.to_owned(),
                                FontId::new(size, family.clone()),
                                INK,
                                size * 7.0,
                            );
                            job.wrap.max_width = size * 7.0;
                            let galley = f.layout_job(job);
                            assert!(galley.rows.len() >= 3, "{tag} {size}px: {} rows", galley.rows.len());
                            let ink = ink_rows(&galley);
                            for pair in 0..ink.len() - 1 {
                                let pitch = galley.rows[pair + 1].pos.y - galley.rows[pair].pos.y;
                                let glyph_height = ink[pair].1 - ink[pair].0;
                                assert!(
                                    pitch >= glyph_height,
                                    "{tag} {family:?} {size}px row {pair}: line pitch {pitch} < glyph height {glyph_height}"
                                );
                                assert!(
                                    ink[pair].1 <= ink[pair + 1].0 + 0.5,
                                    "{tag} {family:?} {size}px: row {pair} ink ends at {} but the next starts at {}",
                                    ink[pair].1,
                                    ink[pair + 1].0
                                );
                            }
                        });
                    }
                }
            });
        }
    }

    /// **Greek is drawn from one face.** Archivo carries four Greek letters
    /// (Delta, Omega, mu, pi); left mapped they were drawn from Archivo while
    /// the rest of the word came from Segoe UI. With Greek active they are
    /// unmapped and fall through to Segoe, so every Greek letter has the
    /// advance Segoe gives it; outside a Greek session nothing changes.
    #[test]
    fn greek_letters_all_come_from_the_greek_face() {
        if !has_system_faces("el") {
            return;
        }
        let segoe_only = {
            let mut fonts = egui::FontDefinitions::default();
            let mut data = first_system_face(&[("segoeui.ttf", 0)]).expect("Segoe UI");
            data.tweak.y_offset_factor = 0.0;
            fonts.font_data.insert("S".to_owned(), Arc::new(data));
            fonts.families.insert(FontFamily::Proportional, vec!["S".to_owned()]);
            fonts
        };
        let width = |ctx: &egui::Context, text: &str| {
            ctx.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), FontId::new(30.0, FontFamily::Proportional), INK).size().x)
        };
        let reference = ctx_with(segoe_only);
        let greek = crate::i18n::with_language("el", || ctx_with(font_definitions()));
        let english = ctx_with(font_definitions());
        for letter in ["\u{3c0}", "\u{3bc}", "\u{394}", "\u{3a9}", "\u{3b1}\u{3c0}\u{3b1}"] {
            assert_eq!(width(&greek, letter), width(&reference, letter), "{letter:?} is not drawn by Segoe UI in Greek");
        }
        assert_ne!(
            width(&english, "\u{3c0}"),
            width(&reference, "\u{3c0}"),
            "outside Greek, pi must stay where it was (Archivo)"
        );
    }

    /// **Latin does not move in any language.** A system face goes BEHIND each
    /// Archivo cut, so "Password" in a Japanese session measures exactly what
    /// it does in an English one.
    #[test]
    fn latin_text_measures_the_same_in_every_language() {
        let measure = || {
            let ctx = ctx_with(font_definitions());
            let font = FontId::new(15.0, FontFamily::Name(SEMIBOLD.into()));
            ctx.fonts_mut(|f| f.layout_no_wrap("Password 0123 Unlock".to_owned(), font, INK).size())
        };
        let english = measure();
        for tag in ["el", "ja", "zh-Hans", "zh-Hant", "ko", "ru", "vi"] {
            if !has_system_faces(tag) {
                continue;
            }
            assert_eq!(crate::i18n::with_language(tag, measure), english, "{tag} moved the Latin text");
        }
    }

    /// Each system face sits behind its Archivo cut and its Cyrillic face and
    /// ahead of egui's defaults, exactly where the Cyrillic faces sit.
    #[test]
    fn system_faces_sit_behind_archivo_and_ahead_of_eguis_fallbacks() {
        if !has_system_faces("ja") {
            return;
        }
        let fonts = crate::i18n::with_language("ja", font_definitions);
        for family in [
            FontFamily::Proportional,
            FontFamily::Name(SEMIBOLD.into()),
            FontFamily::Name(BOLD.into()),
            FontFamily::Name(EXTRABOLD.into()),
        ] {
            let stack = &fonts.families[&family];
            let at = stack.iter().position(|f| f.starts_with("System-Japanese")).expect("a Japanese face");
            assert_eq!(at, 2, "{family:?}: {stack:?}");
            assert!(stack[0].starts_with("Archivo") && stack[1].starts_with("NotoSans-Cyrillic"), "{stack:?}");
        }
        // And the end of the monospace family, behind Consolas and Hack.
        let mono = &fonts.families[&FontFamily::Monospace];
        assert!(mono.last().unwrap().starts_with("System-Japanese"), "{mono:?}");
    }

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

    /// The v2 shield's ink spans x 0.5..23.5 and y 1..27.2 of its 24x28 box
    /// (design 1c): a wider body and a deeper tip than v1's 2..22 by 2..26.
    #[test]
    fn quadrant_outlines_stay_inside_the_v2_ink_box() {
        for outline in quadrant_outlines() {
            for p in outline {
                assert!(
                    (0.49..=23.51).contains(&p.x) && (0.99..=27.21).contains(&p.y),
                    "point {p:?} escapes the v2 shield's ink box"
                );
            }
        }
        let xs = quadrant_outlines().iter().flatten().map(|p| p.x);
        let (lo, hi) = xs.fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
        assert!((lo - 0.5).abs() < 0.01 && (hi - 23.5).abs() < 0.01, "the body is {lo}..{hi} wide");
    }

    #[test]
    fn the_quadrants_meet_at_the_centre_and_the_tip() {
        let near = |a: Pos2, b: Pos2| (a.x - b.x).abs() < 0.01 && (a.y - b.y).abs() < 0.01;
        let outlines = quadrant_outlines();
        for outline in outlines {
            assert!(outline.iter().any(|p| near(*p, Pos2::new(12.0, 14.0))), "{outline:?}");
        }
        for bottom in &outlines[2..] {
            assert!(bottom.iter().any(|p| near(*p, Pos2::new(12.0, 27.2))));
        }
    }

    #[test]
    fn reversed_and_sidebar_colourways_match_the_design() {
        assert_eq!(MARK_REVERSED, [Color32::WHITE, BLUE_EDGE, BLUE_SOFT, BLUE_BRIGHT]);
        assert_eq!(mark_on_sidebar(Color32::WHITE), [Color32::WHITE, BLUE_BRIGHT, BLUE_SOFT, BLUE_EDGE]);
        assert_eq!(mark_on_sidebar(BLUE_DEEP)[0], BLUE_DEEP);
    }

    /// The ramp in design 1c, down to the 14 px tray and field badge: at the
    /// display scales Windows offers, the mark never paints outside its box.
    #[test]
    fn the_mark_stays_inside_its_box_at_every_ramp_size() {
        for (w, h) in [(14.0, 16.0), (22.0, 26.0), (32.0, 37.0), (48.0, 56.0)] {
            for ppp in [1.0, 1.25, 1.5] {
                let ctx = egui::Context::default();
                ctx.set_pixels_per_point(ppp);
                let target = Rect::from_min_size(Pos2::new(10.3, 7.7), Vec2::new(w, h));
                let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    paint_mark(ui.painter(), target);
                });
                let mut points = Vec::new();
                for clipped in &output.shapes {
                    if let egui::Shape::Path(path) = &clipped.shape {
                        points.extend(path.points.iter().copied());
                    }
                }
                assert!(!points.is_empty(), "nothing painted at {w}x{h}");
                for p in points {
                    assert!(target.expand(0.01).contains(p), "{p:?} outside {target:?} at {ppp}");
                }
            }
        }
    }

    #[test]
    fn window_icon_is_opaque_in_the_shield_and_transparent_outside() {
        let icon = window_icon();
        assert_eq!((icon.width, icon.height), (64, 64));
        assert_eq!(icon.rgba.len(), 64 * 64 * 4);
        let alpha = |x: usize, y: usize| icon.rgba[(y * 64 + x) * 4 + 3];
        // Corners are outside the shield; the center is deep inside it.
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(63, 0), 0);
        assert_eq!(alpha(0, 63), 0);
        assert_eq!(alpha(63, 63), 0);
        assert_eq!(alpha(32, 28), 255);
    }

    /// **The window's icon and the exe's are one shield.** They are drawn by
    /// two programs -- this module at runtime, `assets/generate-icon.py` into
    /// the `.ico` -- and they once framed it differently: the generator spread
    /// the 24-unit viewbox across the whole square, this module kept its
    /// proportions, and two taskbar buttons showed one fat shield and one
    /// slim. The full one is the chosen one. Compared here against the
    /// `.ico`'s own 64 px frame, pixel by pixel, in coverage.
    #[test]
    fn the_window_icon_is_the_shield_the_exe_carries() {
        let ico = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/deskwarden.ico"))
            .expect("assets/deskwarden.ico");
        let u16_at = |at: usize| u16::from_le_bytes([ico[at], ico[at + 1]]) as usize;
        let u32_at = |at: usize| {
            u32::from_le_bytes([ico[at], ico[at + 1], ico[at + 2], ico[at + 3]]) as usize
        };
        let entry = (0..u16_at(4))
            .map(|i| 6 + 16 * i)
            .find(|&e| ico[e] == 64)
            .expect("the .ico has a 64 px frame");
        // A 40-byte BITMAPINFOHEADER, then BGRA rows from the bottom up.
        let pixels = u32_at(entry + 12) + 40;
        let ico_alpha = |x: usize, y: usize| ico[pixels + ((63 - y) * 64 + x) * 4 + 3];

        let icon = window_icon();
        let ours = |x: usize, y: usize| icon.rgba[(y * 64 + x) * 4 + 3];
        let mut apart = Vec::new();
        for y in 0..64 {
            for x in 0..64 {
                let (a, b) = (ours(x, y), ico_alpha(x, y));
                // The two flatten the curves differently, so an edge pixel
                // may differ by a few samples; a stretched shield differs by
                // whole columns of full coverage.
                if a.abs_diff(b) > 64 {
                    apart.push((x, y, a, b));
                }
            }
        }
        assert!(apart.is_empty(), "the window icon and the .ico differ at {apart:?}");
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

    // -- the multiple-choice row ------------------------------------------

    // Every press test below runs a settling frame first and then the frame
    // carrying the click, rather than going through a helper that does both.
    // That is not repetition for its own sake: egui decides what a pointer is
    // over from the widget rects of the frame BEFORE the one carrying the
    // press -- a control clicked on its very first frame has never been
    // anywhere for the pointer to be over -- and each of those tests wants
    // the settling frame's own painted cells to aim at, so the two frames are
    // written out where the geometry between them is used.

    /// An indented block draws its text and nothing beside it: egui's default
    /// guide line hung left of the Sends pane's empty notice as a stray bar.
    #[test]
    fn an_indented_block_draws_no_guide_line_beside_it() {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        apply(&ctx);
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.indent("guide", |ui| {
                ui.label("You have no Sends.");
            });
        });
        let lines = output
            .shapes
            .iter()
            .filter(|c| matches!(c.shape, egui::Shape::LineSegment { .. }))
            .count();
        assert_eq!(lines, 0, "the indent drew a guide line");
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
mod pixel_snapping_tests {
    use super::*;

    /// **THE REPORT: "Amazon has 5 pixels to the left and 6 to the right",
    /// and the same 5/6 top to bottom.** The artwork was centred correctly;
    /// the TILE was at a fractional coordinate, so a symmetric 6pt gap
    /// rasterised as 5 one side and 6 the other.
    ///
    /// Asserted on the geometry the report describes, end to end: a 32pt tile
    /// at x = 79.33, a 20pt icon centred in it, and the claim that both gaps
    /// come out equal and whole.
    #[test]
    fn a_tile_at_a_fractional_coordinate_gives_its_artwork_two_equal_gaps() {
        // THE PREMISE, asserted first: plain centring really does produce the
        // uneven gaps this exists to remove. Without this the loop below could
        // pass against arithmetic that never had the defect, which is exactly
        // how the tile-snapping pass shipped believing it had fixed this.
        {
            let tile = Rect::from_min_size(Pos2::new(79.2, 68.0), Vec2::splat(32.0));
            let naive = Rect::from_center_size(tile.center(), Vec2::splat(20.0));
            let px = |v: f32| v * 1.25;
            assert_ne!(
                px(naive.left()) - px(tile.left()),
                (px(tile.right()) - px(naive.right())).round(),
                "plain centring already lands on the pixel grid at 1.25x, so this test \
                 proves nothing about the arithmetic that replaced it"
            );
        }

        // Every scale factor a Windows display offers, because the defect is
        // invisible at 1.0 -- 6pt is 6px there and splits evenly by luck. A
        // test that only ran at 1.0 is what let the first fix ship.
        for ppp in [1.0f32, 1.25, 1.5, 1.75, 2.0] {
            let allocated = Rect::from_min_size(Pos2::new(79.33, 68.4), Vec2::splat(32.0));
            let tile = Rect::from_min_size(snapped_min(allocated.min, ppp), allocated.size());
            let (art, _) = avatar_artwork(tile, Vec2::splat(48.0), ppp);

            let px = |v: f32| (v * ppp).round();
            let (left, right) = (px(art.left()) - px(tile.left()), px(tile.right()) - px(art.right()));
            let (top, bottom) = (px(art.top()) - px(tile.top()), px(tile.bottom()) - px(art.bottom()));
            assert_eq!(
                (left, top),
                (right, bottom),
                "at {ppp}x the artwork sits {left}px from the left and {right}px from the \
                 right, {top}px from the top and {bottom}px from the bottom -- which is \
                 the report, in the units the owner counted it in"
            );
            for (edge, name) in
                [(art.left(), "left"), (art.top(), "top"), (art.right(), "right"), (art.bottom(), "bottom")]
            {
                let pixels = edge * ppp;
                assert!(
                    (pixels - pixels.round()).abs() < 1e-3,
                    "at {ppp}x the artwork's {name} edge is at {pixels} device pixels, so it \
                     is drawn with a soft fringe on that side"
                );
            }
        }
    }

    /// Rounded in PHYSICAL pixels, not points. At 150% scaling a whole point
    /// is two thirds of a pixel, so rounding to points would leave exactly the
    /// fringe this removes -- and this is the assertion that says which of the
    /// two is meant.
    #[test]
    fn snapping_lands_on_a_device_pixel_not_on_a_whole_point() {
        let snapped = snapped_min(Pos2::new(79.33, 68.4), 1.5);
        assert_ne!(
            (snapped.x.fract(), snapped.y.fract()),
            (0.0, 0.0),
            "both axes landed on whole POINTS, which is the rounding this test rules out; \
             at 1.5x a whole point is a pixel and a half"
        );
        for (v, axis) in [(snapped.x, "x"), (snapped.y, "y")] {
            let pixels = v * 1.5;
            assert!(
                (pixels - pixels.round()).abs() < 1e-4,
                "{axis} = {v} is {pixels} device pixels, which is not a whole one"
            );
        }
    }

    /// A degenerate scale factor cannot be divided back out. The position is
    /// returned untouched rather than becoming NaN and taking the tile with
    /// it.
    #[test]
    fn a_non_positive_scale_factor_leaves_the_position_alone() {
        let at = Pos2::new(79.33, 68.4);
        assert_eq!(snapped_min(at, 0.0), at);
        assert_eq!(snapped_min(at, -1.0), at);
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

    // The identifying features of the icons the probes look for. The kit draws
    // them with the same counts; they live here, with the probes that read
    // them, so a retuned shape the probes stopped finding fails a test.
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
    const STAR_VERTICES: usize = STAR_CORNERS * (STAR_ROUND_SEGMENTS + 1);

    /// Samples along the eye's upper lid.
    const EYE_LID_SEGMENTS: usize = 12;

    /// Vertices in the eye's almond outline: the upper lid sampled
    /// `EYE_LID_SEGMENTS + 1` times, and the lower lid the `EYE_LID_SEGMENTS - 1`
    /// times strictly between the two corners it shares with it.
    const EYE_VERTICES: usize = EYE_LID_SEGMENTS * 2;

    /// The kebab's dot radius. Also what tells its three circles apart from the
    /// eye's pupil, which is deliberately a different size.
    const KEBAB_DOT_RADIUS: f32 = 1.7;

    /// Vertices in the envelope's body: a plain rectangle, stroked as a closed
    /// path rather than emitted as an `egui::Shape::Rect` **so that
    /// [`icon_probe`] can find it the way it finds every other mark in this
    /// family** -- by point count. A `Rect` shape would be indistinguishable
    /// from the strip's own white fill and every card behind it.
    const ENVELOPE_VERTICES: usize = 4;

    /// Vertices in the envelope's flap: the two top corners and the point the
    /// fold meets in the middle.
    ///
    /// Three, which is also the point count `icon_probe::stars` walks for a
    /// star's fill triangles -- and it does not collide, because that probe
    /// additionally requires a non-transparent `fill` and this path is stroked
    /// with no fill at all. [`the_envelope_flap_is_not_findable_as_a_star_fill`]
    /// is the live guard on that, not this comment.
    const ENVELOPE_FLAP_VERTICES: usize = 3;

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
    const FOLDER_VERTICES: usize = 6;

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
    const TUNE_FADERS: usize = 3;

    /// Half the length of each fader's track, so each spans 16.7px inside the
    /// 28px button and the mark stands 18.0px tall with its stroke -- the extent
    /// the drawn-icon family shares.
    const TUNE_TRACK_HALF_HEIGHT: f32 = 8.35;

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

    /// The mock's `ic('share', 21, ...)`: the Share control's glyph box.
    const SHARE_GLYPH: f32 = 21.0;

    /// The head of the header's Share glyph: `r="3.7"` in the 24-unit box, at
    /// [`SHARE_GLYPH`]. What `icon_probe::share_controls` finds the control by --
    /// a ring radius nothing else in this crate strokes (the clock is 8.5, the
    /// kebab's dots 1.7 and filled, the dialog's own 18-point glyph 2.8).
    const SHARE_HEAD_RADIUS: f32 = 3.7 * SHARE_GLYPH / 24.0;

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

    /// The header's Share controls (mock 6a), each as its head's ring and
    /// the colour it was stroked in -- the only way `share_record_button` is
    /// visible to a test, since it paints no fill and no string.
    ///
    /// **Found by the HEAD**, at [`SHARE_HEAD_RADIUS`], for [`clocks`]'s
    /// reason: a ring radius is a thing one mark owns, where the plus's two
    /// short axis-aligned segments are the shape the clock's hands also are.
    pub fn share_controls(shape: &egui::Shape) -> Vec<(Rect, Color32)> {
        let mut out = Vec::new();
        walk_rings(shape, SHARE_HEAD_RADIUS, &mut out);
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
            arms.len().is_multiple_of(2),
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


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
    Shadow, Stroke, StrokeKind, TextStyle, Ui, Vec2,
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
/// Border of a [`DANGER_WASH`] surface.
pub const DANGER_EDGE: Color32 = Color32::from_rgb(0xe8, 0xa9, 0xa2);
/// Text on a [`DANGER_WASH`] surface. Darker than [`ERROR`], which is red on
/// white; this is red on pink and needs the extra depth to hold its contrast.
pub const DANGER_INK: Color32 = Color32::from_rgb(0x8c, 0x3c, 0x33);

/// **The ink on a [`secret_band`]. The one red, and a caller that is not
/// drawing a secret is wrong.**
///
/// An ALIAS of [`DANGER_INK`] and deliberately not a fourth hex literal. The
/// value was written out privately three times in this crate before it was
/// written here once -- `preflight_card::SECRET_INK`,
/// `detail_edit::SECRET_STEP_INK`, and `DANGER_INK` itself -- and the fourth
/// copy is the one that would have drifted, exactly as
/// `preflight_card::SECRET_EDGE` already has (`#f2dedb` where its two
/// neighbours are `#e8a9a2`; two spellings of one token, and nothing on screen
/// says which is right).
///
/// It is named separately from `DANGER_INK` because the two say different
/// things about a caller. `DANGER_INK` is a PALETTE entry -- the ink of any
/// tinted red surface, which includes a `Revoked` pill and a refusal band.
/// `SECRET_INK` is a ROLE: design 4e's rule is that red means "this is a
/// secret and not ordinary UI", and the value of one red is that exactly one
/// kind of thing wears it. Reaching for this constant on something that is not
/// a secret spends the only signal the product has.
pub const SECRET_INK: Color32 = DANGER_INK;

/// §5a's quiet red: the ink on the password row's tag and on the masked value
/// beside it (`#a2554d`).
///
/// A fourth entry in this trio and not a re-use of [`DANGER_INK`], because
/// §5a draws both on the same row and means two different things by them: the
/// name of the field is the loud one, and the tag and the mask are the two
/// pieces of evidence under it. Collapsing them would make a row with three
/// red things on it shout three times.
pub const DANGER_QUIET: Color32 = Color32::from_rgb(0xa2, 0x55, 0x4d);

/// §5a's tag ground on the password row (`#fbeae8`) -- a step darker than
/// [`DANGER_WASH`], which is the row it sits on, so the tag reads as a tag
/// and not as a gap in the tint.
pub const DANGER_PILL: Color32 = Color32::from_rgb(0xfb, 0xea, 0xe8);

/// **The band a secret wears, with the rule attached.** Design 4a/4b's
/// `#fdf3f2` on a `#e8a9a2` hairline: the keystroke list's password step, the
/// preflight card's secret row, and nothing else.
///
/// **A primitive and not three loose tokens**, which is what design 4's own
/// pass asked for in as many words and what `detail_edit`'s
/// `SECRET_STEP_FILL` doc has been asking for since it was written: *"if a
/// later pass wants one home for these, it should be a named secret-step
/// primitive with the rule attached, not three loose `Color32`s"*. The
/// argument is 4e's -- one red means something only while exactly one kind of
/// thing wears it, and three bare constants in a shared palette are an
/// invitation for a fourth surface to reach for a colour that then means
/// nothing. A `Frame` cannot be half-adopted: a caller gets the tint and the
/// edge together or not at all, so there is no way to end up with the pink
/// ground and somebody else's border.
///
/// **The radius and the padding are deliberately NOT set here**, and that is
/// the one thing this primitive leaves open. A secret's band is a row in a
/// list on one surface (`detail_edit`, 10pt radius, tight `8x5` margin) and a
/// band across a card on another (4b, 8pt radius, `11x10`); those are the
/// SHAPE of the surface it sits on, and forcing one of them would have made
/// the password step a different shape from the four ordinary steps above it
/// in the same list. What must never vary is the pair of colours, and that is
/// what this hands out.
///
/// **No `ui` parameter**, which is a departure from the shape the task
/// suggested (`secret_band(ui) -> Frame`). The frame reads nothing off the
/// `Ui` -- not its width, not its style, not its theme -- and a parameter that
/// is accepted and ignored is a claim about a dependency that does not exist.
/// A later pass that needs one can add it in the one place this is defined.
pub fn secret_band() -> egui::Frame {
    egui::Frame::new().fill(DANGER_WASH).stroke(Stroke::new(1.0, DANGER_EDGE))
}

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
/// The mark on a [`CAUTION_WASH`] surface -- 5c's `stroke="#8a5a06"`.
pub const CAUTION_MARK: Color32 = Color32::from_rgb(0x8a, 0x5a, 0x06);
/// Text on a [`CAUTION_WASH`] surface -- 5c's `color: #7a4f05`.
pub const CAUTION_INK: Color32 = Color32::from_rgb(0x7a, 0x4f, 0x05);
/// Border of a [`CAUTION_WASH`] surface -- 8a's `border: 1px solid #f2d99b`,
/// on its `Unsaved changes` pill and on the `Changed now` note beside the
/// password it is about.
///
/// The third of the trio, added when a caution surface finally needed an
/// EDGE: 5c's band is a full-width strip that reads as a band without one,
/// and a pill without a border is a smear of amber. [`DANGER_EDGE`] and
/// [`DONE_EDGE`] have been here since their washes were.
pub const CAUTION_EDGE: Color32 = Color32::from_rgb(0xf2, 0xd9, 0x9b);

/// The design's one green, in the same three values.
///
/// **Green is as rationed as red.** The design uses it in exactly two places
/// -- 4d's rehearsal tick and 5b/5c's `Used` pill -- both of which mean "this
/// finished", never "this is good". `scratch_window::CHECK_GREEN` and
/// `totp_add::VALID_MARK_INK` are the two existing private copies of
/// [`DONE_MARK`], left where they are for [`DANGER_WASH`]'s reason.
pub const DONE_WASH: Color32 = Color32::from_rgb(0xe8, 0xf3, 0xec);
/// Border of a [`DONE_WASH`] surface.
pub const DONE_EDGE: Color32 = Color32::from_rgb(0xa9, 0xd2, 0xba);
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

    fonts
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

/// [`letterspaced_mono_in`] in the monospace family's BOLD cut.
///
/// The live code and nothing else: §6c and §6d both declare it at
/// `font-weight: 700`, and every other monospace run in this app -- the
/// keycaps, the parameter chips, the seed field, the masked rows -- is
/// correctly regular. See [`MONO_BOLD`], whose family falls back to the
/// regular face on a machine that has no `consolab.ttf`.
pub fn letterspaced_mono_bold(
    text: &str,
    size: f32,
    tracking: f32,
    color: Color32,
    line_height: f32,
) -> egui::text::LayoutJob {
    letterspaced_in(
        text,
        FontId::new(size, FontFamily::Name(MONO_BOLD.into())),
        tracking,
        color,
        Some(line_height),
    )
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

/// One line of text with no extra tracking, **in a line box of the caller's
/// choosing** -- the plain-text twin of [`letterspaced_mono_in`].
///
/// For the one thing a `RichText` cannot be given: its own line height. Every
/// caller passes [`ascent_of`]'s answer, which makes the box the ink and so
/// makes box-centring -- which is all egui does, and all a caller painting a
/// galley at `band.center().y - size.y / 2.0` does -- centre the ink.
///
/// **This exists because a run whose box is not its ascent cannot be made
/// level with one whose box is.** Two faces reserve different descenders, so
/// two box-centred runs of different faces sit at different heights however
/// carefully each is centred; the only way to make a row level is to give
/// every run in it the same rule. See [`ink_drop`] for the measurement.
pub fn text_in(
    text: &str,
    font_id: FontId,
    color: Color32,
    line_height: f32,
) -> egui::text::LayoutJob {
    letterspaced_in(text, font_id, 0.0, color, Some(line_height))
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
/// The other five `eframe` windows in this crate still call this on their first
/// frame only. They are not transparent, they still get the near-black clear,
/// and giving them a transparent clear is what [`crate::window_host`] argues
/// against.
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
    let scale = (rect.width() / MARK_ARTBOARD.0).min(rect.height() / MARK_ARTBOARD.1);
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
    let scale = (rect.width() / MARK_ARTBOARD.0).min(rect.height() / MARK_ARTBOARD.1);
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

/// Paints `texture` inside an [`avatar_tile`] -- centred, never magnified,
/// shrunk only as far as it must be to fit -- and re-draws the tile's border
/// over it.
///
/// **The sizing rule and its history.** This is the fourth pass, and the next
/// person here will be holding one of these reports and not the other three:
///
/// 1. The favicon was drawn at the full 32pt with `fit_to_exact_size`, and the
///    report was "the favicon fills its tile edge-to-edge and feels too big".
/// 2. So it was inset 4pt a side -- a 24pt image in a 32pt tile. The report on
///    THAT was "icon is not fully taking the rounded rectangle".
/// 3. So it went full bleed: every icon stretched to the tile whatever its
///    size. That is what this pass overturns, and the reason is the one thing
///    the first three passes all missed -- they argued about how much MARGIN
///    the tile should have, when the defect was RESAMPLING. A 16x16 favicon,
///    which is what most sites still serve, was being magnified 2x to fill a
///    32pt tile, and no choice of margin fixes a blown-up image.
/// 4. So: never magnify. The margin is not a number this code picks at all --
///    it is whatever is left over once the artwork is drawn at its own size,
///    centred. A big icon still fills the tile; a small one sits small and
///    sharp inside it, which is the look the report asked for ("small inside
///    the rounded square with visible paddings ... never extrapolated").
///
/// [`avatar_artwork`] is the geometry, with the corner-radius reasoning.
///
/// **The border earns its place; the fill no longer does.** The tile behind a
/// favicon is drawn by [`avatar_artwork_tile`], which does not fill at all --
/// see there for the owner's "no backgound inside of tile". The BORDER is
/// still re-drawn ON TOP of the artwork, so the tile has ONE border in every
/// case instead of two code paths that have to agree about which. It is
/// `StrokeKind::Inside`: see [`avatar_box`] for why that word is the whole
/// fix for "5 pixels to the left and 6 to the right".
///
/// NOTE FOR WHOEVER CHANGES EITHER SIDE OF THIS: `favicon::decode_rgba`
/// resamples every icon to a 64px longest edge, a number chosen for a 32pt
/// draw at 200% scaling. Nothing in the code links that constant to this one.
/// It covers a 32pt draw exactly, so a tile that ever grows past 32pt needs
/// `decode_rgba`'s constant raised with it -- and note that under the rule
/// above a bigger tile would no longer STRETCH to hide the shortfall, it would
/// leave the icon sitting at 64pt in a larger square.
pub fn avatar_image(ui: &Ui, tile: Rect, texture: &egui::TextureHandle, emphasized: bool) {
    let (art, art_rounding) =
        avatar_artwork(tile, texture.size_vec2(), ui.ctx().pixels_per_point());
    egui::Image::new((texture.id(), texture.size_vec2()))
        .corner_radius(art_rounding)
        .paint_at(ui, art);
    ui.painter().rect_stroke(
        tile,
        avatar_corner_radius(tile.width()),
        avatar_tile_stroke(emphasized),
        StrokeKind::Inside,
    );
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
fn snapped_min(min: Pos2, pixels_per_point: f32) -> Pos2 {
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
/// Gap between a [`PillMark`] and the label. The design's `gap: 6px`.
const PILL_GAP: f32 = 6.0;
/// The dot's diameter, and the tick's box.
const PILL_DOT: f32 = 5.0;
const PILL_CHECK: f32 = 10.0;

/// Width a [`state_pill`] will occupy, without drawing it.
///
/// Separate from the painter so a caller laying a row out right-to-left can
/// reserve the slot before it knows where the slot starts -- which is how
/// every control on the Sends row is placed, and why none of them can be
/// pushed off the pane at the minimum window size.
pub fn state_pill_width(painter: &egui::Painter, tone: PillTone, text: &str) -> f32 {
    let galley = painter.layout_no_wrap(
        text.to_string(),
        FontId::new(PILL_TEXT_PX, FontFamily::Name(BOLD.into())),
        tone.ink,
    );
    let lead = match tone.mark {
        PillMark::None => 0.0,
        PillMark::Dot(_) => PILL_DOT + PILL_GAP,
        PillMark::Check(_) => PILL_CHECK + PILL_GAP,
    };
    // `+ 2.0` is the border, which the design's content-box padding excludes.
    // See [`PILL_HEIGHT`].
    lead + galley.size().x + PILL_PAD_X * 2.0 + 2.0
}

/// Paints design 5b's state pill with its left edge at `left_center`, and
/// hands back the rectangle it covered.
///
/// # Why this is not [`status_pill`]
///
/// [`status_pill`] is the toolbar readout from design 2b and it differs in
/// every dimension that matters here: it is 28 tall against this one's 20, it
/// draws 12px text where this draws 10, it is **unfilled** where every one of
/// these carries a tinted ground, its border and text colour are fixed by the
/// design to one grey pair where these vary per state, and its dot is
/// mandatory -- which is the disqualifying difference, because three of the
/// design's four Send states carry no dot and one carries a tick instead.
/// Reaching `status_pill` here would have meant four `Option` parameters on a
/// widget whose own doc says the dot is "the only thing that varies".
///
/// So it is a second pill, and it is in this file rather than in the Sends
/// screen for the reason the module doc gives: 5b draws it in the list, 5b
/// draws it again in the detail header, 5d draws it a third time on a record,
/// and a widget drawn in three places by three screens is the design
/// language, not one screen's decoration.
///
/// **A painter and an anchor, not a `Ui`.** The Sends row places every one of
/// its cells into an explicit rectangle against a painter, deliberately --
/// see `send_ui::draw_row` -- because a nested horizontal layout is what has
/// repeatedly pushed a control off this pane.
pub fn state_pill(
    painter: &egui::Painter,
    left_center: Pos2,
    tone: PillTone,
    text: &str,
) -> Rect {
    let galley = painter.layout_no_wrap(
        text.to_string(),
        FontId::new(PILL_TEXT_PX, FontFamily::Name(BOLD.into())),
        tone.ink,
    );
    let width = state_pill_width(painter, tone, text);
    let rect = Rect::from_min_size(
        Pos2::new(left_center.x, left_center.y - PILL_HEIGHT / 2.0),
        Vec2::new(width, PILL_HEIGHT),
    );
    // `border-radius: 999px` on a fixed-height pill is "fully rounded"; half
    // the height is the largest radius that still reads as a stadium.
    let rounding = CornerRadius::same((PILL_HEIGHT / 2.0) as u8);
    painter.rect_filled(rect, rounding, tone.fill);
    painter.rect_stroke(rect, rounding, Stroke::new(1.0, tone.edge), StrokeKind::Inside);

    let mut x = rect.min.x + 1.0 + PILL_PAD_X;
    match tone.mark {
        PillMark::None => {}
        PillMark::Dot(colour) => {
            painter.circle_filled(
                Pos2::new(x + PILL_DOT / 2.0, rect.center().y),
                PILL_DOT / 2.0,
                colour,
            );
            x += PILL_DOT + PILL_GAP;
        }
        PillMark::Check(colour) => {
            // The design's `M20 6 9 17l-5-5` in a 24-unit box, scaled to
            // `PILL_CHECK` and centred on the pill's own baseline.
            let unit = PILL_CHECK / 24.0;
            let at = |ux: f32, uy: f32| {
                Pos2::new(x + ux * unit, rect.center().y - PILL_CHECK / 2.0 + uy * unit)
            };
            painter.add(egui::Shape::line(
                vec![at(20.0, 6.0), at(9.0, 17.0), at(4.0, 12.0)],
                // `stroke-width: 2.4` in the same 24-unit box, so it scales
                // with the glyph rather than being a second literal.
                Stroke::new(2.4 * unit, colour),
            ));
            x += PILL_CHECK + PILL_GAP;
        }
    }
    // Optically centred, like every other single line this app sets in a
    // band -- see `centred_galley_top`. Box-centring put the label a couple
    // of points high in every pill in the app, which is the same defect the
    // owner reported on the parameter chips.
    let font = FontId::new(PILL_TEXT_PX, FontFamily::Name(BOLD.into()));
    let drop = painter.ctx().fonts_mut(|f| {
        let probe = f.layout_no_wrap(ASCENT_PROBE.to_string(), font.clone(), Color32::BLACK);
        probe.rows.first().and_then(|row| row.glyphs.first()).map_or(0.0, |g| {
            let above = g.pos.y + g.uv_rect.offset.y;
            let below = g.font_height - g.pos.y;
            ((below - above) / 2.0).max(0.0)
        })
    });
    painter.galley(
        Pos2::new(x, rect.center().y - galley.size().y / 2.0 + drop),
        galley,
        tone.ink,
    );
    rect
}

/// **§5a's opt-in box: the square tick beside a field's name.**
///
/// `width: 17px; height: 17px; border-radius: 5px` -- filled in `tone` with a
/// white tick when it is on, and white inside a [`BORDER_STRONG`] hairline
/// when it is off. Painted rather than `egui::Checkbox`, which draws its own
/// square at its own size in its own palette and cannot be told otherwise.
///
/// `tone` rather than [`BLUE`] outright because §5a tints ONE of these rows:
/// the password's tick is `#b42318`, and a red tick beside a red label on a
/// red row is the design saying, in the only three ways a row has, that this
/// is the field that travels in the clear.
///
/// Answers the response so the caller can act on a click. The whole ROW is
/// the target, not this square -- see the callers -- so this takes
/// [`Sense::hover`] and the row does the clicking.
pub fn opt_in_box(ui: &mut Ui, on: bool, tone: Color32) -> Rect {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(OPT_IN_BOX), Sense::hover());
    let painter = ui.painter();
    if on {
        painter.rect_filled(rect, CornerRadius::same(OPT_IN_RADIUS), tone);
        // §5a's `M20 6 9 17l-5-5` at `width: 11`, in the 24-unit box the
        // path is written in -- the same tick `state_pill` draws, at this
        // box's size rather than a pill's.
        let unit = OPT_IN_CHECK / 24.0;
        let origin = rect.center() - Vec2::splat(OPT_IN_CHECK / 2.0);
        let at = |ux: f32, uy: f32| origin + Vec2::new(ux * unit, uy * unit);
        painter.add(egui::Shape::line(
            vec![at(20.0, 6.0), at(9.0, 17.0), at(4.0, 12.0)],
            // §5a's `stroke-width: 3.2` in the same 24-unit box.
            Stroke::new(3.2 * unit, CARD),
        ));
    } else {
        painter.rect(
            rect,
            CornerRadius::same(OPT_IN_RADIUS),
            CARD,
            Stroke::new(1.0, BORDER_STRONG),
            StrokeKind::Inside,
        );
    }
    rect
}

/// §5a's `width: 17px; height: 17px` on the opt-in box.
pub const OPT_IN_BOX: f32 = 17.0;

/// §5a's `border-radius: 5px` on it.
const OPT_IN_RADIUS: u8 = 5;

/// §5a's `<svg width="11" height="11">` inside it.
const OPT_IN_CHECK: f32 = 11.0;

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

// --- design 6a's ↵ keycap (added by the TOTP picker's design pass) ---------

/// The corner radius and the horizontal padding of the keycap design 6a hangs
/// off its default row (`border-radius: 5px; padding: 3px 7px`), which is
/// [`kbd_chip_on_card`]'s 3h geometry in [`kbd_chip`]'s on-primary colours —
/// white ink on [`BLUE`], and so neither of the two treatments above.
///
/// `RETURN_KEYCAP_PAD_X` is `pub` because the caller has to know how wide the
/// cap will be *before* it lays the row out: 6a's text column is `flex: 1`,
/// so the room the two lines of type get is what the affordance leaves.
const RETURN_KEYCAP_RADIUS: u8 = 5;
pub const RETURN_KEYCAP_PAD_X: f32 = 7.0;

/// Paints that keycap into `rect`, which the caller has already measured and
/// placed.
///
/// A painter and not a `Ui` widget, unlike the three chips above, because the
/// picker's rows are painted into rects they allocated for themselves rather
/// than laid out by egui — and the arrow is DRAWN rather than typed for
/// [`primary_button`]'s measured reason: U+21B5 is carried by neither Archivo
/// nor egui's fallback stack and reaches the screen as a tofu box.
pub fn paint_return_keycap(painter: &egui::Painter, rect: Rect) {
    painter.rect_filled(rect, CornerRadius::same(RETURN_KEYCAP_RADIUS), BLUE);
    paint_return_arrow(painter, rect.center(), RETURN_GLYPH_SIZE, Color32::WHITE);
}

// --- end of design 6a's keycap --------------------------------------------

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
/// `enabled == false` paints [`OFF_FILL`] / [`OFF_EDGE`] / [`OFF_INK`] and
/// senses nothing; see [`OFF_FILL`] for why that replaced a faded [`BLUE`].
/// `detail_edit`'s `the_disabled_save_button_does_not_look_enabled` asserts
/// on the painted fill, and is what holds the two apart.
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

/// §8a's footer buttons: `height: 34px; padding: 0 14px; border-radius: 8px`
/// on both `Save changes` and `Cancel`, with `gap: 10px` between them.
///
/// **Not [`BUTTON_HEIGHT`]'s 32 and 7**, which is what the edit form's footer
/// drew until it was measured against §8a: two points shorter and one point
/// squarer than the design, on the two controls the whole form ends in. They
/// are 2b's `+ New` metrics exactly ([`primary_button_matching_field`]), which
/// is not a coincidence the constants lean on -- §8a declares its own numbers
/// and these are those, named for the footer they belong to.
///
/// `Save changes` is `font-weight: 700` in §8a and `Cancel` 600, and that is
/// now what the two are drawn in -- [`bold`] and [`semibold`], the theme's
/// own names for those two weights.
///
/// **This reverses an earlier reading of an earlier report.** "One bold and
/// not bold now for some reason" was written when Save was a bare
/// `egui::Button`, which takes egui's default `Proportional` stack, beside a
/// Cancel in Archivo SemiBold: two font STACKS, read as a weight difference
/// nobody had designed. The answer then was to put both in one named face,
/// which fixed the stack and flattened §8a's real 700/600 step as a side
/// effect -- and the owner's next look at this strip was "buttons text not
/// bold". So the rule the pin carries is the one that was always meant: each
/// button wears a weight the theme NAMES, and which weight is the design's
/// call. See `detail_edit`'s
/// `each_footer_button_is_set_in_a_face_the_theme_names`.
pub const SECTION_FOOTER_BUTTON_HEIGHT: f32 = 34.0;
/// See [`SECTION_FOOTER_BUTTON_HEIGHT`].
pub const SECTION_FOOTER_BUTTON_RADIUS: u8 = 8;
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

/// [`primary_button_enabled`] at §8a's footer metrics. See
/// [`SECTION_FOOTER_BUTTON_HEIGHT`].
pub fn section_footer_primary_button(
    ui: &mut Ui,
    label: &str,
    // **8a's `CTRL+S` chip**, drawn inside the button beside its label. An
    // `Option` because the caller owns the question this chip answers: a hint
    // for a chord nothing binds is a control that lies, so the form passes
    // `Some` only where it really reads the chord. See `detail_edit`'s footer.
    kbd: Option<&str>,
    enabled: bool,
) -> Response {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding.x = SECTION_FOOTER_BUTTON_PAD_X;
        // Without a chord there is nothing to lay out beside the label, so
        // this is an ordinary button -- through the SAME helper as the
        // chipped branch below, with no width floor.
        // `primary_button_with_metrics` would be the obvious call and is the
        // wrong one: it is every other primary button in the app, at 600, and
        // taking it here would mean a footer whose Save changed weight
        // depending on whether the form happened to pass a chord.
        let Some(kbd) = kbd else {
            return primary_button_with_metrics_sized(
                ui,
                label,
                SECTION_FOOTER_BUTTON_HEIGHT,
                SECTION_FOOTER_BUTTON_RADIUS,
                enabled,
                0.0,
            );
        };
        // **The button is drawn EMPTY and both runs are painted into it.**
        //
        // §8a's Save is a flex row of two spans -- `Save changes` at 13/700 in
        // Archivo, then `CTRL+S` at 10px in `ui-monospace` at `opacity: 0.8`,
        // `gap: 9px` between them -- inside `padding: 0 14px`. Three ways to
        // get that out of egui were tried and two of them are wrong:
        //
        // * **One string** (`format!("{label}  {k}")`, which is what
        //   `primary_button_with_metrics` does for its own hints) puts the
        //   chord at the label's size, weight and face. One galley cannot be
        //   two faces.
        // * **Label in the button, chip painted beside it.** egui CENTRES a
        //   button's galley in whatever width the button has, so reserving
        //   the chip's room through `min_size` pushes the label right by half
        //   of it, and a chip hung off the label's right edge then ends
        //   `(gap + chip) / 2 - pad` PAST the button -- ten points of
        //   `CTRL+S` clipped off, which is what the owner screenshotted,
        //   above a left inset half the row wide. No `min_size` fixes that:
        //   the overhang and the inset are the same centring, and widening
        //   the button moves both.
        //
        // So the button carries no text at all, and the row is laid out here:
        // measured as one group, centred as one group, and the two galleys
        // placed left to right inside it. At the natural width that is
        // exactly §8a -- `pad`, label, `gap`, chip, `pad` -- and when
        // `section_footer_save_width`'s floor makes the button wider than its
        // content, the whole row stays centred rather than the padding going
        // lopsided.
        let chip_font = FontId::new(SECTION_FOOTER_CHIP_PX, FontFamily::Monospace);
        let chip = ui.painter().layout_no_wrap(
            kbd.to_string(),
            chip_font.clone(),
            Color32::WHITE,
        );
        let label_font = FontId::new(SECTION_FOOTER_TEXT_PX, FontFamily::Name(BOLD.into()));
        let label_width = section_footer_label_width(ui.painter(), label);
        let response = ui.scope(|ui| {
            ui.spacing_mut().button_padding.x = SECTION_FOOTER_BUTTON_PAD_X;
            primary_button_with_metrics_sized(
                ui,
                "",
                SECTION_FOOTER_BUTTON_HEIGHT,
                SECTION_FOOTER_BUTTON_RADIUS,
                enabled,
                section_footer_save_width(ui.painter(), label, kbd),
            )
        })
        .inner;
        // The button's own ink, and -- for the chip -- §8a's `opacity: 0.8` on
        // top of it, so a disabled Save does not carry a chord at full
        // strength.
        let ink = if enabled { Color32::WHITE } else { OFF_INK };
        let row = label_width + SECTION_FOOTER_CHIP_GAP + chip.size().x;
        let left = response.rect.center().x - row / 2.0;
        let caption = ui.painter().layout_no_wrap(label.to_string(), label_font.clone(), ink);
        // `centred_galley_top` and not `center().y - height / 2.0`: the face
        // inks only the upper part of its row box, so a box-centred line
        // reads high in a band -- this module's standing rule, and the reason
        // the owner's word on the footer was "not centered". The two runs
        // take the correction for their OWN faces, which is the whole point
        // of doing it per galley: 13pt Archivo and 10pt mono do not reserve
        // the same descender band, so one offset for both would trade a pair
        // of high lines for a misaligned pair.
        let caption_at =
            centred_galley_top(ui.ctx(), response.rect, &caption, &label_font, left);
        ui.painter().galley(caption_at, caption, ink);
        let chip_at = centred_galley_top(
            ui.ctx(),
            response.rect,
            &chip,
            &chip_font,
            left + label_width + SECTION_FOOTER_CHIP_GAP,
        );
        ui.painter().galley(chip_at, chip, faded(ink, SECTION_FOOTER_CHIP_OPACITY));
        response
    })
    .inner
}

/// [`secondary_button`] at §8a's footer metrics. See
/// [`SECTION_FOOTER_BUTTON_HEIGHT`].
///
/// **Its label is painted too**, though it has no chip and needs no row. The
/// reason is the one in [`centred_galley_top`]: egui box-centres a button's
/// galley, which reads high, and the Save beside this one is optically
/// centred. One corrected and one not is a worse footer than two of either --
/// the owner's report was "not centered both", on a strip whose two controls
/// sit on one line and are read against each other.
pub fn section_footer_secondary_button(ui: &mut Ui, label: &str) -> Response {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding.x = SECTION_FOOTER_BUTTON_PAD_X;
        let font = FontId::new(SECTION_FOOTER_TEXT_PX, FontFamily::Name(SEMIBOLD.into()));
        // The width the label would have taken had egui laid it out, so this
        // button is the size it always was -- `action_button_width` is
        // `semibold` at 13, which is exactly the face above.
        let width = action_button_width(ui.painter(), label, SECTION_FOOTER_BUTTON_PAD_X);
        let response = ui.add(
            egui::Button::new("")
                .fill(CARD)
                .stroke(Stroke::new(1.0, BORDER_STRONG))
                .corner_radius(CornerRadius::same(SECTION_FOOTER_BUTTON_RADIUS))
                .min_size(Vec2::new(width, SECTION_FOOTER_BUTTON_HEIGHT)),
        );
        let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), INK);
        let at = centred_galley_top(
            ui.ctx(),
            response.rect,
            &galley,
            &font,
            response.rect.center().x - galley.size().x / 2.0,
        );
        ui.painter().galley(at, galley, INK);
        response
    })
    .inner
}

/// The filled button that confirms **destroying** something -- today, the
/// delete modal's confirm and nothing else.
///
/// [`primary_button`]'s metrics exactly, with [`ERROR`] where [`BLUE`] goes.
/// It has to carry a primary button's weight, because it is the thing its
/// modal exists for and burying it in a [`secondary_button`] would make the
/// destructive answer the quieter of the two. It cannot wear [`BLUE`],
/// because the habit that fills in a form and presses the blue button is
/// precisely what a confirmation is there to interrupt -- a red button is a
/// hand on the arm, and this app already spends [`ERROR`] on exactly this
/// meaning (the kebab's Delete words, the sidebar's folder ×).
///
/// No `kbd` parameter, and no Enter binding at the call site either: a
/// keyboard shortcut for "yes, destroy it" is a shortcut for doing it by
/// accident, which is the whole thing the modal was put in the way of.
pub fn destructive_button(ui: &mut Ui, label: &str) -> Response {
    ui.add(
        egui::Button::new(semibold(label, 13.0).color(Color32::WHITE))
            .fill(ERROR)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(7))
            .min_size(Vec2::new(0.0, BUTTON_HEIGHT)),
    )
}

/// **What any control in this design system looks like when it is switched
/// off**, and the reason it is three constants rather than one.
///
/// `disabled_field_box` states the rule this obeys: greyed means fill, border
/// and ink move TOGETHER -- [`CANVAS`] where [`CARD`] was, [`BORDER`] where
/// [`BORDER_STRONG`] was, [`TEXT_GHOST`] where the body colour was -- because
/// any one of them alone reads as a styling accident rather than as a control
/// that cannot be pressed. The fields obeyed it and the buttons did not.
///
/// **What a disabled primary used to be, and why it was wrong.** It was
/// [`BLUE`] run through egui's own `fade_out_to_color`, which is a linear
/// blend of the enabled fill toward the window colour: a pale blue button.
/// The owner's report on the Send composer named it exactly -- "`Create link`
/// is a pale blue when disabled" -- and the complaint is not that the fade is
/// subtle. It is that a washed-out version of the live colour says *"this is
/// the button, dimmed"*, which reads as a rendering state rather than as a
/// refusal; a user who cannot see why it is pale tries to press it. The three
/// greys below say *"this is not a control right now"* in the vocabulary this
/// app already uses for exactly that, one row up in the same form.
///
/// The disabled button also **senses nothing** rather than running inside a
/// disabled `Ui`, and that is what makes the colours possible: a disabled
/// `Ui` fades everything its painter touches, so an explicitly grey button
/// drawn inside one would come out a paler grey still and the whole point
/// would be lost. `Sense::hover()` is the same refusal by a different means
/// -- no click reaches it, and `Response::clicked()` is `false` -- and it is
/// what `disabled_text_field` already does for the same reason.
pub const OFF_FILL: Color32 = CANVAS;
/// See [`OFF_FILL`].
pub const OFF_EDGE: Color32 = BORDER;
/// See [`OFF_FILL`].
pub const OFF_INK: Color32 = TEXT_GHOST;

/// §8a's `font-size: 13px` on both footer buttons. The WEIGHTS differ (700
/// on Save, 600 on Cancel); the size does not.
pub const SECTION_FOOTER_TEXT_PX: f32 = 13.0;

/// How wide Save's label is, in the face Save is really set in.
///
/// [`action_button_width`] cannot answer this: it measures in [`semibold`],
/// which is right for every other button that calls it and two per cent
/// narrow for this one. Two per cent of `Save (needs a name)` is a point and
/// a half, and it lands on the footer's line-fitting decision and on where
/// the chord chip is painted -- both of which are measured to the point.
pub fn section_footer_label_width(painter: &egui::Painter, label: &str) -> f32 {
    painter
        .layout_no_wrap(
            label.to_string(),
            FontId::new(SECTION_FOOTER_TEXT_PX, FontFamily::Name(BOLD.into())),
            INK,
        )
        .size()
        .x
}

/// The whole width Save takes: §8a's `padding: 0 14px`, its bold label, the
/// `gap: 9px`, and the chord chip.
///
/// **The width the button is built at AND the width the footer measures**,
/// which is the property that matters: `section_footer_primary_button` paints
/// its two runs itself, so anything this function is short by is clipped off
/// the chip rather than leaving the button a little tight.
/// [`action_button_width`] cannot stand in -- it measures in [`semibold`],
/// which is right for every other button that calls it and two per cent
/// narrow for this one.
///
/// The floor is applied to the WHOLE row rather than to the label's own box,
/// so a short caption cannot be padded up to `ACTION_BUTTON_MIN_WIDTH` and
/// then have the chip added outside the floor.
pub fn section_footer_save_width(painter: &egui::Painter, label: &str, kbd: &str) -> f32 {
    (section_footer_label_width(painter, label)
        + section_footer_chip_width(painter, kbd)
        + SECTION_FOOTER_BUTTON_PAD_X * 2.0)
        .max(ACTION_BUTTON_MIN_WIDTH)
}

/// What [`section_footer_primary_button`] adds to a plain button's width for
/// 8a's chord chip: the gap and the chip itself.
///
/// Exported because the footer measures its own row before it draws it -- the
/// change summary is laid beside the buttons only if it fits there -- and a
/// measurement that left the chip out would put the summary off the pane. It
/// did: `nothing_on_the_widest_sparse_form_is_painted_outside_the_minimum_pane`
/// caught `1 change` painted 38 points past the edge.
pub fn section_footer_chip_width(painter: &egui::Painter, kbd: &str) -> f32 {
    SECTION_FOOTER_CHIP_GAP
        + painter
            .layout_no_wrap(
                kbd.to_string(),
                FontId::new(SECTION_FOOTER_CHIP_PX, FontFamily::Monospace),
                Color32::WHITE,
            )
            .size()
            .x
}

/// [`primary_button_with_metrics`] in 8a's footer weight, with a width floor
/// for a caller that has something of its own to paint inside the button.
///
/// **Two differences from the function it is named after**, and both belong
/// to this one footer:
///
/// * the label is [`bold`] -- 8a's `font-weight: 700` on `Save changes`,
///   against the 600 every other primary button in the app wears. See
///   [`SECTION_FOOTER_BUTTON_HEIGHT`] for why that step is drawn now and was
///   not before.
/// * `min_width`, which is how Save reserves room for 8a's chord chip -- see
///   [`section_footer_primary_button`], which explains why the chip is
///   painted rather than appended to the label.
///
/// Private, and called only from that one helper, so no other button in the
/// app can pick up the footer's weight by reaching for the nearer name.
fn primary_button_with_metrics_sized(
    ui: &mut Ui,
    label: &str,
    height: f32,
    radius: u8,
    enabled: bool,
    min_width: f32,
) -> Response {
    let (ink, fill, edge, sense) = if enabled {
        (Color32::WHITE, BLUE, Stroke::NONE, Sense::click())
    } else {
        (OFF_INK, OFF_FILL, Stroke::new(1.0, OFF_EDGE), Sense::hover())
    };
    let response = ui.add(
        egui::Button::new(bold(label, SECTION_FOOTER_TEXT_PX).color(ink))
            .fill(fill)
            .stroke(edge)
            .sense(sense)
            .corner_radius(CornerRadius::same(radius))
            .min_size(Vec2::new(min_width, height)),
    );
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
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
    // The switched-off look is painted at FULL strength and sensed as a
    // hover, rather than run through a disabled `Ui`; see [`OFF_FILL`] for
    // why the fade had to go and why removing the sense is what replaces it.
    let (ink, fill, edge, sense) = if enabled {
        (Color32::WHITE, BLUE, Stroke::NONE, Sense::click())
    } else {
        (OFF_INK, OFF_FILL, Stroke::new(1.0, OFF_EDGE), Sense::hover())
    };
    let response = ui.add(
        egui::Button::new(semibold(text, 13.0).color(ink))
            .fill(fill)
            .stroke(edge)
            .sense(sense)
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
            if enabled { Color32::from_white_alpha(204) } else { OFF_INK },
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

// ---------------------------------------------------------------------------
// Design 5b's action row: design-system buttons placed into a MEASURED RECT
// ---------------------------------------------------------------------------

/// **Why this exists beside [`primary_button`] and friends, which look like
/// they already do the job.**
///
/// Those three are `ui.add`: they take the space the surrounding layout gives
/// them, which is right for a form footer and impossible for a pane that
/// places every cell into a rectangle it measured first. Design 5b's Sends
/// screen is exactly such a pane -- its detail column is 250pt of content at
/// `settings::MIN_VAULT_WINDOW_SIZE` and its own history is of controls
/// pushed off the edge by nested layouts, which is why it reserves each slot
/// before it fills one.
///
/// So that screen built its controls out of bare `egui::Button`s, and the
/// result is the defect [`primary_button_enabled`]'s own doc already records
/// one screen earlier: *"the two footer buttons looked like they came from
/// different families -- they did"*. A bare button picks up egui's fill,
/// egui's radius and egui's proportional face; every button that came from
/// this module is `semibold`, `CARD` over [`BORDER_STRONG`], and a design
/// radius. On the Sends screen the difference was three grey system boxes
/// where 5b draws an outlined control beside a solid red one.
///
/// This is the missing half: the design system's tones, in a rectangle the
/// caller owns. It is NOT a fourth look -- [`ActionTone`] maps onto the three
/// buttons above -- and it deliberately does not re-implement their bodies,
/// because a second spelling of "the primary button" is the thing this
/// module exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionTone {
    /// [`primary_button`]'s look: [`BLUE`] fill, white label. The one thing
    /// on the row the screen is *for*.
    Primary,
    /// [`secondary_button`]'s look: white over [`BORDER_STRONG`]. 5b's
    /// `Open record`.
    Secondary,
    /// 5b's `Revoke`: [`ERROR`] fill, white label, [`destructive_button`]'s
    /// meaning at this size.
    Destructive,
    /// **An outlined button whose LABEL is [`ERROR`].** Not in 5b, and it is
    /// here for a reason 5b did not have to face: 5b's red button is the only
    /// destructive control on its header, and this app's Sends header carries
    /// a destructive control *beside* two ordinary ones. A solid red there is
    /// the loudest thing on a screen whose subject is the link, not its
    /// deletion -- and the confirmation that follows it is where this app
    /// spends [`destructive_button`]. So the first step is red WORDS and the
    /// second step is a red BUTTON, which is the same escalation the kebab's
    /// Delete and the delete modal already use.
    DestructiveQuiet,
}

/// 5b's action control: `height: 32px; padding: 0 14px; border-radius: 8px`.
///
/// **32 and not the 26 this screen used**, which is the whole of why its
/// header read as a toolbar of system widgets rather than as the design's
/// action row: at 26 with a 12px label the control is smaller than the pill
/// beside it and smaller than every other button in the app.
pub const ACTION_BUTTON_HEIGHT: f32 = 32.0;
/// 5b's `border-radius: 8px` on the same control. One point off
/// [`primary_button`]'s 7, because 5b says 8 and this is 5b's row.
pub const ACTION_BUTTON_RADIUS: u8 = 8;
/// 5b's `padding: 0 14px`.
///
/// The **comfortable** padding. A caller that cannot fit its row at this
/// width passes a smaller one to [`action_button_width`]; see its doc for why
/// shrinking the padding is the right thing to give up first.
pub const ACTION_BUTTON_PAD_X: f32 = 14.0;
/// The label: 5b's `font-size: 13px; font-weight: 600`, which is this
/// module's [`semibold`] at 13 -- the same face and size every other button
/// here wears.
pub const ACTION_BUTTON_TEXT_PX: f32 = 13.0;
/// A floor, so a one-word control is still a target rather than a sliver.
pub const ACTION_BUTTON_MIN_WIDTH: f32 = 64.0;

/// How wide `label` needs its button to be at `pad_x`.
///
/// **Measured rather than tabulated**, because the alternative is a table of
/// widths that goes stale the first time a label is reworded -- and because
/// the caller has to know the total before it can decide whether the row fits
/// on one line, which is a question no `ui.add` can answer after the fact.
///
/// `pad_x` is a parameter and not [`ACTION_BUTTON_PAD_X`] for one reason: a
/// row of three of these has to fit a 250pt pane, and when it cannot, the
/// padding is the right thing to give up. Shrinking the LABEL would hide what
/// the control does; shrinking the HEIGHT would make it a different component
/// from the one beside it on a wider window; shrinking the padding leaves a
/// button that is visibly the same button, slightly tighter.
pub fn action_button_width(painter: &egui::Painter, label: &str, pad_x: f32) -> f32 {
    let galley = painter.layout_no_wrap(
        label.to_string(),
        FontId::new(ACTION_BUTTON_TEXT_PX, FontFamily::Name(SEMIBOLD.into())),
        INK,
    );
    (galley.size().x + pad_x * 2.0).max(ACTION_BUTTON_MIN_WIDTH)
}

/// One action button, drawn into `rect`.
///
/// The rect is the caller's: this allocates nothing of its own, which is what
/// lets a pane reserve every slot on a row before it fills any of them. See
/// [`ActionTone`] for why this exists at all.
pub fn action_button(ui: &mut Ui, rect: Rect, label: &str, tone: ActionTone) -> Response {
    let (fill, stroke, ink) = match tone {
        ActionTone::Primary => (BLUE, Stroke::NONE, Color32::WHITE),
        ActionTone::Secondary => (CARD, Stroke::new(1.0, BORDER_STRONG), INK),
        ActionTone::Destructive => (ERROR, Stroke::NONE, Color32::WHITE),
        ActionTone::DestructiveQuiet => (CARD, Stroke::new(1.0, BORDER_STRONG), ERROR),
    };
    // **`rect` is the button, exactly, and these two lines are what make that
    // true.**
    //
    // `Ui::put` is not a promise about size: egui takes the widget's own
    // desired size and grows past the rect whenever that is larger. A
    // `Button`'s desired size is its galley plus `spacing.button_padding`
    // (this app's is `12 x 6`) -- so a slot measured here at a tighter
    // padding is a slot the button silently overflows, and a label egui
    // decides to wrap makes it overflow downwards as well.
    //
    // Both were live. The Sends header measures its slots at a padding that
    // shrinks to fit a 250pt pane, and its Cancel -- which must land in
    // Delete's exact rectangle, because that equality IS the mis-click
    // defence -- came out two points wider and eight points taller than the
    // Delete it replaced. Zeroing the padding for this one widget makes the
    // caller's measurement the whole story, which is what a rect-placed
    // control needs; `Extend` stops the label wrapping inside a slot that was
    // measured for one line.
    // **Set and restored in place, NOT wrapped in a `Ui::scope`.** A scope is
    // the obvious way to bound a style change and it is wrong here: it
    // allocates its own content in the parent's flow, and this rect is
    // routinely ABOVE the parent's cursor (the Sends strip draws its controls
    // inside a band it has already allocated), so the scope's union reaches
    // back up and advances the cursor by the whole distance. That is not a
    // theory -- it pushed every row of the Sends list thirty points down the
    // column. Two assignments around `put` change nothing but the style.
    let saved = ui.spacing().button_padding;
    ui.spacing_mut().button_padding = Vec2::ZERO;
    let response = ui.put(
        rect,
        egui::Button::new(semibold(label, ACTION_BUTTON_TEXT_PX).color(ink))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(CornerRadius::same(ACTION_BUTTON_RADIUS))
            .wrap_mode(egui::TextWrapMode::Extend)
            .min_size(rect.size()),
    );
    ui.spacing_mut().button_padding = saved;
    response
}

// ---------------------------------------------------------------------------
// Design 5b's receded row
// ---------------------------------------------------------------------------

/// `box-shadow: 0 1px 2px rgba(45, 43, 43, 0.06)` -- the design's SELECTED
/// list row. Alpha is `0.06 * 255`, rounded.
///
/// **Here rather than in `item_list`, where it lived**, because design 5b's
/// caption for the Sends screen is `SAME LIST + DETAIL AS THE VAULT` and the
/// two columns really do swap places in one slot of one window. A row
/// treatment spelled privately in one of them is a row treatment the other
/// re-invents -- which is exactly what happened: the Sends row was rebuilt by
/// hand, and came out in a lighter face, a fainter subtitle and with no
/// shadow under the selection. Whatever answers "what does a picked row look
/// like" has to be one value, and this module is where that question is
/// answered in this codebase.
pub const SELECTED_ROW_SHADOW: Shadow = Shadow {
    offset: [0, 1],
    blur: 2,
    spread: 0,
    color: Color32::from_rgba_unmultiplied_const(45, 43, 43, 15),
};

/// Design 5b's `opacity: 0.72` on a Send whose link has ended.
///
/// **A whole-row property, not a colour**, which is why it is a number here
/// rather than a fourth set of greys. 5b draws its Expired and Revoked rows
/// at 72% of everything -- tile, name, subtitle and pill together -- and that
/// is the point: a list where the ended rows are simply *quieter* lets the
/// live ones come forward, which is the one thing a column of five identical
/// bands cannot do. Recolouring the text instead would have said "this text
/// is less important" about a name that is exactly as important as the one
/// above it; it is the row's STATE that is over.
pub const ENDED_ROW_OPACITY: f32 = 0.72;

/// `colour` at `opacity`, for [`ENDED_ROW_OPACITY`]'s use.
///
/// Composited against nothing -- the alpha is simply scaled -- because every
/// caller paints over a ground it has just filled, so blending here would
/// mean knowing the ground twice.
pub fn faded(colour: Color32, opacity: f32) -> Color32 {
    let a = (colour.a() as f32 * opacity.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), a)
}

/// A whole [`PillTone`] faded by [`faded`], so 5b's row opacity reaches the
/// pill as well as the words beside it.
///
/// The mark's colour goes with it: a tick or a dot left at full strength on a
/// faded pill is the brightest thing in the row it is supposed to be
/// receding.
pub fn faded_pill(tone: PillTone, opacity: f32) -> PillTone {
    PillTone {
        fill: faded(tone.fill, opacity),
        edge: faded(tone.edge, opacity),
        ink: faded(tone.ink, opacity),
        mark: match tone.mark {
            PillMark::None => PillMark::None,
            PillMark::Dot(c) => PillMark::Dot(faded(c, opacity)),
            PillMark::Check(c) => PillMark::Check(faded(c, opacity)),
        },
    }
}

/// One cell of a [`segmented_control`]: what it says, and whether it is one
/// of the choices currently in force.
///
/// A named pair rather than a `(&str, bool)` tuple, because the two fields
/// are the same shape as each other and a caller that transposed them would
/// still compile: `Segment { label, selected }` cannot be got wrong at the
/// call site, and every one of this control's callers builds its cells in a
/// loop where an argument order is easy to lose track of.
pub struct Segment<'a> {
    /// The word or phrase on the cell. Sized to this, plus
    /// [`SEGMENT_PADDING`].
    pub label: &'a str,
    /// Painted as the choice in force -- [`BLUE`] behind white.
    ///
    /// A `bool` per cell rather than one selected *index* for the run,
    /// because this app has both kinds of group: the backend picker is one
    /// of two, and the Local API key form's access pair is any of two. One
    /// index could not express the second, and a control that existed in two
    /// variants would be two controls with one name.
    pub selected: bool,
}

/// **The app's multiple-choice row: a run of cells joined into one pill.**
///
/// Returns the index of the cell that was pressed this frame, or `None` --
/// pressing the cell already in force is a press like any other and is
/// reported, because "you chose the thing you already had" is a question for
/// the caller (`prefs_ui`'s backend picker turns it into a no-op precisely so
/// no confirmation appears) rather than something to swallow here.
///
/// # Why the cells are joined and not spaced
///
/// Separated cells are what this app drew before, and they read as a row of
/// independent buttons: three things you might press, rather than one control
/// with three positions. Joining them -- no gap, straight interior edges, a
/// single rounded outline round the whole run -- is what says the cells are
/// alternatives to each other and that exactly one region of the row will be
/// lit. The rounding is therefore applied to the run and not to the cell:
/// only the first cell's left corners and the last cell's right corners are
/// [`SEGMENT_RADIUS`], and everything between them is square.
///
/// # The one-pixel overlap is not a rounding error
///
/// Each cell paints its own 1px stroke *inside* its own rect, so two cells
/// laid end to end would draw two adjacent hairlines and the interior edges
/// would come out twice as heavy as the outer ones. Every cell after the
/// first therefore starts one point back, over its neighbour's right edge, so
/// the two strokes land on the same pixel -- the same thing CSS segmented
/// controls do with a negative left margin. The run's total width is short by
/// one point per seam for exactly that reason.
///
/// # Selected is [`BLUE`], not the nav's wash
///
/// The wash (`BLUE_WASH` behind `BLUE_DEEP`) says "this is the row you are
/// reading" in the nav, where a whole column of rows is on screen and only
/// one of them may shout. A multiple-choice row is the opposite situation:
/// the cell in force is the answer to the question in the label above it, and
/// it is competing with two or six neighbours a point away rather than with
/// nothing. It gets the full fill and white text, which is the same weight
/// this design system already gives [`primary_button`] -- the other place
/// where one control in a group is the one that matters.
pub fn segmented_control(ui: &mut Ui, segments: &[Segment<'_>]) -> Option<usize> {
    let widths = segment_widths(ui, segments);
    let (run, response) =
        ui.allocate_exact_size(Vec2::new(run_width(&widths), SEGMENT_HEIGHT), Sense::click());
    // Read once: `hover_pos` is a pointer position and this control is
    // hit-testing it cell by cell, so a second read mid-loop could put the
    // hover on one cell and the click on another within one frame.
    let hovered = response.hover_pos().and_then(|at| segment_at(run, &widths, at));
    if hovered.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    for (index, segment) in segments.iter().enumerate() {
        let (fill, ink) = if segment.selected {
            (BLUE, Color32::WHITE)
        } else if hovered == Some(index) {
            (CANVAS, INK)
        } else {
            (CARD, INK)
        };
        // The selected cell's edge is its own fill rather than the run's
        // grey: a grey hairline drawn round a blue cell reads as a ring
        // hanging off the end of the pill, and the outline is supposed to be
        // the outline of one control.
        let edge = if segment.selected { BLUE } else { BORDER };
        paint_segment(
            ui,
            cell_rect(run, &widths, index),
            segments.len(),
            index,
            fill,
            ink,
            edge,
            segment.label,
        );
    }
    response.clicked().then(|| hovered).flatten()
}

/// [`segmented_control`]'s inert twin, for a question whose answer is fixed
/// by something outside the row.
///
/// A separate function rather than an `enabled` flag, which is the shape this
/// design system already uses for the same distinction ([`toggle_pill`] and
/// [`toggle_pill_disabled`]): the callers that have no disabled state gain
/// nothing from carrying one, and the two paint different enough that a flag
/// inside one body would be a branch on every line of it.
///
/// **Inert, not merely grey.** The run senses hover only, so there is no path
/// by which a cell can be pressed and nothing for a caller to ignore.
///
/// **The cell in force stays identifiable**, in [`BLUE_WASH`] rather than the
/// live control's [`BLUE`]: a run that greyed every cell identically would
/// tell the reader they have no answer at all, when what is true is that they
/// have this one and cannot change it. The wash is the weight the answer
/// deserves when it is not a control -- present, legible, and visibly not
/// something to press.
pub fn segmented_control_disabled(ui: &mut Ui, segments: &[Segment<'_>]) {
    let widths = segment_widths(ui, segments);
    let (run, _) =
        ui.allocate_exact_size(Vec2::new(run_width(&widths), SEGMENT_HEIGHT), Sense::hover());
    for (index, segment) in segments.iter().enumerate() {
        let fill = if segment.selected { BLUE_WASH } else { CARD };
        paint_segment(
            ui,
            cell_rect(run, &widths, index),
            segments.len(),
            index,
            fill,
            TEXT_GHOST,
            BORDER,
            segment.label,
        );
    }
}

/// The height every cell in a run shares, and the height of the run.
///
/// The same 28 the Preferences window's steppers, key buttons and form fields
/// are, because a multiple-choice row sits in cards beside all three and a
/// control that were a couple of points taller would make the card's rows
/// look mismeasured rather than deliberate.
pub const SEGMENT_HEIGHT: f32 = 28.0;

/// A cell's horizontal breathing room, both sides together -- so a cell is
/// its label's width plus this, and the cells in a run are therefore
/// different widths from one another. Sizing every cell to the widest label
/// would make "Card" as wide as "The official Bitwarden CLI" and turn a row
/// of alternatives into a row of mostly empty boxes.
pub const SEGMENT_PADDING: f32 = 20.0;

/// The run's outer corner radius: [`secondary_button`]'s and
/// [`primary_button`]'s own 7, so the multiple-choice row is rounded like
/// every other pressable thing in this app.
const SEGMENT_RADIUS: u8 = 7;

/// How far each cell after the first is pulled back over its neighbour, so
/// the two 1px strokes at a seam land on one pixel. See
/// [`segmented_control`]'s note on the overlap.
///
/// Public for the same reason [`SEGMENT_HEIGHT`] is: "these cells are joined
/// rather than spaced" is a claim a surface's own tests make about the run
/// they draw, and a surface that restated the number to make it would be a
/// second copy of this measurement waiting to disagree with the control.
pub const SEGMENT_SEAM: f32 = 1.0;

/// Each cell's width: its own label at the run's font, plus
/// [`SEGMENT_PADDING`].
fn segment_widths(ui: &Ui, segments: &[Segment<'_>]) -> Vec<f32> {
    segments
        .iter()
        .map(|segment| segment_galley(ui, segment.label, INK).size().x + SEGMENT_PADDING)
        .collect()
}

/// The whole run, seams deducted -- [`SEGMENT_SEAM`] per join, of which there
/// is one fewer than there are cells.
fn run_width(widths: &[f32]) -> f32 {
    widths.iter().sum::<f32>() - SEGMENT_SEAM * widths.len().saturating_sub(1) as f32
}

/// Where cell `index` sits inside `run`.
fn cell_rect(run: Rect, widths: &[f32], index: usize) -> Rect {
    let left = run.left() + widths[..index].iter().sum::<f32>() - SEGMENT_SEAM * index as f32;
    Rect::from_min_size(Pos2::new(left, run.top()), Vec2::new(widths[index], run.height()))
}

/// Which cell `at` is over, if any.
///
/// The seams overlap, so a point on one belongs to two cells; the first match
/// wins, which puts the shared pixel on the left-hand cell. Either answer is
/// defensible and the point is that ONE of them is always given -- a hit test
/// that returned two answers is how a click could light one cell and select
/// another.
fn segment_at(run: Rect, widths: &[f32], at: Pos2) -> Option<usize> {
    (0..widths.len()).find(|&index| cell_rect(run, widths, index).contains(at))
}

/// One cell, painted: fill, the run's outline where this cell is on it, and
/// the label centred.
#[allow(clippy::too_many_arguments)]
fn paint_segment(
    ui: &Ui,
    rect: Rect,
    count: usize,
    index: usize,
    fill: Color32,
    ink: Color32,
    edge: Color32,
    label: &str,
) {
    ui.painter().rect(
        rect,
        segment_corners(count, index),
        fill,
        Stroke::new(1.0, edge),
        StrokeKind::Inside,
    );
    let galley = segment_galley(ui, label, ink);
    ui.painter().galley(
        Pos2::new(rect.center().x - galley.size().x / 2.0, rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
}

/// **The rounding belongs to the run, not the cell.** The first cell rounds
/// its left corners, the last rounds its right ones, and a cell that is both
/// -- a run of one -- rounds all four; everything in between is square, which
/// is what makes the interior edges read as seams rather than as gaps between
/// separate buttons.
fn segment_corners(count: usize, index: usize) -> CornerRadius {
    let first = if index == 0 { SEGMENT_RADIUS } else { 0 };
    let last = if index + 1 == count { SEGMENT_RADIUS } else { 0 };
    CornerRadius { nw: first, sw: first, ne: last, se: last }
}

/// A cell's label, laid out. One place, so the width a cell is ALLOCATED and
/// the text later PAINTED into it cannot be measured at two different fonts
/// -- which is how a label ends up a point wider than the box round it.
fn segment_galley(ui: &Ui, label: &str, color: Color32) -> Arc<egui::Galley> {
    ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::new(SEGMENT_TEXT_SIZE, FontFamily::Name(SEMIBOLD.into())),
        color,
    )
}

/// The cell label's size: 12px, the same as the Preferences window's other
/// in-card controls.
const SEGMENT_TEXT_SIZE: f32 = 12.0;

// ---------------------------------------------------------------------------
// The dropdown: the multiple-choice control for a set a run cannot hold
// ---------------------------------------------------------------------------

/// One row of a [`dropdown`]: what it says, and whether it is the answer
/// currently in force.
///
/// The same shape as [`Segment`], and for the same stated reason -- two fields
/// a caller could transpose and still compile.
pub struct Choice<'a> {
    /// The words on the row, and, when `selected`, the words in the closed
    /// box. One string for both, so the control cannot say one thing shut and
    /// another open.
    pub label: &'a str,
    /// Painted as the answer in force: [`BLUE_WASH`] behind [`BLUE_DEEP`].
    pub selected: bool,
}

/// **The app's multiple-choice control for a set too large to lay out.**
///
/// Returns the index of the row that was chosen this frame, or `None`.
///
/// # When this and not [`segmented_control`]
///
/// A segmented run is the right control while two things hold: the
/// alternatives fit on one line, and seeing them beside each other helps you
/// choose. Both are properties of the *set*, not of the question, and both
/// fail at the same place -- somewhere around five or six cells a run stops
/// being a row of alternatives and becomes a wall of boxes, and past the width
/// of its container it stops being anything at all.
///
/// This control is what the question becomes on the other side of that line.
/// It costs one extra click, states the answer in force in the same space a
/// run's widest cell would have taken, and does not grow when the set does.
/// **It is not a replacement for the run**: a two- or three-way choice still
/// belongs in a segmented control, where the alternatives are worth reading
/// together and a click is a click.
///
/// # It is one box wide, and the caller says how wide
///
/// Unlike a run, whose width is the sum of its labels, a dropdown is a fixed
/// box -- so it can sit in a column of controls that line up, which is exactly
/// what §5a's Access block is. The caller passes the width because the
/// container knows it and this function cannot: a control that sized itself to
/// its longest row would jump about as the answer changed.
///
/// # The popup is egui's menu layer, painted in this app's vocabulary
///
/// The layer, the click-outside behaviour and the escape key are
/// `egui::Popup::menu`'s, which is what the detail pane's kebab menu already
/// uses -- inventing a second floating-layer implementation for one control is
/// exactly the kind of parallel machinery this module exists to prevent. What
/// is *painted* into it is this file's own: no egui button frames, the same
/// hover wash a segmented cell gets, and the answer in force in the wash
/// rather than in a tick.
pub fn dropdown(ui: &mut Ui, width: f32, current: &str, choices: &[Choice<'_>]) -> Option<usize> {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, DROPDOWN_HEIGHT), Sense::click());
    let hovered = response.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().rect(
        rect,
        CornerRadius::same(SEGMENT_RADIUS),
        if hovered { CANVAS } else { CARD },
        Stroke::new(1.0, BORDER_STRONG),
        StrokeKind::Inside,
    );
    let galley = segment_galley(ui, current, INK);
    ui.painter().galley(
        Pos2::new(rect.left() + DROPDOWN_PAD_X, rect.center().y - galley.size().y / 2.0),
        galley,
        INK,
    );
    paint_chevron(ui, chevron_rect(rect), TEXT_MUTED);

    let mut chosen = None;
    egui::Popup::menu(&response).show(|ui| {
        ui.set_min_width(width);
        for (index, choice) in choices.iter().enumerate() {
            if dropdown_row(ui, width, choice) {
                chosen = Some(index);
                ui.close();
            }
        }
    });
    chosen
}

/// [`dropdown`]'s inert twin, for a question whose answer is fixed by
/// something outside the control.
///
/// A separate function rather than an `enabled` flag, which is the split this
/// design system already makes twice ([`segmented_control_disabled`],
/// [`toggle_pill_disabled`]) -- and the reason is the same one, sharpened.
/// `add_enabled_ui(false)` round a live dropdown fades the whole box, answer
/// included, to egui's disabled opacity; what the reader needs while a publish
/// is running is not "you have no lifetime" but "you have this one and cannot
/// change it". So the box keeps its full-strength text on [`CARD_TINT`], loses
/// its chevron (there is nothing to open) and senses nothing at all.
pub fn dropdown_disabled(ui: &mut Ui, width: f32, current: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, DROPDOWN_HEIGHT), Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(SEGMENT_RADIUS),
        CARD_TINT,
        Stroke::new(1.0, BORDER),
        StrokeKind::Inside,
    );
    let galley = segment_galley(ui, current, TEXT_MUTED);
    ui.painter().galley(
        Pos2::new(rect.left() + DROPDOWN_PAD_X, rect.center().y - galley.size().y / 2.0),
        galley,
        TEXT_MUTED,
    );
}

/// The closed box's height: [`BUTTON_HEIGHT`], because a dropdown stands in a
/// column beside text fields and buttons and a box a few points off would make
/// the column look mismeasured.
pub const DROPDOWN_HEIGHT: f32 = BUTTON_HEIGHT;

/// The gap between the box's edge and its text, both sides.
pub const DROPDOWN_PAD_X: f32 = 10.0;

/// One open row's height. Shorter than the closed box: a list of rows is read
/// as a list, and rows the height of buttons read as a stack of buttons.
pub const DROPDOWN_ROW_HEIGHT: f32 = 24.0;

/// How much of a [`dropdown`]'s width is NOT available to its text: a pad each
/// side, plus the chevron the closed box carries.
///
/// `pub` for the reason [`SEGMENT_SEAM`] is: "every row fits the box" is a
/// claim a caller's own test has to make about the set it passes -- a fixed
/// box clips rather than overflows, so nothing else would catch a row that
/// grew -- and a caller that restated this arithmetic would be a second copy
/// of it waiting to disagree with the control.
pub const DROPDOWN_TEXT_BUDGET: f32 = DROPDOWN_PAD_X * 3.0 + CHEVRON_HALF * 2.0;

/// How wide `label` is in a [`dropdown`], at the exact font the control lays
/// it out in.
///
/// Exists so a caller's fit test measures what the control measures, rather
/// than a guess at the font. See [`DROPDOWN_TEXT_BUDGET`].
pub fn dropdown_text_width(ui: &Ui, label: &str) -> f32 {
    segment_galley(ui, label, INK).size().x
}

/// Half the chevron's width, and the distance it stands in from the box's
/// right edge.
const CHEVRON_HALF: f32 = 4.0;

/// Where the chevron sits inside a closed [`dropdown`] box.
fn chevron_rect(box_rect: Rect) -> Rect {
    let centre =
        Pos2::new(box_rect.right() - DROPDOWN_PAD_X - CHEVRON_HALF, box_rect.center().y);
    Rect::from_center_size(centre, Vec2::splat(CHEVRON_HALF * 2.0))
}

/// The "there is more under this" glyph: two strokes, pointing down.
///
/// Drawn rather than typed. A `▾` is a font's idea of a triangle and this app
/// paints every other glyph it uses -- see the star, the eye and the kebab --
/// so a character here would be the one mark whose weight and size came from
/// somewhere else.
fn paint_chevron(ui: &Ui, rect: Rect, color: Color32) {
    let stroke = Stroke::new(ICON_STROKE, color);
    let painter = ui.painter();
    painter.line_segment(
        [Pos2::new(rect.left(), rect.top()), Pos2::new(rect.center().x, rect.bottom())],
        stroke,
    );
    painter.line_segment(
        [Pos2::new(rect.center().x, rect.bottom()), Pos2::new(rect.right(), rect.top())],
        stroke,
    );
}

/// One open row, returning whether it was chosen.
fn dropdown_row(ui: &mut Ui, width: f32, choice: &Choice<'_>) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, DROPDOWN_ROW_HEIGHT), Sense::click());
    let hovered = response.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // The answer in force is the wash and not a tick in the margin: a tick
    // needs a gutter every row pays for, and the wash is the weight this
    // design system already gives "the one you have" in the nav.
    let (fill, ink) = if choice.selected {
        (BLUE_WASH, BLUE_DEEP)
    } else if hovered {
        (CANVAS, INK)
    } else {
        (Color32::TRANSPARENT, INK)
    };
    ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
    let galley = segment_galley(ui, choice.label, ink);
    ui.painter().galley(
        Pos2::new(rect.left() + DROPDOWN_PAD_X, rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
    response.clicked()
}

// ---------------------------------------------------------------------------
// The date picker: one month, and only the days that are answers
// ---------------------------------------------------------------------------

/// A `(year, month, day)` on the proleptic-Gregorian calendar
/// [`crate::local_time`] works in. Months and days are 1-based.
///
/// A tuple struct would have been two `u32`s and an `i64` a caller could put
/// in any order; this cannot be got wrong at a call site, which is the reason
/// [`Segment`] is a struct too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Day {
    pub year: i64,
    pub month: u32,
    pub day: u32,
}

/// **One month of a calendar, with the days that are not answers drawn inert.**
///
/// Returns the day that was clicked this frame, or `None`.
///
/// # Which month is on screen is this widget's own business
///
/// It lives in `egui`'s memory under `id_salt`, not on the caller's draft.
/// Which month a calendar is scrolled to is the same kind of fact as how far a
/// list is scrolled: it is not part of the answer, it is not published, it is
/// not validated, and a screen that carried it would have it in its `Default`,
/// its `Debug` and its equality for no reason. It opens on the month of
/// whatever is already chosen, or on `first` when nothing is.
///
/// # Why a month grid and not three steppers
///
/// The grid **cannot express a date that does not exist**. A year box, a month
/// box and a day box can say 31 February, so they need a rule about what
/// happens when they do, and every such rule is a way for the form to answer a
/// question the user did not ask. There are no impossible cells here: February
/// draws 28 days because February has 28 days.
///
/// It also cannot express a date outside the window the caller allows. Days
/// before `first` and after `last` are painted inert rather than hidden --
/// hidden days make a month with a hole in it, which reads as a drawing bug;
/// greyed ones say "this day exists and is not on offer", which is the true
/// statement.
///
/// # Why month arrows and no year control
///
/// Because the window is bounded and small. This app offers a picked date at
/// most twelve months out (`send::MAX_PICKED_MONTHS`), so the far end is
/// twelve presses away and the common case -- a date in the next few weeks --
/// is none or one. A year control would be a second navigation axis earning
/// its keep only outside a range this picker refuses to show, and the arrows
/// stop dead at the ends of the window rather than wandering into months where
/// every cell is grey.
///
/// # It is a fixed size
///
/// Seven columns of [`CALENDAR_CELL`] and six week rows, always six, whether
/// or not the month needs the last one. A grid that changed height as the
/// month changed would shove everything under it up and down as the user
/// pressed the arrows.
pub fn date_picker(
    ui: &mut Ui,
    id_salt: &str,
    selected: Option<Day>,
    first: Day,
    last: Day,
) -> Option<Day> {
    let memory_id = egui::Id::new(("date-picker", id_salt));
    // Clamped on the way in as well as on the way out: the window moves as the
    // clock does, so a month remembered yesterday can be behind `first` today,
    // and a calendar that opened on a month with no offered day in it would
    // look broken rather than bounded.
    let remembered = ui
        .ctx()
        .memory(|m| m.data.get_temp::<Day>(memory_id))
        .unwrap_or_else(|| month_of(selected.unwrap_or(first)));
    let mut shown = remembered.clamp(month_of(first), month_of(last));
    let picked = date_picker_at(ui, &mut shown, selected, first, last);
    ui.ctx().memory_mut(|m| m.data.insert_temp(memory_id, shown));
    picked
}

/// [`date_picker`]'s body, with the month on screen passed in rather than
/// remembered.
///
/// Split out so the drawing is a pure function of its arguments -- a test can
/// put the calendar on any month and read what it painted in one frame,
/// without having to drive `egui`'s memory to get there.
pub fn date_picker_at(
    ui: &mut Ui,
    shown: &mut Day,
    selected: Option<Day>,
    first: Day,
    last: Day,
) -> Option<Day> {
    let (frame, _) = ui.allocate_exact_size(
        Vec2::new(CALENDAR_WIDTH, CALENDAR_HEIGHT),
        Sense::hover(),
    );
    ui.painter().rect(
        frame,
        CornerRadius::same(SEGMENT_RADIUS),
        CARD_TINT,
        Stroke::new(1.0, BORDER),
        StrokeKind::Inside,
    );

    // ---- header: ‹  Mar 2027  › -----------------------------------------
    let header = Rect::from_min_size(frame.min, Vec2::new(CALENDAR_WIDTH, CALENDAR_HEADER_H));
    let back = Rect::from_min_size(
        Pos2::new(header.left() + 4.0, header.top() + 4.0),
        Vec2::splat(CALENDAR_HEADER_H - 8.0),
    );
    let forward = Rect::from_min_size(
        Pos2::new(header.right() - 4.0 - (CALENDAR_HEADER_H - 8.0), header.top() + 4.0),
        Vec2::splat(CALENDAR_HEADER_H - 8.0),
    );
    if calendar_arrow(ui, back, false, *shown > month_of(first)) {
        *shown = step_month(*shown, -1);
    }
    if calendar_arrow(ui, forward, true, *shown < month_of(last)) {
        *shown = step_month(*shown, 1);
    }
    let title = ui.painter().layout_no_wrap(
        format!("{} {}", crate::local_time::month_name(shown.month), shown.year),
        FontId::new(SEGMENT_TEXT_SIZE, FontFamily::Name(SEMIBOLD.into())),
        INK,
    );
    ui.painter().galley(
        Pos2::new(
            header.center().x - title.size().x / 2.0,
            header.center().y - title.size().y / 2.0,
        ),
        title,
        INK,
    );

    // ---- the weekday strip ----------------------------------------------
    //
    // Monday first, and one letter each. Monday-first is what a European
    // desktop expects and is what this app's only other calendar-shaped
    // thing -- nothing -- has to agree with; one letter because seven
    // three-letter headings do not fit a grid whose cells are sized to two
    // digits.
    for (column, letter) in WEEKDAY_INITIALS.iter().enumerate() {
        let cell = calendar_cell(frame, column, None);
        let galley = ui.painter().layout_no_wrap(
            (*letter).to_owned(),
            FontId::new(CALENDAR_WEEKDAY_PX, FontFamily::Name(SEMIBOLD.into())),
            TEXT_GHOST,
        );
        ui.painter().galley(
            Pos2::new(cell.center().x - galley.size().x / 2.0, cell.center().y - galley.size().y / 2.0),
            galley,
            TEXT_GHOST,
        );
    }

    // ---- the days --------------------------------------------------------
    let first_of_month = crate::local_time::days_from_civil(shown.year, shown.month, 1);
    let lead = monday_column(first_of_month);
    let length = crate::local_time::days_in_month(shown.year, shown.month);
    let mut picked = None;
    for day in 1..=length {
        let index = lead + (day as usize) - 1;
        let cell = calendar_cell(frame, index % 7, Some(index / 7));
        let here = Day { year: shown.year, month: shown.month, day };
        if calendar_day(ui, cell, day, selected == Some(here), here >= first && here <= last) {
            picked = Some(here);
        }
    }
    picked
}

/// The seven column headings, Monday first.
pub const WEEKDAY_INITIALS: [&str; 7] = ["M", "T", "W", "T", "F", "S", "S"];

/// One day cell's size. Two digits at [`SEGMENT_TEXT_SIZE`] plus room to be a
/// click target; seven of them are [`CALENDAR_WIDTH`].
pub const CALENDAR_CELL: Vec2 = Vec2::new(28.0, 24.0);

/// The month strip's height -- the two arrows and the month's name.
const CALENDAR_HEADER_H: f32 = 28.0;

/// The weekday initials' height.
const CALENDAR_WEEKDAY_H: f32 = 16.0;

/// The weekday initials' size. Smaller than a day: they are a key to the grid,
/// not part of it.
const CALENDAR_WEEKDAY_PX: f32 = 10.0;

/// Six week rows, always. See [`date_picker`] on why the grid does not shrink
/// for a short month.
const CALENDAR_ROWS: usize = 6;

/// The picker's outer width: seven columns and a point of padding each side.
pub const CALENDAR_WIDTH: f32 = CALENDAR_CELL.x * 7.0 + CALENDAR_PAD * 2.0;

/// The picker's outer height.
pub const CALENDAR_HEIGHT: f32 = CALENDAR_HEADER_H
    + CALENDAR_WEEKDAY_H
    + CALENDAR_CELL.y * CALENDAR_ROWS as f32
    + CALENDAR_PAD;

/// The inset between the grid and the card round it.
const CALENDAR_PAD: f32 = 6.0;

/// Where a grid cell sits. `row` is `None` for the weekday strip, which is the
/// row above the first week.
fn calendar_cell(frame: Rect, column: usize, row: Option<usize>) -> Rect {
    let top = match row {
        None => frame.top() + CALENDAR_HEADER_H,
        Some(row) => {
            frame.top() + CALENDAR_HEADER_H + CALENDAR_WEEKDAY_H + row as f32 * CALENDAR_CELL.y
        }
    };
    let height = match row {
        None => CALENDAR_WEEKDAY_H,
        Some(_) => CALENDAR_CELL.y,
    };
    Rect::from_min_size(
        Pos2::new(frame.left() + CALENDAR_PAD + column as f32 * CALENDAR_CELL.x, top),
        Vec2::new(CALENDAR_CELL.x, height),
    )
}

/// Which column a Unix epoch day falls in, Monday = 0.
///
/// Epoch day 0 is Thursday 1 January 1970, so Monday is three days before it;
/// `+ 3` and a Euclidean remainder puts Monday at zero for dates either side
/// of the epoch.
fn monday_column(epoch_day: i64) -> usize {
    (epoch_day + 3).rem_euclid(7) as usize
}

/// The month a day belongs to, with the day flattened to the 1st, so two
/// [`Day`]s can be compared by month alone.
fn month_of(day: Day) -> Day {
    Day { day: 1, ..day }
}

/// `shown` moved one month in `direction`, staying on the 1st.
fn step_month(shown: Day, direction: i64) -> Day {
    let zero_based = shown.month as i64 - 1 + direction;
    Day {
        year: shown.year + zero_based.div_euclid(12),
        month: (zero_based.rem_euclid(12) + 1) as u32,
        day: 1,
    }
}

/// One month arrow, returning whether it was pressed. A disabled arrow is
/// drawn and senses nothing -- [`segmented_control_disabled`]'s rule: an end
/// of the range should look like an end, not like a control that has gone
/// missing.
fn calendar_arrow(ui: &mut Ui, rect: Rect, forward: bool, enabled: bool) -> bool {
    let response = ui.interact(
        rect,
        ui.id().with(("calendar-arrow", forward)),
        if enabled { Sense::click() } else { Sense::hover() },
    );
    let hovered = enabled && response.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        ui.painter().rect_filled(rect, CornerRadius::same(4), CANVAS);
    }
    let color = if enabled { INK } else { TEXT_GHOST };
    let stroke = Stroke::new(ICON_STROKE, color);
    let tip_x = if forward { rect.center().x + 2.5 } else { rect.center().x - 2.5 };
    let back_x = if forward { rect.center().x - 2.0 } else { rect.center().x + 2.0 };
    let painter = ui.painter();
    painter.line_segment(
        [Pos2::new(back_x, rect.center().y - 4.0), Pos2::new(tip_x, rect.center().y)],
        stroke,
    );
    painter.line_segment(
        [Pos2::new(tip_x, rect.center().y), Pos2::new(back_x, rect.center().y + 4.0)],
        stroke,
    );
    enabled && response.clicked()
}

/// One day cell, returning whether it was chosen. Out-of-window days sense
/// nothing at all, so there is no click for a caller to have to ignore.
fn calendar_day(ui: &mut Ui, rect: Rect, day: u32, selected: bool, offered: bool) -> bool {
    let response = ui.interact(
        rect,
        ui.id().with(("calendar-day", rect.left() as i32, rect.top() as i32)),
        if offered { Sense::click() } else { Sense::hover() },
    );
    let hovered = offered && response.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let (fill, ink) = if selected {
        (BLUE, Color32::WHITE)
    } else if hovered {
        (CANVAS, INK)
    } else if offered {
        (Color32::TRANSPARENT, INK)
    } else {
        (Color32::TRANSPARENT, TEXT_GHOST)
    };
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(
            Rect::from_center_size(rect.center(), Vec2::splat(CALENDAR_CELL.y - 2.0)),
            CornerRadius::same(4),
            fill,
        );
    }
    let galley = ui.painter().layout_no_wrap(
        day.to_string(),
        FontId::new(SEGMENT_TEXT_SIZE, FontFamily::Name(REGULAR.into())),
        ink,
    );
    ui.painter().galley(
        Pos2::new(rect.center().x - galley.size().x / 2.0, rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
    offered && response.clicked()
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
        mark(ui, CARD_HEADER_MARK_H);
        // Real tracking (0.1em at 11px), not spaces between letters.
        ui.label(letterspaced(
            WORDMARK_CAPS,
            CARD_HEADER_WORD_PX,
            BOLD,
            CARD_HEADER_TRACKING,
            TEXT_SECONDARY,
        ));
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

/// The mark's hit target, square.
///
/// Sixteen points around a glyph that is seven across, so more than half of
/// what the pointer can hit is empty space. That is deliberate and it is the
/// number that must not be shrunk to fit the drawing: the visible ✕ is the
/// smallest control in this app, and a hit target cut down to the ink is a
/// control a person with an ordinary hand misses.
pub const CLOSE_MARK_HIT: f32 = 16.0;

/// Half the diagonal extent of each arm, so the drawn ✕ is
/// [`CLOSE_MARK_SPAN`] across -- seven points inside a sixteen-point box.
const CLOSE_MARK_ARM: f32 = 3.5;

/// How wide the ✕'s ink is, which is **not** [`CLOSE_MARK_HIT`].
///
/// `pub` because it is the number the paint tests in four other files find the
/// mark BY: a headless frame sees two diagonal strokes, and the size of them
/// is what tells the dismiss mark apart from every other line this app draws.
/// Those tests must not re-type a 7.0, for the reason the mark itself is
/// shared -- a stand-in that stopped matching would go on passing.
pub const CLOSE_MARK_SPAN: f32 = CLOSE_MARK_ARM * 2.0;

/// The arms' weight. Below 1.0 the mark disappears on a 100% display and
/// above 1.5 it reads as a delete rather than as a dismiss.
const CLOSE_MARK_STROKE: f32 = 1.3;

/// What the pointer is told the mark does. One word, and the same word on
/// every card: "Close" would be a promise about the window.
const CLOSE_MARK_TOOLTIP: &str = "Dismiss";

/// **How far the mark's hit box is held off the card's right-hand edge**, and
/// the one number in this family that had to be CHOSEN rather than lifted.
///
/// The two marks that existed before this family disagreed. `prefs_ui` centred
/// its 16pt box 22 points in from the card's edge -- a box whose right edge is
/// therefore 14 points off it. `totp_add` right-aligned its box inside a
/// header padded 18, so its box ended 18 points off the edge. Same glyph, same
/// size, four points apart, and neither file knew the other had answered the
/// question.
///
/// **Fourteen, and the argument is that it is the one that lines the GLYPH up
/// rather than the box.** [`CLOSE_MARK_HIT`] carries 4.5 points of empty
/// margin around the ink on every side, so a box inset 14 puts the visible arm
/// tip 18.5 points off the card's edge -- which is within half a point of the
/// 18 `totp_add`'s header pads its content by and inside the 20 the
/// hand-built cards use. Inset 18 or 20 would align the invisible rectangle
/// and push the mark a visible four to six points further in than the card's
/// own content column, which is the arrangement that reads as a mark floating
/// loose in the corner. Fourteen is also what the app's most-used modal
/// already shipped, so the card a user opens most often does not move.
pub const MODAL_CLOSE_INSET: f32 = 14.0;

/// The mark's ink at rest on a coloured header band.
///
/// White at an alpha that is to an accent fill what [`TEXT_GHOST`] is to a
/// white card: present, quiet, and clearly not the thing the card is asking
/// about. Full white at rest would make the dismiss the loudest thing in a
/// band whose job is to carry the card's title.
/// Written out premultiplied -- white at alpha 190 is (190, 190, 190, 190) --
/// because `Color32::from_white_alpha` is not a `const fn` and this has to be
/// a constant to sit beside the rest of this app's inks.
const CLOSE_MARK_ON_ACCENT: Color32 = Color32::from_rgba_premultiplied(190, 190, 190, 190);

/// Which surface the mark is being drawn on, and therefore which pair of inks
/// it wears.
///
/// **The surface, not the colours.** A call site that passed two `Color32`s
/// could pass any two, and the first card to pass a red pair would be a
/// dismiss control that reads as a delete. There are exactly two surfaces a
/// modal in this app puts a header on, so there are exactly two answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseInk {
    /// A white or near-white header: the hand-built cards' title line,
    /// `prefs_ui`'s titlebar-style band, `totp_add`'s ruled strip.
    OnCard,
    /// [`modal_card`]'s accent band, where the card's own colour is behind the
    /// mark and ghost-grey would be invisible.
    OnAccent,
}

impl CloseInk {
    /// `(at rest, under the pointer)`.
    fn pair(self) -> (Color32, Color32) {
        match self {
            CloseInk::OnCard => (TEXT_GHOST, INK),
            CloseInk::OnAccent => (CLOSE_MARK_ON_ACCENT, Color32::WHITE),
        }
    }
}

/// The mark's two strokes, its hover ink and its cursor, given a rectangle and
/// the response that was already registered for it.
///
/// Shared by both entry points so that "what the mark looks like" is answered
/// once. A disabled response hovers nothing, so a mark inside a disabled
/// `Ui` paints at its resting ink and offers no pointing hand -- which is the
/// treatment `login_ui`'s chrome already gives a ✕ that will not answer.
fn paint_close_mark(ui: &Ui, rect: Rect, response: &Response, ink: CloseInk) {
    let (rest, hot) = ink.pair();
    let color = if response.hovered() { hot } else { rest };
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let stroke = Stroke::new(CLOSE_MARK_STROKE, color);
    let arm = CLOSE_MARK_ARM;
    let c = rect.center();
    let painter = ui.painter();
    painter.line_segment([c + Vec2::new(-arm, -arm), c + Vec2::new(arm, arm)], stroke);
    painter.line_segment([c + Vec2::new(arm, -arm), c + Vec2::new(-arm, arm)], stroke);
}

/// The dismiss ✕ itself: a [`CLOSE_MARK_HIT`] hit target with the design's
/// ghost-grey glyph, darkening to ink on hover so it reads as clickable
/// despite having no button chrome (the design draws it as bare text).
///
/// Stroked as two crossing lines rather than drawn as the character U+2715:
/// neither the bundled Archivo faces nor egui's fallback stack carry that
/// codepoint, so as text it renders as a tofu box. Two strokes are also
/// sharper at this size than any glyph would be.
///
/// **This is the allocating half of the mark**, for a header built out of
/// widgets in a layout. A card that knows the rectangle its header occupies --
/// which is every modal in this crate -- wants [`modal_dismiss_mark`].
pub fn close_glyph(ui: &mut Ui) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(CLOSE_MARK_HIT), Sense::click());
    paint_close_mark(ui, rect, &response, CloseInk::OnCard);
    response.on_hover_text(CLOSE_MARK_TOOLTIP)
}

/// The dismiss ✕ pinned to the right-hand end of `line`, which is the
/// rectangle of the header row the mark belongs on -- a coloured band, a
/// ruled strip, or just the line a hand-built card's title is set on.
///
/// The mark's box is [`MODAL_CLOSE_INSET`] off `line.right()` and centred on
/// `line.center().y`, so a caller's only job is to hand over a rectangle whose
/// right edge is the CARD's edge and whose vertical middle is the header's.
///
/// # It does not allocate, and that is what makes it safe to add
///
/// `ui.interact` at an absolute rect touches neither the `Ui`'s cursor nor its
/// min rect, so dropping this into a card that already exists moves nothing
/// the card already paints. Seven cards took the mark in one pass without a
/// single layout test having to be re-measured, which would not have been true
/// of a widget that claimed space on the title's line.
///
/// # It must be called AFTER [`modal_drag_handle`], and that is not a style
/// note
///
/// egui hit-tests clicks and drags separately but not independently: where the
/// topmost widget under the pointer senses drags and not clicks, it swallows
/// the click rather than passing it down. Every modal in this app is dragged
/// by its header, so the drag strip lies over exactly the band this mark sits
/// in -- registered first it is UNDERNEATH, egui reports the mark for the
/// click and the strip for the drag, and both work. Registered the other way
/// round the mark is drawn, is hovered, and does nothing, which is the precise
/// defect this app has shipped before. See [`modal_drag_handle`], and
/// `modal_drag_tests::a_click_in_the_header_still_reaches_the_control_the_handle_lies_under`.
pub fn modal_dismiss_mark(ui: &mut Ui, line: Rect, ink: CloseInk) -> Response {
    let at = Rect::from_center_size(
        Pos2::new(line.right() - MODAL_CLOSE_INSET - CLOSE_MARK_HIT / 2.0, line.center().y),
        Vec2::splat(CLOSE_MARK_HIT),
    );
    // The `Ui`'s own id and not the layer's: `totp_add` draws its card inside
    // the vault window's panel layer rather than in an `Area` of its own, so a
    // layer-derived id would be shared with whatever else that layer marks.
    let response = ui.interact(at, ui.id().with("modal-dismiss"), Sense::click());
    paint_close_mark(ui, at, &response, ink);
    response.on_hover_text(CLOSE_MARK_TOOLTIP)
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

/// Corners in the modal header's warning triangle, and the samples each of
/// them is rounded across -- [`star_outline`]'s scheme and, endpoints
/// included, its arithmetic.
const WARNING_CORNERS: usize = 3;
const WARNING_ROUND_SEGMENTS: usize = 3;

/// Vertices in the warning triangle's outline: one rounding arc per
/// [`WARNING_CORNERS`].
///
/// **Declared with this family rather than beside its painter, because what
/// the number has to satisfy is this family's rule and not the modal's.** A
/// triangle drawn with the three points its geometry actually has is an
/// unfilled closed path of [`ENVELOPE_FLAP_VERTICES`] points, which is
/// exactly what `icon_probe::envelopes` anchors on -- and that probe *panics*
/// on a flap it cannot find a four-point body around, so the collision would
/// not have been a misreport but a crash in any test walking a frame that
/// carried both marks. That is the hazard the old hand-drawn glyph dodged by
/// stroking three loose segments, at the cost of its own corners; rounding
/// them puts the count at twelve, which nothing else in this crate closes
/// over. [`no_two_drawn_icons_share_a_vertex_count`] is the live guard.
pub const WARNING_VERTICES: usize = WARNING_CORNERS * (WARNING_ROUND_SEGMENTS + 1);

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
///
/// # Its OFF state used to rest at `TEXT_FAINT`, and that was overshooting
///
/// Reported as "star on details page doesn't look as bold as the rest", and
/// the report is right: [`kebab_button`], [`send_record_button`],
/// [`add_totp_button`] and [`close_pane_button`] all rest at
/// [`TEXT_SECONDARY`], and the star was the one control on the strip resting
/// a shade paler than the rest of it, even though every mark here shares the
/// same [`ICON_STROKE`]/[`STAR_STROKE`] weight -- so what read as "not as
/// bold" was colour, not weight.
///
/// The old reasoning was "the star's fainter grey is the colour of a toggle
/// that is OFF" -- see the note that reasoning left on
/// [`send_record_button`] and [`close_pane_button`] -- and that is being
/// overruled here, not silently dropped: a favourite that is off is still a
/// live, always-clickable control sitting in a strip of live, always-
/// clickable controls, and the strip reading as one weight of ink matters
/// more than marking "off" with a second shade the rest of the strip does
/// not use anywhere else.
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
        TEXT_SECONDARY
    };
    paint_star(ui, rect.center(), STAR_OUTER, on, color);
    response
}

/// The detail header's overflow control: three dots stacked vertically, the
/// menu affordance every desktop app spells the same way.
///
/// **No state of its own.** It took an `armed` flag that turned the dots
/// [`ERROR`] red, because the Delete inside this menu confirmed itself with
/// a two-click arm and that arm could outlive the menu being closed -- so
/// the button had to carry a state the entry could no longer show. The
/// confirmation is a modal now (`vault_window::delete_modal`), which is
/// visible on its own, so a menu button that changes colour to report
/// something happening elsewhere has nothing left to report.
pub fn kebab_button(ui: &mut Ui) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::splat(HEADER_BUTTON_HEIGHT), Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let color = if response.hovered() { INK } else { TEXT_SECONDARY };
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
    // The strip's resting grey, TEXT_SECONDARY -- the star now rests here
    // too; see the note on `star_toggle`'s doc for why its old, fainter
    // resting colour was retired.
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
/// they do. (The favourite star used to rest a shade fainter, on the theory
/// that [`TEXT_FAINT`] was the colour of a TOGGLE THAT IS OFF; see the note
/// on [`star_toggle`]'s doc for why that no longer holds -- the star rests
/// at [`TEXT_SECONDARY`] too now, so this control has no fainter sibling
/// left to be confused with.)
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
/// The hover ink is [`send_record_button`]'s: the strip's own [`INK`] on
/// hover, [`TEXT_SECONDARY`] at rest -- the same resting grey [`star_toggle`]
/// now uses too.
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
    paint_pencil(ui.painter(), rect, color);
    response.on_hover_text("Edit folder")
}

/// [`pencil_glyph_at`]'s SHAPE, with no interaction and no hover state: the
/// same pencil painted into a rect at a colour the caller chooses.
///
/// Split out for §8a's edit badge, which is not a control -- it is the mark on
/// the corner of the item's avatar that says the pane below is a form. Drawn
/// from one place so the two pencils in this app cannot become two drawings
/// of a pencil; `ui.interact` is what the other one adds, and it is exactly
/// what a badge must not have (it would take the pointing hand and a click
/// target over the avatar it sits on).
pub fn paint_pencil(painter: &egui::Painter, rect: Rect, color: Color32) {
    paint_pencil_scaled(painter, rect, color, 1.0);
}

/// [`paint_pencil`] at a fraction of its drawn size.
///
/// The shape is built in absolute local units -- a 13-point pencil however
/// big the rect it is centred in -- which is right for the sidebar, where
/// the rect is a row and the glyph is a glyph. It is wrong inside §8a's
/// 18-point badge, where the same 13 points run corner to corner and the
/// nib and tail are too small to tell apart: it reads as a line struck
/// THROUGH the disc rather than as a pencil in it. Measured on the render,
/// not foreseen.
pub fn paint_pencil_scaled(painter: &egui::Painter, rect: Rect, color: Color32, scale: f32) {

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

    let to_screen =
        |v: Vec2| Pos2::new(bbox_center.x, bbox_center.y) + (v - bbox_center) * scale + offset;
    painter.add(egui::Shape::convex_polygon(
        body.iter().map(|p| to_screen(*p)).collect(),
        color,
        Stroke::NONE,
    ));
    painter.add(egui::Shape::convex_polygon(
        nib.iter().map(|p| to_screen(*p)).collect(),
        color,
        Stroke::NONE,
    ));
}

/// §8a's edit badge: the little white disc on the bottom-right corner of the
/// item's avatar, with a pencil in it.
///
/// It is the one thing on the edit pane's header band that the READ pane's
/// band does not draw, and that is deliberate -- the two bands are otherwise
/// the same strip showing the same item, so the badge is what says which of
/// them you are looking at. §8a draws it in exactly this place and this is
/// where the design puts the answer to "am I editing this?"; the alternative
/// the form used to ship -- a 19-point `Edit login` heading above the pane --
/// was a second title arguing with the record's own name, which is the report
/// this replaces.
///
/// Painted over the tile rather than allocated beside it, so it costs the
/// band no width: the tile is 40 points and the badge hangs off its corner
/// the way §8a's `right: -6px; bottom: -6px` does.
pub fn edit_badge(painter: &egui::Painter, tile: Rect) {
    paint_edit_badge(painter, edit_badge_rect(tile), TEXT_MUTED, CARD);
}

/// Where [`edit_badge`] draws, for a tile of `tile`.
///
/// Public because the badge is a CONTROL now (see [`edit_badge_button`]) and
/// a test that wants to click it has to know where it is -- there is no
/// widget rect to read it off, since the badge is painted over the avatar
/// rather than allocated beside it.
pub fn edit_badge_rect(tile: Rect) -> Rect {
    Rect::from_center_size(
        Pos2::new(
            tile.right() - EDIT_BADGE / 2.0 + EDIT_BADGE_OVERHANG,
            tile.bottom() - EDIT_BADGE / 2.0 + EDIT_BADGE_OVERHANG,
        ),
        Vec2::splat(EDIT_BADGE),
    )
}

fn paint_edit_badge(painter: &egui::Painter, rect: Rect, ink: Color32, fill: Color32) {
    painter.circle(rect.center(), EDIT_BADGE / 2.0, fill, Stroke::new(1.0, BORDER_STRONG));
    // The pencil at §8a's own `width: 10`, inside an 18-point disc.
    paint_pencil_scaled(
        painter,
        Rect::from_center_size(rect.center(), Vec2::splat(EDIT_BADGE_GLYPH)),
        ink,
        EDIT_BADGE_GLYPH / PENCIL_DRAWN_SIZE,
    );
}

/// [`edit_badge`] **as a button**, which is what it is on the edit pane.
///
/// The badge says "you are editing this"; the owner's word on it was "no
/// icons on pencil", and what hangs off it now is everything this app can do
/// with an item's picture -- the menu that used to be a submenu of the item
/// row's right-click menu and of the read pane's kebab, moved here whole.
/// The pencil sits ON the avatar, which is the picture those three entries
/// are about, so it is the one place in the window where the control and the
/// thing it changes are the same object.
///
/// Painted, interacted and returned in one call rather than left to the
/// caller, because the rect is this module's arithmetic ([`edit_badge_rect`])
/// and a caller reconstructing it to place a `Popup` is a second copy of the
/// design's `right: -6px; bottom: -6px`.
pub fn edit_badge_button(ui: &mut Ui, tile: Rect) -> Response {
    let rect = edit_badge_rect(tile);
    // `ui.id().with(..)` and not `next_auto_id`: this is painted over another
    // widget rather than allocated in the layout, so there is no auto id
    // sequence position of its own to take.
    let response = ui.interact(rect, ui.id().with("edit-badge"), Sense::click());
    // The same lift every other quiet icon control in this app takes under
    // the pointer: the ink comes up to `INK`, and the disc keeps its fill so
    // the badge does not change SHAPE on hover.
    let ink = if response.hovered() { INK } else { TEXT_MUTED };
    paint_edit_badge(ui.painter(), rect, ink, CARD);
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// The edit badge's disc: §8a's `width: 18px; height: 18px`.
pub const EDIT_BADGE: f32 = 18.0;

/// How far the badge hangs off the tile's corner: §8a's `right: -6px;
/// bottom: -6px`.
const EDIT_BADGE_OVERHANG: f32 = 6.0;

/// The pencil inside the disc: §8a's `<svg width="10" height="10">`.
const EDIT_BADGE_GLYPH: f32 = 10.0;

/// The size [`paint_pencil`] draws at when it is not scaled -- the height of
/// its own bounding box, which is what [`EDIT_BADGE_GLYPH`] is a fraction
/// OF. Written out because the shape's points are literals inside that
/// function and a caller cannot ask it.
const PENCIL_DRAWN_SIZE: f32 = 13.0;

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
/// The caller MUST give it a container whose right padding is ZERO -- the
/// lane replaces that padding -- and MUST make the reservation unconditional,
/// one of the two ways below. Both requirements are load-bearing:
///
/// * The lane is reserved by `floating_allocated_width`, which egui only
///   applies on the axes it is showing a bar for. Under the default
///   `VisibleWhenNeeded` the lane would therefore appear and disappear as the
///   content crossed the overflow threshold, and the content's right edge --
///   the row tiles' -- would jump 10pt sideways with it. There are two cures
///   and this app ships both, because they suit different panes:
///   - `.scroll_bar_visibility(ScrollBarVisibility::AlwaysVisible)`, which
///     makes egui reserve on every frame, paired with [`hide_scrollbar`] on
///     the frames the content FITS so the always-shown bar is not painted
///     down a pane that cannot move. The item list, the edit form and the
///     read pane take this: each of them knows from its own geometry whether
///     it fits.
///   - capping the scroll area's CONTENT at the width the lane leaves
///     (`ui.set_max_width` as the first thing inside the `show` closure),
///     which reserves it from the other side and leaves the mode alone. The
///     sidebar rail takes this, because "does it fit" is not a question it
///     can answer before it has drawn itself -- its height is the sum of two
///     sections, two dividers, a header and a user-owned folder list. It
///     keeps `VisibleWhenNeeded` and so shows a bar only when there is
///     something to scroll.
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
/// which is the whole reason [`scrollbar_in_gutter`]'s `AlwaysVisible` half
/// exists. (The sidebar rail reaches the same end by capping its content
/// instead and never calls this; see that function.)
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

/// **The design's section eyebrow**: the small all-caps, letterspaced,
/// ghost-grey line that names the block under it.
///
/// It is the same run of CSS everywhere the design uses it — `font-size:
/// 11px; font-weight: 700; letter-spacing: 0.1em; text-transform: uppercase;
/// color: #9b9797` — and it is used in nearly every panel on the page: the
/// sidebar's `VAULT` and `SHARING` headers (§5b), the Send composer's
/// `RECORD` / `INCLUDE` / `ACCESS` blocks (§5a), the history card's
/// `ACTIVITY` and the `STATES` legend (§5c), and the password-health and
/// preferences cards besides.
///
/// **It is a design-system element and not a call-site flourish**, which is
/// the whole reason it is here. Five surfaces in this crate already spell the
/// same five arguments out by hand — `sidebar::section_label`,
/// `password_health`, `prefs_ui` twice, `detail`, `totp_add` — and the two
/// that happen to disagree do so by a tenth of a pixel of tracking, which is
/// exactly the kind of drift that nobody can see in one screenshot and
/// everybody can see in two. A sixth hand-spelled copy inside the Send
/// composer would have been the one that made "the eyebrow" a matter of
/// opinion.
///
/// **Uppercasing is NOT done here.** `text-transform: uppercase` is a
/// presentation rule in CSS and an irreversible string edit in Rust: a caller
/// that handed this an already-capitalised constant would get the constant it
/// can grep for, and a caller that handed it a sentence would get a shout it
/// never asked for. Every caller in this crate passes a literal that is
/// already in the case the design draws it in, and that literal is the thing
/// a paint test can search the painted glyphs for.
///
/// The tracking is 1.2 and not the 1.1 [`CARD_HEADER_TRACKING`] uses. Both
/// are `0.1em` at 11px in the design; the wordmark's is tuned a tenth tighter
/// against the mark it sits beside, and this one is the value the sidebar's
/// headers were measured to.
pub fn eyebrow(ui: &mut Ui, text: &str) {
    ui.label(letterspaced(text, EYEBROW_PX, BOLD, EYEBROW_TRACKING, TEXT_GHOST));
}

/// [`eyebrow`]'s type size. `font-size: 11px` in the design, everywhere it
/// appears.
pub const EYEBROW_PX: f32 = 11.0;

/// [`eyebrow`]'s letter tracking: the design's `letter-spacing: 0.1em` at
/// [`EYEBROW_PX`], which is 1.1 points, drawn at the 1.2 the sidebar's
/// section headers were measured to and which this app has therefore been
/// shipping since design 4.8.
///
/// Both numbers are `pub` for the reason [`SEGMENT_SEAM`] is: a surface's own
/// paint test that wants to say "this line is the eyebrow" should measure
/// against the design system's value rather than restate it.
pub const EYEBROW_TRACKING: f32 = 1.2;

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

/// **The band a line of `font` actually INKS, as a height to lay it out in.**
///
/// egui's row box for a face is `ascent + descent`, and the faces this app
/// bundles reserve a generous descender. Measured on the monospace cut at 14
/// points: the row is 16.4, the baseline sits 10 below its top, and a capital
/// inks from 1 to 10. So the ink occupies the row's UPPER two thirds, and
/// anything that centres the ROW -- which is what centring a galley, a label
/// or a `ui.put` rect does -- puts the text about two and a half points high.
/// Two and a half points is invisible in a paragraph and unmistakable in a
/// 38-point box, which is why this was reported on a field, on a code strip
/// and on a card title within the same hour and never on a sentence.
///
/// **The rule is: make the line box the ascent.** Then the ink band and the
/// row are concentric to within a third of a point, so geometric centring IS
/// optical centring and no site needs a fudge factor of its own. It also
/// fixes the caret, which egui draws at the row's height: against nine points
/// of capital a 16.4-point caret is what the owner called huge, and the
/// ascent is the band a caret is supposed to cover.
///
/// **For a single line only.** `line_height` applies to every row of a
/// galley, so wrapped copy set this way would have its lines crash together.
/// Every caller here is a box, a chip or a title that draws exactly one line.
///
/// Measured from the face rather than assumed: a ratio written out here
/// would be one face's, and this app bundles four.
pub fn ascent_of(ctx: &egui::Context, font: &FontId) -> f32 {
    ctx.fonts_mut(|f| {
        let galley = f.layout_no_wrap(ASCENT_PROBE.to_string(), font.clone(), Color32::BLACK);
        galley
            .rows
            .first()
            .and_then(|row| row.glyphs.first())
            .map_or_else(|| f.row_height(font), |glyph| glyph.font_ascent)
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

/// **How tall the INK of a capital is** in `font` -- the cap height, which
/// is what a reader sees as "the size of the text".
///
/// [`ascent_of`] answers something larger: the face's ascent reserves room
/// for accents no unaccented line uses, about 23% above the caps on the
/// bundled cuts. That is the right measurement for a line BOX, and the wrong
/// one for anything that has to look the size of the letters beside it --
/// see [`FieldShape::line`], which is the one caller.
pub fn cap_height_of(ctx: &egui::Context, font: &FontId) -> f32 {
    ctx.fonts_mut(|f| {
        let galley = f.layout_no_wrap(ASCENT_PROBE.to_string(), font.clone(), Color32::BLACK);
        galley
            .rows
            .first()
            .and_then(|row| row.glyphs.first())
            .map_or_else(|| f.row_height(font), |glyph| glyph.uv_rect.size[1] as f32)
    })
}

/// The character [`ascent_of`] measures.
///
/// A capital with no descender and no accent, so the glyph's own ink box is
/// the band this is about. The value read is `font_ascent`, which is the
/// FACE's metric and not this glyph's, so the choice only has to be a
/// character the face certainly carries.
const ASCENT_PROBE: &str = "X";

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
    field_box(ui, value, FieldShape { password, ..FieldShape::wide(ui) }).0
}

/// [`text_field`] **in a §8a section row**: the same box at
/// [`SECTION_FIELD_HEIGHT`] with [`SECTION_FIELD_PX`] text.
///
/// The one place the edit form's boxes differ from the login window's, and
/// the reason is in [`SECTION_FIELD_HEIGHT`]'s doc. Everything else -- the
/// fill, the border, the radius, the focus halo, the 10-point inset -- is
/// [`field_box`]'s, so a row box is visibly the same control as the name box
/// above it, four points shorter.
pub fn section_text_field(ui: &mut Ui, value: &mut String, password: bool) -> Response {
    field_box(ui, value, FieldShape { password, ..FieldShape::section(ui) }).0
}

/// §8a's item-name box: the record's own name, set as the heading it is.
///
/// `font-size: 20px; font-weight: 800` inside the design's ordinary field, at
/// [`TITLE_FIELD_WIDTH`] and no wider. It is the ONE field on the edit form
/// whose content is the subject of the screen rather than a property of it,
/// and §8a marks that by size alone -- the box, the border and the focus halo
/// are the same ones every other row draws, so a name being edited still
/// reads as a field and not as a title someone has drawn a line under.
///
/// The ExtraBold cut, not [`BOLD`]: `800` is what §8a asks for, it is what the
/// read pane's own title is set in (`detail::title_text`), and the two panes
/// show the same name one keystroke apart -- a name that changed WEIGHT when
/// the form opened would read as a different name.
pub fn title_field(ui: &mut Ui, value: &mut String) -> Response {
    title_field_within(ui, value, ui.available_width())
}

/// [`title_field`] told how much of the row is really its own.
///
/// The edit band's name box is drawn LEFT, beside the tile, with the
/// `Unsaved changes` pill on the far right of the same row -- so the room the
/// box may take is the row minus that pill, and `ui.available_width()` at the
/// moment the box is added is the whole rest of the row. Passing it the
/// difference is what lets the two be laid out in reading order without the
/// box eating the pill's place. See `detail_edit`'s `edit_header`.
pub fn title_field_within(ui: &mut Ui, value: &mut String, room: f32) -> Response {
    let font = FontId::new(TITLE_FIELD_PX, FontFamily::Name(EXTRABOLD.into()));
    // The caret, cut to the height of the capitals beside it. See
    // `FieldShape::line`.
    let line = Some(cap_height_of(ui.ctx(), &font));
    field_box(
        ui,
        value,
        FieldShape {
            width: room.min(TITLE_FIELD_WIDTH),
            font,
            line,
            ..FieldShape::wide(ui)
        },
    )
    .0
}

/// The type size in [`title_field`]: §8a's `font-size: 20px`.
pub const TITLE_FIELD_PX: f32 = 20.0;

/// The widest [`title_field`] gets: §8a's own `max-width: 480px`.
///
/// A cap rather than the full line, because the band it sits in is as wide as
/// the window and a name box running to the far edge of a 1240-point pane
/// would be a text field the length of a sentence for a value that is three
/// words. Below the cap it takes what it is given, so the narrow pane loses
/// nothing.
pub const TITLE_FIELD_WIDTH: f32 = 480.0;

/// [`text_field`] **with a placeholder in it**.
///
/// It exists because the two Send composers needed one and there was none, so
/// both had reached for a bare `egui::TextEdit::singleline(..).hint_text(..)`
/// instead -- which is a different control: egui's own frame, egui's own
/// radius, egui's own one-line height, and none of the focus halo. Put beside
/// this design system's boxes on the same card, the difference is what the
/// owner reported as "the inputs are bare outlines"; put beside the vault's
/// item form, it is two spellings of "a field" in one app.
///
/// A separate function rather than a `hint` parameter on [`text_field`]
/// because [`text_field`]'s own callers have nothing to put in one -- every
/// field on the item form carries a [`field_label`] above it, which is where
/// that form says what a box is for.
/// `password` masks what is typed, the way [`text_field`]'s own flag does --
/// `record_ui`'s seed passphrase is a hinted field AND a secret, and a second
/// function for that one difference would be the third spelling of this box.
pub fn hinted_field(ui: &mut Ui, value: &mut String, hint: &str, password: bool) -> Response {
    field_box(ui, value, FieldShape { hint, password, ..FieldShape::wide(ui) }).0
}

/// A field **in a row beside its label**, at [`BUTTON_HEIGHT`] rather than
/// [`FIELD_HEIGHT`], and as wide as the caller asks.
///
/// §5a's Access rows are the reason for both departures. Their boxes are the
/// design's own `height: 30px` inside a 1px border -- 32 read as a border-box,
/// which is [`BUTTON_HEIGHT`] exactly -- and one of them (the view cap) is a
/// two-digit number in a 60-point box rather than a full-width line. A row of
/// 38-point boxes would be a row of form fields laid sideways; these are the
/// small controls a settings row carries.
///
/// `width` is taken and not measured from the `Ui`, because these sit inside
/// a horizontal layout where `available_width` is the rest of the row.
pub fn inline_field(
    ui: &mut Ui,
    value: &mut String,
    hint: &str,
    width: f32,
    password: bool,
) -> Response {
    field_box(
        ui,
        value,
        FieldShape {
            hint,
            password,
            width,
            height: BUTTON_HEIGHT,
            right_pad: 10.0,
            font: FontId::new(14.0, FontFamily::Proportional),
            line: None,
        },
    )
    .0
}

/// How tall one row of a [`text_area`] is, laid out rather than guessed.
fn text_area_row(ui: &Ui) -> f32 {
    let font = FontId::new(14.0, FontFamily::Proportional);
    ui.ctx().fonts_mut(|f| f.row_height(&font))
}

/// The padding a [`text_area`] keeps above and below its text.
///
/// [`FIELD_HEIGHT`]'s own: a 38-point box round a ~19-point row leaves 9.5
/// either side, and a multi-line box whose first line sat at a different
/// inset from the single-line box above it would read as a different control
/// rather than as a taller one.
const TEXT_AREA_PAD_Y: f32 = 9.0;

/// **A multi-line box in [`text_field`]'s treatment**: the same fill, the same
/// border, the same radius and the same focus halo, `rows` text rows tall.
///
/// The one field on either Send composer that [`text_field`] could not
/// already have been: the text a Send carries is a paragraph, and until this
/// existed the composer drew it as a bare `egui::TextEdit::multiline`. That
/// left the two boxes stacked on one card -- a name and the body under it --
/// wearing two different chromes, which is the single most visible thing in
/// the screenshot this pass was opened over.
///
/// It is here rather than in `send_ui` for this module's standing rule: a
/// control drawn privately on one screen is a second design system, and this
/// one is a *variant of a control this file already owns*, which is the
/// strongest case of all for it living beside its sibling.
pub fn text_area(ui: &mut Ui, value: &mut String, hint: &str, rows: usize) -> Response {
    let bg = ui.painter().add(egui::Shape::Noop);
    let row_height = text_area_row(ui);
    let height = row_height * rows as f32 + TEXT_AREA_PAD_Y * 2.0;
    let (outer, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height),
        Sense::hover(),
    );
    let inner = Rect::from_min_max(
        Pos2::new(outer.min.x + 10.0, outer.min.y + TEXT_AREA_PAD_Y),
        Pos2::new(outer.max.x - 10.0, outer.max.y - TEXT_AREA_PAD_Y),
    );
    let response = ui.put(
        inner,
        egui::TextEdit::multiline(value)
            .hint_text(hint)
            .frame(egui::Frame::new())
            .font(FontId::new(14.0, FontFamily::Proportional))
            .margin(Margin::ZERO)
            .desired_width(inner.width())
            .desired_rows(rows),
    );
    let border = field_border(ui, outer, response.has_focus());
    ui.painter().set(
        bg,
        egui::epaint::RectShape::new(
            outer,
            CornerRadius::same(FIELD_RADIUS),
            CARD,
            border,
            StrokeKind::Middle,
        ),
    );
    response
}

/// The design's input-box radius, shared by [`text_field`], [`text_area`] and
/// [`inline_field`] so a box cannot change shape by changing height.
const FIELD_RADIUS: u8 = 8;

/// The border a field box wears, **and the focus halo painted round it**.
///
/// Split out of [`field_box`] when [`text_area`] became the second control
/// that needed it: the ring is `box-shadow: 0 0 0 3px #dbe4f7` in the design
/// and is drawn OUTSIDE the box, so it has to go on the painter directly
/// while the border goes into the reserved shape behind the text. Two callers
/// spelling that pair out twice is how one of them ends up with a ring and
/// the other with a blue line.
fn field_border(ui: &Ui, outer: Rect, focused: bool) -> Stroke {
    if !focused {
        return Stroke::new(1.0, BORDER_STRONG);
    }
    // expand(2.0) with a 3px stroke covers 0.5..3.5px outside the rect:
    // flush against the 1px border's outer edge, like the mock's box-shadow.
    ui.painter().rect_stroke(
        outer.expand(2.0),
        CornerRadius::same(FIELD_RADIUS),
        Stroke::new(3.0, FOCUS_RING),
        StrokeKind::Middle,
    );
    Stroke::new(1.0, BLUE)
}

/// A password field with the design's in-field "Show"/"Hide" reveal toggle
/// (3h's master-password input). Same box treatment as [`text_field`];
/// `revealed` is the caller's persistent toggle state.
pub fn password_field(ui: &mut Ui, value: &mut String, revealed: &mut bool) -> Response {
    password_field_shaped(ui, value, revealed, FieldShape::wide(ui))
}

/// [`password_field`] **in a §8a section row**, at [`SECTION_FIELD_HEIGHT`].
/// See [`section_text_field`].
pub fn section_password_field(ui: &mut Ui, value: &mut String, revealed: &mut bool) -> Response {
    password_field_shaped(ui, value, revealed, FieldShape::section(ui))
}

/// Both password fields, in one body: the box `shape` describes, masked
/// unless `revealed`, with the in-field toggle on its right.
fn password_field_shaped(
    ui: &mut Ui,
    value: &mut String,
    revealed: &mut bool,
    shape: FieldShape<'_>,
) -> Response {
    // The wide right inset keeps typed text from running under the toggle.
    let (response, box_rect) =
        field_box(ui, value, FieldShape { password: !*revealed, right_pad: 52.0, ..shape });

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
    disabled_field_box(ui, text, 10.0, FIELD_HEIGHT, 14.0)
}

/// [`disabled_text_field`] **in a §8a section row**: the greyed box at
/// [`SECTION_FIELD_HEIGHT`] with [`SECTION_FIELD_PX`] text, so a row the
/// form cannot edit -- the `Item` card's `Type`, a create form's website --
/// is the same height as the rows it can.
pub fn section_disabled_text_field(ui: &mut Ui, text: &str) -> Rect {
    disabled_field_box(ui, text, 10.0, SECTION_FIELD_HEIGHT, SECTION_FIELD_PX)
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
    let box_rect = disabled_field_box(ui, &masked_readout(), 52.0, FIELD_HEIGHT, 14.0);
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
fn disabled_field_box(ui: &mut Ui, text: &str, right_pad: f32, height: f32, font_px: f32) -> Rect {
    let (outer, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height),
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
        egui::TextFormat::simple(FontId::new(font_px, FontFamily::Proportional), TEXT_GHOST),
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

/// What [`field_box`] is being asked for, named rather than positional.
///
/// Five arguments, of which three are numbers of the same type, is how a box
/// ends up 32 wide and 60 tall at one call site; [`Segment`] and
/// `send_ui::AccessControls` are here for the same reason. [`Self::wide`] is
/// the full-width 38-point box the item form has always drawn, so the
/// original callers read as they did.
struct FieldShape<'a> {
    /// The placeholder, or `""` for a box whose label sits above it.
    hint: &'a str,
    /// Whether the typed value is masked.
    password: bool,
    /// The box's outer width.
    width: f32,
    /// The box's outer height.
    height: f32,
    /// How much room to leave at the right for an in-field affordance.
    right_pad: f32,
    /// The type size the box sets what is typed in it.
    ///
    /// A field rather than the 14 every box used to hardcode, because
    /// [`title_field`] is a box whose CONTENT is a heading -- §8a's item name
    /// at `font-size: 20px` -- and a 38-point box holding 14-point text is
    /// what made the edit form's name row read as one more setting.
    font: FontId,
    /// **The line box this field's text is laid in**, or `None` for
    /// [`ascent_of`] -- which is what every box but one takes and what the
    /// comment inside [`field_box`] argues for at length.
    ///
    /// It exists for the CARET. egui draws the caret at the row's height, so
    /// the line box is the caret, and the ascent is about 23% taller than the
    /// letters it is standing beside. At 13 points nobody notices two of
    /// them; at [`TITLE_FIELD_PX`]'s 20, in a 38-point box, it is four -- the
    /// owner's "text cursor in header seems wrong size", reported twice.
    ///
    /// So the name box lays its line at the cap height instead
    /// ([`cap_height_of`]) and the caret comes out the height of the capitals
    /// it sits against. Only that box: the rule everywhere else is still the
    /// ascent, and a form-wide change here would move the text in every field
    /// in the app to fix a caret in one of them.
    line: Option<f32>,
}

impl<'a> FieldShape<'a> {
    /// The design's full-width form field: [`FIELD_HEIGHT`], no placeholder,
    /// the plain 10-point right inset.
    fn wide(ui: &Ui) -> Self {
        Self {
            hint: "",
            password: false,
            width: ui.available_width(),
            height: FIELD_HEIGHT,
            right_pad: 10.0,
            font: FontId::new(14.0, FontFamily::Proportional),
            line: None,
        }
    }

    /// §8a's row field: [`Self::wide`] at [`SECTION_FIELD_HEIGHT`] with
    /// [`SECTION_FIELD_PX`] text. See [`SECTION_FIELD_HEIGHT`] for why the
    /// two are not one box.
    fn section(ui: &Ui) -> Self {
        Self {
            height: SECTION_FIELD_HEIGHT,
            font: FontId::new(SECTION_FIELD_PX, FontFamily::Proportional),
            ..Self::wide(ui)
        }
    }
}

/// Allocates one of the design's input boxes, places a frameless `TextEdit`
/// inside it (10px left inset, `shape.right_pad` right inset), and paints the
/// box: 1px border at rest, blue border with a flush 3px `FOCUS_RING` halo
/// when focused — a treatment egui's own `TextEdit` frame can't produce.
///
/// The *box* is what gets allocated, not the text row: a frameless TextEdit
/// only allocates its text height, and painting a 38px box around a 16px
/// allocation made the box overlap the label above and shift left of its
/// right inset (asymmetric padding). Returns the response and the box rect
/// (for in-field affordances like the reveal toggle).
fn field_box(ui: &mut Ui, value: &mut String, shape: FieldShape<'_>) -> (Response, Rect) {
    // Placeholder so the box paints *under* the text egui draws in ui.put.
    let bg = ui.painter().add(egui::Shape::Noop);
    let right_pad = shape.right_pad;
    let (outer, _) = ui.allocate_exact_size(
        Vec2::new(shape.width, shape.height),
        Sense::hover(),
    );
    // **The TextEdit gets a rect of the font's ASCENT, centred in the box.**
    //
    // It used to get the font's full row -- ascent plus descent -- with a
    // 9% fudge pushing it down, and that fudge was this file's own guess at
    // the problem [`ascent_of`] now measures. On the bundled faces it is
    // about a third of what the error actually is, which is why the owner
    // read every box in this app as "text in field not centered" and, in the
    // same breath, "cursor is huge": egui draws the caret at the ROW's
    // height, so a descender band the value never uses was being drawn as
    // caret on every field in the app.
    //
    // With the line box set to the ascent both go away at once and no fudge
    // is left behind -- see [`ascent_of`], which carries the measurements.
    let font = shape.font.clone();
    // The line box, which is the ascent unless the caller says otherwise --
    // see `FieldShape::line`, and the paragraph below for why it is the
    // ascent at all.
    let ascent = shape.line.unwrap_or_else(|| ascent_of(ui.ctx(), &font));
    let inner = Rect::from_center_size(
        Pos2::new((outer.min.x + 10.0 + outer.max.x - right_pad) / 2.0, outer.center().y),
        Vec2::new(outer.width() - 10.0 - right_pad, ascent),
    );
    // The face is carried in through a layouter rather than `.font()`, which
    // takes a `FontId` and can express no line height -- the same reason
    // `detail::title_text` and `totp_add::secret_field` reach for one.
    let password = shape.password;
    let mut layouter = |ui: &Ui, buffer: &dyn egui::TextBuffer, wrap: f32| {
        let mut job = egui::text::LayoutJob::default();
        job.wrap = egui::text::TextWrapping::no_max_width();
        let _ = wrap;
        // **Masked here, because a custom layouter is handed the RAW text.**
        // egui masks inside its DEFAULT layouter only (`mask_if_password`),
        // so a field that sets a layouter and leaves `.password(true)` on
        // draws the secret in the clear. The replacement character is
        // epaint's own, so a masked field still looks like every masked
        // field in every other egui app.
        let shown: String = if password {
            buffer
                .as_str()
                .chars()
                .map(|_| egui::epaint::text::PASSWORD_REPLACEMENT_CHAR)
                .collect()
        } else {
            buffer.as_str().to_owned()
        };
        job.append(
            &shown,
            0.0,
            egui::TextFormat {
                line_height: Some(ascent),
                font_id: font.clone(),
                color: ui.visuals().text_color(),
                ..Default::default()
            },
        );
        ui.fonts_mut(|f| f.layout_job(job))
    };
    // **The whole box takes a click, not just the line of text in it.**
    //
    // `ui.put` gives the `TextEdit` one rect for BOTH its layout and its hit
    // area, and the rect that lays the text out correctly is one line tall --
    // so in a 38-point field the top and bottom thirds did nothing when
    // clicked. That was already true and is simply more true now the line box
    // is the ascent, which is how it was noticed: a folder-modal test aimed
    // at the gap between two labels, landed a point below the text row, and
    // typed into nothing.
    //
    // Registered BEFORE the `TextEdit`, so egui's topmost-wins ordering leaves
    // the text itself to the editor -- click-to-place-caret and
    // drag-to-select are untouched -- and this catches only the padding round
    // it.
    let surround = ui.interact(outer, ui.next_auto_id().with("field-box"), Sense::click());
    let response = ui.put(
        inner,
        egui::TextEdit::singleline(value)
            .hint_text(shape.hint)
            .frame(egui::Frame::new())
            .margin(Margin::ZERO)
            .desired_width(inner.width())
            .layouter(&mut layouter),
    );

    // **The box claims the height it allocated**, which `ui.put` above had
    // just given back.
    //
    // `put` places a widget AND reports that rect to the layout, and the rect
    // it is given here is `inner` -- one ascent tall, centred in the box. So
    // after every field in this app the cursor sat at the TEXT's baseline
    // band rather than under the border, and the next widget started five to
    // ten points inside the box it was supposed to follow. On the edit
    // header that put the subtitle 0.7 of a point under a 38-point name box
    // whose layout gap was eight -- the owner's "overlaps", with "some space
    // in between" and an arrow at the air below the strip.
    //
    // Restoring it here rather than at the call sites because the defect is
    // this function's: `outer` is what it allocated and what it paints, and
    // nothing outside can see that `put` moved the cursor back inside it.
    ui.advance_cursor_after_rect(outer);

    if surround.clicked() {
        response.request_focus();
    }
    if surround.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }

    let border = field_border(ui, outer, response.has_focus());
    ui.painter().set(
        bg,
        egui::epaint::RectShape::new(
            outer,
            CornerRadius::same(FIELD_RADIUS),
            CARD,
            border,
            StrokeKind::Middle,
        ),
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

/// §5a's `border-radius: 12px`.
///
/// Four points more than the 8 the two composers used, and the difference is
/// deliberate rather than incidental: 8 is this app's CONTROL radius (every
/// button, every field box), and a card drawn at its buttons' radius reads as
/// a big button. `MODAL_RADIUS`'s 10 is the same argument one step down.
pub const FORM_CARD_RADIUS: u8 = 12;

/// §5a's `box-shadow: 0 14px 34px rgba(45, 43, 43, 0.18)`.
///
/// `0.18` of 255 is 46, which is `MODAL_SHADOW`'s alpha exactly -- the same
/// ink, thrown further. It is what makes the card a thing laid ON the pane
/// rather than a region of it, and it is the one part of §5a's chrome that no
/// amount of border weight substitutes for.
pub const FORM_CARD_SHADOW: Shadow = Shadow {
    offset: [0, 14],
    blur: 34,
    spread: 0,
    color: Color32::from_rgba_unmultiplied_const(45, 43, 43, 46),
};

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

/// The card's horizontal padding **at the width the design draws it**:
/// §5a's and §6c's own 18.
pub const FORM_CARD_PAD_X_WIDE: i8 = 18;

/// The width at which a form card takes the design's padding.
///
/// §6c's card is 470 and §5a's is 690, and both say `padding: ... 18px`. 470
/// is therefore the narrowest card the design itself draws at 18, so it is
/// the threshold rather than a number chosen between the two.
pub const FORM_CARD_WIDE_AT: f32 = 470.0;

/// **How much air a form card's bands put either side of their contents, for
/// a card this wide.**
///
/// One rule rather than one constant, because the two answers are both right
/// and which one applies is a fact about the card in front of the reader.
/// This used to be a single 12 with a doc admitting it was "the one
/// measurement on the card that is deliberately not the design's" -- and that
/// departure was argued entirely from the narrow case, the 250-point card in
/// the Sends screen's detail column. It was then paid for by the wide case:
/// `record_ui`'s composer is §5a's own 690-point card and was drawing §5a's
/// rows inside 5a's card at somebody else's padding, which is part of what
/// the owner was looking at when they asked for the layout to match "1 to 1
/// like sizes, paddings etc".
///
/// The threshold is measured, not chosen: see [`FORM_CARD_WIDE_AT`].
///
/// Asked of the width the band has to fill, so a card that is resized -- the
/// Sends composer is, with its column -- answers for the width it is being
/// drawn at on this frame rather than for the one it opened at.
pub fn form_card_pad_x(width: f32) -> i8 {
    if width >= FORM_CARD_WIDE_AT {
        FORM_CARD_PAD_X_WIDE
    } else {
        FORM_CARD_PAD_X
    }
}

/// The card's vertical band padding. §5a runs 15 / 16 / 13 down its three
/// bands; this is one number for all three, because three near-identical
/// paddings is three chances to disagree and the difference between them is
/// not visible at any width this card is drawn at.
pub const FORM_CARD_PAD_Y: i8 = 12;

/// How tall [`form_card_header`] is, for a caller that has to reserve the
/// strip before drawing it -- `record_ui`'s modal drag handle is the one.
///
/// [`FORM_CARD_PAD_Y`] either side of the 14px heading's ~18-point line, plus
/// the rule that closes the band. Stated rather than measured because the
/// handle is laid down BEFORE the header is drawn; a handle that had to wait
/// for the band's rect would be a handle laid over the band's contents.
pub const FORM_CARD_HEADER_HEIGHT: f32 = FORM_CARD_PAD_Y as f32 * 2.0 + 18.0 + 1.0;

/// The gap between the footer's two answers. §5a's `gap: 9px`, rounded to the
/// 8 this app's other footers use.
pub const FORM_FOOTER_GAP: f32 = 8.0;

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

/// §5a's card: white, edged, rounded and **shadowed**, with no padding of its
/// own -- the three band helpers below pad themselves, because a band has to
/// reach the card's edge to be a band.
///
/// Answers with the card's rectangle, which is what `record_ui` hangs its
/// dismiss ✕ on.
///
/// **The border is painted AFTER the contents**, which is not how
/// `egui::Frame` does it and is the reason this is a function. A `Frame` sets
/// its fill and stroke into a shape index reserved before the body, so
/// anything the body paints edge-to-edge -- which the footer band, by
/// definition, does -- lands on top of the stroke and eats it on three sides.
/// Painting the ring last costs one extra shape and removes the whole class
/// of fixes where a band is inset by a point to let a border show through.
pub fn form_card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> (Rect, R) {
    let framed = egui::Frame::new()
        .fill(CARD)
        .corner_radius(CornerRadius::same(FORM_CARD_RADIUS))
        .shadow(FORM_CARD_SHADOW)
        .show(ui, add);
    let rect = framed.response.rect;
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(FORM_CARD_RADIUS),
        Stroke::new(1.0, BORDER_STRONG),
        StrokeKind::Inside,
    );
    (rect, framed.inner)
}

/// The card's first band: the form's name, and the [`HAIRLINE`] that closes
/// the band off from the body.
///
/// Answers with the rectangle the heading's own glyphs occupy -- the line
/// `modal_corner_mark_gated` hangs a dismiss ✕ level with.
///
/// 14 points and `strong`, which is this app's card-heading size everywhere
/// else and is one point under §5a's `15px/800`. The design page sets this
/// card's title a point larger than its other cards' and there is no reason
/// in the app for the Send composer's heading to be the one heading that is
/// bigger than the rest.
pub fn form_card_header(ui: &mut Ui, title: &str) -> Rect {
    form_card_header_marked(ui, title, false)
}

/// [`form_card_header`] **with §5a's paper plane in front of the title**.
///
/// §5a opens its composer with a send glyph, and it is the one thing on that
/// band that says what KIND of card this is before the words are read. Drawn
/// rather than typed, for the reason every mark in this file is: U+2708 and
/// its neighbours resolve out of egui's fallback emoji face at a weight and
/// an optical size nobody here chose, and §4d's own keycaps were tofu.
///
/// A flag on the existing function rather than a second header, because the
/// band is otherwise identical -- same padding, same 14px, same rule closing
/// it -- and two headers a point apart is this file's most-repeated defect.
pub fn form_card_header_marked(ui: &mut Ui, title: &str, plane: bool) -> Rect {
    let line = egui::Frame::new()
        .inner_margin(Margin::symmetric(form_card_pad_x(ui.available_width()), FORM_CARD_PAD_Y))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if plane {
                    let (mark, _) =
                        ui.allocate_exact_size(Vec2::splat(SEND_PLANE), Sense::hover());
                    paint_send_plane(ui.painter(), mark, BLUE);
                    ui.add_space(SEND_PLANE_GAP);
                }
                ui.label(RichText::new(title).size(14.0).color(INK).strong()).rect
            })
            .inner
        })
        .inner;
    hairline(ui);
    line
}

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

/// §5a's `<svg width="17" height="17">` on the composer's title, and the
/// `gap: 10px` after it.
const SEND_PLANE: f32 = 17.0;
/// See [`SEND_PLANE`].
const SEND_PLANE_GAP: f32 = 10.0;

/// The card's middle band: everything the form is actually asking, at the
/// card's padding.
pub fn form_card_body<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .inner_margin(Margin::symmetric(form_card_pad_x(ui.available_width()), FORM_CARD_PAD_Y))
        .show(ui, add)
        .inner
}

/// The card's last band: a [`HAIRLINE`], then [`CARD_TINT`] out to the card's
/// three edges with the bottom corners rounded to match it.
///
/// The tint and the rule above it are the send preflight's own footer and the
/// modal frame's, reproduced rather than reinvented -- §5a's `#fbfaf9` IS
/// [`CARD_TINT`], and `modal_footer_band` already draws this pair.
///
/// **Why the band exists at all**, since the buttons would sit in the same
/// place without it: a footer is the card saying "the questions are over,
/// here are the answers", and a row of buttons that is simply the last thing
/// in the body says only "here are two more controls". On the Sends composer
/// the body ends in the Access block's own last row, which is a label, a box
/// and a note -- and the un-banded footer put a blue button, a white button
/// and a grey sentence on the line directly below it, in the same rhythm. The
/// band is what tells the eye the form has ended.
pub fn form_card_footer<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    hairline(ui);
    // Reserved before the contents so the tint paints behind them, and filled
    // in once their rect is known: `Frame`'s own idiom, used directly because
    // a `Frame`'s fill is confined to the rect layout gives it and this one
    // has to reach the card's rounded bottom corners.
    let band = ui.painter().add(egui::Shape::Noop);
    let laid_out = egui::Frame::new()
        .inner_margin(Margin::symmetric(form_card_pad_x(ui.available_width()), FORM_CARD_PAD_Y))
        .show(ui, add);
    let rect = laid_out.response.rect;
    ui.painter().set(
        band,
        egui::epaint::RectShape::filled(
            rect,
            CornerRadius {
                nw: 0,
                ne: 0,
                sw: FORM_CARD_RADIUS,
                se: FORM_CARD_RADIUS,
            },
            CARD_TINT,
        ),
    );
    laid_out.inner
}

/// **§5a's caution band: the strip between a form's body and its footer.**
///
/// A rule, a tinted band, a warning mark and one sentence. §5a puts it under
/// the composer and above the answers, which is the last thing crossed on the
/// way to the button -- the same placement, and the same argument,
/// `totp_add::caution_band` makes for 6c's.
///
/// **This one is RED where 6c's is amber**, and the two are not one component
/// wearing two palettes. 6c warns that something is about to be overwritten;
/// §5a warns that a secret is about to leave the machine in the clear. The
/// design draws them in its two different reds and ambers on purpose, and
/// this app's [`DANGER_WASH`]/[`ERROR`]/[`DANGER_INK`] trio is that red
/// already.
///
/// Square at both ends: a footer follows it, so neither edge of it is the
/// card's corner.
pub fn form_card_caution(ui: &mut Ui, text: &str) {
    hairline(ui);
    let band = ui.painter().add(egui::Shape::Noop);
    let laid_out = egui::Frame::new()
        .inner_margin(Margin::symmetric(form_card_pad_x(ui.available_width()), CAUTION_BAND_PAD_Y))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                // §5a's `<svg width="15" height="15">` with its own
                // `margin-top: 1px`: the mark sits on the sentence's first
                // line rather than centred on a paragraph that may wrap.
                let (mark, _) = ui.allocate_exact_size(
                    Vec2::splat(CAUTION_BAND_GLYPH),
                    Sense::hover(),
                );
                paint_warning_glyph(
                    ui.painter(),
                    mark.translate(Vec2::new(0.0, CAUTION_BAND_GLYPH_DROP)),
                    ERROR,
                );
                ui.add_space(CAUTION_BAND_GAP);
                ui.label(RichText::new(text).size(12.0).color(DANGER_INK));
            });
        });
    ui.painter().set(
        band,
        egui::epaint::RectShape::filled(
            laid_out.response.rect,
            CornerRadius::ZERO,
            DANGER_WASH,
        ),
    );
}

/// §5a's `padding: 13px 18px` on the caution band -- the x is
/// [`FORM_CARD_PAD_X`]'s, which is the same 18.
const CAUTION_BAND_PAD_Y: i8 = 13;
/// §5a's `<svg width="15">` warning mark, its `margin-top: 1px`, and the
/// `gap: 9px` after it.
const CAUTION_BAND_GLYPH: f32 = 15.0;
/// See [`CAUTION_BAND_GLYPH`].
const CAUTION_BAND_GLYPH_DROP: f32 = 1.0;
/// See [`CAUTION_BAND_GLYPH`].
const CAUTION_BAND_GAP: f32 = 9.0;

/// The footer's standing note: §5a's `Appears in Shared`, in §5a's own
/// treatment -- 12px in [`TEXT_GHOST`], pushed to the right of the answers.
///
/// It is a function rather than a `ui.label` at two call sites for the reason
/// [`eyebrow`] is one: two forms spelling out the same size and the same grey
/// is how they end up a point apart.
pub fn form_footer_note(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(TEXT_GHOST));
}

/// How wide [`form_footer_note`] would paint `text`, so a footer can decide
/// whether the note fits beside the answers or belongs on its own line.
///
/// §5a puts the note on the buttons' line because §5a's card is 690 points
/// wide. This one is 250 at the window floor, where two answers alone take
/// most of the row -- and a note elided to `Appears in Sen…` is worse than no
/// note, because it says nothing AND looks broken. Measuring is what lets one
/// footer be §5a's at the width §5a was drawn for and still be readable at
/// the width this app can actually be dragged to.
pub fn form_footer_note_width(ui: &Ui, text: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(
            text.to_string(),
            FontId::new(12.0, FontFamily::Proportional),
            TEXT_GHOST,
        )
        .size()
        .x
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
pub const SECTION_CARD_RADIUS: u8 = 10;

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

/// **How much air a section card's bands put either side of their contents,
/// for a card this wide.** [`form_card_pad_x`]'s rule for §8a's card:
/// [`SECTION_CARD_PAD_X_WIDE`] from [`SECTION_CARD_WIDE_AT`] up,
/// [`SECTION_CARD_PAD_X`] below it.
///
/// Asked of the width the band has to fill, on this frame, for the reason
/// `form_card_pad_x` gives: the detail pane is resized with the window.
pub fn section_card_pad_x(width: f32) -> i8 {
    if width >= SECTION_CARD_WIDE_AT {
        SECTION_CARD_PAD_X_WIDE
    } else {
        SECTION_CARD_PAD_X
    }
}

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

/// How far §8a's card column stands off the rule under the title bar:
/// its body's `padding: 20px 28px 0`, in the vertical.
///
/// The read pane's body is 2b's `padding: 18px 24px`, and the edit form's
/// title bar IS the read pane's -- but the column under it is §8a's, and §8a
/// says 20.
///
/// **The drawn number, spent net of `item_spacing`.** What used to stand
/// here was `ui.add_space(12.0)` -- and it DREW 20, because egui had already
/// put eight points of item spacing under the rule before the twelve were
/// added. The first draft of this constant replaced the 12 with 20 and drew
/// 28, which its own test caught; the caller now subtracts the spacing, so
/// the constant is the gap the reader sees and not an addend to a number
/// nobody wrote down.
pub const SECTION_COLUMN_TOP: f32 = 20.0;

/// The section card title's type size: §8a's `font-size: 12px`.
pub const SECTION_TITLE_PX: f32 = 12.0;

/// The section card title's tracking: §8a's `letter-spacing: 0.06em` at
/// [`SECTION_TITLE_PX`], which is 0.72 points.
///
/// **Not [`EYEBROW_TRACKING`], and not an oversight.** §8a draws BOTH on the
/// same screen and means two different things by them: its rail's `SECTIONS`
/// is the eyebrow exactly -- 11px, `0.1em`, `#9b9797` -- naming a region of
/// chrome, while a card's own title is 12px, `0.06em`, `#605d5d`: one step
/// larger, one step tighter and one step darker, because it names a thing the
/// user is about to edit rather than a heading over a list. Collapsing the
/// two would make every card on this form as quiet as the rail above it.
pub const SECTION_TITLE_TRACKING: f32 = 0.72;

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
pub const SECTION_FIELD_HEIGHT: f32 = 34.0;

/// The type size inside a §8a row field: `font-size: 13px`, against the 14
/// [`FieldShape::wide`] sets for the 38-point box. One step down with the
/// box, so the text keeps the same proportion of it.
pub const SECTION_FIELD_PX: f32 = 13.0;

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

/// Whether a card this wide can draw §8a's three-column grid.
pub fn section_grid_fits_at(width: f32) -> bool {
    width >= 3.0 * SECTION_GRID_CELL_FLOOR + 2.0 * SECTION_GRID_GAP
}

/// One cell of that grid: §8a's 12-point caption over its control, with the
/// design's `gap: 6px` between them.
///
/// **[`TEXT_FAINT`], not [`field_label`]'s [`TEXT_MUTED`].** §8a gives this
/// caption `color: #7d7979`, which is the same ink its label COLUMN takes --
/// and it has to be, because the grid arm and the row arm of the `ITEM` card
/// are the same three captions at two widths. `every_per_kind_caption_is_
/// the_same_grey_as_the_item_cards` reads `Folder` as its reference for
/// every other caption on the form, so a cell in the wrong grey would have
/// re-tinted the whole screen.
pub fn section_grid_cell<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.label(RichText::new(label).size(12.0).color(TEXT_FAINT));
    ui.add_space(EYEBROW_GAP);
    add(ui)
}

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

/// Whether a section card this wide can draw §8a's label-beside-control rows,
/// or must stack the label above the control instead.
///
/// Asked of the `Ui` the row is about to be added to, so one card can answer
/// differently from another on the same form -- which is what happens the
/// moment two cards sit side by side in a grid.
pub fn section_rows_fit(ui: &Ui) -> bool {
    section_rows_fit_at(ui.available_width())
}

/// [`section_rows_fit`], asked of a width rather than of a `Ui`.
///
/// The whole of the decision, so the `Ui` form above is one call and not a
/// second copy of the arithmetic. It is separate because a TEST cannot build
/// the `Ui` the row will be added to without drawing the form first, and a
/// test that asserts about one of the two arms needs to be able to say which
/// arm it is asserting about -- otherwise a floor that moved would turn a
/// claim about the stacked row into a vacuous claim about the other one.
pub fn section_rows_fit_at(width: f32) -> bool {
    width >= SECTION_LABEL_WIDTH + SECTION_ROW_GAP + SECTION_ROW_CONTROL_FLOOR
}

/// §8a's card: white, edged in [`HAIRLINE`], rounded to
/// [`SECTION_CARD_RADIUS`] and **unshadowed**, with no padding of its own --
/// the band helpers below pad themselves, because a band has to reach the
/// card's edge to be a band.
///
/// Answers with the card's rectangle, which is what the section rail scrolls
/// to.
///
/// **The border is painted AFTER the contents**, for the reason
/// [`form_card`]'s doc sets out in full: a `Frame`'s stroke is reserved before
/// its body, so anything the body paints edge to edge -- a row rule, by
/// definition -- lands on top of it.
pub fn section_card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> (Rect, R) {
    let framed = egui::Frame::new()
        .fill(CARD)
        .corner_radius(CornerRadius::same(SECTION_CARD_RADIUS))
        .show(ui, add);
    let rect = framed.response.rect;
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(SECTION_CARD_RADIUS),
        Stroke::new(1.0, HAIRLINE),
        StrokeKind::Inside,
    );
    (rect, framed.inner)
}

/// The section card's first band: its name in §8a's treatment, an optional
/// trailing note, and the [`HAIRLINE`] that closes the band.
///
/// `note` is §8a's own second element on this strip -- its `One-time code`
/// carries a `2FA` chip, its `Autofill targets` the sentence `where this
/// login is offered` -- drawn at [`TEXT_GHOST`] because it explains the title
/// rather than competing with it.
///
/// `changed` draws the dirty mark: [`CHANGED_PILL`] in the caution tone,
/// pushed to the right edge of the band. §8a puts its `Changed` list in the
/// rail; this puts a mark on each card as well, and the reason is the pane
/// this form lives in -- the rail is chrome the form cannot always afford
/// (see `section_rows_fit`'s neighbours), and a dirty state that is only
/// visible in a column that is only sometimes on screen is a dirty state the
/// user cannot rely on.
pub fn section_card_header(ui: &mut Ui, title: &str, note: &str, changed: bool) -> Rect {
    section_card_header_at(ui, SECTION_CARD_PAD_X, title, note, changed)
}

/// [`section_card_header`] at a stated horizontal padding -- the one
/// [`section_card_pad_x`] answers for the card's width.
///
/// A second entry point rather than a width read inside the band, because
/// the band cannot know whether its caller wants the width rule at all: the
/// edit form does, and the sequence builder's 252-point rail cards do not.
/// The plain function above is the builder's and keeps the number it always
/// had.
pub fn section_card_header_at(
    ui: &mut Ui,
    pad_x: i8,
    title: &str,
    note: &str,
    changed: bool,
) -> Rect {
    let line = egui::Frame::new()
        .inner_margin(Margin::symmetric(pad_x, SECTION_CARD_HEADER_PAD_Y))
        .show(ui, |ui| {
            // **The row is its TITLE's height and nothing else's.**
            //
            // egui floors a horizontal row at `interact_size.y`, which this
            // theme sets to 20 -- so a band holding one 12-point caption came
            // out 22 + 20 = 42, where the READ pane's heading is 22 + its
            // galley = 36. Six points, on every card of the form, against a
            // pane one click away. The owner: "too tall headers for those
            // tiles".
            //
            // Zeroed rather than the caption padded, because the floor is
            // about CONTROLS being clickable and there is no control in this
            // band -- the pill below paints, it does not sense.
            //
            // **Set on the OUTER `Ui`, before `horizontal`.** `Ui::horizontal`
            // reads the spacing when it builds the row, so zeroing it inside
            // the closure is two points late and measurably so: 42 became 40
            // rather than 36.
            ui.spacing_mut().interact_size.y = 0.0;
            ui.horizontal(|ui| {
                // §8a's `gap: 10px` between the title and what follows it.
                ui.spacing_mut().item_spacing.x = SECTION_HEADER_GAP;
                // **Uppercased here rather than stored uppercased.** §8a's
                // band declares `text-transform: uppercase` over titles
                // written in sentence case, and the RAIL beside it prints the
                // same strings untransformed -- `Item`, `One-time code`. So
                // the capitals belong to this band and to nothing else, which
                // is exactly what a render-time transform says and what an
                // uppercase constant would not. It also puts these titles back
                // in register with the READ pane's cards, whose headings are
                // `LOGIN CREDENTIALS` and `AUTOFILL TARGETS`: the two panes
                // are one click apart and were reading as two applications.
                let title_rect = ui
                    .label(letterspaced(
                        &title.to_uppercase(),
                        SECTION_TITLE_PX,
                        BOLD,
                        SECTION_TITLE_TRACKING,
                        TEXT_MUTED,
                    ))
                    .rect;
                if !note.is_empty() {
                    ui.label(RichText::new(note).size(12.0).color(TEXT_GHOST));
                }
                if changed {
                    // Right-aligned by measuring rather than by a
                    // `Layout::right_to_left` scope, because the pill has to
                    // be able to give up its place: at the pane's floor a
                    // card title and a pill do not both fit, and a
                    // right-to-left layout would push the pill out of the
                    // card rather than leave it off.
                    let width = state_pill_width(ui.painter(), CHANGED_TONE, CHANGED_PILL);
                    if ui.available_width() >= width {
                        // **Zero height, and centred on the TITLE.** The slot
                        // claims the width it needs and none of the height:
                        // a `PILL_HEIGHT` allocation would make the band 42
                        // when the card is dirty and 38 when it is not, so
                        // every card on the form would grow four points as
                        // the user typed. The pill is 20 tall and the band's
                        // padding is 11 a side, so it sits comfortably inside
                        // the strip without defining it.
                        let (rect, _) = ui
                            .allocate_exact_size(Vec2::new(ui.available_width(), 0.0), Sense::hover());
                        state_pill(
                            ui.painter(),
                            Pos2::new(rect.right() - width, title_rect.center().y),
                            CHANGED_TONE,
                            CHANGED_PILL,
                        );
                    }
                }
                title_rect
            })
            .inner
        })
        .inner;
    hairline(ui);
    line
}

/// The section card's body band: its rows, at the card's padding.
pub fn section_card_body<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    section_card_body_at(ui, SECTION_CARD_PAD_X, add)
}

/// [`section_card_body`] at a stated horizontal padding. See
/// [`section_card_header_at`] for why the padding is the caller's to state.
pub fn section_card_body_at<R>(ui: &mut Ui, pad_x: i8, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .inner_margin(Margin::symmetric(pad_x, SECTION_CARD_PAD_Y))
        .show(ui, add)
        .inner
}

/// One §8a row: a [`SECTION_LABEL_WIDTH`] label column, a
/// [`SECTION_ROW_GAP`], and the control -- **or, below
/// [`section_rows_fit`], the label stacked above the control instead.**
///
/// The stacked arm is not a degraded fallback: it is the shape this form has
/// always had, it is what every other form in this app draws, and it is the
/// only shape a 224-point card body can hold. What the wide arm buys is §8a's
/// reading order -- the eye runs down a column of labels and across to the
/// value -- which is precisely what a 640-point pane has the room for and a
/// 298-point one does not.
pub fn section_row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    section_row_impl(ui, label, None::<fn(&mut Ui)>, add)
}

/// **The same row with a CONTROL in its label cell**, under the caption.
///
/// This exists because the form was not consistent between its own cards. The
/// `Item` and `Login credentials` cards drew §8a's label column; the Identity,
/// Card and SSH bodies could not, because every optional row on them carries a
/// `Remove` chip beside its caption and there was no row idiom that could hold
/// one. Each card was internally consistent and the FORM was not -- one screen
/// in two layouts, which is the thing a reader notices before they notice
/// either layout.
///
/// **A closure over the whole label cell was the shape suggested, and this is
/// narrower on purpose.** A cell-closure would hand every caller the caption
/// as well as the chip, and the caption's treatment -- 12pt, [`TEXT_FAINT`],
/// wrapped at [`SECTION_LABEL_WIDTH`], optically aligned against the first
/// line of the control -- is precisely the kind of thing that becomes four
/// slightly different spellings the moment four call sites own it. That is the
/// same failure as the three private copies of one red this pass is also
/// unpicking, one file up. So the theme keeps the caption and the caller
/// supplies only the thing the theme cannot know about: the chip.
///
/// **The aside goes UNDER the caption, not beside it.** The cell is 130 points
/// wide; `Cardholder name` at 12pt is about 95 of them and the chip is about
/// 55, so "beside" is not a layout, it is a wish. Underneath, the chip lands in
/// the dead space to the left of a 38-point field, which is the one part of a
/// §8a row that has nothing in it -- the row costs no extra height at all.
/// Below [`section_rows_fit`] there is no cell, so the stacked arm puts the two
/// in a `horizontal_wrapped` instead, which is exactly the shape those rows
/// already shipped and which has the whole card's width to spend.
pub fn section_row_aside<R>(
    ui: &mut Ui,
    label: &str,
    aside: impl FnOnce(&mut Ui),
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    section_row_impl(ui, label, Some(aside), add)
}

/// Both rows above, in one body.
///
/// `aside` is an `Option` rather than a no-op closure so that the plain row
/// keeps its exact shape: an empty label with no aside draws NOTHING in the
/// stacked arm, and a `horizontal_wrapped` holding nothing would still cost
/// the row an `item_spacing.y` the early return does not.
fn section_row_impl<R>(
    ui: &mut Ui,
    label: &str,
    aside: Option<impl FnOnce(&mut Ui)>,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    if !section_rows_fit(ui) {
        // An EMPTY label is a row that belongs to the one above it -- the
        // generator under its password box. Stacked, there is no label column
        // to indent it into, so the row simply follows its neighbour; drawing
        // the empty string would still cost a line box and a gap, which is a
        // blank the reader has to account for.
        match aside {
            None => {
                if !label.is_empty() {
                    field_label(ui, label);
                    ui.add_space(EYEBROW_GAP);
                }
            }
            // **Wrapped, not `horizontal`**, for the reason every multi-control
            // row on the edit form is: an unwrapped row does not shrink to fit,
            // it pushes the card past the pane and inflates every
            // `available_width()` measured after it.
            Some(aside) => {
                ui.horizontal_wrapped(|ui| {
                    if !label.is_empty() {
                        field_label(ui, label);
                    }
                    aside(ui);
                });
                ui.add_space(EYEBROW_GAP);
            }
        }
        return add(ui);
    }
    ui.horizontal_top(|ui| {
        // **The label column is a child `Ui`, not a bare allocation**, which is
        // the one structural change the aside needed. A rect allocated in a
        // `horizontal_top` advances the cursor ACROSS, so there is no way to
        // put a second widget under the caption; a fixed-width vertical child
        // gives the cell a cursor of its own and clips the chip to the column
        // rather than letting it push the control sideways.
        //
        // The caption itself is still painted rather than added, so its
        // position is byte-for-byte what it was before this variant existed.
        ui.allocate_ui_with_layout(
            Vec2::new(SECTION_LABEL_WIDTH, SECTION_FIELD_HEIGHT),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(SECTION_LABEL_WIDTH);
                let galley = ui.painter().layout(
                    label.to_string(),
                    FontId::new(12.0, FontFamily::Proportional),
                    TEXT_FAINT,
                    SECTION_LABEL_WIDTH,
                );
                // **The cell reserves the whole field's height, EXCEPT when
                // something follows the caption inside it.**
                //
                // A §8a caption is optically centred against a
                // [`SECTION_FIELD_HEIGHT`] field but is itself about 15
                // points tall, so the bottom half of the cell is slack.
                // Reserving the whole field and then putting the chip below
                // that is what the first render of this row did, and it cost
                // the row 28 points -- an `Email` row half again as tall as
                // the `First name` row above it, in a column whose whole job
                // is to look like a column. Reserving down to the caption's
                // own bottom edge instead spends the slack, and the row grows
                // by only what the chip cannot fit inside it.
                //
                // The caption is painted at the same y either way, so a row
                // with no aside is byte-for-byte what it was before this
                // variant existed. Centred against the ROW field's 34 and not
                // [`FIELD_HEIGHT`]'s 38, which is what it was measured against
                // when every box on the form was 38: (34 - 15) / 2 is §8a's
                // own `padding-top: 9px` on its multi-line rows, to the half
                // point.
                let top = (SECTION_FIELD_HEIGHT - galley.size().y) / 2.0;
                let reserved =
                    if aside.is_some() { top + galley.size().y } else { SECTION_FIELD_HEIGHT };
                let (cell, _) = ui.allocate_exact_size(
                    Vec2::new(SECTION_LABEL_WIDTH, reserved),
                    Sense::hover(),
                );
                // Top-aligned against the first line of the control beside it,
                // not centred in the cell: a row whose control is three boxes
                // tall would otherwise put its label level with the middle box.
                // §8a's own `padding-top: 9px` on exactly those rows is this
                // measurement.
                ui.painter().galley(Pos2::new(cell.left(), cell.top() + top), galley, TEXT_FAINT);
                if let Some(aside) = aside {
                    // Directly under the caption, with none of egui's usual
                    // row spacing between them: the chip belongs to the word
                    // above it, and every point of air here is a point the
                    // whole row grows by.
                    ui.spacing_mut().item_spacing.y = 0.0;
                    aside(ui);
                }
            },
        );
        ui.add_space(SECTION_ROW_GAP - ui.spacing().item_spacing.x);
        ui.vertical(|ui| add(ui)).inner
    })
    .inner
}

/// The word on the dirty mark. §8a's rail says `Changed`; the pill says the
/// same word, because two spellings of one state is how a user learns that
/// they are two states.
pub const CHANGED_PILL: &str = "Changed";

/// The dirty mark's colours: §8a's `Unsaved changes` pill exactly --
/// `color: #7a4f05; background: #fef6e7; border: 1px solid #f2d99b`, which is
/// [`CAUTION_INK`] on [`CAUTION_WASH`] inside [`CAUTION_EDGE`].
///
/// [`PillMark::None`]: the word carries the whole meaning, and a dot would
/// say "running" -- see [`PillMark`]'s own doc.
pub const CHANGED_TONE: PillTone = PillTone {
    fill: CAUTION_WASH,
    edge: CAUTION_EDGE,
    ink: CAUTION_INK,
    mark: PillMark::None,
};

/// §8a's password strength readout: four bars followed by the rating and the
/// length, e.g. `Strong \u{b7} 20 characters`.
///
/// **Four bars, always, with the unearned ones drawn in [`TOGGLE_OFF`]** --
/// §8a shows four filled and says nothing about the empty state, and a meter
/// that drew only the bars it had earned would be a meter whose TRACK changed
/// length with the score. A two-bar meter and a four-bar meter side by side
/// say nothing about each other.
///
/// The word is [`BLUE_DEEP`] and semibold at full strength (§8a's
/// `color: #14307a; font-weight: 600`) and [`TEXT_FAINT`] below it: the
/// design colours the readout only when there is something to be pleased
/// about, and a `Weak` painted in the brand's own blue would be praise.
pub fn strength_meter(ui: &mut Ui, filled: usize, word: &str, characters: usize) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = STRENGTH_BAR_GAP;
        let full = filled >= STRENGTH_BARS;
        for bar in 0..STRENGTH_BARS {
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(STRENGTH_BAR.x, STRENGTH_BAR.y), Sense::hover());
            // Centred on the row rather than sitting on its baseline: these
            // are 4 points tall beside a 12-point line, and left to egui they
            // would hang off the top of it.
            let bar_rect = Rect::from_center_size(
                Pos2::new(rect.center().x, rect.center().y),
                STRENGTH_BAR,
            );
            ui.painter().rect_filled(
                bar_rect,
                CornerRadius::same((STRENGTH_BAR.y / 2.0) as u8),
                if bar < filled { BLUE } else { TOGGLE_OFF },
            );
        }
        ui.add_space(STRENGTH_WORD_GAP - STRENGTH_BAR_GAP);
        let ink = if full { BLUE_DEEP } else { TEXT_FAINT };
        let face = FontId::new(
            12.0,
            if full { FontFamily::Name(SEMIBOLD.into()) } else { FontFamily::Proportional },
        );
        // **The count is dropped before the word is.**
        //
        // 8a's readout is `Strong \u{b7} 20 characters` on a card with 850
        // points to spend. This one sits in a section row's control column,
        // which is around 200 points in the shipped detail pane once a
        // 130-point label column and four 26-point bars are off it -- and an
        // unwrapped label in a horizontal row does not shrink, it runs past
        // the card's edge. That is exactly what it did on the first render of
        // this meter.
        //
        // So the row is measured and the LENGTH goes first. The word is the
        // rating; the count is context for it, and "Strong" alone still says
        // the thing the user needs. Truncating instead would produce
        // `Strong \u{b7} 20 char\u{2026}`, which spends the width on the half
        // that matters least.
        let counted = if characters == 1 {
            "1 character".to_string()
        } else {
            format!("{characters} characters")
        };
        let long = format!("{word} \u{b7} {counted}");
        let room = ui.available_width();
        let fits = |text: &str| {
            ui.painter().layout_no_wrap(text.to_string(), face.clone(), ink).size().x <= room
        };
        let text = if fits(&long) { long } else { word.to_string() };
        // Still elided if even the word does not fit -- a 40-point column is
        // not a width this row can be honest in, and a galley running under
        // the card's border is worse than an ellipsis.
        let styled = if full {
            semibold(text, 12.0).color(ink)
        } else {
            RichText::new(text).size(12.0).color(ink)
        };
        let galley = truncated_galley(ui, styled, room, TextStyle::Body);
        ui.add(egui::Label::new(galley));
    });
}

/// How many bars [`strength_meter`] draws. §8a's four, which is also how many
/// ratings `password_strength::Strength` has -- the two agreeing is what lets
/// the meter be a picture of the rating rather than a second scale.
pub const STRENGTH_BARS: usize = 4;

/// One bar's box: §8a's `width: 26px; height: 4px; border-radius: 2px`.
pub const STRENGTH_BAR: Vec2 = Vec2::new(26.0, 4.0);

/// The gap between one bar and the next: §8a's `gap: 3px` on the bar run.
///
/// Set on the row rather than left to `item_spacing`, whose 8 is what the
/// bars used to be drawn at -- four bars 8 apart are four dashes, and 3 apart
/// they are one meter with notches in it, which is the thing §8a draws.
const STRENGTH_BAR_GAP: f32 = 3.0;

/// The gap between the last bar and the word: §8a's `gap: 12px` on the
/// readout row. Spent as `12 - 3`, because the row's item spacing is already
/// [`STRENGTH_BAR_GAP`] by the time the word is placed.
const STRENGTH_WORD_GAP: f32 = 12.0;

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

/// How far a modal dims the window behind it.
///
/// Ninety, which is what `folder_modal`, the launch confirmation and
/// `prefs_ui`'s own modal each already dim by. It is a constant here so that
/// the next one to be built cannot arrive at a fifth copy of the number and
/// then quietly disagree with it.
pub const MODAL_SCRIM_ALPHA: u8 = 90;

/// The card's corner radius, applied to the outer frame **and to nothing
/// inside it except the two bands that touch a corner**.
///
/// Ten, the radius the cards this replaced already had. The header rounds its
/// top two corners and squares its bottom pair, the footer does the opposite,
/// and the body squares all four -- which is what makes three stacked bands
/// read as one card rather than as three cards in a pile.
const MODAL_RADIUS: u8 = 10;

/// The card's border, and the reason [`MODAL_BAND_BLEED`] is not
/// [`MODAL_RADIUS`].
const MODAL_STROKE: f32 = 1.0;

/// How far a coloured band is painted PROUD of the rect it was allocated, so
/// that it covers the card's own fill instead of stopping short of it.
///
/// # The white line, measured
///
/// `egui::Frame` paints its rectangle expanded by its stroke and strokes it
/// down the middle, so with a 1pt border the card's fill runs to the rect's
/// edge and the border covers only the outer HALF of that last point. A band
/// laid out inside the frame stops one full point short. What is left is
/// half a point of the card's fill, uncovered, all the way round.
///
/// It was invisible for as long as that fill was white behind a [`BORDER`]
/// stroke -- white against near-white -- and became a white line the moment
/// the stroke took the accent. Reported twice: once as "some white line along
/// the curve", and again after a first fix that only made it uniform. That
/// fix shrank the bands' corner radius so their arcs were concentric with the
/// card's, which was true and was not the problem: concentric arcs one point
/// apart still leave the gap, just evenly.
///
/// So the bands are painted OUT to the card's own rect, with the card's own
/// radius, and the border is drawn over them. Nothing is left to show
/// through, at a corner or anywhere else.
/// `pub` because both modals' test harnesses find a band by its width, and
/// that width is the card's plus this on each side.
pub const MODAL_BAND_BLEED: f32 = MODAL_STROKE;

/// The coloured header band's height.
pub const MODAL_HEADER_HEIGHT: f32 = 40.0;

/// The margin down both sides of the card, shared by the body and by the
/// footer's row of answers so that the buttons line up under the text rather
/// than nearly lining up under it.
///
/// Private, as are the radius and the scrim's alpha above it: a caller hands
/// this frame its content and its width and gets a card back, and nothing
/// outside this file has to know how the card is spaced. The one number that
/// IS `pub` is [`MODAL_HEADER_HEIGHT`], because the two modals' own tests
/// identify the header band by its height.
const MODAL_PAD_X: i8 = 20;

/// Breathing room above and below the body's own content.
const MODAL_BODY_PAD_Y: i8 = 18;

/// The same, for the footer band. Tighter than the body's, because the
/// buttons carry their own 32px of height and the band would otherwise read
/// as taller than the text it is answering.
const MODAL_FOOTER_PAD_Y: i8 = 14;

/// The gap between the two answers. The rest of the footer's width is split
/// evenly between them, so this number is the only thing that decides how
/// wide either button is.
const MODAL_FOOTER_GAP: f32 = 10.0;

/// The header glyph's box, sized against the 14px title beside it.
const MODAL_GLYPH_SIZE: f32 = 15.0;

/// The header title's size. A step up from the 13px the body and the buttons
/// are set in: it is the one line on the card that says what the card is.
const MODAL_TITLE_PX: f32 = 14.0;

/// The gap between the header's glyph and its title.
const MODAL_GLYPH_GAP: f32 = 8.0;

/// The item tile beside the body's subject line.
///
/// Smaller than `item_list::AVATAR_SIZE`'s 40 and larger than nothing: the
/// card is 340-360 wide against a list pane of 390, so a tile at the row's
/// own size would be the same tile at very nearly the same width, and the
/// modal would read as a row that had been lifted rather than as a card about
/// one. 28 is a tile that is recognisably the row's, at the scale the card's
/// other content is set in.
const MODAL_SUBJECT_TILE: f32 = 28.0;

/// The subject line's two type sizes and the gap between them.
///
/// `item_list`'s `TITLE_SIZE`, `SUBTITLE_SIZE` and `TITLE_GAP_Y`, restated
/// here rather than imported: this file is under `item_list` in the
/// dependency order and reaching up into it for three numbers would put the
/// design system behind the window that uses it. The numbers agreeing is what
/// makes the line read as the same line; `the_subject_line_is_the_items_row`
/// in `delete_modal` is what notices when it stops.
const MODAL_SUBJECT_NAME_PX: f32 = 13.0;
const MODAL_SUBJECT_USERNAME_PX: f32 = 11.0;
const MODAL_SUBJECT_GAP_Y: f32 = 2.0;

/// `box-shadow: 0 6px 20px rgba(45, 43, 43, 0.18)`, and the one thing on this
/// card that is not also on a card somewhere else in the app.
///
/// A modal is the only surface in this window that is *above* the window
/// rather than part of it, and the scrim alone does not say so -- a dimmed
/// background with a flat white rectangle on it reads as a panel that has had
/// the lights turned down around it. The shadow is what lifts it off.
const MODAL_SHADOW: egui::Shadow = egui::Shadow {
    offset: [0, 6],
    blur: 20,
    spread: 0,
    color: Color32::from_rgba_unmultiplied_const(45, 43, 43, 46),
};

/// The mark in the header band, left of the title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalGlyph {
    /// Nothing at all, and the title starts at the card's own margin.
    ///
    /// The design draws a glyph for a refusal because a refusal has one thing
    /// to say before its words are read. An ordinary question does not, and
    /// inventing a second mark to fill the slot would be putting a symbol in
    /// front of the user that means nothing in particular.
    None,
    /// The warning triangle, for the destructive and the refused.
    Warning,
}

/// Everything the frame needs to know that is not the card's own content.
pub struct ModalCard<'a> {
    /// The header band's fill **and the card's own outline**. [`ERROR`] for a
    /// destructive question, [`BLUE`] for an ordinary one.
    ///
    /// The report was "red modal should have red frame", about the delete
    /// confirmation, and it does not by itself say whether the blue card gets
    /// a blue one. It does, and the argument is that the alternative is two
    /// rules where the shape has one. The frame is not an alarm signal that
    /// happens to be painted on an edge -- it is the card's edge taking the
    /// card's own accent, the same way the header band, the confirm button
    /// and (on the delete card) the refusal sentence already do. Read the
    /// other way round, a rule saying "the outline is [`BORDER`] unless the
    /// accent is [`ERROR`]" makes the icon modal the one card in the app
    /// whose accent stops at the band, and leaves the next accent anybody
    /// adds -- an amber caution, say -- with no answer at all.
    ///
    /// It also costs nothing on the quiet card: at 1px, [`BLUE`] against the
    /// scrim is very nearly [`BORDER`] against it, and what the eye reads is
    /// the header it continues.
    pub accent: Color32,
    /// The mark beside the title.
    pub glyph: ModalGlyph,
    /// The header's words, in bold white. This is the card's heading and the
    /// body must not repeat it.
    pub title: &'a str,
    /// The card's width. The body and both buttons are laid out from it.
    pub width: f32,
    /// The outlined left-hand answer's words -- the way out, always on the
    /// left, always the quieter of the two.
    pub dismiss: &'a str,
}

/// Which of the footer's two answers was pressed, if either.
///
/// Both flags rather than an enum with a `None`: the caller's own action type
/// already has that variant, and a modal that reported "one of these" would
/// have to be asked which one anyway.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModalPress {
    /// The outlined button on the left.
    pub dismissed: bool,
    /// The filled button on the right.
    pub confirmed: bool,
}

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

/// The top strip of a hand-built card -- one that is an `egui::Frame` with a
/// `Margin::same(20)` and a bold 15px title on its first line, rather than
/// [`modal_card`]'s coloured band.
///
/// Six cards in this crate are built that way (`folder_modal`,
/// `detail_edit`'s discard confirm, `vault_window`'s two export cards and its
/// revoke report, and its launch confirm), and this is the height of what a
/// user reads as their header: 20 points of margin plus the ~19 the 15px line
/// occupies. It is deliberately a POINT SHORT of where the next widget starts
/// -- every one of those cards follows its title with `add_space(10.0)` -- so
/// the grab strip cannot reach a control.
pub const MODAL_PLAIN_HEADER_HEIGHT: f32 = 40.0;

/// The dismiss ✕ in the corner of a hand-built card: `card` is the rectangle
/// that card's own `egui::Frame` measured out to, `line` the rectangle of the
/// title the mark belongs beside. Answers whether it was pressed.
///
/// Call it from the card's OUTER `Ui`, immediately after the frame closes, so
/// that `card` is `framed.response.rect` and `line` is the title's own rect
/// carried out of the frame's closure.
///
/// # Why the frame's measured rect, and not the padding arithmetic
///
/// The inset has to be measured from the CARD's edge, and a `Ui` inside a
/// padded frame cannot see that edge: `ui.max_rect().right()` is the content
/// column's. The obvious repair -- add the frame's own inner margin back --
/// assumes the card is exactly as wide as the column it was told to lay out
/// in, and two of these cards are not. `record_ui`'s import form sets a
/// 360-wide maximum and then overflows it, painting a 449-wide card around a
/// 336-wide column; adding 12 to the column would have put the mark a hundred
/// points adrift of the corner it is supposed to be in, on the one card in the
/// app where the mark is the ONLY way out.
///
/// `egui::AreaState` was the other candidate and is worse: it is the
/// rectangle from the PREVIOUS pass, which is right for
/// [`modal_drag_handle`] -- that one has no choice, it must be registered
/// before the card is laid out -- and needlessly stale here, where the frame
/// has just finished measuring itself and can simply say.
///
/// # It claims no space
///
/// The mark goes down through [`modal_dismiss_mark`], which interacts at an
/// absolute rect and allocates nothing, so dropping this onto a card that
/// already exists moves nothing that card already paints. It is also the
/// reason the call order works: registered after the frame's contents it is on
/// top of them, and after [`modal_drag_handle`] it is on top of the drag strip
/// -- which is what stops the strip swallowing the click.
///
/// The mark is vertically centred on the TITLE's own line rather than on
/// [`MODAL_PLAIN_HEADER_HEIGHT`], because that constant measures from the top
/// of the card and includes the frame's margin -- a mark centred in it would
/// float nine points above the words it belongs beside.
pub fn modal_corner_mark(ui: &mut Ui, card: Rect, line: Rect) -> bool {
    modal_corner_mark_gated(ui, card, line, true)
}

/// [`modal_corner_mark`] for a card whose dismiss is not always available.
///
/// `dismissable` is `false` while the card is refusing to be closed -- a form
/// with a child process in flight is the only case in this crate today, and
/// `record_ui`'s two forms are it. The mark still PAINTS, faded the way egui
/// fades any disabled widget, and reports nothing; it does not disappear,
/// because a control that comes and goes reads as the card changing shape
/// rather than as an answer being unavailable.
///
/// **Only the mark is gated, never the title.** The title is drawn by the
/// caller inside the frame and is not touched by this at all, so a card that
/// is busy does not appear to grey out its own heading -- which is a much
/// louder statement than "the way out is not available for a moment".
pub fn modal_corner_mark_gated(
    ui: &mut Ui,
    card: Rect,
    line: Rect,
    dismissable: bool,
) -> bool {
    let row = Rect::from_min_max(
        Pos2::new(line.left(), line.top()),
        Pos2::new(card.right(), line.bottom()),
    );
    ui.add_enabled_ui(dismissable, |ui| {
        modal_dismiss_mark(ui, row, CloseInk::OnCard).clicked()
    })
    .inner
}

/// How far one modal has been dragged away from its centred position, and the
/// pass it was last drawn on.
///
/// **An offset from the centre, not a position, and the card stays
/// ANCHORED.** `.anchor(CENTER_CENTER, offset)` keeps everything the anchor
/// was already doing for free: with the offset at zero the builder is
/// character for character the one that was there before, so a card opens
/// exactly where it opened yesterday and every paint test that measured it
/// still measures the same rectangle; and a window the user RESIZES
/// re-centres the card under it, instead of leaving it stranded beside an
/// edge that has moved. A stored absolute position would have had to be
/// migrated by hand on every resize, and would have got the first frame wrong
/// -- the frame on which nothing has measured the card yet.
///
/// **`last_pass` is how the offset is thrown away when the modal closes.** A
/// closed modal draws nothing, so there is no "on close" callback to hang
/// this on; what there IS is the pass number the card was last seen on. A gap
/// in that -- not drawn on the pass before this one -- is a card that went
/// away and came back, and that is the moment the position resets. See
/// [`modal_offset`] for why resetting is the right default.
#[derive(Clone, Copy, Default)]
struct ModalOffset {
    /// The drag, accumulated. Always already clamped: see [`modal_offset`].
    by: Vec2,
    /// [`egui::Context::cumulative_pass_nr`] when this card was last drawn.
    last_pass: u64,
}

/// **The clamp, and it is the whole rule: a modal may not be dragged so that
/// any part of it leaves the window.**
///
/// The requirement is that the card stay reachable -- that the user can
/// always get at the header to drag it back, and at whatever control dismisses
/// it. The tempting version of that rule is "keep the header and the ✕ in",
/// and it does not survive contact with these cards, because they do not
/// agree on where the dismiss control IS: `prefs_ui` draws a ✕ at the right
/// end of its header, [`modal_card`] draws two answers in a FOOTER band at
/// the very bottom, `folder_modal` puts a Cancel button in the body, and
/// `totp_add` moves its dismiss between stages. A helper that clamped "the
/// dismiss control" would have to be told where each card keeps one, and
/// would be wrong the first time a card moved it.
///
/// Containing the whole card needs none of that: whatever the dismiss control
/// is and wherever it lives, it is inside the card, so a card inside the
/// window has it inside the window too. It is also the rule a user can
/// predict without being told -- the card stops at the edge, the way a window
/// stops at the edge of a screen.
///
/// The arithmetic falls out symmetric, which is worth naming because it is
/// what makes the rule one line: a card centred in a window has exactly half
/// the leftover width free on its left and half on its right, so the travel
/// is `±(window - card) / 2` per axis.
///
/// **A card LARGER than the window gets no travel at all**, rather than
/// negative travel: `slack` floors at zero, so the clamp collapses to
/// `offset = 0`, which is the centred position the card opened at. That is
/// the least-bad place for a card that cannot fit -- it is the position that
/// wastes the least of it off either edge -- and it is the one the user
/// already knows, so a window shrunk until the card no longer fits snaps the
/// card back to the middle rather than parking it against a corner.
///
/// **egui agrees with this rule, and that is not a reason to drop it.** An
/// `Area` is `constrain`ed to the window by default, so the card egui DRAWS
/// is already kept inside whatever this returns -- measured, not assumed:
/// disable this clamp and `the_card_cannot_be_dragged_out_of_the_window`
/// still passes. What egui does not do is stop the offset THIS file
/// remembers from running away, and an un-clamped offset five thousand points
/// past the edge is five thousand points the user must drag back before the
/// card so much as twitches.
/// `a_card_shoved_past_the_edge_comes_back_on_the_very_next_drag` is the one
/// that dies without this, and it is the reason the clamp exists.
///
/// It also keeps [`movable_modal_at`] honest, where egui's constraint would
/// be an active hazard rather than a spare net: that caller PAINTS from the
/// rectangle it was handed, so a position egui had to pull back into the
/// window would be a card whose chrome and whose widgets disagreed.
pub fn clamp_modal_offset(window: Rect, card: Vec2, by: Vec2) -> Vec2 {
    let slack = ((window.size() - card) / 2.0).max(Vec2::ZERO);
    Vec2::new(by.x.clamp(-slack.x, slack.x), by.y.clamp(-slack.y, slack.y))
}

/// Reads this modal's drag offset, resets it if the modal has just been
/// reopened, re-clamps it to the window as it is NOW, and writes it back.
///
/// `size` is the card's, when the caller knows it; `None` on the frame a
/// self-measuring card has not been measured on yet, where there is nothing
/// to clamp against and nothing has been dragged either.
///
/// # The position resets when the modal closes
///
/// A card dragged into a corner and then reopened there is a card the user
/// has to go and look for, and the cost is not symmetric: the frame the modal
/// opens on is the ONE moment this app can guarantee the card is findable, so
/// spending it is cheap and not spending it is how a user ends up hunting for
/// a dialog they are sure they opened. Remembering the position would buy the
/// user who drags the same card aside twice in a row one drag; it would cost
/// every other user the assumption that a modal appears in the middle of the
/// window.
///
/// # Why the clamp is applied to the STORED offset and not just to the drawn
/// position
///
/// The vault window is resizable, down to [`crate::settings::MIN_VAULT_WINDOW_SIZE`],
/// so a card dragged to the edge of a large window is a card whose offset no
/// longer fits after the window is dragged smaller. Re-clamping every pass is
/// what keeps it reachable: the card walks back in as the window closes on
/// it, rather than sliding out of the window and taking its dismiss control
/// with it.
///
/// Clamping the STORED value -- rather than keeping the raw offset and
/// clamping only for display -- is the deliberate half. The alternative
/// remembers the drag that no longer fits and springs the card back out when
/// the window is widened again, minutes later, with no pointer anywhere near
/// it. A card that moves on its own is worse than a card that forgot how far
/// it was once pushed, and this way the operation is idempotent: what is
/// stored is always a position the card is actually in.
fn modal_offset(ctx: &egui::Context, id: egui::Id, size: Option<Vec2>) -> Vec2 {
    let pass = ctx.cumulative_pass_nr();
    let mut state = ctx.data(|d| d.get_temp::<ModalOffset>(id)).unwrap_or_default();
    // Strictly `<`: drawn on the pass before this one is a modal that stayed
    // open, and egui runs more than one pass for a single frame whenever
    // something asks for a re-layout, so "the same pass" has to count as open
    // too.
    if state.last_pass + 1 < pass {
        state.by = Vec2::ZERO;
    }
    if let Some(size) = size {
        state.by = clamp_modal_offset(ctx.content_rect(), size, state.by);
    }
    state.last_pass = pass;
    ctx.data_mut(|d| d.insert_temp(id, state));
    state.by
}

/// **The way every self-measuring modal in this app gets its `Area`.**
///
/// Hand it the same `egui::Area::new(egui::Id::new("..."))` the call site used
/// to write, and get back one that is on [`egui::Order::Foreground`], centred,
/// and offset by however far the user has dragged it. With nothing dragged
/// that is exactly `.order(Foreground).anchor(CENTER_CENTER, Vec2::ZERO)` --
/// the line it replaces -- so the first frame of every card, and every test
/// that measured one, is unchanged.
///
/// **The `Area` is the CALLER's**, for [`modal_scrim`]'s reason: the ids are
/// literals at the call sites because `item_list::MODAL_SCRIM_AREAS` and its
/// source walk read them there, and a helper taking a bare string would move
/// the declaration out of that walk's sight.
///
/// The caller must also call [`modal_drag_handle`] as the FIRST thing inside
/// the area, or the card is centred and still unmovable. That is two calls
/// rather than one because the second one needs a `Ui` that only exists
/// inside `show`, and because only the card knows how tall its own header is.
pub fn movable_modal(ctx: &egui::Context, area: egui::Area) -> egui::Area {
    let id = area.layer().id;
    // The size egui measured last pass. `None` before the card has ever been
    // drawn, which is the frame an anchored `Area` paints nothing on anyway
    // -- see `prefs_ui`'s `an_anchored_area_paints_nothing_on_its_first_frame`.
    let size = modal_last_size(ctx, id);
    let by = modal_offset(ctx, id, size);
    area.order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, by)
}

/// The size egui measured a modal's `Area` at on the last pass, or `None`
/// before it has ever been laid out. What [`movable_modal`] clamps the drag
/// offset against.
pub fn modal_last_size(ctx: &egui::Context, area_id: egui::Id) -> Option<Vec2> {
    egui::AreaState::load(ctx, area_id).and_then(|state| state.size)
}

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

/// [`movable_modal`] for a card that COMPUTES its own rectangle instead of
/// letting egui measure it: the area, **and the rectangle the card is now
/// at**.
///
/// `prefs_ui` is the one of those. Its card is a fixed size worked out from
/// the window, placed with `fixed_pos` rather than an anchor so that it paints
/// on the very first frame instead of spending one being measured; feeding it
/// through the anchor would undo that. It then paints itself in ABSOLUTE
/// coordinates taken from that rectangle -- header, ✕, body panel and all --
/// so moving the `Area` alone would move where the card's widgets are laid
/// out and leave its painted chrome behind. Hence both halves come back from
/// one call: whatever the caller derives its geometry from must be the
/// returned rect and not the one it passed in.
///
/// Clamped against the size the caller already knows, which is why this one
/// needs no warm-up frame before the clamp is real.
pub fn movable_modal_at(
    ctx: &egui::Context,
    area: egui::Area,
    card: Rect,
) -> (egui::Area, Rect) {
    let id = area.layer().id;
    let moved = card.translate(modal_offset(ctx, id, Some(card.size())));
    (area.order(egui::Order::Foreground).fixed_pos(moved.min), moved)
}

/// **The grabbable strip: a card is dragged by its header and by nothing
/// else.**
///
/// Call it as the first statement inside a [`movable_modal`] area, passing
/// the height of that card's own header -- [`MODAL_HEADER_HEIGHT`] for
/// [`modal_card`]'s coloured band, [`MODAL_PLAIN_HEADER_HEIGHT`] for a
/// hand-built card's title line, or the card's own constant where it has one.
///
/// # Why not the whole card
///
/// These cards are full of text fields, tick-boxes, segmented runs and
/// buttons, and a drag that started anywhere on the card would fight every
/// one of them: a drag is how a text field selects, how a segmented control
/// is swiped, how a scrolling body is flung. The header is the one band of
/// every card that holds nothing a pointer does anything with, which is what
/// makes it the handle -- the same reason a title bar is a title bar.
///
/// # Why it is registered FIRST, and why it senses only drags
///
/// egui hit-tests clicks and drags SEPARATELY, but not independently: where
/// the topmost widget under the pointer senses drags and not clicks, it
/// swallows the click rather than letting it through to whatever is below
/// (`egui::hit_test`, the `(Some(click), Some(drag))` arm -- "it would be
/// confusing if clicking a drag-widget would actually click something else
/// below it"). A strip registered after the header's contents would therefore
/// eat the ✕ that `prefs_ui` and `totp_add` draw there. Registered before
/// them it is the one UNDERNEATH, egui reports the ✕ for the click and this
/// strip for the drag, and both work.
///
/// Registering first has a price and it is one frame: the strip has to be
/// placed before the card has been laid out, so it is placed over the rect
/// the card occupied on the previous pass. On the pass a card changes size --
/// `totp_add` moving between stages, an error line appearing -- the strip is
/// momentarily up to half the size change out of position. The alternative
/// costs a dismiss button.
///
/// # `Sense::drag()`, and `ui.interact` rather than `ui.allocate_rect`
///
/// `allocate_rect` would grow the `Ui`'s min rect, which IS the size egui
/// centres the area by, so the strip would push the card off centre by its
/// own height.
///
/// # The cursor
///
/// `Grab` on hover and `Grabbing` while held. `PointingHand` is this app's
/// mark for "this does something when you click it" ([`close_glyph`] sets it,
/// so do the buttons), and the header does nothing when clicked -- pointing a
/// finger at it would promise an action that is not there. The grab pair is
/// the platform's own word for "this moves", and it also reports back: the
/// cursor changing under the pointer is the only thing on screen that says
/// the header is a handle at all.
pub fn modal_drag_handle(ui: &mut Ui, header_height: f32) {
    let id = ui.layer_id().id;
    let Some(state) = egui::AreaState::load(ui.ctx(), id) else {
        return;
    };
    if state.size.is_none() {
        // Never drawn: there is no rect to put a handle on yet.
        return;
    }
    let card = state.rect();
    modal_drag_handle_at(
        ui,
        Rect::from_min_max(
            card.min,
            Pos2::new(card.max.x, (card.min.y + header_height).min(card.max.y)),
        ),
    );
}

/// [`modal_drag_handle`] for a card that knows its own header rectangle this
/// frame, rather than having to read last frame's off the `Area`.
///
/// [`movable_modal_at`]'s partner, and its advantage is the same one: a card
/// that computes its geometry can be grabbed on the first frame it is drawn
/// and stays exact through a resize, where a self-measuring card's handle is
/// one pass behind its own layout.
pub fn modal_drag_handle_at(ui: &mut Ui, header: Rect) {
    // The area's own id, taken from the layer rather than passed in, so it
    // cannot drift out of step with the id `movable_modal` stored under.
    let id = ui.layer_id().id;
    let response = ui.interact(header, id.with("modal-drag"), Sense::drag());
    if response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let by = response.drag_delta();
        if by != Vec2::ZERO {
            ui.ctx().data_mut(|d| {
                let mut offset = d.get_temp::<ModalOffset>(id).unwrap_or_default();
                offset.by += by;
                d.insert_temp(id, offset);
            });
            // The clamp and the redraw both happen in `movable_modal` on the
            // next pass, which is the pass this asks for. Without it a drag
            // that ends between repaints leaves the card a few points behind
            // the pointer until something else wakes the window.
            ui.ctx().request_repaint();
        }
    } else if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
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

pub fn modal_scrim(ctx: &egui::Context, area: egui::Area) {
    let screen = ctx.content_rect();
    area.order(SCRIM_ORDER)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            // Allocating the full screen is what makes the block real; an
            // area that allocates nothing has a near-zero stored rect and
            // catches nothing outside the card.
            ui.allocate_response(screen.size(), Sense::click());
            ui.painter().rect_filled(
                screen,
                CornerRadius::ZERO,
                Color32::from_black_alpha(MODAL_SCRIM_ALPHA),
            );
        });
}

/// Draws one modal card, centred until the user moves it, and reports which
/// of its two answers was pressed.
///
/// **Movable, and its callers do nothing to get that.** The area comes from
/// [`movable_modal`] and the handle from [`modal_drag_handle`], both called
/// here, so `delete_modal` and `icon_modal` are draggable by the header band
/// this function draws for them without either file mentioning a drag.
///
/// `body` fills the white middle band; it is given a `Ui` already inset by
/// the card's margins and already the right width for a wrapping label.
/// `confirm` draws the filled right-hand button and hands back its
/// `Response` -- a closure rather than a label, because the button is
/// [`destructive_button`] on one card and [`primary_button_enabled`] on
/// another, and which of those a modal wears is the modal's own decision.
///
/// **Two closures rather than one, and they must not both borrow the same
/// state mutably.** The body writes (a text field edits through it) and the
/// confirm reads, so a caller whose confirm depends on what the body edits
/// works out that dependency BEFORE the call -- see `icon_modal`, which
/// computes whether its address box has anything in it and passes the answer
/// in by value.
pub fn modal_card(
    ctx: &egui::Context,
    area: egui::Area,
    card: ModalCard<'_>,
    body: impl FnOnce(&mut Ui),
    confirm: impl FnOnce(&mut Ui) -> Response,
) -> ModalPress {
    let mut press = ModalPress::default();
    movable_modal(ctx, area)
        .show(ctx, |ui| {
            // First, before anything the header draws -- see
            // [`modal_drag_handle`] on why the order is the whole trick.
            modal_drag_handle(ui, MODAL_HEADER_HEIGHT);
            let framed = egui::Frame::new()
                .fill(CARD)
                .corner_radius(CornerRadius::same(MODAL_RADIUS))
                // The accent and not [`BORDER`] -- see [`ModalCard::accent`]
                // for why every card gets its own colour here and not just
                // the destructive one.
                //
                // Set here for the GEOMETRY -- `egui::Frame` expands its own
                // rect by its stroke, and the bands are laid out against
                // that -- and drawn again below for the PAINT. A `Frame`
                // strokes before its contents, and the bands are painted out
                // over the border so that none of the card's fill shows past
                // them (see [`MODAL_BAND_BLEED`]); the footer's tint is not
                // the accent, so it covered the border along the bottom and
                // the lower corners. Reported: "the red frame should go all
                // the way around and not stop at the bottom."
                .stroke(Stroke::new(MODAL_STROKE, card.accent))
                .shadow(MODAL_SHADOW)
                .show(ui, |ui| {
                    ui.set_width(card.width);
                    // The three bands butt against each other, so the card's
                    // own stack gets no item spacing at all -- egui's default
                    // 8px would open a white seam under the coloured header
                    // and another above the footer, and two hairlines of bare
                    // card is exactly what "three stacked cards" looks like.
                    // The body gets the inherited spacing back, because its
                    // contents are ordinary stacked widgets that want it.
                    let inherited = ui.spacing().item_spacing;
                    ui.spacing_mut().item_spacing.y = 0.0;

                    let marked = modal_header_band(ui, &card);
                    egui::Frame::new()
                        .inner_margin(Margin::symmetric(MODAL_PAD_X, MODAL_BODY_PAD_Y))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing = inherited;
                            body(ui);
                        });
                    press = modal_footer_band(ui, &card, confirm);
                    // **The ✕ sets the SAME flag the footer's left-hand answer
                    // does**, or-ed in after the footer has reported rather
                    // than before it, because `modal_footer_band` returns a
                    // whole [`ModalPress`] and assigning it would throw a
                    // header press away. Or-ed rather than assigned so that a
                    // frame in which both somehow reported still dismisses --
                    // which on the delete card is the cautious answer winning,
                    // the ordering `delete_modal` already argues for its own
                    // two buttons. See [`modal_header_band`].
                    press.dismissed |= marked;
                });
            // **The border again, over everything.** Same rect, same colour,
            // same width as the one the `Frame` drew -- this is not a second
            // border, it is the same one painted where the bands cannot
            // reach it. Drawing it twice is cheaper than the alternatives:
            // dropping the `Frame`'s stroke would shrink the rect the bands
            // are laid out against, and un-bleeding the bands would put the
            // white line back.
            //
            // **`Inside`, and the `Middle` it replaces is why the owner saw a
            // "double red line to the right" on the delete card.** The claim
            // above -- that this is the same border and not a second one --
            // is only true if the two strokes land on the same pixels, and
            // they did not: a `Frame` draws its stroke `Inside` the rect, and
            // `Middle` centres it ON the edge, half a stroke-width further
            // out. At `MODAL_STROKE`'s weight that is a visible parallel
            // line, and on the delete card it is in alarm red.
            //
            // The same slip, in the same shape, was fixed in `totp_add`'s
            // `stage_card` one commit earlier -- a `Frame` stroke plus a
            // `Middle` repaint -- and these two are the only places in the
            // crate that stroke one rect twice. Every other `StrokeKind` here
            // is a lone stroke, where `Middle` straddles the edge on purpose
            // and has nothing to disagree with.
            ui.painter().rect_stroke(
                framed.response.rect,
                CornerRadius::same(MODAL_RADIUS),
                Stroke::new(MODAL_STROKE, card.accent),
                egui::StrokeKind::Inside,
            );
        });
    press
}

/// The coloured band across the card's top: the glyph, then the title, then
/// the dismiss ✕ at the far end. Answers whether the ✕ was pressed.
///
/// **The ✕ is the footer's left-hand answer, not a third one.** It reports
/// through [`ModalPress::dismissed`], which is the same flag
/// [`ModalCard::dismiss`]'s button sets, so the two callers of this card --
/// `delete_modal` and `icon_modal` -- get the mark wired to their own Cancel
/// without either file changing a line. That is the rule the whole pass is
/// built on: the mark is an existing gesture given a second surface, never a
/// new outcome. On the delete card in particular it is the SAFE answer and
/// could not be anything else, which is why the mark could be added there
/// without touching the file that asks the destructive question.
fn modal_header_band(ui: &mut Ui, card: &ModalCard<'_>) -> bool {
    let (band, _) =
        ui.allocate_exact_size(Vec2::new(card.width, MODAL_HEADER_HEIGHT), Sense::hover());
    let painter = ui.painter();
    // Top corners rounded, bottom pair square. Per-corner rather than
    // `prefs_ui`'s "fill it round, then fill the bottom strip square again"
    // trick: one shape, and no second rectangle to keep in step with the
    // first when the radius moves.
    painter.rect_filled(
        // Out to the card's edge on the three sides that touch it, and left
        // alone on the fourth: the body begins where this band's allocated
        // rect ends, so bleeding downward would put the accent under it.
        Rect::from_min_max(
            Pos2::new(band.left() - MODAL_BAND_BLEED, band.top() - MODAL_BAND_BLEED),
            Pos2::new(band.right() + MODAL_BAND_BLEED, band.bottom()),
        ),
        CornerRadius { nw: MODAL_RADIUS, ne: MODAL_RADIUS, sw: 0, se: 0 },
        card.accent,
    );

    let mut x = band.left() + f32::from(MODAL_PAD_X);
    if card.glyph == ModalGlyph::Warning {
        let at = Rect::from_center_size(
            Pos2::new(x + MODAL_GLYPH_SIZE / 2.0, band.center().y),
            Vec2::splat(MODAL_GLYPH_SIZE),
        );
        paint_warning_glyph(painter, at, Color32::WHITE);
        x = at.right() + MODAL_GLYPH_GAP;
    }
    painter.text(
        Pos2::new(x, band.center().y),
        egui::Align2::LEFT_CENTER,
        card.title,
        FontId::new(MODAL_TITLE_PX, FontFamily::Name(BOLD.into())),
        Color32::WHITE,
    );

    // `band` IS the card's inside edge -- it was allocated at `card.width`,
    // which is the width the body and footer are laid out to -- so the mark
    // ends up [`MODAL_CLOSE_INSET`] off the card exactly as it does on every
    // other header in this crate. [`CloseInk::OnAccent`] because the surface
    // under it is the card's own colour and ghost-grey on red is not a mark.
    //
    // Drawn AFTER the title rather than before: the title is painted straight
    // onto the band and would run under a long enough one. It does not today
    // -- the widest heading on either card stops well short -- and the order
    // is still worth having, because the mark ending up on top is the
    // failure that stays clickable while it is unreadable.
    modal_dismiss_mark(ui, band, CloseInk::OnAccent).clicked()
}

/// An equilateral triangle's height as a fraction of its base, which is the
/// one number that makes this mark the shape the design names rather than
/// "a triangle". sqrt(3)/2, written out because `f32::sqrt` is not const.
const WARNING_EQUILATERAL: f32 = 0.866_025_4;

/// How much of each edge a corner's rounding eats, as a fraction of that
/// edge. The triangle is equilateral, so all three edges are the same length
/// and a fraction of the vector to a neighbouring corner IS a fraction of the
/// edge -- the identity [`star_outline`] leans on for the same purpose.
///
/// Just under a sixth. Enough to take the chip off the 60-degree apex, which
/// is the sharpest corner anything in this app strokes and the one the eye
/// lands on first; more than this and the mark stops reading as a triangle at
/// [`MODAL_GLYPH_SIZE`].
const WARNING_ROUND: f32 = 0.16;

/// The warning triangle's stroke, and the bang's inside it.
///
/// Heavier than [`ICON_STROKE`], and deliberately not part of that family:
/// every mark that wears 1.3 is grey ink on a white card, where this one is
/// white ink on a saturated band. A light stroke on a strong ground loses
/// perceived weight to the colour bleeding around it, and this is also the
/// only mark in the app that has to hold its own beside 14px bold type.
const WARNING_STROKE: f32 = 1.6;

/// The bang's dot, as a radius.
///
/// **A circle rather than the filled square this used to be.** The square was
/// chosen to stay clear of `icon_probe`, and it worked, but a 1.6px square is
/// visibly a square at 100% scaling and the design draws a dot. The radius is
/// what keeps the probe clear now, and it is a constraint and not a taste:
/// `icon_probe::kebab_dots` matches a circle on radius alone, so a dot at
/// [`KEBAB_DOT_RADIUS`] would be counted by every "the header paints exactly
/// three dots" assertion in `detail.rs`.
/// [`the_drawn_circles_do_not_share_a_radius`] is the live guard.
const WARNING_DOT_RADIUS: f32 = 1.15;

/// Where the bang's bar begins and ends, and where its dot sits, as signed
/// fractions of the triangle's half-height measured from its centre --
/// negative is up.
///
/// Not symmetric about the centre, because the triangle is not: its ink
/// crowds the bottom edge and thins to nothing at the apex, so a bang centred
/// on the geometric middle sits visibly high. At [`MODAL_GLYPH_SIZE`] these
/// put the bang's own ink around y = +1.5 against the triangle's centroid at
/// +2.2, which is as close as the mark gets to balanced while leaving the dot
/// clear of the base by more than half the base's own stroke and the bar's
/// gap wider than the stroke it is a gap in.
const WARNING_BAR_TOP: f32 = -0.30;
const WARNING_BAR_BOTTOM: f32 = 0.22;
const WARNING_DOT_DROP: f32 = 0.58;

/// The warning triangle, **stroked rather than typed**.
///
/// U+26A0 is not in Archivo and is not in egui's fallback stack either, which
/// is the same measurement [`close_glyph`] records for U+2715 and the same
/// answer: a codepoint this app's face does not carry renders as a tofu box.
///
/// **One closed path and not three line segments, which is most of what "does
/// not match the design" was.** The segments were how the first version kept
/// clear of `icon_probe::envelopes` -- see [`WARNING_VERTICES`] for that
/// hazard, which is real and unchanged -- and they cost the mark its corners:
/// `egui::Stroke` carries no join style, so three flat-capped segments meeting
/// at a 60-degree apex leave the point chipped open. Rounding the corners in
/// the path is the fix [`STAR_STROKE`]'s own history records for the same
/// defect, and it carries the point count clear of the flap's three at the
/// same time, so the constraint and the design agree here rather than trade.
///
/// **Public for the caution bands that are not [`modal_card`]'s**, which is
/// design 6c's -- a `#fef6e7` strip over the footer of the one-time-code
/// card, drawn in `vault_window::totp_add` because that card is built there
/// rather than out of the shared modal. `color` is the caller's for the same
/// reason: this band strokes the sign in the design's `#8a5a06` on its own
/// tint where the modal's header band strokes it white on [`ERROR`].
pub fn paint_warning_glyph(painter: &egui::Painter, rect: Rect, color: Color32) {
    let stroke = Stroke::new(WARNING_STROKE, color);
    painter.add(egui::Shape::Path(egui::epaint::PathShape {
        points: warning_outline(rect),
        closed: true,
        // The mark is an outline, so this is what it draws -- and it is also
        // what keeps it in the half of `icon_probe` that walks UNFILLED
        // closed paths, where a triangle belongs, rather than making the
        // point count the only thing standing between it and a filled
        // family. The folder mark and the envelope's flap state the same
        // rule for the same reason.
        fill: Color32::TRANSPARENT,
        stroke: stroke.into(),
    }));
    // The bang inside it: a bar, then a gap, then a dot.
    let half_height = rect.width() * WARNING_EQUILATERAL / 2.0;
    let bang_x = rect.center().x;
    let at = |fraction: f32| rect.center().y + half_height * fraction;
    painter.line_segment(
        [Pos2::new(bang_x, at(WARNING_BAR_TOP)), Pos2::new(bang_x, at(WARNING_BAR_BOTTOM))],
        stroke,
    );
    painter.circle_filled(
        Pos2::new(bang_x, at(WARNING_DOT_DROP)),
        WARNING_DOT_RADIUS,
        color,
    );
}

/// The triangle's rounded outline: an equilateral triangle as wide as `rect`,
/// point up, centred in it, with every corner cut back by [`WARNING_ROUND`]
/// and carried across the cut by a quadratic Bezier.
///
/// **It is inscribed in the width and centred in the height, rather than
/// filling the box.** The version this replaces put its apex on the box's top
/// edge and its base on the bottom one, which at [`MODAL_GLYPH_SIZE`] is a
/// triangle 15 wide and 15 tall -- 15% taller than equilateral, and beside a
/// 14px title it read as a narrow wedge rather than as the sign it is meant
/// to be. Taking the height from the width is what makes "equilateral" a
/// property of the mark instead of a property of the box it happens to be
/// handed.
fn warning_outline(rect: Rect) -> Vec<Pos2> {
    let half_base = rect.width() / 2.0;
    let half_height = rect.width() * WARNING_EQUILATERAL / 2.0;
    let corners = [
        Vec2::new(0.0, -half_height),
        Vec2::new(half_base, half_height),
        Vec2::new(-half_base, half_height),
    ];
    let mut points: Vec<Vec2> = Vec::with_capacity(WARNING_VERTICES);
    for i in 0..WARNING_CORNERS {
        let here = corners[i];
        let before = corners[(i + WARNING_CORNERS - 1) % WARNING_CORNERS];
        let after = corners[(i + 1) % WARNING_CORNERS];
        let from = here + (before - here) * WARNING_ROUND;
        let to = here + (after - here) * WARNING_ROUND;
        for step in 0..=WARNING_ROUND_SEGMENTS {
            let t = step as f32 / WARNING_ROUND_SEGMENTS as f32;
            let u = 1.0 - t;
            points.push(from * (u * u) + here * (2.0 * u * t) + to * (t * t));
        }
    }
    debug_assert_eq!(points.len(), WARNING_VERTICES);
    points.into_iter().map(|p| rect.center() + p).collect()
}

/// The footer: a hairline, then the tinted band, then the two answers filling
/// the width between the card's margins.
///
/// The tint and the rule above it are the send preflight's own footer,
/// reproduced rather than reinvented -- that card fills its footer with
/// [`CARD_TINT`] under a [`HAIRLINE`] rule, and this is the same two colours
/// in the same order.
fn modal_footer_band(
    ui: &mut Ui,
    card: &ModalCard<'_>,
    confirm: impl FnOnce(&mut Ui) -> Response,
) -> ModalPress {
    let (rule, _) = ui.allocate_exact_size(Vec2::new(card.width, 1.0), Sense::hover());
    ui.painter().rect_filled(rule, CornerRadius::ZERO, HAIRLINE);

    let mut press = ModalPress::default();
    // **Painted behind, not by the `Frame`**, so the band can bleed out to
    // the card's edge the way the header does -- see [`MODAL_BAND_BLEED`]. A
    // `Frame`'s own fill is confined to the rect layout gives it, which is
    // one point inside the card and is exactly what left a line showing. The
    // reserved-index idiom is `Frame`'s own: claim a slot before the
    // contents, fill it in once their rect is known.
    let band = ui.painter().add(egui::Shape::Noop);
    let laid_out = egui::Frame::new()
        .inner_margin(Margin::symmetric(MODAL_PAD_X, MODAL_FOOTER_PAD_Y))
        .show(ui, |ui| {
            let inner = ui.available_width();
            let half = ((inner - MODAL_FOOTER_GAP) / 2.0).max(0.0);
            let (row, _) = ui.allocate_exact_size(Vec2::new(inner, BUTTON_HEIGHT), Sense::hover());
            press.dismissed = modal_answer(
                ui,
                Rect::from_min_size(row.min, Vec2::new(half, BUTTON_HEIGHT)),
                |ui| secondary_button(ui, card.dismiss),
            );
            // Measured from the row's RIGHT edge rather than from the left
            // one plus a gap, so the two buttons meet the card's two margins
            // exactly and any rounding error lands in the gap between them
            // where nobody can see it.
            press.confirmed = modal_answer(
                ui,
                Rect::from_min_size(
                    Pos2::new(row.max.x - half, row.min.y),
                    Vec2::new(half, BUTTON_HEIGHT),
                ),
                confirm,
            );
        });
    // The band, now that its rect is known: out to the card's edge on the
    // three sides it touches, and stopping at its own top where the hairline
    // above it already separates it from the body.
    let rect = laid_out.response.rect;
    ui.painter().set(
        band,
        egui::epaint::RectShape::filled(
            Rect::from_min_max(
                Pos2::new(rect.left() - MODAL_BAND_BLEED, rect.top()),
                Pos2::new(rect.right() + MODAL_BAND_BLEED, rect.bottom() + MODAL_BAND_BLEED),
            ),
            CornerRadius { nw: 0, ne: 0, sw: MODAL_RADIUS, se: MODAL_RADIUS },
            CARD_TINT,
        ),
    );
    press
}

/// One footer answer, drawn into the half of the row it was given.
///
/// The child `Ui` is **cross-justified**, which is what makes an
/// `egui::Button` -- which otherwise sizes itself to its own words -- fill
/// the rect instead. Without it the two answers would be as wide as their
/// labels, and "Cancel" beside "Delete forever" is the lopsided pair the
/// design's evenly split row exists to avoid.
fn modal_answer(ui: &mut Ui, at: Rect, add: impl FnOnce(&mut Ui) -> Response) -> bool {
    let mut slot = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(at)
            .layout(egui::Layout::top_down_justified(egui::Align::Center)),
    );
    add(&mut slot).clicked()
}

/// The body's opening line: the item's own tile, its name, and `username`
/// under the name -- **the row the results list draws, at the top of the card
/// that is asking about it**.
///
/// Both modals that use the frame are about one vault item, and both used to
/// name it in a faint 11px line under the heading -- which is where a caption
/// goes, not where the subject of a question goes. The design puts the item's
/// own tile in front of its own name and sets the name in the body's weight,
/// so that the thing about to be deleted (or given a picture) is the first
/// thing on the card that is read.
///
/// **And it is the SAME line the list draws, not a smaller echo of it.** The
/// first version drew a monogram tile and the name alone, so a card opened
/// from a row carrying a bank's favicon and a login name showed neither --
/// which asks the reader to match a name against a row they can no longer
/// see, when the row itself is what they recognised in the first place. The
/// two sizes and the two colours below are `item_list`'s `TITLE_SIZE` /
/// `SUBTITLE_SIZE` and its [`INK`] / [`TEXT_FAINT`], and the gap is its
/// `TITLE_GAP_Y`.
///
/// **The tile is a closure, and that is the module boundary rather than a
/// style.** What goes in it -- the favicon when the icon cache has one, the
/// item's KIND mark when it does not, the name's monogram when the kind has
/// none -- is a decision about vault items, and this file knows nothing about
/// vault items and should not start now. `vault_window::ModalSubject` is
/// where that branch is made, once, for both cards.
pub fn modal_subject(ui: &mut Ui, name: &str, username: &str, tile: impl FnOnce(&mut Ui, f32)) {
    ui.horizontal(|ui| {
        tile(ui, MODAL_SUBJECT_TILE);
        // A vertical of its own rather than two labels in the horizontal:
        // `ui.horizontal` centres its children on the cross axis, so the
        // column of one or two lines is centred against the tile exactly as
        // `item_row`'s is -- and a one-line subject (an item with no login)
        // stays centred rather than riding up to the tile's top edge.
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = MODAL_SUBJECT_GAP_Y;
            ui.add(egui::Label::new(semibold(name, MODAL_SUBJECT_NAME_PX).color(INK)).truncate());
            // Truncated and not wrapped, and skipped entirely when there is
            // none: a card is a fixed width, and a second line that wrapped
            // would push the sentence under it -- the sentence that says what
            // the button does -- down by a line for some items and not others.
            if !username.is_empty() {
                ui.add(
                    egui::Label::new(
                        RichText::new(username)
                            .size(MODAL_SUBJECT_USERNAME_PX)
                            .color(TEXT_FAINT),
                    )
                    .truncate(),
                );
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The two scroll bars in this app are ONE scroll bar, field by field,
    /// and there is no longer an exception.**
    ///
    /// The report this began as: "make scroll same small as result have".
    /// egui's own default `ScrollStyle` is `floating()`, whose `bar_width` is
    /// 10.0 -- so the sidebar rail, which set nothing, drew a 10pt bar beside
    /// an item list whose `scrollbar_in_gutter` drew a 6pt one. Two sizes, a
    /// few hundred points apart.
    ///
    /// Fixing the WIDTH did not settle it: "Sidebar scroll bar - doesn't match
    /// the results", on the build that had the width fix in it. So this test
    /// stopped being about `bar_width` and became about the whole struct.
    ///
    /// **And the second report was not about the struct at all**, which is why
    /// this test's shape changed too. It used to compare
    /// [`scrollbar_in_gutter`] against a second helper, `floating_scrollbar`,
    /// which set the two widths and deliberately reserved NO lane -- the rail
    /// could not afford one while every row inset in it was pinned against the
    /// panel's width. That is exactly what a reader was seeing: the rail's bar
    /// painted on its rows (x=196..202 in a 212pt panel) against the list's
    /// beside its tiles (x=384..390 on a 390pt pane). The fix gave the rail a
    /// lane out of its panel frame's right margin, `floating_scrollbar` lost
    /// its only caller, and it was deleted rather than left as a helper nobody
    /// calls -- which would have made every line below a comparison against
    /// something no pane on screen uses.
    ///
    /// So the two `Ui`s here are both [`scrollbar_in_gutter`], each with **its
    /// real caller's gutter**: `sidebar::PANEL_PAD_X` and
    /// `item_list::LIST_PADDING`, read from those modules rather than written
    /// out again. Every field must agree, the lane included, so the next
    /// divergence -- in an opacity, in a margin, in the handle's minimum
    /// length, or in one pane's padding drifting from the other's -- fails
    /// here instead of being reported from a screenshot.
    ///
    /// **Exhaustively destructured on purpose.** There is no `..` in the
    /// pattern below, so a field added to egui's `ScrollStyle` in a future
    /// version breaks this file's compile rather than quietly joining the set
    /// of things nobody compared. That is the whole reason the comparison is
    /// written out field by field instead of as one `assert_eq!` on the two
    /// structs, which would have been shorter and would have said nothing
    /// about a new field either way.
    ///
    /// # Controls
    ///
    /// Two, because "every field agrees" is a claim two calls that both did
    /// NOTHING would also satisfy. egui's untouched default is read from the
    /// same `Ui` and required to differ from the pair on both widths -- so a
    /// day when `SCROLLBAR_WIDTH` is edited to 10, or when the helper stops
    /// writing, is a day this test is red.
    #[test]
    fn the_two_panes_scroll_bars_are_one_bar_field_for_field() {
        // The REAL gutters, from the two modules that pass them, so this
        // cannot agree with itself while the panes disagree on screen. They
        // are the same number today -- design 4.8 gives the sidebar panel and
        // the item list the same 10pt padding -- and the assertion on the lane
        // below is what says so.
        let rail_gutter = f32::from(crate::vault_window::sidebar::PANEL_PAD_X);
        let list_gutter = crate::vault_window::item_list::LIST_PADDING;
        let ctx = egui::Context::default();
        let mut untouched = None;
        let mut list = None;
        let mut rail = None;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            untouched = Some(ui.spacing().scroll);
            // Each in its own scope, so neither call is reading the other's
            // leftovers: `ui.scope` hands the closure a child `Ui` with a
            // cloned style, and the parent's is put back afterwards.
            ui.scope(|ui| {
                scrollbar_in_gutter(ui, list_gutter);
                list = Some(ui.spacing().scroll);
            });
            ui.scope(|ui| {
                scrollbar_in_gutter(ui, rail_gutter);
                rail = Some(ui.spacing().scroll);
            });
        });
        let untouched = untouched.expect("the ui ran");
        let list = list.expect("the ui ran");
        let egui::style::ScrollStyle {
            floating,
            content_margin,
            bar_width,
            handle_min_length,
            bar_inner_margin,
            bar_outer_margin,
            floating_width,
            floating_allocated_width,
            foreground_color,
            dormant_background_opacity,
            active_background_opacity,
            interact_background_opacity,
            dormant_handle_opacity,
            active_handle_opacity,
            interact_handle_opacity,
            fade,
        } = rail.expect("the ui ran");

        // The bar's own shape and colour. A difference in any of these is a
        // difference a reader can see with both panes on screen at once,
        // which is the report.
        assert_eq!(floating, list.floating, "one bar floats and the other takes space");
        assert_eq!(
            content_margin, list.content_margin,
            "the two areas inset their content differently"
        );
        assert_eq!(
            bar_width, list.bar_width,
            "the two bars are different widths -- the original report"
        );
        assert_eq!(
            floating_width, list.floating_width,
            "the two bars are different widths while the pointer is away from them"
        );
        assert_eq!(
            bar_width, SCROLLBAR_WIDTH,
            "the pair agree, but on a width this app does not use"
        );
        assert_eq!(
            floating_width, SCROLLBAR_WIDTH,
            "a bar that grows under the pointer is a second size, not one"
        );
        assert_eq!(
            handle_min_length, list.handle_min_length,
            "a short rail and a short list would stop at different handle lengths"
        );
        assert_eq!(
            bar_inner_margin, list.bar_inner_margin,
            "different gaps between bar and content"
        );
        assert_eq!(
            bar_outer_margin, list.bar_outer_margin,
            "the two bars stand off their pane's outer edge by different amounts"
        );
        assert_eq!(
            foreground_color, list.foreground_color,
            "one handle is drawn in the text colour and the other in a widget fill"
        );

        // All six opacities, and not just the pair that happens to matter on
        // a given frame: egui picks one of the three pairs per frame from how
        // close the pointer is (dormant / pointer-in-the-area / pointer-on-
        // the-bar), so a divergence in any of them is a divergence a reader
        // reaches by moving the mouse. See [`hide_scrollbar`], which zeroes
        // the same six for the same reason.
        let paler = "the rail's bar and the list's are different strengths of the same colour";
        assert_eq!(
            dormant_background_opacity, list.dormant_background_opacity,
            "{paler}, dormant track"
        );
        assert_eq!(
            active_background_opacity, list.active_background_opacity,
            "{paler}, hovered track"
        );
        assert_eq!(
            interact_background_opacity, list.interact_background_opacity,
            "{paler}, grabbed track"
        );
        assert_eq!(dormant_handle_opacity, list.dormant_handle_opacity, "{paler}, dormant handle");
        assert_eq!(active_handle_opacity, list.active_handle_opacity, "{paler}, hovered handle");
        assert_eq!(interact_handle_opacity, list.interact_handle_opacity, "{paler}, grabbed handle");

        // The content fade at the scroll area's own ends, which is not the
        // bar but is the other thing scrolling looks like.
        assert_eq!(fade, list.fade, "the two panes fade their scrolled content differently");

        // THE LANE, which used to be the one field allowed to differ and is
        // now the last one to join. Both panes reserve their own padding, both
        // paddings are design 4.8's 10pt, so both bars stand the same
        // `gutter - SCROLLBAR_WIDTH` clear of their content and the same 0
        // from their pane's outer edge. If a future design really does give
        // the two panes different paddings, this is the line to change -- and
        // changing it is a decision about what the reader sees, not a
        // formality, which is why it fails rather than being skipped.
        assert_eq!(
            floating_allocated_width, list.floating_allocated_width,
            "the rail's lane is {floating_allocated_width}pt and the item list's is {}pt, so \
             the two bars stand off their content by different amounts",
            list.floating_allocated_width
        );
        assert_eq!(
            floating_allocated_width, rail_gutter,
            "`scrollbar_in_gutter` stopped reserving its caller's gutter, so the rail's bar is \
             back over its rows -- the second report"
        );
        assert_eq!(
            list.floating_allocated_width, list_gutter,
            "`scrollbar_in_gutter` stopped reserving its caller's gutter, so the item list's \
             bar is back over its tiles"
        );
        assert!(
            rail_gutter > SCROLLBAR_WIDTH,
            "control: the rail's {rail_gutter}pt gutter does not even fit the {SCROLLBAR_WIDTH}pt \
             bar, so there is no lane to be flush to the outer edge of"
        );

        assert!(
            untouched.bar_width > SCROLLBAR_WIDTH,
            "control: egui's default bar is already {SCROLLBAR_WIDTH}pt wide, so the helper \
             changes nothing and the two panes agreed all along"
        );
        assert!(
            untouched.floating_width < SCROLLBAR_WIDTH,
            "control: egui's default dormant bar is already {SCROLLBAR_WIDTH}pt wide, so the \
             helper's `floating_width` line is doing nothing"
        );
    }

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

    // -- the multiple-choice row ------------------------------------------

    /// One frame of a [`segmented_control`], read back the way every other
    /// widget in this crate is: real shapes off a real frame, rather than a
    /// claim about the code that emitted them.
    #[derive(Default)]
    struct PaintedRun {
        /// The cells, left to right -- everything the run painted at
        /// [`SEGMENT_HEIGHT`], which excludes the panel background the test
        /// context paints behind it.
        cells: Vec<egui::epaint::RectShape>,
        /// Each cell's label and the colour it was painted in, in the same
        /// order.
        labels: Vec<(String, Color32)>,
        /// What the control reported this frame.
        pressed: Option<usize>,
    }

    impl PaintedRun {
        fn label(&self, needle: &str) -> Color32 {
            self.labels
                .iter()
                .find(|(text, _)| text == needle)
                .unwrap_or_else(|| panic!("{needle:?} was never painted; got {:?}", self.labels))
                .1
        }

        /// The cell behind `needle`, located by the label inside it rather
        /// than by index, so an assertion about a cell names the cell.
        fn cell(&self, needle: &str) -> egui::epaint::RectShape {
            let at = self
                .labels
                .iter()
                .position(|(text, _)| text == needle)
                .unwrap_or_else(|| panic!("{needle:?} was never painted; got {:?}", self.labels));
            self.cells[at].clone()
        }
    }

    /// The cells this crate's own pickers are built from, at their real
    /// widths: two long names that are nothing like the same length, which
    /// is the case per-cell sizing exists for.
    fn backend_cells(official: bool) -> [Segment<'static>; 2] {
        [
            Segment { label: "The official Bitwarden CLI", selected: official },
            Segment { label: "Deskwarden's built-in client", selected: !official },
        ]
    }

    /// One frame of `segments` on `ctx`, with `events` delivered to it.
    fn run_frame(
        ctx: &egui::Context,
        segments: &[Segment<'_>],
        events: Vec<egui::Event>,
        live: bool,
    ) -> PaintedRun {
        let mut painted = PaintedRun::default();
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 400.0))),
                events,
                ..Default::default()
            },
            |ui| {
                if live {
                    painted.pressed = segmented_control(ui, segments);
                } else {
                    segmented_control_disabled(ui, segments);
                }
            },
        );
        for clipped in &output.shapes {
            collect_run(&clipped.shape, &mut painted);
        }
        painted
    }

    fn collect_run(shape: &egui::Shape, painted: &mut PaintedRun) {
        match shape {
            egui::Shape::Rect(rect)
                if (rect.rect.height() - SEGMENT_HEIGHT).abs() < 0.5 =>
            {
                painted.cells.push(rect.clone());
            }
            egui::Shape::Text(text) => {
                let color = text.override_text_color.unwrap_or_else(|| {
                    text.galley
                        .job
                        .sections
                        .first()
                        .map(|section| section.format.color)
                        .unwrap_or(Color32::TRANSPARENT)
                });
                painted.labels.push((text.galley.text().to_string(), color));
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_run(shape, painted);
                }
            }
            _ => {}
        }
    }

    // Every press test below runs a settling frame first and then the frame
    // carrying the click, rather than going through a helper that does both.
    // That is not repetition for its own sake: egui decides what a pointer is
    // over from the widget rects of the frame BEFORE the one carrying the
    // press -- a control clicked on its very first frame has never been
    // anywhere for the pointer to be over -- and each of those tests wants
    // the settling frame's own painted cells to aim at, so the two frames are
    // written out where the geometry between them is used.

    /// A press at `pos`, in the three events egui reads one as.
    fn click_at(pos: Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    /// **The cells are one pill and not a row of buttons**, which is the
    /// whole of what this control is for. Two things are asserted together
    /// because either alone is satisfiable by the wrong picture: that no
    /// space is left between neighbours, and that they overlap by exactly
    /// [`SEGMENT_SEAM`] so the two 1px strokes at a join land on one pixel
    /// rather than painting a seam twice as heavy as the run's outer edge.
    #[test]
    fn the_cells_of_a_run_are_joined_rather_than_spaced() {
        let ctx = ctx_with_fonts();
        let painted = run_frame(&ctx, &backend_cells(true), Vec::new(), true);
        assert_eq!(painted.cells.len(), 2, "got {:?}", painted.labels);
        let (left, right) = (painted.cells[0].rect, painted.cells[1].rect);
        assert_eq!(
            left.right() - right.left(),
            SEGMENT_SEAM,
            "the cells are {:.1} points apart; a multiple-choice row whose cells do not touch \
             reads as two independent buttons, and one that merely abuts draws its interior \
             edge at twice the weight of its outer one",
            right.left() - left.right()
        );
    }

    /// **Every cell in a run is the same height**, so the pill has one top
    /// edge and one bottom edge rather than a silhouette that steps.
    #[test]
    fn every_cell_in_a_run_shares_one_height() {
        let ctx = ctx_with_fonts();
        let painted = run_frame(&ctx, &backend_cells(true), Vec::new(), true);
        for cell in &painted.cells {
            assert_eq!(cell.rect.height(), SEGMENT_HEIGHT);
            assert_eq!(cell.rect.top(), painted.cells[0].rect.top(), "a cell sits off the run");
        }
    }

    /// **Each cell is measured from its own label**, rather than every cell
    /// taking the widest one's width: "Card" as wide as "The official
    /// Bitwarden CLI" would turn a row of alternatives into a row of mostly
    /// empty boxes.
    #[test]
    fn each_cell_is_sized_to_the_words_actually_in_it() {
        let ctx = ctx_with_fonts();
        let cells = [
            Segment { label: "Card", selected: true },
            Segment { label: "Secure note", selected: false },
        ];
        let painted = run_frame(&ctx, &cells, Vec::new(), true);
        let short = painted.cell("Card").rect.width();
        let long = painted.cell("Secure note").rect.width();
        assert!(
            long > short,
            "the two cells came out {short:.1} and {long:.1} points wide, so the run is not \
             measuring the words in it"
        );
        // And the padding really is around the text rather than the text
        // being cropped to a fixed box: the narrower cell still has the
        // design's breathing room in it.
        assert!(
            short > SEGMENT_PADDING,
            "a cell is narrower than its own padding, so the label has nowhere to sit"
        );
    }

    /// **The rounding belongs to the run.** Only the first cell's left
    /// corners and the last cell's right corners are rounded; the interior
    /// edges are square, which is what makes the joins read as seams in one
    /// control rather than as gaps between three.
    #[test]
    fn only_the_ends_of_a_run_are_rounded() {
        let ctx = ctx_with_fonts();
        let cells = [
            Segment { label: "Below field", selected: true },
            Segment { label: "Above", selected: false },
            Segment { label: "At cursor", selected: false },
        ];
        let painted = run_frame(&ctx, &cells, Vec::new(), true);
        assert_eq!(painted.cells.len(), 3);
        let corners: Vec<CornerRadius> = painted.cells.iter().map(|c| c.corner_radius).collect();
        assert_eq!(corners[0].nw, SEGMENT_RADIUS, "the run's leading edge is not rounded");
        assert_eq!(corners[0].sw, SEGMENT_RADIUS);
        assert_eq!(corners[0].ne, 0, "the first cell is rounded into the second");
        assert_eq!(corners[0].se, 0);
        assert_eq!(
            corners[1],
            CornerRadius::ZERO,
            "a cell in the middle of a run is rounded, so the pill reads as separate buttons"
        );
        assert_eq!(corners[2].ne, SEGMENT_RADIUS, "the run's trailing edge is not rounded");
        assert_eq!(corners[2].se, SEGMENT_RADIUS);
        assert_eq!(corners[2].nw, 0, "the last cell is rounded into the second");
        assert_eq!(corners[2].sw, 0);
    }

    /// A run of one is both ends at once, and rounds all four corners --
    /// otherwise a picker that happened to offer a single answer would paint
    /// a box with two square corners and no explanation.
    #[test]
    fn a_run_of_one_cell_is_rounded_all_the_way_round() {
        let ctx = ctx_with_fonts();
        let painted =
            run_frame(&ctx, &[Segment { label: "Everything", selected: true }], Vec::new(), true);
        assert_eq!(painted.cells.len(), 1);
        assert_eq!(painted.cells[0].corner_radius, CornerRadius::same(SEGMENT_RADIUS));
    }

    /// **The cell in force is [`BLUE`] behind white**, and the rest are the
    /// card's own white behind [`INK`]. This is the difference between a
    /// control that answers the question above it and a row of boxes one of
    /// which is faintly tinted.
    #[test]
    fn the_cell_in_force_is_the_apps_blue_behind_white_text() {
        let ctx = ctx_with_fonts();
        let painted = run_frame(&ctx, &backend_cells(true), Vec::new(), true);
        assert_eq!(painted.cell("The official Bitwarden CLI").fill, BLUE);
        assert_eq!(painted.label("The official Bitwarden CLI"), Color32::WHITE);
        assert_eq!(painted.cell("Deskwarden's built-in client").fill, CARD);
        assert_eq!(painted.label("Deskwarden's built-in client"), INK);
    }

    /// The outline is the run's, in [`BORDER`] -- except where the run is
    /// blue, which wears its own fill so the pill's end does not come out
    /// ringed in grey.
    #[test]
    fn the_run_is_outlined_in_the_border_grey_and_the_blue_cell_in_blue() {
        let ctx = ctx_with_fonts();
        let painted = run_frame(&ctx, &backend_cells(true), Vec::new(), true);
        assert_eq!(painted.cell("The official Bitwarden CLI").stroke.color, BLUE);
        assert_eq!(painted.cell("Deskwarden's built-in client").stroke.color, BORDER);
        for cell in &painted.cells {
            assert_eq!(cell.stroke.width, 1.0, "the outline is not a hairline");
        }
    }

    /// **The selection follows the argument and nothing else** -- a control
    /// that painted the first cell blue whatever it was told would pass every
    /// assertion above.
    #[test]
    fn the_blue_moves_to_whichever_cell_is_in_force() {
        let ctx = ctx_with_fonts();
        let painted = run_frame(&ctx, &backend_cells(false), Vec::new(), true);
        assert_eq!(painted.cell("Deskwarden's built-in client").fill, BLUE);
        assert_eq!(painted.cell("The official Bitwarden CLI").fill, CARD);
    }

    /// **A press reports the cell it landed on, by index.** The whole point
    /// of one control rather than a run of buttons is that the caller is
    /// told which alternative was chosen.
    #[test]
    fn pressing_a_cell_reports_that_cell() {
        let cells = backend_cells(true);
        let ctx = ctx_with_fonts();
        let first = run_frame(&ctx, &cells, Vec::new(), true);
        let target = first.cell("Deskwarden's built-in client").rect.center();
        let pressed = run_frame(&ctx, &cells, click_at(target), true).pressed;
        assert_eq!(pressed, Some(1), "the press was reported as {pressed:?}");
    }

    /// **Pressing the cell already in force is reported too**, rather than
    /// swallowed here. `prefs_ui`'s backend picker turns that press into a
    /// no-op deliberately, so that no confirmation appears for a user who
    /// clicked the client they were already on -- and it can only do that if
    /// it is told the press happened.
    #[test]
    fn pressing_the_cell_already_in_force_is_reported_rather_than_swallowed() {
        let cells = backend_cells(true);
        let ctx = ctx_with_fonts();
        let first = run_frame(&ctx, &cells, Vec::new(), true);
        let target = first.cell("The official Bitwarden CLI").rect.center();
        assert_eq!(run_frame(&ctx, &cells, click_at(target), true).pressed, Some(0));
    }

    /// A frame with no press in it reports none -- the control's answer is
    /// about this frame and does not latch.
    #[test]
    fn a_frame_with_no_press_in_it_reports_nothing() {
        let cells = backend_cells(true);
        let ctx = ctx_with_fonts();
        let first = run_frame(&ctx, &cells, Vec::new(), true);
        let target = first.cell("The official Bitwarden CLI").rect.center();
        assert_eq!(run_frame(&ctx, &cells, click_at(target), true).pressed, Some(0));
        assert_eq!(
            run_frame(&ctx, &cells, Vec::new(), true).pressed,
            None,
            "the press is still being reported a frame later, so a caller would act on it twice"
        );
    }

    /// A press that misses the run reports nothing. Without this, a control
    /// that answered `Some(0)` for every click anywhere would satisfy the
    /// two tests above.
    #[test]
    fn a_press_that_misses_the_run_reports_nothing() {
        let cells = backend_cells(true);
        let ctx = ctx_with_fonts();
        let first = run_frame(&ctx, &cells, Vec::new(), true);
        let below = Pos2::new(first.cells[0].rect.center().x, first.cells[0].rect.bottom() + 40.0);
        assert_eq!(run_frame(&ctx, &cells, click_at(below), true).pressed, None);
    }

    /// **More than one cell may be lit**, because [`Segment::selected`] is a
    /// flag per cell rather than one index for the run. The Local API key
    /// form's access pair is the caller this exists for: read and write are
    /// two halves of one answer, and either, both or neither is a thing a
    /// user can mean.
    #[test]
    fn a_run_can_have_more_than_one_cell_in_force() {
        let ctx = ctx_with_fonts();
        let cells =
            [Segment { label: "Read", selected: true }, Segment { label: "Write", selected: true }];
        let painted = run_frame(&ctx, &cells, Vec::new(), true);
        assert_eq!(painted.cell("Read").fill, BLUE);
        assert_eq!(painted.cell("Write").fill, BLUE);
    }

    /// ...and none at all, which is the state the key form calls "this key
    /// may do nothing" and has to be able to show.
    #[test]
    fn a_run_can_have_no_cell_in_force() {
        let ctx = ctx_with_fonts();
        let cells = [
            Segment { label: "Read", selected: false },
            Segment { label: "Write", selected: false },
        ];
        let painted = run_frame(&ctx, &cells, Vec::new(), true);
        for cell in &painted.cells {
            assert_eq!(cell.fill, CARD, "a cell is lit in a run where nothing was chosen");
        }
    }

    /// **The inert run is the live one's geometry exactly**, so a row that is
    /// ghosted does not change size when whatever ghosted it goes away.
    #[test]
    fn the_disabled_run_is_laid_out_exactly_like_the_live_one() {
        let ctx = ctx_with_fonts();
        let live = run_frame(&ctx, &backend_cells(true), Vec::new(), true);
        let inert = run_frame(&ctx, &backend_cells(true), Vec::new(), false);
        let boxes = |painted: &PaintedRun| -> Vec<Rect> {
            painted.cells.iter().map(|cell| cell.rect).collect()
        };
        assert_eq!(boxes(&live), boxes(&inert));
    }

    /// **Ghosted, the run still says which answer is in force**, in
    /// [`BLUE_WASH`] rather than the live control's [`BLUE`]: a run that
    /// greyed every cell identically would tell the reader they have no
    /// answer at all, when what is true is that they have this one and
    /// cannot change it.
    #[test]
    fn the_disabled_run_keeps_the_answer_visible_and_greys_every_label() {
        let ctx = ctx_with_fonts();
        let painted = run_frame(&ctx, &backend_cells(true), Vec::new(), false);
        assert_eq!(painted.cell("The official Bitwarden CLI").fill, BLUE_WASH);
        assert_eq!(painted.cell("Deskwarden's built-in client").fill, CARD);
        for (label, color) in &painted.labels {
            assert_eq!(color, &TEXT_GHOST, "{label:?} is not painted as disabled");
        }
    }

    /// **Inert, not merely grey.** The disabled run senses hover only, so a
    /// press on it cannot be reported -- there is no return value to ignore
    /// and therefore no way for a caller to act on one by accident.
    ///
    /// Asserted as a source pin because the property is an ABSENCE: there is
    /// nothing the disabled control hands back that a frame could read, so
    /// the only place "it does not sense clicks" is written down is the
    /// `Sense` it allocates itself with.
    #[test]
    fn the_disabled_run_senses_no_click_at_all() {
        let source = include_str!("theme.rs");
        let body = source
            .split("pub fn segmented_control_disabled")
            .nth(1)
            .expect("the disabled run is gone");
        let body = body.split("\r\n}").next().expect("an unterminated function");
        assert!(
            body.contains("Sense::hover()"),
            "the disabled run allocates something other than a hover sense"
        );
        assert!(
            !body.contains("Sense::click()"),
            "the disabled run senses clicks, so a cell a user cannot change is pressable"
        );
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
            // The modal header's triangle joined this list when it stopped
            // being three loose segments. Every pair, and the flap's pair
            // first: three points is the count it would have had if its
            // corners were left sharp, and `icon_probe::envelopes` PANICS on
            // an unfilled three-point path it cannot find a body around.
            (
                WARNING_VERTICES,
                "the warning triangle",
                ENVELOPE_FLAP_VERTICES,
                "the envelope's flap",
            ),
            (
                WARNING_VERTICES,
                "the warning triangle",
                ENVELOPE_VERTICES,
                "the envelope's body",
            ),
            (WARNING_VERTICES, "the warning triangle", FOLDER_VERTICES, "the folder mark"),
            (WARNING_VERTICES, "the warning triangle", EYE_VERTICES, "the eye"),
            (WARNING_VERTICES, "the warning triangle", STAR_VERTICES, "the star"),
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

    /// **The star's OFF state rests at the strip's own resting grey, not a
    /// fainter one.**
    ///
    /// Reported as "star on details page doesn't look as bold as the rest".
    /// Of the strip's five controls, four -- [`kebab_button`],
    /// [`send_record_button`], [`add_totp_button`] and [`close_pane_button`]
    /// -- rest at [`TEXT_SECONDARY`]; only the star rested at the paler
    /// [`TEXT_FAINT`], which is what read as thinner even though every mark
    /// here shares the same [`ICON_STROKE`]/[`STAR_STROKE`] weight -- the
    /// difference was colour, not weight.
    #[test]
    fn the_off_star_rests_at_the_strips_own_grey_not_a_fainter_one() {
        let (_, marks) = control(|ui| star_toggle(ui, false));
        let stars = icon_probe::stars(&egui::Shape::Vec(marks));
        assert_eq!(stars.len(), 1, "star_toggle(false) painted no star");
        assert_eq!(
            stars[0].stroke, TEXT_SECONDARY,
            "the off star rests at {:?}, not the strip's own resting grey \
             ({:?}) that the kebab, envelope, clock and close mark share",
            stars[0].stroke, TEXT_SECONDARY
        );
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
            ("⋮", control(|ui| kebab_button(ui)).1),
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
            // The warning bang's dot joined this list when it stopped being a
            // filled square. It is drawn on a modal rather than on the detail
            // header, so no frame carries it and a kebab at once today -- but
            // that is a fact about where two widgets happen to be used, and
            // this list is about what `icon_probe` can tell apart.
            (WARNING_DOT_RADIUS, "the warning bang's dot"),
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
        let (_, kebab) = control(|ui| kebab_button(ui));

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

#[cfg(test)]
mod modal_card_tests {
    //! Real frames of [`modal_scrim`] and [`modal_card`], read back through
    //! the shapes egui emitted -- the same headless technique
    //! `vault_window::delete_modal` and `vault_window::icon_modal` drive
    //! their own cards with, run here because the card is now one piece of
    //! code and a geometry regression in it is a regression in both of them.
    use super::*;
    use eframe::egui::epaint::RectShape;

    /// The window the card is centred in. The two modals' own harness
    /// constant, so what is measured here is measured where they are drawn.
    const BODY: Vec2 = Vec2::new(900.0, 700.0);
    /// The card's width, and `delete_modal`'s.
    const WIDTH: f32 = 340.0;
    const TITLE: &str = "Delete item";
    const DISMISS: &str = "Cancel";
    const CONFIRM: &str = "Delete";
    const SENTENCE: &str = "It moves to the Trash.";

    /// **Not `-scrim`.** `item_list`'s census walks `src/` for every
    /// `Area::new(egui::Id::new(...))` whose id ends in `-scrim` and demands
    /// that `MODAL_SCRIM_AREAS` name it. A test harness's dimmer is not a
    /// modal this window's arrow keys have to be gated behind, so it
    /// deliberately does not answer to that suffix.
    const SHADE_ID: &str = "theme-modal-test-shade";
    const CARD_ID: &str = "theme-modal-test-card";

    #[derive(Default)]
    struct Painted {
        texts: Vec<(String, Rect)>,
        rects: Vec<RectShape>,
        segments: Vec<[Pos2; 2]>,
        /// The same strokes as [`Self::segments`], carrying the INK as well.
        ///
        /// A second list rather than a widened first one, because the colour
        /// only matters to one question and widening `segments` would have
        /// touched every reader of it. That question is the dismiss ✕ on the
        /// accent band: ghost-grey there is a mark nobody can see, and a test
        /// that only found two diagonals in the right place would pass for a
        /// mark painted in the band's own colour.
        inked_segments: Vec<(Stroke, [Pos2; 2])>,
        /// Closed outlines, kept whole rather than as bounding boxes: the
        /// warning triangle is identified by its point count, which is the
        /// only thing that keeps it out of `icon_probe::envelopes`.
        paths: Vec<egui::epaint::PathShape>,
        circles: Vec<egui::epaint::CircleShape>,
    }

    impl Painted {
        /// The one FULL-WIDTH rectangle filled `fill`, or a failure naming
        /// every fill that WAS painted -- which turns "the band is gone" into
        /// a readable message rather than an index panic.
        ///
        /// The width is part of the question and not a nicety. A band's fill
        /// is not unique on this card: the confirm button is filled in the
        /// same accent as the header it sits under, and the warning glyph's
        /// dot is white, which is exactly [`CARD`]. Only the three bands run
        /// the card's whole width.
        fn band(&self, fill: Color32, what: &str) -> RectShape {
            let found: Vec<&RectShape> = self
                .rects
                .iter()
                .filter(|r| r.fill == fill && (r.rect.width() - (WIDTH + 2.0)).abs() < 0.5)
                .collect();
            assert_eq!(
                found.len(),
                1,
                "expected exactly one full-width {what} filled {fill:?}, found {}; the card \
                 painted {:?}",
                found.len(),
                self.rects.iter().map(|r| (r.fill, r.rect.width())).collect::<Vec<_>>()
            );
            found[0].clone()
        }

        fn rect_of(&self, label: &str) -> Rect {
            self.texts
                .iter()
                .find(|(t, _)| t == label)
                .map(|(_, r)| *r)
                .unwrap_or_else(|| {
                    panic!("the card never painted {label:?}; it painted {:?}", self.texts)
                })
        }

        /// The card itself, which is **two points wider than the width it was
        /// asked for**: `egui::Frame` paints its rectangle expanded by its own
        /// 1px stroke, so the card's rect is the border ring and the three
        /// bands run edge to edge *inside* it. Hence a lookup of its own
        /// rather than one more [`band`](Self::band).
        fn card(&self) -> RectShape {
            self.rects
                .iter()
                .find(|r| r.fill == CARD && (r.rect.width() - (WIDTH + 2.0)).abs() < 0.5)
                .cloned()
                .expect("the card's own rectangle was never painted")
        }

        /// The outlined answer, found by the one stroke colour
        /// [`secondary_button`] wears and nothing else on this card does.
        fn outlined_answer(&self) -> Rect {
            self.rects
                .iter()
                .find(|r| r.stroke.color == BORDER_STRONG)
                .map(|r| r.rect)
                .expect("the outlined answer was never painted")
        }
    }

    fn walk(shape: &egui::Shape, painted: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => painted.texts.push((
                text.galley.text().to_string(),
                Rect::from_min_size(text.pos, text.galley.size()),
            )),
            egui::Shape::Rect(rect) => painted.rects.push(rect.clone()),
            egui::Shape::LineSegment { points, stroke } => {
                painted.segments.push(*points);
                painted.inked_segments.push(((*stroke).into(), *points));
            }
            egui::Shape::Path(path) => painted.paths.push(path.clone()),
            egui::Shape::Circle(circle) => painted.circles.push(*circle),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, painted);
                }
            }
            _ => {}
        }
    }

    /// **`time` is what makes every colour assertion in this module mean
    /// anything.**
    ///
    /// An `egui::Area` fades itself in over `Style::animation_time`, and the
    /// fade is applied as an opacity over every shape the layer emits -- so a
    /// frame taken while it is a quarter of the way in reports the card's
    /// white as a premultiplied mid-grey, its red header as a dark brown, and
    /// the scrim at a third of its alpha. Nothing about that is visible to a
    /// test that only reads strings and rectangles, which is why the two
    /// modals' own harnesses never noticed. A headless context's clock does
    /// not advance on its own, so the frames below hand it one.
    fn raw_input(events: &[egui::Event], time: f64) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, BODY)),
            time: Some(time),
            events: events.to_vec(),
            ..Default::default()
        }
    }

    /// One context, its clock, and the card it is drawing.
    struct Harness {
        ctx: egui::Context,
        accent: Color32,
        glyph: ModalGlyph,
        /// Seconds on the context's clock, a tenth of a second per frame --
        /// comfortably more than the default `animation_time`, so a card is
        /// fully faded in one frame after it appears.
        clock: std::cell::Cell<f64>,
    }

    impl Harness {
        /// A styled context with the card already up and fully opaque.
        ///
        /// The two throwaway frames before `apply` are the ones every other
        /// harness in this crate runs: a font set registered during a frame is
        /// only usable from the start of the next. The frames after it are the
        /// `Area`'s own -- one sizing pass, which tessellates to nothing, then
        /// one to appear on and one for the fade to finish.
        fn opened(accent: Color32, glyph: ModalGlyph) -> (Self, Drawn) {
            let ctx = egui::Context::default();
            let harness =
                Harness { ctx, accent, glyph, clock: std::cell::Cell::new(0.0) };
            let _ = harness.ctx.run_ui(raw_input(&[], harness.tick()), |_ui| {});
            apply(&harness.ctx);
            let _ = harness.ctx.run_ui(raw_input(&[], harness.tick()), |_ui| {});

            let sizing = harness.frame(&[]);
            assert!(
                sizing.painted.rects.is_empty(),
                "the sizing pass painted after all; the frame counts in these tests may be off \
                 by one"
            );
            let _ = harness.frame(&[]);
            let drawn = harness.frame(&[]);
            assert!(!drawn.painted.rects.is_empty(), "the card painted nothing at all");
            (harness, drawn)
        }

        fn tick(&self) -> f64 {
            self.clock.set(self.clock.get() + 0.1);
            self.clock.get()
        }
    }

    struct Drawn {
        press: ModalPress,
        /// Where the filled answer landed, taken from its own `Response`
        /// rather than guessed from a fill colour -- this is the rect the
        /// caller's button really occupies.
        confirm: Rect,
        painted: Painted,
    }

    impl Harness {
        /// One frame of the scrim and the card, with `events` delivered to it.
        fn frame(&self, events: &[egui::Event]) -> Drawn {
            let confirm = std::cell::Cell::new(Rect::NOTHING);
            let mut press = ModalPress::default();
            let (accent, glyph) = (self.accent, self.glyph);
            let output = self.ctx.run_ui(raw_input(events, self.tick()), |ui| {
                let ctx = ui.ctx();
                modal_scrim(ctx, egui::Area::new(egui::Id::new(SHADE_ID)));
                press = modal_card(
                    ctx,
                    egui::Area::new(egui::Id::new(CARD_ID)),
                    ModalCard { accent, glyph, title: TITLE, width: WIDTH, dismiss: DISMISS },
                    |ui| {
                        ui.add(
                            egui::Label::new(RichText::new(SENTENCE).size(12.0).color(TEXT_MUTED))
                                .wrap(),
                        );
                    },
                    |ui| {
                        let response = destructive_button(ui, CONFIRM);
                        confirm.set(response.rect);
                        response
                    },
                );
            });
            let mut painted = Painted::default();
            for clipped in &output.shapes {
                walk(&clipped.shape, &mut painted);
            }
            Drawn { press, confirm: confirm.get(), painted }
        }
    }

    fn click(pos: Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    /// **Three bands, rounded as one piece.** The whole point of the shape:
    /// the accent runs the full width with only its top corners rounded, the
    /// footer only its bottom pair, and the card behind them carries the
    /// radius on all four. Get any of those wrong and the card reads as three
    /// cards in a pile.
    /// **The border is painted ABOVE the bands, so it runs all the way
    /// round.**
    ///
    /// Reported: "the red frame should go all the way around and not stop at
    /// the bottom." The bands are painted out over the border so that none of
    /// the card's fill shows past them, and an `egui::Frame` strokes BEFORE
    /// its contents -- so the footer, whose tint is not the accent, painted
    /// the border out along the bottom edge and the two lower corners. The
    /// header did the same at the top and nobody could tell, because its fill
    /// is the accent.
    ///
    /// Asserted as paint ORDER rather than as "a stroke of the accent
    /// exists": the `Frame`'s own stroke has always existed and was always
    /// the accent, and it was covered. What is actually required is that a
    /// stroke of the card's colour, on the card's rect, comes after every
    /// band -- and the last index wins in a painter's list.
    #[test]
    fn the_border_is_drawn_over_the_bands_and_not_under_them() {
        let (_harness, drawn) = Harness::opened(ERROR, ModalGlyph::Warning);
        let card = drawn.painted.card();

        let last_band = drawn
            .painted
            .rects
            .iter()
            .rposition(|r| {
                (r.rect.width() - card.rect.width()).abs() < 0.5
                    && (r.fill == ERROR || r.fill == CARD_TINT)
            })
            .expect("neither band was painted");
        let border = drawn
            .painted
            .rects
            .iter()
            .rposition(|r| {
                r.stroke.color == ERROR
                    && (r.rect.width() - card.rect.width()).abs() < 0.5
                    && (r.rect.height() - card.rect.height()).abs() < 0.5
            })
            .expect("no stroke of the accent covers the whole card");

        assert!(
            border > last_band,
            "the card's border is painted at {border} and a band at {last_band}, so the band \
             covers it -- the frame stops wherever that band's fill is not the accent"
        );
    }

    /// **The bands cover the card's fill, edge to edge, so nothing shows past
    /// them.**
    ///
    /// Reported twice. First as "there is some white line along the curve",
    /// and again after a fix that only made it uniform.
    ///
    /// `egui::Frame` paints its rectangle expanded by its stroke and strokes
    /// it down the middle, so with a 1pt border the card's fill runs to the
    /// rect's edge and the border covers only the outer HALF of that last
    /// point. A band laid out inside the frame stops a full point short, and
    /// what was left was half a point of the card's fill, uncovered, all the
    /// way round -- invisible while that fill was white behind a pale border,
    /// and a white line the moment the border took the accent.
    ///
    /// The first fix shrank the bands' radius so their arcs were concentric
    /// with the card's. That was true and was not the problem: concentric
    /// arcs one point apart still leave the gap, just evenly. The measurement
    /// that settled it came off a real frame -- card fill at
    /// `[279,274]-[621,426]`, band at `[280,275]-[620,315]`.
    ///
    /// Asserted as the bleed's arithmetic rather than by sampling pixels: the
    /// gap is sub-pixel and anti-aliased, so a colour probe at a corner reads
    /// a blend either way and would pass against the bug.
    #[test]
    fn the_bands_are_painted_out_to_the_cards_own_edge() {
        assert_eq!(
            MODAL_BAND_BLEED, MODAL_STROKE,
            "the bands bleed by something other than the border they have to cover, so a              fraction of the card's fill is left showing past them"
        );
        // A bleed of zero is what the bug WAS, and it would satisfy an
        // equality against a border of zero -- so the border is asserted to
        // exist as well.
        assert!(MODAL_STROKE > 0.0, "the card has no border for the bands to cover");
    }

    /// **And the geometry the arithmetic describes**: the band really does
    /// start at the card's own edge, not one border inside it.
    ///
    /// The constant above is a claim about two numbers; without this they
    /// could agree with each other while the band was painted somewhere else
    /// entirely. This test asserted the opposite until the white line was
    /// tracked down -- it required the inset that WAS the bug.
    #[test]
    fn the_bands_start_at_the_cards_own_edge() {
        let (_harness, drawn) = Harness::opened(ERROR, ModalGlyph::Warning);
        let card = drawn.painted.card();
        let header = drawn.painted.band(ERROR, "header band");
        assert!(
            (header.rect.left() - card.rect.left()).abs() < 0.5,
            "the header starts {} from the card's edge, so the fill shows down its side",
            header.rect.left() - card.rect.left()
        );
        assert!(
            (header.rect.top() - card.rect.top()).abs() < 0.5,
            "the header starts {} below the card's top, so the fill shows above it",
            header.rect.top() - card.rect.top()
        );
    }

    #[test]
    fn the_card_is_three_bands_rounded_as_one_piece() {
        let (_harness, drawn) = Harness::opened(ERROR, ModalGlyph::Warning);
        let card = drawn.painted.card();
        let header = drawn.painted.band(ERROR, "header band");
        let footer = drawn.painted.band(CARD_TINT, "footer band");

        assert_eq!(card.corner_radius, CornerRadius::same(MODAL_RADIUS));
        assert!(
            (card.rect.width() - (WIDTH + 2.0)).abs() < 0.5,
            "the card is {} wide against the {WIDTH} it was asked for plus its 1px border on \
             each side",
            card.rect.width()
        );

        // The card's own radius, because the bands sit ON the card's rect now
        // rather than inside it, and two rounded rectangles sharing an edge
        // share a radius. A first attempt at the white line made these
        // `MODAL_RADIUS - 1`, on the reasoning that arcs one point apart are
        // concentric and so cannot cross. True, and not the problem: a gap
        // that is even is still a gap.
        assert_eq!(
            header.corner_radius,
            CornerRadius { nw: MODAL_RADIUS, ne: MODAL_RADIUS, sw: 0, se: 0 },
            "the header band's bottom corners are rounded, so the body begins under a curve"
        );
        assert_eq!(
            footer.corner_radius,
            CornerRadius { nw: 0, ne: 0, sw: MODAL_RADIUS, se: MODAL_RADIUS },
            "the footer band's top corners are rounded, so it reads as a card of its own"
        );

        // **Out to the card's own edge, not inside its border ring.** They
        // used to stop one point short, which left half a point of the card's
        // fill showing past them once the border took the accent -- see
        // `MODAL_BAND_BLEED`. They cover it now, and the border is drawn over
        // them.
        for (band, name) in [(&header, "header"), (&footer, "footer")] {
            assert!(
                (band.rect.width() - card.rect.width()).abs() < 0.5,
                "the {name} band is {} wide against a card {} wide, so the card's fill shows \
                 past it",
                band.rect.width(),
                card.rect.width()
            );
        }
        assert!(
            (header.rect.top() - card.rect.top()).abs() < 0.5,
            "the header starts {} below the card's top, so the fill shows above it",
            header.rect.top() - card.rect.top()
        );
        assert!(
            (footer.rect.bottom() - card.rect.bottom()).abs() < 0.5,
            "the footer stops {} short of the card's bottom edge",
            card.rect.bottom() - footer.rect.bottom()
        );

        // The body is the gap between them, and it is a real band rather than
        // a seam: the sentence is painted in it.
        assert!(header.rect.bottom() < footer.rect.top());
        let sentence = drawn.painted.rect_of(SENTENCE);
        assert!(
            sentence.top() >= header.rect.bottom() && sentence.bottom() <= footer.rect.top(),
            "the body's sentence at {sentence:?} is not between the two bands"
        );

        // And the hairline sits directly on top of the footer, which is the
        // send preflight's own footer rule reproduced rather than reinvented.
        let rule = drawn
            .painted
            .rects
            .iter()
            .find(|r| r.fill == HAIRLINE && r.rect.height() < 1.5)
            .cloned()
            .expect("no hairline above the footer band");
        assert!(
            (rule.rect.bottom() - footer.rect.top()).abs() < 0.5,
            "the rule at {:?} does not meet the footer at {}",
            rule.rect,
            footer.rect.top()
        );

        // The title is IN the accent, not floating above or below it.
        let title = drawn.painted.rect_of(TITLE);
        assert!(
            header.rect.contains_rect(title),
            "the header's title at {title:?} is outside its band at {:?}",
            header.rect
        );
    }

    /// **The two answers split the row between the card's own margins.**
    /// Half each is what makes the footer read as a choice rather than as one
    /// button with something small beside it; an `egui::Button` left to
    /// itself is as wide as its words, and "Cancel" next to "Delete forever"
    /// is exactly the lopsided pair that produces.
    #[test]
    fn the_two_answers_split_the_row_between_the_cards_margins() {
        let (_harness, drawn) = Harness::opened(ERROR, ModalGlyph::Warning);
        // The footer band's own edges, which are the card's inside edges --
        // the card's rect is the border ring around them.
        // **The card's CONTENT edge, not the footer band's.** The band is
        // painted out past it, over the border, so that nothing of the card's
        // fill shows past it -- see `MODAL_BAND_BLEED`. The margins the
        // answers are laid out against are the content's, which is the band's
        // rect shrunk back by that same bleed.
        let inside = drawn.painted.band(CARD_TINT, "footer band").rect.shrink(MODAL_BAND_BLEED);
        let dismiss = drawn.painted.outlined_answer();
        let confirm = drawn.confirm;

        assert!(
            (dismiss.width() - confirm.width()).abs() < 1.0,
            "the answers are {} and {} wide, so the row is not an even split",
            dismiss.width(),
            confirm.width()
        );
        assert!(
            (dismiss.height() - BUTTON_HEIGHT).abs() < 0.5
                && (confirm.height() - BUTTON_HEIGHT).abs() < 0.5,
            "the answers are {} and {} tall, not the app's {BUTTON_HEIGHT}",
            dismiss.height(),
            confirm.height()
        );
        assert!(
            dismiss.left() < confirm.left(),
            "the outlined answer is not the left-hand one"
        );
        assert!(
            (dismiss.left() - (inside.left() + f32::from(MODAL_PAD_X))).abs() < 0.5,
            "the left answer starts at {} against a card margin of {}",
            dismiss.left(),
            inside.left() + f32::from(MODAL_PAD_X)
        );
        assert!(
            (confirm.right() - (inside.right() - f32::from(MODAL_PAD_X))).abs() < 0.5,
            "the right answer ends at {} against a card margin of {}",
            confirm.right(),
            inside.right() - f32::from(MODAL_PAD_X)
        );
        assert!(
            (confirm.left() - dismiss.right() - MODAL_FOOTER_GAP).abs() < 1.0,
            "the gap between the answers is {}, not {MODAL_FOOTER_GAP}",
            confirm.left() - dismiss.right()
        );
    }

    /// **Each answer reports itself and only itself.** Clicked at the
    /// coordinates the card really painted, so this fails if either button
    /// stops being drawn, stops being hit-testable, or starts reporting the
    /// other one's answer -- which on the delete confirmation is the mistake
    /// that makes Cancel delete.
    #[test]
    fn each_answer_reports_only_itself() {
        let (harness, drawn) = Harness::opened(ERROR, ModalGlyph::Warning);
        let at = drawn.painted.outlined_answer().center();
        let left = harness.frame(&click(at));
        assert_eq!(
            left.press,
            ModalPress { dismissed: true, confirmed: false },
            "clicking the outlined answer reported {:?}",
            left.press
        );

        let (harness, drawn) = Harness::opened(ERROR, ModalGlyph::Warning);
        let right = harness.frame(&click(drawn.confirm.center()));
        assert_eq!(
            right.press,
            ModalPress { dismissed: false, confirmed: true },
            "clicking the filled answer reported {:?}",
            right.press
        );

        // The control: a frame with no pointer in it answers neither, so
        // nothing above passes against a card that reports on every frame.
        let (_harness, idle) = Harness::opened(ERROR, ModalGlyph::Warning);
        assert_eq!(idle.press, ModalPress::default());
    }

    /// **The scrim covers the window and dims it.** It is what stops a click
    /// aimed past the card from reaching the vault behind it, and an area
    /// that allocates nothing has a near-zero stored rect and catches nothing
    /// at all -- so the painted region and the blocked region are asserted to
    /// be the same rectangle.
    #[test]
    fn the_scrim_covers_the_whole_window_and_dims_it() {
        let (harness, drawn) = Harness::opened(ERROR, ModalGlyph::Warning);
        let ctx = &harness.ctx;
        let screen = Rect::from_min_size(Pos2::ZERO, BODY);
        let shade = drawn
            .painted
            .rects
            .iter()
            .find(|r| r.fill == Color32::from_black_alpha(MODAL_SCRIM_ALPHA))
            .cloned()
            .expect("nothing on the frame is painted in the scrim's dim");
        assert_eq!(shade.rect, screen, "the scrim does not cover the whole window");

        // And the BLOCK is the same rectangle as the paint. An `Area` that
        // allocated nothing looks identical on screen and catches the pointer
        // nowhere, so the corners -- the furthest a click can land from the
        // card -- are asked who would receive it.
        let shade_layer = egui::LayerId::new(SCRIM_ORDER, egui::Id::new(SHADE_ID));
        assert!(ctx.memory(|m| m.areas().is_visible(&shade_layer)));
        for corner in [
            screen.min + Vec2::splat(2.0),
            Pos2::new(screen.max.x - 2.0, screen.min.y + 2.0),
            screen.max - Vec2::splat(2.0),
            Pos2::new(screen.min.x + 2.0, screen.max.y - 2.0),
        ] {
            assert_eq!(
                ctx.memory(|m| m.layer_id_at(corner)),
                Some(shade_layer),
                "a click at {corner:?} reaches past the scrim to whatever is behind it"
            );
        }
    }

    /// **The warning triangle is drawn, and an ordinary header has no mark at
    /// all.** U+26A0 is not in this app's face -- the same measurement
    /// `close_glyph` records for U+2715 -- so drawn it must be; and the slot
    /// stays empty on a card that is asking rather than refusing, because a
    /// symbol invented to fill it would mean nothing in particular.
    ///
    /// **This used to count four line segments and look for a small filled
    /// rectangle**, which is what the mark was when it was three loose sides
    /// and a square dot. Both moved, and neither loosened: the triangle is
    /// now one closed [`WARNING_VERTICES`]-point path (asserted on the count,
    /// because the count is what keeps it out of `icon_probe::envelopes`) and
    /// the dot is a circle at [`WARNING_DOT_RADIUS`] (asserted on the radius,
    /// because the radius is what keeps it out of `icon_probe::kebab_dots`).
    /// The bang's bar is the one segment left.
    #[test]
    fn the_warning_glyph_is_a_drawn_triangle_and_an_ordinary_header_has_none() {
        let (_harness, warned) = Harness::opened(ERROR, ModalGlyph::Warning);
        let band = warned.painted.band(ERROR, "header band").rect;

        let triangles: Vec<&egui::epaint::PathShape> = warned
            .painted
            .paths
            .iter()
            .filter(|p| band.contains_rect(p.visual_bounding_rect()))
            .collect();
        assert_eq!(
            triangles.len(),
            1,
            "the header band carries {} closed outlines, not the one triangle",
            triangles.len()
        );
        let triangle = triangles[0];
        assert!(triangle.closed, "the warning triangle is an open path, so its apex is a gap");
        assert_eq!(
            triangle.points.len(),
            WARNING_VERTICES,
            "the triangle closes over {} points; at {ENVELOPE_FLAP_VERTICES} it is findable \
             as an envelope flap, and `icon_probe::envelopes` panics on a flap with no body",
            triangle.points.len()
        );
        assert_eq!(
            triangle.fill,
            Color32::TRANSPARENT,
            "the triangle is filled, so it is an outline no longer"
        );

        // Equilateral, which is the design's word for it and the one thing
        // the shape this replaced got wrong -- that one was as tall as the
        // square box it was handed.
        let ink = triangle.visual_bounding_rect();
        let stroke_slop = WARNING_STROKE + 0.5;
        assert!(
            (ink.height() - ink.width() * WARNING_EQUILATERAL).abs() < stroke_slop,
            "the triangle's ink is {}x{}; equilateral at that width is {} tall",
            ink.width(),
            ink.height(),
            ink.width() * WARNING_EQUILATERAL
        );

        // The bang: one vertical bar and one dot, both inside the triangle.
        //
        // **Filtered to the VERTICAL segments**, which it did not have to be
        // until the band grew a dismiss ✕ at its far end. That mark is two
        // crossing diagonals and they are the only other segments the band
        // carries, so "vertical" separates the bang's bar from them exactly
        // and without naming a count that the next thing added to the band
        // would break again. `the_header_band_carries_a_dismiss_mark` is the
        // one that asserts about those two.
        let bars: Vec<&[Pos2; 2]> = warned
            .painted
            .segments
            .iter()
            .filter(|[a, b]| {
                band.contains(*a) && band.contains(*b) && (a.x - b.x).abs() < 0.01
            })
            .collect();
        assert_eq!(
            bars.len(),
            1,
            "the band carries {} vertical line segments; the bang's bar is the only one, now \
             that the triangle is a path and the ✕ is two diagonals",
            bars.len()
        );
        assert!(
            (bars[0][0].x - bars[0][1].x).abs() < 0.01,
            "the bang's bar is not vertical: {:?}",
            bars[0]
        );
        let dots: Vec<&egui::epaint::CircleShape> = warned
            .painted
            .circles
            .iter()
            .filter(|c| band.contains(c.center))
            .collect();
        assert_eq!(dots.len(), 1, "the warning glyph has a bar with no dot under it");
        assert!(
            (dots[0].radius - WARNING_DOT_RADIUS).abs() < 0.01,
            "the bang's dot is radius {}, not {WARNING_DOT_RADIUS} -- at {KEBAB_DOT_RADIUS} it \
             would be counted by every kebab assertion in `detail.rs`",
            dots[0].radius
        );
        assert!(
            dots[0].center.y > bars[0][0].y.max(bars[0][1].y),
            "the dot is not below the bar, so the bang reads upside down"
        );

        assert_eq!(
            warned.painted.texts.iter().filter(|(_, r)| band.contains_rect(*r)).count(),
            1,
            "the header paints something besides its title, so the glyph is being typed"
        );

        let (_plain_harness, plain) = Harness::opened(BLUE, ModalGlyph::None);
        let band = plain.painted.band(BLUE, "header band").rect;
        // **Nothing UPRIGHT**, rather than nothing at all. The dismiss ✕ is on
        // every header now, glyph or no glyph, and it is two diagonals; the
        // bang this is looking for is a vertical bar. Counting strokes would
        // have made this test a second assertion about the mark, which
        // `the_header_band_carries_a_dismiss_mark` already owns.
        assert!(
            plain.painted.segments.iter().all(|[a, b]| {
                !band.contains(*a) || !band.contains(*b) || (a.x - b.x).abs() > 0.01
            }),
            "the glyph-less header drew the warning's upright bar anyway"
        );
        assert!(
            plain.painted.paths.is_empty() && plain.painted.circles.is_empty(),
            "the glyph-less header drew an outline or a dot anyway"
        );
        assert!(
            // From the band's CONTENT edge: it is painted out over the
            // border (see `MODAL_BAND_BLEED`), and the title is laid out
            // inside it.
            (plain.painted.rect_of(TITLE).left()
                - (band.left() + MODAL_BAND_BLEED + f32::from(MODAL_PAD_X)))
            .abs()
                < 1.0,
            "the title on a glyph-less header is indented as if a glyph were there"
        );
    }

    /// **The card's outline is the accent, on every card and not only the red
    /// one.** The report was "red modal should have red frame"; the rule the
    /// shape carries is that the frame is the card's own edge in the card's
    /// own colour, so the blue card gets a blue one -- see
    /// [`ModalCard::accent`] for why the alternative is two rules where the
    /// shape has one. Both accents are driven, because "the red card's frame
    /// is red" alone passes just as well against a frame hard-coded to
    /// [`ERROR`].
    #[test]
    fn the_cards_outline_carries_the_accent_whatever_the_accent_is() {
        for (accent, name) in [(ERROR, "the destructive card"), (BLUE, "the ordinary card")] {
            let (_harness, drawn) = Harness::opened(accent, ModalGlyph::None);
            let card = drawn.painted.card();
            assert_eq!(
                card.stroke.color, accent,
                "{name}'s outline is {:?}, not its own accent {accent:?}",
                card.stroke.color
            );
            assert!(
                card.stroke.width > 0.0,
                "{name} has an accent-coloured outline of zero width, which is no outline"
            );
            // And the outline really is the card's, not the header band's:
            // the band it continues starts inside it.
            let band = drawn.painted.band(accent, "header band").rect;
            assert!(
                card.rect.contains_rect(band),
                "{name}'s outline at {:?} does not enclose its header band at {band:?}",
                card.rect
            );
        }
    }

    /// **The subject line is the item's own row: its tile, its name, and its
    /// username under the name.** Both cards that use the frame are about one
    /// vault item, and the row is what the reader recognised in the list a
    /// moment ago -- a name on its own asks them to match it against a row
    /// they can no longer see.
    ///
    /// **This used to assert a monogram tile**, which is what the line drew
    /// before the tile became the caller's to draw. The monogram is now one
    /// of three things that can go in the slot (favicon, kind mark, initials)
    /// and which one is `vault_window::ModalSubject`'s decision, so what is
    /// pinned here is the slot's size and the two lines beside it; the branch
    /// itself is pinned in `delete_modal`.
    #[test]
    fn the_subject_line_is_the_items_row() {
        // A panel rather than the card, because the subject line is a
        // stand-alone widget: what is under test is the tile and the two
        // lines, not where in a modal they land.
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw_input(&[], 0.1), |_ui| {});
        apply(&ctx);
        let _ = ctx.run_ui(raw_input(&[], 0.2), |_ui| {});
        let mut painted = Painted::default();
        let asked = std::cell::Cell::new(0.0_f32);
        let output = ctx.run_ui(raw_input(&[], 0.3), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.set_width(300.0);
                modal_subject(ui, "Ledgerline Bank", "a.novak@ledgerline.com", |ui, size| {
                    asked.set(size);
                    // The monogram tile stands in for whatever the caller
                    // draws: it allocates the same square every branch of
                    // `ModalSubject` allocates.
                    avatar(ui, &initials("Ledgerline Bank"), size, false);
                });
            });
        });
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        assert!(
            (asked.get() - MODAL_SUBJECT_TILE).abs() < f32::EPSILON,
            "the tile closure was handed {}, not the line's own {MODAL_SUBJECT_TILE}",
            asked.get()
        );
        let tile = painted.rect_of("LB");
        let name = painted.rect_of("Ledgerline Bank");
        let username = painted.rect_of("a.novak@ledgerline.com");
        assert!(
            tile.right() <= name.left(),
            "the tile at {tile:?} is not in front of the name at {name:?}"
        );
        assert!(
            username.top() >= name.top(),
            "the username at {username:?} is not under the name at {name:?}"
        );
        assert!(
            (username.left() - name.left()).abs() < 1.0,
            "the two lines start at {} and {}, so they are not one column",
            name.left(),
            username.left()
        );
        assert!(
            painted.rects.iter().any(|r| {
                (r.rect.width() - r.rect.height()).abs() < 0.5
                    && (r.rect.width() - MODAL_SUBJECT_TILE).abs() < 0.5
            }),
            "there is no square tile beside the name; it painted {:?}",
            painted.rects.iter().map(|r| r.rect).collect::<Vec<_>>()
        );

        // An item with no login has nothing to put on the second line, and
        // draws no second line rather than an empty one -- a blank row would
        // push the sentence under it down by a line for some items only.
        let mut alone = Painted::default();
        let output = ctx.run_ui(raw_input(&[], 0.4), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.set_width(300.0);
                modal_subject(ui, "Recovery codes", "", |ui, size| {
                    avatar(ui, &initials("Recovery codes"), size, false);
                });
            });
        });
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut alone);
        }
        assert_eq!(
            alone.texts.iter().filter(|(t, _)| t != "RC").count(),
            1,
            "a subject with no username painted more than its name: {:?}",
            alone.texts
        );
    }

    // -----------------------------------------------------------------------
    // The dismiss ✕ on the header band
    //
    // This card is drawn by `delete_modal` and `icon_modal`, and neither file
    // mentions a ✕: the mark is on the frame, so both got it in one edit and
    // neither can lose it separately. That is exactly why the assertions have
    // to live here, and why they are in two halves -- a mark that PAINTS and
    // does not answer is the defect this app has shipped before, and a mark
    // that answers while painted in the band's own colour is one nobody can
    // find.
    // -----------------------------------------------------------------------

    /// The two arms of the dismiss ✕ inside `band`, as
    /// `(stroke, [start, end])` each.
    ///
    /// Found by geometry and nothing else: a diagonal run inside the header.
    /// The only other strokes this card ever paints in that band are the
    /// warning bang's upright bar, which is not diagonal.
    fn dismiss_arms(painted: &Painted, band: Rect) -> Vec<(Stroke, [Pos2; 2])> {
        painted
            .inked_segments
            .iter()
            .filter(|(_, [a, b])| {
                band.contains(*a)
                    && band.contains(*b)
                    && (b.x - a.x).abs() > 0.5
                    && ((b.x - a.x).abs() - (b.y - a.y).abs()).abs() < 0.01
            })
            .cloned()
            .collect()
    }

    /// **The mark is DRAWN: two crossing arms, the right size, in a white the
    /// accent cannot swallow, at the card's own corner inset.**
    ///
    /// Every clause is a way the mark has failed elsewhere in this app. A
    /// single arm is half a ✕. An arm at the wrong length is a mark that does
    /// not match the six others. [`TEXT_GHOST`] on a red band is a mark that
    /// is technically present and practically invisible, which is what
    /// [`CloseInk`] exists to prevent. And an inset taken from the content
    /// column rather than from the card is how `prefs_ui` and `totp_add` ended
    /// up four points apart before this family existed.
    #[test]
    fn the_header_band_carries_a_dismiss_mark() {
        let (_harness, drawn) = Harness::opened(ERROR, ModalGlyph::Warning);
        let band = drawn.painted.band(ERROR, "header band").rect;
        let arms = dismiss_arms(&drawn.painted, band);
        assert_eq!(
            arms.len(),
            2,
            "the header band carries {} diagonal strokes; a ✕ is exactly two",
            arms.len()
        );

        for (stroke, [a, b]) in &arms {
            assert!(
                ((b.x - a.x).abs() - CLOSE_MARK_SPAN).abs() < 0.01,
                "an arm spans {} points across; `CLOSE_MARK_SPAN` makes it {CLOSE_MARK_SPAN}",
                (b.x - a.x).abs()
            );
            assert_eq!(
                stroke.color,
                CLOSE_MARK_ON_ACCENT,
                "the mark is painted {:?} on an {ERROR:?} band. Ghost-grey is the ink for a \
                 WHITE header; on the accent it is a mark the user cannot see",
                stroke.color
            );
        }

        // The two arms cross, so this is a ✕ and not two parallel slashes, and
        // the crossing point is where the click below has to land.
        let centre = arms[0].1[0].lerp(arms[0].1[1], 0.5);
        assert!(
            (arms[1].1[0].lerp(arms[1].1[1], 0.5) - centre).length() < 0.01,
            "the two arms have different midpoints, so they do not cross"
        );

        // And the mark is `MODAL_CLOSE_INSET` off the CARD's edge, measured
        // through the hit box the arms sit in the middle of.
        let card = drawn.painted.card().rect;
        let box_right = centre.x + CLOSE_MARK_HIT / 2.0;
        assert!(
            (card.right() - MODAL_STROKE - box_right - MODAL_CLOSE_INSET).abs() < 0.5,
            "the mark's hit box ends {} points inside the card, not `MODAL_CLOSE_INSET`'s {}",
            card.right() - MODAL_STROKE - box_right,
            MODAL_CLOSE_INSET
        );
        assert!(
            (centre.y - band.center().y).abs() < 1.0,
            "the mark is not centred on the band it sits in"
        );
    }

    /// **And pressing it DISMISSES -- as the footer's left-hand answer, not as
    /// a third outcome.**
    ///
    /// The half that matters, and the half that a paint test alone cannot
    /// reach: a keycap in this app was drawn and dead for weeks. It also pins
    /// which answer the mark gives, which is the whole of why `delete_modal`
    /// needed no edit: [`ModalPress::dismissed`] is that card's Cancel, so the
    /// mark on the destructive card cannot be a delete.
    ///
    /// Driven against the DESTRUCTIVE card deliberately -- if the wiring were
    /// ever crossed, this is the card where it would cost something.
    #[test]
    fn pressing_the_header_mark_dismisses_and_never_confirms() {
        let (harness, drawn) = Harness::opened(ERROR, ModalGlyph::Warning);
        let band = drawn.painted.band(ERROR, "header band").rect;
        let arms = dismiss_arms(&drawn.painted, band);
        assert_eq!(arms.len(), 2, "no mark was painted, so pressing it proves nothing");
        let at = arms[0].1[0].lerp(arms[0].1[1], 0.5);

        // The frame before carried no press, so what comes back is this click
        // and not a flag that was already set.
        let idle = harness.frame(&[]);
        assert_eq!(idle.press, ModalPress::default(), "the card reported a press with no input");

        let pressed = harness.frame(&click(at));
        assert!(
            pressed.press.dismissed,
            "the header's ✕ painted at {at:?} reported nothing -- either the drag handle over \
             the band swallowed the click, or the mark is drawn and dead"
        );
        assert!(
            !pressed.press.confirmed,
            "the header's ✕ reported the CONFIRM, which on `delete_modal` is the delete"
        );
    }

    /// **The mark is on the card even when the card has no glyph**, which is
    /// `icon_modal`'s shape. A version of this that only put the ✕ beside the
    /// warning triangle would have left the app's ordinary question card
    /// exactly as undismissable as it was.
    #[test]
    fn the_ordinary_card_carries_the_mark_too() {
        let (_harness, drawn) = Harness::opened(BLUE, ModalGlyph::None);
        let band = drawn.painted.band(BLUE, "header band").rect;
        assert_eq!(
            dismiss_arms(&drawn.painted, band).len(),
            2,
            "the glyph-less card's header carries no ✕"
        );
    }
}

#[cfg(test)]
mod one_border_is_one_line_tests {
    /// **A rectangle stroked twice must be stroked the same way twice.**
    ///
    /// Two cards in this app paint one border twice -- `theme::modal_card` and
    /// `totp_add`'s `stage_card` -- because a `Frame`'s stroke is drawn under
    /// its contents and a band that bleeds to the edge covers it. Painting the
    /// same border again over the top is the fix, and it is only a fix while
    /// the two strokes land on the same pixels.
    ///
    /// They did not, in both cards, for the same reason: a `Frame` draws its
    /// stroke `Inside` the rect and `StrokeKind::Middle` centres it ON the
    /// edge, half a stroke-width further out. The owner saw it twice and named
    /// it the same way both times -- "two lines next to each other like last
    /// time on the right edge", and then "double red line to the right - same
    /// as before".
    ///
    /// So the rule is pinned where it can only be broken deliberately: every
    /// repaint of a `Frame`'s own rect is `Inside`. This counts them off the
    /// source across the whole crate, because the next card to do this will be
    /// written in a third file.
    ///
    /// **`Middle` elsewhere is untouched and correct.** A lone stroke has
    /// nothing to disagree with; straddling the edge is what it is for. This
    /// asserts only about rects that are stroked on top of a `Frame` that
    /// already stroked them, which is what `framed.response.rect` spells.
    #[test]
    fn a_border_painted_twice_is_painted_the_same_way_twice() {
        let files = [
            ("theme.rs", include_str!("theme.rs")),
            ("vault_window/totp_add.rs", include_str!("vault_window/totp_add.rs")),
            ("vault_window/detail.rs", include_str!("vault_window/detail.rs")),
            ("vault_window/record_ui.rs", include_str!("vault_window/record_ui.rs")),
            ("vault_window/send_ui.rs", include_str!("vault_window/send_ui.rs")),
            ("vault_window/item_list.rs", include_str!("vault_window/item_list.rs")),
        ];
        let needle = concat!("framed.response.", "rect,");
        let mut found = 0usize;
        for (name, text) in files {
            let text = text.replace("\r\n", "\n");
            for (at, _) in text.match_indices(needle) {
                // The `StrokeKind` of this call, which is inside the next few
                // lines: a `rect_stroke` takes rect, rounding, stroke, kind.
                let tail = &text[at..(at + 400).min(text.len())];
                let Some(kind_at) = tail.find("StrokeKind::") else { continue };
                found += 1;
                let kind = &tail[kind_at..];
                assert!(
                    kind.starts_with("StrokeKind::Inside"),
                    "{name} repaints a `Frame`'s own rect with something other than \
                     `StrokeKind::Inside`, so the two strokes of one border land half a \
                     stroke-width apart and the card is outlined twice: {}",
                    &kind[..kind.len().min(40)]
                );
            }
        }
        assert!(
            found >= 2,
            "the two cards that repaint their own border were not found, so this test is \
             asserting about nothing"
        );
    }
}

#[cfg(test)]
mod form_card_padding_tests {
    use super::*;

    /// **The two answers, and the measured threshold between them.**
    ///
    /// 5a is a 690-point card and 6c a 470-point one, and both say
    /// `padding: ... 18px`; the Sends screen's detail column is 250 points of
    /// card at the app's minimum window size, where 18 a side is 14% of it.
    /// So the rule is a fact about the card's width, and this pins both ends
    /// of it against the numbers the design and the window really are.
    #[test]
    fn a_form_card_takes_the_designs_padding_at_the_width_the_design_draws_it() {
        // 5a's card, and 6c's -- the two the design draws at 18.
        assert_eq!(form_card_pad_x(690.0), FORM_CARD_PAD_X_WIDE);
        assert_eq!(form_card_pad_x(470.0), FORM_CARD_PAD_X_WIDE);
        // The Sends screen's detail column at `MIN_VAULT_WINDOW_SIZE`: 298
        // points of column, 250 of card.
        assert_eq!(form_card_pad_x(250.0), FORM_CARD_PAD_X);
        // And the threshold is a threshold rather than a range: one point
        // under it is the narrow answer.
        assert_eq!(form_card_pad_x(FORM_CARD_WIDE_AT - 1.0), FORM_CARD_PAD_X);
        assert!(
            FORM_CARD_PAD_X_WIDE > FORM_CARD_PAD_X,
            "the wide answer is meant to be the roomier one"
        );
    }

    /// **Every band of the card asks the RULE**, and none of them still holds
    /// the narrow constant.
    ///
    /// The four bands -- header, body, footer and the caution strip -- are
    /// four separate `inner_margin` calls, and a card whose header indented
    /// 18 while its body indented 12 would be a worse defect than the one
    /// this rule fixes. Counted off the source because that is the only way
    /// to see all four at once.
    #[test]
    fn all_four_bands_ask_the_rule_rather_than_the_narrow_constant() {
        let source = include_str!("theme.rs");
        let asks = concat!("form_card_pad_x(ui.", "available_width())");
        assert_eq!(
            source.matches(asks).count(),
            4,
            "expected all four of the card's bands to ask {asks:?} -- header, body, footer \
             and the caution strip. One that did not would indent differently from the \
             other three on the same card"
        );
        let hard_coded = concat!("Margin::symmetric(FORM_CARD_PAD_X", ",");
        assert_eq!(
            source.matches(hard_coded).count(),
            0,
            "a band is still padded with the narrow constant directly ({hard_coded:?}), so it \
             stays at 12 on a card the design draws at 18"
        );
    }

    /// **§8a's card has the same two answers, at its own measured threshold.**
    ///
    /// The shipped detail pane at the 1240-point window is 638 points -- a
    /// 628-point card -- and §8a's narrowest card is 507; the pane at
    /// `settings::MIN_VAULT_WINDOW_SIZE` is 298, a 288-point card. Both ends
    /// are pinned against the widths the window really has, and the
    /// threshold against the design's own arithmetic.
    #[test]
    fn a_section_card_takes_8as_padding_at_the_width_8a_draws_it() {
        assert_eq!(section_card_pad_x(628.0), SECTION_CARD_PAD_X_WIDE);
        // (1028 - 14) / 2: one half of §8a's two-column grid, the narrowest
        // card the design draws at 16.
        assert_eq!(section_card_pad_x(507.0), SECTION_CARD_PAD_X_WIDE);
        assert_eq!(section_card_pad_x(SECTION_CARD_WIDE_AT - 1.0), SECTION_CARD_PAD_X);
        assert_eq!(section_card_pad_x(288.0), SECTION_CARD_PAD_X);
        assert_eq!(SECTION_CARD_PAD_X_WIDE, 16, "§8a's `padding: 11px 16px`");
        assert!(
            SECTION_CARD_PAD_X_WIDE > SECTION_CARD_PAD_X,
            "the wide answer is meant to be the roomier one"
        );
    }
}

#[cfg(test)]
mod modal_drag_tests {
    //! Real frames of a movable modal, driven by a real pointer: where the
    //! card opens, what moves it, what does not, how far it may go, and what
    //! is left of that the next time it opens.
    //!
    //! Driven through [`movable_modal`] and [`modal_drag_handle`] rather than
    //! through [`modal_card`], because what is under test is the WINDOW
    //! BEHAVIOUR every modal in this app now shares, and a card of its own
    //! keeps these assertions from turning into assertions about the delete
    //! confirmation's padding. `modal_card` itself is checked once at the
    //! bottom, so the wiring between the two is not taken on trust.
    use super::*;

    /// The window these frames run in.
    const WINDOW: Vec2 = Vec2::new(900.0, 700.0);
    /// The test card. `delete_modal`'s width, and a height that leaves plenty
    /// of slack in both directions so a clamp that fired early would show.
    const CARD: Vec2 = Vec2::new(340.0, 240.0);
    /// The card's grabbable strip, and the height of the click target drawn
    /// inside it -- the ✕ `prefs_ui` and `totp_add` keep there.
    const HEADER: f32 = MODAL_HEADER_HEIGHT;
    const CARD_ID: &str = "theme-modal-drag-test-card";

    /// One window, drawing one modal, across as many frames as a test needs.
    struct Window {
        ctx: egui::Context,
        /// Resizable, because a card that fitted in a large window and no
        /// longer fits in a small one is half of what the clamp is for.
        size: std::cell::Cell<Vec2>,
        /// Whether the modal is drawn at all. A closed modal is a modal that
        /// draws nothing, which is the only signal the offset has to reset on.
        open: std::cell::Cell<bool>,
        /// Frames the header's own click target reported a click on.
        marked: std::cell::Cell<u32>,
    }

    impl Window {
        /// A window with the modal open and settled: the sizing pass egui
        /// spends measuring an anchored area, then two live frames, so
        /// everything below is asserting about a card that is really on
        /// screen.
        fn opened() -> Self {
            let window = Window {
                ctx: egui::Context::default(),
                size: std::cell::Cell::new(WINDOW),
                open: std::cell::Cell::new(true),
                marked: std::cell::Cell::new(0),
            };
            for _ in 0..3 {
                window.frame(&[]);
            }
            assert_eq!(
                window.card().size(),
                CARD,
                "the card never reached its real size, so every position below is measured \
                 against the wrong rectangle"
            );
            window
        }

        fn frame(&self, events: &[egui::Event]) {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size.get())),
                events: events.to_vec(),
                ..Default::default()
            };
            let _ = self.ctx.run_ui(input, |ui| {
                if !self.open.get() {
                    return;
                }
                let ctx = ui.ctx();
                movable_modal(ctx, egui::Area::new(egui::Id::new(CARD_ID))).show(ctx, |ui| {
                    // First, exactly as every converted call site calls it.
                    modal_drag_handle(ui, HEADER);
                    let (card, _) = ui.allocate_exact_size(CARD, Sense::hover());
                    // A click target in the header, at the right-hand end and
                    // the size of the real thing, where `prefs_ui` and
                    // `totp_add` keep their ✕ -- registered AFTER the handle,
                    // which is the whole of why it still works. Its geometry
                    // is `prefs_ui::tests::close_rect`'s.
                    if ui
                        .interact(self.mark(card), egui::Id::new("header-mark"), Sense::click())
                        .clicked()
                    {
                        self.marked.set(self.marked.get() + 1);
                    }
                    // And something in the BODY that wants drags of its own,
                    // the way a text field or a scrolling band does.
                    let body = Rect::from_min_max(
                        Pos2::new(card.min.x, card.min.y + HEADER),
                        card.max,
                    );
                    ui.interact(body, egui::Id::new("body-field"), Sense::click_and_drag());
                });
            });
        }

        /// Where the card is right now.
        fn card(&self) -> Rect {
            egui::AreaState::load(&self.ctx, egui::Id::new(CARD_ID))
                .expect("the modal has never been drawn")
                .rect()
        }

        /// Where a card that has not been moved sits: the middle of the
        /// window, which is what the anchor these cards used to carry meant.
        fn centre(&self) -> Rect {
            Rect::from_center_size(
                Rect::from_min_size(Pos2::ZERO, self.size.get()).center(),
                CARD,
            )
        }

        /// Press at `from`, move to `to`, release. Five frames because that is
        /// what a real drag is: egui hit-tests a press against the widget
        /// rects of the PREVIOUS frame, the move is a frame of its own, and
        /// the offset it accumulates is read by the anchor on the frame after
        /// that.
        fn drag(&self, from: Pos2, to: Pos2) {
            self.frame(&[egui::Event::PointerMoved(from)]);
            self.frame(&[egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }]);
            self.frame(&[egui::Event::PointerMoved(to)]);
            self.frame(&[egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            self.frame(&[]);
        }

        /// The middle of the card's grabbable strip, well clear of the ✕ that
        /// shares the band with it.
        fn header_grip(&self) -> Pos2 {
            let card = self.card();
            Pos2::new(card.center().x, card.min.y + HEADER / 2.0)
        }

        /// Where the header's own click target sits on a card: the right-hand
        /// end of the band, at [`modal_dismiss_mark`]'s geometry exactly.
        ///
        /// **Derived from the shipped constants rather than from the literal
        /// `card.max.x - 22.0` that stood here.** The two agreed -- 22 is
        /// `MODAL_CLOSE_INSET + CLOSE_MARK_HIT / 2` -- and agreeing by
        /// coincidence is the arrangement this whole pass exists to end: a
        /// stand-in that stopped matching the real mark would leave this test
        /// green while proving nothing about it.
        fn mark(&self, card: Rect) -> Rect {
            Rect::from_center_size(
                Pos2::new(
                    card.max.x - MODAL_CLOSE_INSET - CLOSE_MARK_HIT / 2.0,
                    card.min.y + HEADER / 2.0,
                ),
                Vec2::splat(CLOSE_MARK_HIT),
            )
        }
    }

    // -----------------------------------------------------------------------
    // Where it opens
    // -----------------------------------------------------------------------

    /// **The requirement that everything else is measured against.** These
    /// cards were anchored `CENTER_CENTER`; making them movable must not have
    /// moved them. If this fails, every paint test in `delete_modal`,
    /// `icon_modal`, `prefs_ui` and this file is asserting about a card that
    /// has quietly drifted.
    #[test]
    fn a_modal_opens_exactly_where_the_anchor_used_to_put_it() {
        let window = Window::opened();
        assert_eq!(window.card(), window.centre());
    }

    // -----------------------------------------------------------------------
    // What moves it, and what does not
    // -----------------------------------------------------------------------

    #[test]
    fn dragging_the_header_moves_the_card_by_what_the_pointer_moved() {
        let window = Window::opened();
        let from = window.header_grip();
        window.drag(from, from + Vec2::new(120.0, -80.0));
        assert_eq!(
            window.card(),
            window.centre().translate(Vec2::new(120.0, -80.0)),
            "the card did not follow the pointer"
        );
    }

    /// **The body is not a handle.** The cards are full of text fields, tick
    /// lists and scrolling bands, every one of which reads a drag as its own;
    /// a card that moved when any of them was dragged would fight all of them.
    #[test]
    fn dragging_the_body_moves_nothing() {
        let window = Window::opened();
        let at = window.card().center();
        window.drag(at, at + Vec2::new(120.0, 60.0));
        assert_eq!(
            window.card(),
            window.centre(),
            "a drag inside the body moved the card, so every control in it now fights the drag"
        );
    }

    /// **The handle does not eat the header's own controls.** egui hit-tests
    /// clicks and drags separately but not independently: a drag-sensing strip
    /// laid OVER a click-sensing one swallows the click. `prefs_ui` and
    /// `totp_add` both keep their ✕ in the header, so the handle has to be
    /// registered underneath them -- and this is what says it still is.
    /// `prefs_ui::tests::the_header_cross_closes_the_modal` is the same
    /// assertion against the real card.
    #[test]
    fn a_click_in_the_header_still_reaches_the_control_the_handle_lies_under() {
        let window = Window::opened();
        let at = window.mark(window.card()).center();
        window.frame(&[egui::Event::PointerMoved(at)]);
        window.frame(&[egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }]);
        window.frame(&[egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert_eq!(
            window.marked.get(),
            1,
            "the drag handle swallowed the click aimed at the header's own control -- the ✕ on \
             the real cards is now unreachable"
        );
        assert_eq!(window.card(), window.centre(), "a click alone moved the card");
    }

    // -----------------------------------------------------------------------
    // How far it may go
    // -----------------------------------------------------------------------

    #[test]
    fn the_card_cannot_be_dragged_out_of_the_window() {
        let window = Window::opened();
        let from = window.header_grip();
        window.drag(from, from + Vec2::new(5_000.0, 5_000.0));
        let card = window.card();
        let screen = Rect::from_min_size(Pos2::ZERO, WINDOW);
        assert!(
            screen.contains_rect(card),
            "the card left the window: {card:?} is not inside {screen:?}"
        );
        // And it went as far as it is allowed to go, rather than refusing the
        // drag: the far corner, exactly.
        assert_eq!(card.max, screen.max);
    }

    /// **The reason the clamp is applied to the STORED offset and not only to
    /// the drawn position.** egui's own `constrain` already keeps an `Area`
    /// inside the window, so a card shoved at the edge LOOKS right either way.
    /// What it does not do is stop the offset this file remembers from running
    /// away: a drag five thousand points past the edge, un-clamped, is five
    /// thousand points the user has to drag back before the card twitches.
    /// This fails outright without `clamp_modal_offset` writing its answer
    /// back.
    #[test]
    fn a_card_shoved_past_the_edge_comes_back_on_the_very_next_drag() {
        let window = Window::opened();
        let from = window.header_grip();
        window.drag(from, from + Vec2::new(5_000.0, 0.0));
        let parked = window.card();

        let grip = window.header_grip();
        window.drag(grip, grip - Vec2::new(40.0, 0.0));
        assert_eq!(
            window.card(),
            parked.translate(Vec2::new(-40.0, 0.0)),
            "the card did not answer the drag back, so the offset kept counting past the edge"
        );
    }

    /// **The window is resizable and the card has to survive that.** The vault
    /// window goes down to `settings::MIN_VAULT_WINDOW_SIZE`, so a card parked
    /// against the edge of a large window is a card whose offset no longer
    /// fits once the window is dragged smaller. Re-clamping every pass is what
    /// walks it back in rather than letting it slide out with its dismiss
    /// control.
    #[test]
    fn shrinking_the_window_walks_a_parked_card_back_inside_it() {
        let window = Window::opened();
        let from = window.header_grip();
        window.drag(from, from + Vec2::new(5_000.0, 5_000.0));

        let small = Vec2::new(700.0, 480.0);
        window.size.set(small);
        window.frame(&[]);
        window.frame(&[]);
        let card = window.card();
        let screen = Rect::from_min_size(Pos2::ZERO, small);
        assert!(
            screen.contains_rect(card),
            "the card stayed outside the window it was shrunk into: {card:?} vs {screen:?}"
        );
        assert_eq!(card.max, screen.max, "it did not stay in the corner it was parked in");
    }

    // -----------------------------------------------------------------------
    // What is left of it next time
    // -----------------------------------------------------------------------

    /// **Opening is the one moment this app can promise the card is
    /// findable.** A card dragged into a corner and reopened there is a card
    /// the user has to hunt for; see [`modal_offset`] for the full argument.
    #[test]
    fn closing_the_modal_puts_it_back_in_the_middle() {
        let window = Window::opened();
        let from = window.header_grip();
        window.drag(from, from + Vec2::new(150.0, 90.0));
        assert_ne!(window.card(), window.centre(), "the card never moved, so this proves nothing");

        window.open.set(false);
        window.frame(&[]);
        window.open.set(true);
        window.frame(&[]);
        assert_eq!(
            window.card(),
            window.centre(),
            "the modal reopened where it was dragged to rather than in the middle"
        );
    }

    /// The other half of the one above: a modal that is merely STILL OPEN does
    /// not forget. Without this, a reset keyed on something coarser than "was
    /// it drawn on the pass before" would pass the test above by snapping the
    /// card back every frame, which is not a fix, it is a card that cannot be
    /// moved at all.
    #[test]
    fn a_modal_that_stays_open_stays_where_it_was_put() {
        let window = Window::opened();
        let from = window.header_grip();
        window.drag(from, from + Vec2::new(150.0, 90.0));
        let moved = window.card();
        for _ in 0..10 {
            window.frame(&[]);
        }
        assert_eq!(window.card(), moved, "the card crept back to the middle on its own");
    }

    // -----------------------------------------------------------------------
    // The clamp on its own
    // -----------------------------------------------------------------------

    /// The rule stated as arithmetic: half the leftover room, each way.
    #[test]
    fn the_travel_is_half_the_slack_in_each_direction() {
        let window = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0));
        let card = Vec2::new(340.0, 240.0);
        assert_eq!(
            clamp_modal_offset(window, card, Vec2::new(1000.0, 1000.0)),
            Vec2::new(280.0, 230.0)
        );
        assert_eq!(
            clamp_modal_offset(window, card, Vec2::new(-1000.0, -1000.0)),
            Vec2::new(-280.0, -230.0)
        );
        // Inside the range, untouched.
        assert_eq!(
            clamp_modal_offset(window, card, Vec2::new(12.0, -34.0)),
            Vec2::new(12.0, -34.0)
        );
    }

    /// A card too big for the window gets no travel rather than negative
    /// travel: it stays centred, which is both where it opened and the
    /// position that wastes the least of it off either edge.
    #[test]
    fn a_card_larger_than_its_window_is_centred_and_cannot_be_moved() {
        let window = Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 200.0));
        let card = Vec2::new(340.0, 240.0);
        assert_eq!(clamp_modal_offset(window, card, Vec2::new(80.0, -80.0)), Vec2::ZERO);
    }

    /// The clamp is idempotent, which is what makes it safe to run on the
    /// stored value every pass: clamping an already-clamped offset changes
    /// nothing, so a card never drifts by being looked at.
    #[test]
    fn clamping_twice_is_clamping_once() {
        let window = Rect::from_min_size(Pos2::new(-40.0, 17.0), Vec2::new(900.0, 700.0));
        let card = Vec2::new(340.0, 240.0);
        let once = clamp_modal_offset(window, card, Vec2::new(4000.0, -4000.0));
        assert_eq!(clamp_modal_offset(window, card, once), once);
    }

    // -----------------------------------------------------------------------
    // And the real card, once
    // -----------------------------------------------------------------------

    /// [`modal_card`] is the card `delete_modal` and `icon_modal` draw, and
    /// neither of those files says a word about dragging -- they get it
    /// because this function asks for a [`movable_modal`] and lays a
    /// [`modal_drag_handle`] over its own header band. This is the test that
    /// says so, so the day someone rewrites the area line in `modal_card`,
    /// two modals do not silently go back to being pinned.
    #[test]
    fn the_shared_card_is_dragged_by_the_header_band_it_draws_itself() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("theme-modal-card-drag-test");
        let mut clock = 0.0;
        let blank = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, WINDOW)),
            ..Default::default()
        };
        // The two throwaway frames every harness in this crate runs before
        // `apply`: a font set registered during a frame is only usable from
        // the start of the next, and this card's title is set in Archivo.
        let _ = ctx.run_ui(blank(), |_ui| {});
        apply(&ctx);
        let _ = ctx.run_ui(blank(), |_ui| {});

        let mut frame = |events: &[egui::Event]| {
            clock += 0.1;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, WINDOW)),
                time: Some(clock),
                events: events.to_vec(),
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                modal_card(
                    ui.ctx(),
                    egui::Area::new(id),
                    ModalCard {
                        accent: ERROR,
                        glyph: ModalGlyph::Warning,
                        title: "Delete item",
                        width: 340.0,
                        dismiss: "Cancel",
                    },
                    |ui| {
                        ui.label(RichText::new("It moves to the Trash.").size(12.0));
                    },
                    |ui| destructive_button(ui, "Delete"),
                );
            });
        };
        // One sizing pass, which paints nothing, then two live ones.
        for _ in 0..3 {
            frame(&[]);
        }
        let before = egui::AreaState::load(&ctx, id).expect("the card was never drawn").rect();
        let grip = Pos2::new(before.center().x, before.min.y + MODAL_HEADER_HEIGHT / 2.0);
        let to = grip + Vec2::new(90.0, 70.0);
        frame(&[egui::Event::PointerMoved(grip)]);
        frame(&[egui::Event::PointerButton {
            pos: grip,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }]);
        frame(&[egui::Event::PointerMoved(to)]);
        frame(&[egui::Event::PointerButton {
            pos: to,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        frame(&[]);
        let after = egui::AreaState::load(&ctx, id).expect("the card vanished").rect();
        assert_eq!(
            after,
            before.translate(Vec2::new(90.0, 70.0)),
            "`modal_card`'s own header band did not drag the card"
        );
    }
}

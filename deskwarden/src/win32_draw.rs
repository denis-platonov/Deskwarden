//! GDI owner-draw shared by the daemon's windows.
//!
//! **Why this file exists.** A control Windows paints does not match the
//! design: the unlock prompt shipped with a stock grey `Cancel` -- system
//! font, square corners, gradient fill -- beside a correctly drawn `Unlock`.
//! Both the button and the picker's list rows need the same hand-drawn
//! button, so it lives here rather than twice.
//!
//! Every colour and dimension comes from [`crate::theme`], the same module
//! egui reads, so a theme change moves both renderers at once.

use crate::app_candidates::Candidate;
use std::ffi::c_void;
use std::sync::OnceLock;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    AddFontMemResourceEx, CreateFontIndirectW, CreatePen, CreateSolidBrush, DeleteObject,
    DrawTextW, Ellipse, GetCurrentObject, GetObjectW, GetStockObject,
    GetTextExtentPoint32W, HBRUSH, HGDIOBJ, NULL_BRUSH, Polygon, Polyline, RoundRect,
    ScreenToClient, SelectObject, SetBkMode, SetTextCharacterExtra, SetTextColor, DRAW_TEXT_FORMAT,
    DT_CENTER,
    DT_END_ELLIPSIS,
    DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FW_BOLD, FW_NORMAL, HDC, HFONT, LOGFONTW,
    OBJ_FONT, PS_SOLID,
    TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{HTCAPTION, HTCLIENT};

/// `theme`'s `Color32` as GDI's BGR `COLORREF`.
///
/// One conversion, used everywhere, so that no hex value in this file is a
/// palette entry written out a second time. Moved out of `unlock_prompt`'s
/// `win32` module, which had the only copy.
pub(crate) fn rgb(c: eframe::egui::Color32) -> COLORREF {
    COLORREF((c.r() as u32) | ((c.g() as u32) << 8) | ((c.b() as u32) << 16))
}

// ---------------------------------------------------------------------------
// Cyrillic on the GDI cards.
//
// **The defect.** A vault item called "Сбербанк" rendered on one of these
// cards in a visibly different typeface from "Netflix" beside it. The reason
// is not subtle once stated: all four bundled Archivo cuts carry **zero**
// codepoints in U+0400-04FF, so there is nothing for GDI to rasterise. GDI
// does not draw blanks -- it font-links the uncovered run to whatever the
// system offers for that script, which on a stock Windows 11 is Segoe UI. So
// the card drew Latin in Archivo and Cyrillic in Segoe UI, in the same line,
// and the mismatch is exactly what the owner reported.
//
// **egui already solved this, and the fix here is to agree with it.**
// `theme::CYRILLIC_FACES` bundles four Noto Sans *Cyrillic-subset* faces, one
// per Archivo cut, and `theme::font_definitions` puts each one directly behind
// its Archivo cut in that weight's family stack. Every egui surface in the app
// therefore draws Cyrillic in Noto Sans at the weight the design asked for.
// These cards were the only surfaces left with a second answer.
//
// **`lfCharSet` is NOT the cause, and is deliberately left alone.** Every
// `LOGFONTW` on these cards leaves `lfCharSet` at its `Default` of 0, which is
// `ANSI_CHARSET` rather than `DEFAULT_CHARSET` (1), and that looks like a
// smoking gun until the fonts are actually read. The GDI font mapper's charset
// penalty is levied against a face whose OS/2 `ulCodePageRange` does not
// declare the requested code page; all four Archivo cuts and all four Noto
// cuts declare bit 0 (cp1252, Latin-1), so `ANSI_CHARSET` costs nothing and
// the exact `lfFaceName` match wins outright in both cases. Changing it would
// be a real behaviour change for no benefit: `DEFAULT_CHARSET` tells the
// mapper *any* charset is acceptable, which loosens matching on a machine
// whose system locale is not Western and could quietly move a face that
// resolves correctly today. The missing glyphs were the whole defect.
//
// **Why the face names here are not the ones in `theme`.** `theme`'s table
// names the Noto faces `NotoSans-Cyrillic-Regular` and so on -- those are
// egui's `font_data` *keys*, which are arbitrary strings egui never shows to
// anything but itself. GDI matches on the font file's own legacy `name`
// records (IDs 1 and 2), and those say something else entirely. Read out of
// the files: Regular and Bold share the legacy family `Noto Sans` and are told
// apart by weight, while SemiBold and ExtraBold each carry their own legacy
// family and are `Regular` *within* it. That is the same four-styles-per-
// family shape Archivo's cuts have, and for the same reason, and it is why a
// GDI table has to exist separately from the egui one rather than reusing its
// strings. `the_gdi_names_are_the_font_files_own_name_records` reads the
// `name` and `OS/2` tables out of the bundled bytes and pins every row.
// ---------------------------------------------------------------------------

/// The four bundled Noto Sans Cyrillic-subset cuts, as `(theme's egui family
/// name, GDI family name, GDI weight, bytes)` -- the same shape, and the same
/// order, as [`crate::theme::ARCHIVO_FACES`], so a row here lines up with the
/// Archivo row it stands behind.
///
/// **Why this table is in `win32_draw` and not in `theme`.** Two reasons, and
/// the second is the one that settles it. First, `theme` is the design
/// system's look -- what a weight *is* -- while this is how Win32's font
/// mapper is made to resolve it; the crate already splits them that way, with
/// `theme::gdi_face_for` as the seam. Second and decisively, `theme`'s
/// `CYRILLIC_FACES` holds egui keys, not GDI names (see the block comment
/// above), so there is no reading of it that this module could have reused --
/// the GDI names had to be read out of the files whatever module they landed
/// in.
///
/// **The `include_bytes!` is a second reference to the same four files, not a
/// second copy of the design's assets.** `theme`'s table is private to that
/// module, so its bytes cannot be reached from here, and `theme` is owned by
/// another workstream right now. `the_cyrillic_assets_are_the_ones_theme_
/// bundles` pins the four paths against `theme.rs`'s own source, so the two
/// renderers cannot drift onto different files; if `CYRILLIC_FACES` is ever
/// made `pub`, this table should keep its GDI names and take its bytes from
/// there.
const CYRILLIC_GDI_FACES: [(&str, &str, i32, &[u8]); 4] = [
    (
        crate::theme::REGULAR,
        "Noto Sans",
        400,
        include_bytes!("../assets/fonts/NotoSans-Cyrillic-Regular.ttf"),
    ),
    (
        crate::theme::SEMIBOLD,
        "Noto Sans SemiBold",
        400,
        include_bytes!("../assets/fonts/NotoSans-Cyrillic-SemiBold.ttf"),
    ),
    (
        crate::theme::BOLD,
        "Noto Sans",
        700,
        include_bytes!("../assets/fonts/NotoSans-Cyrillic-Bold.ttf"),
    ),
    (
        crate::theme::EXTRABOLD,
        "Noto Sans ExtraBold",
        400,
        include_bytes!("../assets/fonts/NotoSans-Cyrillic-ExtraBold.ttf"),
    ),
];

/// Registers every bundled face -- the four Archivo cuts **and** the four Noto
/// Cyrillic cuts -- privately with GDI, once for the whole process.
///
/// `AddFontMemResourceEx` makes a face available to **this process only**:
/// nothing is installed and nothing touches the user's font list. The handles
/// are deliberately never released, because freeing one while a window still
/// has it selected is how a surface repaints in the fallback face.
///
/// **This is one `OnceLock` where there were seven.** Each GDI card carried
/// its own copy of this loop behind its own `OnceLock`, so a session that
/// opened the picker and then the unlock prompt handed GDI a second private
/// copy of all four Archivo cuts -- and `AddFontMemResourceEx` copies the font
/// data into the process font table, so that is roughly 750 KB per repeat, not
/// a refcount. Doubling the table to eight faces would have doubled the waste
/// as well; instead the registration moved here, the cards call this, and the
/// eight faces are installed exactly once however many cards the session
/// opens. `crate::preflight_card` still registers Archivo itself -- it is
/// owned by another workstream -- which costs one extra copy of the four
/// Archivo cuts and nothing else, because it draws its text through
/// [`draw_text`] like every other card and therefore reaches this function
/// anyway on its first Cyrillic run.
///
/// **A failure here is cosmetic and must stay that way.** These cards are the
/// app's fallback surfaces; a warn line and a card in the shell font is a bad
/// afternoon, a card that refuses to open is a locked-out user. Nothing in
/// this function can fail loudly.
pub fn register_fonts() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        for (_, face, _, bytes) in crate::theme::ARCHIVO_FACES {
            register_one(face, bytes);
        }
        for (_, face, _, bytes) in CYRILLIC_GDI_FACES {
            register_one(face, bytes);
        }
    });
}

/// One `AddFontMemResourceEx`, so [`register_fonts`] does not spell the call
/// out twice and the two tables cannot be registered on subtly different
/// terms.
fn register_one(face: &str, bytes: &'static [u8]) {
    // A `Cell` rather than a `mut` local: GDI writes the count back through a
    // `*const u32`, so a plain immutable binding read afterwards is a value
    // the compiler may fold to its initialiser.
    let installed = std::cell::Cell::new(0u32);
    let handle = unsafe {
        AddFontMemResourceEx(
            bytes.as_ptr() as *const c_void,
            bytes.len() as u32,
            None,
            installed.as_ptr(),
        )
    };
    if handle.0.is_null() || installed.get() == 0 {
        log::warn!("could not register the bundled face {face} with GDI; text set in it will fall back to whatever the system offers");
    }
}

/// Every codepoint the bundled Noto Cyrillic subset can actually draw.
///
/// **This is the whole of it -- 104 usable codepoints.** The subset is 15 KB
/// per weight precisely because it was cut down to Cyrillic and nothing else:
/// `U+0400`-`U+045F` entire, the four Ukrainian/Kazakh letters `Ґґ` and `Ұұ`,
/// the space, the no-break space, the combining acute, and `№`. It carries
/// **no Latin letter, no digit, and no ASCII punctuation at all** -- no comma,
/// no full stop, no hyphen, no parenthesis. (`U+0000` and `U+000D` are in the
/// file's `cmap` too and are left out here on purpose: they are the notdef
/// mapping and a control character, not text, and a run made of them is not a
/// run this face should be chosen for.)
///
/// That emptiness is the entire reason [`gdi_face_for_text`] is conservative
/// rather than clever, and
/// `the_coverage_table_is_the_bundled_subsets_own_cmap` reads the `cmap` out
/// of the shipped bytes and pins this function against it, so a future
/// re-subset cannot silently widen or narrow what the app believes it can
/// draw.
const fn cyrillic_subset_covers(unit: u16) -> bool {
    matches!(
        unit,
        0x0020 | 0x00A0 | 0x0301 | 0x2116 | 0x0400..=0x045F | 0x0490 | 0x0491 | 0x04B0 | 0x04B1
    )
}

/// Whether `unit` is one of the Cyrillic **letters** the subset draws, as
/// opposed to the space, the no-break space or `№`, which it also draws.
///
/// Separate from [`cyrillic_subset_covers`] because "every character is
/// coverable" is not on its own a reason to switch face: a run of two spaces
/// is fully covered and must keep Archivo, or a blank label would change
/// width for no visible reason.
const fn is_cyrillic_letter(unit: u16) -> bool {
    matches!(unit, 0x0400..=0x045F | 0x0490 | 0x0491 | 0x04B0 | 0x04B1)
}

/// **Should this run be drawn in the Cyrillic face?** True only when every
/// code unit is one the subset covers *and* at least one of them is a Cyrillic
/// letter.
///
/// **The mixed-script decision, and the argument for it.** `DrawTextW` takes
/// one font per call, so a per-*script-run* answer would mean splitting the
/// string, measuring each piece with `GetTextExtentPoint32W` and advancing the
/// rect by hand. That is not a tuning knob, it is a different text engine:
/// `DT_END_ELLIPSIS` truncates against the rect it is given and would then be
/// truncating each fragment rather than the line, `DT_CENTER` and `DT_VCENTER`
/// would centre each fragment in the whole rect, and the crate's one-and-only
/// `DrawTextW` -- the pin that exists because an empty run through the raw
/// call kills the daemon in its window procedure with no log line -- would
/// have to become a loop. On the app's crash-fallback surfaces that is a bad
/// trade.
///
/// **So the answer is per string, and it is "all or nothing" rather than
/// "mostly Cyrillic".** The reason is [`cyrillic_subset_covers`]: the bundled
/// face has no Latin, no digits and no punctuation, so choosing it for
/// "Netflix RU — Иван" would put *seventeen* characters into GDI's fallback to
/// rescue four, and "Почта 2" would lose the digit that distinguishes it from
/// "Почта". A rule that fires only on a fully covered run cannot make any
/// string worse than it is today: either every character is drawn in Noto at
/// the right weight, or nothing changes and the run is drawn exactly as it was
/// before this function existed.
///
/// The practical reach is still most of the defect. "Сбербанк", "Почта",
/// "Госуслуги", "Яндекс" -- the single-word item names, usernames and folder
/// names that made the owner's report -- are all fully covered, and so is a
/// multi-word Cyrillic name, because the space is in the subset.
fn run_takes_the_cyrillic_face(units: impl Iterator<Item = u16>) -> bool {
    let mut saw_a_letter = false;
    for unit in units {
        if !cyrillic_subset_covers(unit) {
            return false;
        }
        saw_a_letter |= is_cyrillic_letter(unit);
    }
    saw_a_letter
}

/// **The `(GDI family, GDI weight)` to draw `run` in, at the design weight
/// `family`.** [`crate::theme::gdi_face_for`] with the script taken into
/// account.
///
/// This is the pure, testable core of the fix, and the function to reach for
/// from any surface that builds its own `LOGFONTW` up front rather than
/// letting [`draw_text`] swap for it.
///
/// **It falls back to Archivo rather than failing, twice over**: an unknown
/// `family` and a run this face cannot draw both land on exactly what
/// `theme::gdi_face_for` would have returned. That is the same promise
/// `gdi_face_for`'s own doc makes and for the same reason -- a prompt in the
/// wrong face is a cosmetic defect, and these are the surfaces whose whole
/// reason for existing is that they must open when the heavier machinery
/// cannot.
pub fn gdi_face_for_text(family: &str, run: &str) -> (&'static str, i32) {
    let archivo = crate::theme::gdi_face_for(family);
    if !run_takes_the_cyrillic_face(run.encode_utf16()) {
        return archivo;
    }
    CYRILLIC_GDI_FACES
        .iter()
        .find(|(egui_family, ..)| *egui_family == family)
        .map(|(_, gdi, weight, _)| (*gdi, *weight))
        .unwrap_or(archivo)
}

/// The Cyrillic `(GDI family, GDI weight)` standing behind an **already
/// realised** Archivo `LOGFONTW`, or `None` if that font is not one of ours.
///
/// [`gdi_face_for_text`] answers for a caller that knows which design weight
/// it asked for. This answers for [`draw_text_utf16`], which does not: by the
/// time a run reaches the one `DrawTextW`, all that survives is an `HFONT`
/// already selected into the DC. So the lookup runs backwards, from the GDI
/// name and weight in that font's `LOGFONTW` to the design family, and from
/// there to the paired Noto cut.
///
/// The weight comparison is `>= 700` on both sides rather than equality
/// because the cards create their fonts with `FW_BOLD`/`FW_NORMAL` -- 700 and
/// 400 -- while [`crate::theme::ARCHIVO_FACES`] carries the file's own weight;
/// those agree on which side of bold each cut sits, which is all the legacy
/// four-styles-per-family naming can distinguish anyway. `Consolas`, the
/// stock shell font, and anything else the DC might be carrying match no row
/// and get `None`, which is correct: Consolas covers U+0400-04FF itself, and
/// nothing else here is ours to second-guess.
fn cyrillic_pair_for_realised(face: &str, weight: i32) -> Option<(&'static str, i32)> {
    let family = crate::theme::ARCHIVO_FACES
        .iter()
        .find(|(_, gdi, gdi_weight, _)| {
            gdi.eq_ignore_ascii_case(face) && (*gdi_weight >= 700) == (weight >= 700)
        })
        .map(|(egui_family, ..)| *egui_family)?;
    CYRILLIC_GDI_FACES
        .iter()
        .find(|(egui_family, ..)| *egui_family == family)
        .map(|(_, gdi, gdi_weight, _)| (*gdi, *gdi_weight))
}

/// Swap the DC's font for the paired Cyrillic cut, if this run wants it.
///
/// Returns the font it created and the one it displaced, so the caller can put
/// the DC back exactly as it found it; `None` means nothing was touched and
/// the caller must not restore anything.
///
/// **Every step is allowed to decline.** `GetCurrentObject` returning nothing,
/// `GetObjectW` refusing to fill the `LOGFONTW`, a face that is not one of
/// ours, `CreateFontIndirectW` failing, `SelectObject` failing -- each of them
/// returns `None` and the run is drawn exactly as it would have been before
/// this existed. There is no path through here that can panic and none that
/// can leave the DC in a state the caller did not expect. That is a
/// requirement, not a courtesy: this code runs inside a window procedure on
/// the cards the app opens when nothing else can open, and a fatal exception
/// on that stack is not a panic Rust can catch -- Windows turns it into
/// STATUS_FATAL_USER_CALLBACK_EXCEPTION and takes the process without
/// unwinding.
///
/// **Registration is deliberately lazy and deliberately in here.** A card that
/// only ever draws Latin never pays for the four extra faces, and -- more
/// usefully -- a card that never calls [`register_fonts`] itself still gets
/// them, because every run in the crate comes through this one function.
/// `preflight_card`, which is not this workstream's to edit, is fixed by that
/// alone.
///
/// **Every `LOGFONTW` field except the face name and the weight is inherited**
/// from the font the card built: height, escapement, quality, and -- pointedly
/// -- `lfCharSet`, which the cards leave at `ANSI_CHARSET` and which both
/// families declare in their OS/2 code page ranges. A run that swaps face must
/// not also quietly change size or antialiasing, or the fix would read as a
/// second defect.
unsafe fn select_cyrillic_face(hdc: HDC, chars: &[u16]) -> Option<(HFONT, HGDIOBJ)> {
    if !run_takes_the_cyrillic_face(chars.iter().copied()) {
        return None;
    }
    let current = GetCurrentObject(hdc, OBJ_FONT);
    if current.0.is_null() {
        return None;
    }
    let mut lf = LOGFONTW::default();
    let read = GetObjectW(
        current,
        std::mem::size_of::<LOGFONTW>() as i32,
        Some(&mut lf as *mut LOGFONTW as *mut c_void),
    );
    if read == 0 {
        return None;
    }
    let end = lf.lfFaceName.iter().position(|&ch| ch == 0).unwrap_or(lf.lfFaceName.len());
    let realised = String::from_utf16_lossy(&lf.lfFaceName[..end]);
    let (noto, weight) = cyrillic_pair_for_realised(&realised, lf.lfWeight)?;

    register_fonts();

    lf.lfWeight = if weight >= 700 { FW_BOLD.0 as i32 } else { FW_NORMAL.0 as i32 };
    lf.lfFaceName = [0u16; 32];
    for (i, ch) in noto.encode_utf16().take(31).enumerate() {
        lf.lfFaceName[i] = ch;
    }
    let font = CreateFontIndirectW(&lf);
    if font.0.is_null() {
        return None;
    }
    let previous = SelectObject(hdc, font);
    if previous.0.is_null() {
        let _ = DeleteObject(font);
        return None;
    }
    Some((font, previous))
}

// ---------------------------------------------------------------------------
// The one `DrawTextW` in the crate.
//
// **An empty run is an access violation, and it takes the whole process with
// it.** This is written out once, here, because it has now happened TWICE.
//
// An empty `Vec<u16>` never allocates, so `as_mut_ptr()` hands back Rust's
// dangling sentinel: the type's own alignment, which for `u16` is the literal
// address **2**. `DrawTextW` dereferences that pointer even when it is told
// the length is zero -- `DrawTextExWorker` reads the first character before it
// looks at the count -- so an empty string reaches GDI as a read of address
// 0x2 and faults.
//
// **It kills the daemon rather than the card.** The fault happens inside a
// window procedure, on a stack Windows entered through
// `UserCallWinProcCheckWow`. A structured exception raised there is turned
// into STATUS_FATAL_USER_CALLBACK_EXCEPTION (0xc000041d) and the process is
// terminated WITHOUT unwinding: the panic hook never runs, `catch_unwind`
// never sees it, and nothing at all reaches the log. What the owner sees is
// the tray, the vault window and an unlocked session vanishing at once, with
// an empty log and a Windows Error Reporting entry naming `user32.dll` and
// 0xc0000005 -- which is exactly how both occurrences were found, the second
// one only from a minidump whose `.ecxr; k` showed `rdi = 0x2`.
//
// **Why a function and not a comment.** The first occurrence was fixed at the
// two cards that had met it (`picker_prompt` and `locked_card`), each with its
// own `if run.is_empty() { return; }` under a copy of this explanation. The
// knowledge then lived in prose at two call sites, and seven other cards drew
// text without it -- so the eighth card to be written reintroduced the crash
// verbatim. A comment cannot be a precondition; a function can. Every text run
// this crate paints now goes through [`draw_text`] or [`draw_text_utf16`], and
// the pins in this file's test module assert that no tenth card can call
// `DrawTextW` directly.
// ---------------------------------------------------------------------------

/// **Paint one run of text.** The crate's only route to `DrawTextW`.
///
/// Returns `DrawTextW`'s own answer -- the height of the drawn text in logical
/// units, or, with `DT_CALCRECT`, the height it would take -- and **0 for an
/// empty run**, which is the honest answer for text that occupies no lines. No
/// caller in this crate reads it today and none passes `DT_CALCRECT`; it is
/// returned rather than swallowed so that a caller which one day needs to
/// measure does not have to reach around this function to do it.
///
/// `rect` is `&mut` because `DrawTextW` writes into it under `DT_CALCRECT`.
/// **An empty run leaves it untouched** -- there is no measurement to report --
/// so a future `DT_CALCRECT` caller must treat the rect it passed in as the
/// answer for empty text, exactly as it would for a run of zero lines.
///
/// See the block comment above for why the empty case is a crash rather than a
/// no-op, and why that crash takes the whole daemon down without a log line.
pub fn draw_text(hdc: HDC, text: &str, rect: &mut RECT, format: DRAW_TEXT_FORMAT) -> i32 {
    // BEFORE the encode, so no buffer -- and therefore no pointer -- exists on
    // the empty path at all. Guarding after `collect()` would work too, but
    // this way the thing that must not happen cannot be reached rather than
    // merely being skipped.
    if text.is_empty() {
        return 0;
    }
    let mut chars: Vec<u16> = text.encode_utf16().collect();
    draw_text_utf16(hdc, &mut chars, rect, format)
}

/// [`draw_text`] for a caller that already holds the UTF-16 buffer.
///
/// **This exists for the generated password**, and for nothing else. That run
/// is held as a `Zeroizing<Vec<u16>>` so the buffer GDI rasterises the glyphs
/// out of is wiped when the paint ends rather than left on the stack. Routing
/// it through [`draw_text`] would mean turning it back into a `&str` and
/// re-encoding -- a second, plain `Vec<u16>` copy of the secret with no
/// `Drop` that clears it, sitting in the heap until that page is reused. So
/// the buffer is borrowed here instead and the secret is never copied.
///
/// The slice is `&mut` because `DrawTextW` takes it that way: it may modify
/// the buffer in place (it is the same argument `DT_MODIFYSTRING` writes the
/// ellipsis into).
pub fn draw_text_utf16(
    hdc: HDC,
    chars: &mut [u16],
    rect: &mut RECT,
    format: DRAW_TEXT_FORMAT,
) -> i32 {
    // The guard, again, on the buffer itself: this entry point is reachable
    // without going through `draw_text`, and an empty `Zeroizing<Vec<u16>>` --
    // a generated password of zero length -- has the same dangling `2` as any
    // other empty `Vec<u16>`.
    if chars.is_empty() {
        return 0;
    }
    // The Cyrillic swap, and the reason it lives HERE rather than at the nine
    // cards. This is the one place in the crate where a run of text and the
    // font it is about to be rasterised with are both in hand, so it is the
    // one place a per-run font decision can be made at all -- and making it
    // here means every card is fixed at once, including the ones this
    // workstream does not own and the tenth card nobody has written yet. See
    // `select_cyrillic_face`: it declines unless the run is one the bundled
    // subset can draw whole, and it restores the DC before returning.
    let swapped = unsafe { select_cyrillic_face(hdc, chars) };
    let height = unsafe { DrawTextW(hdc, chars, rect, format) };
    if let Some((cyrillic, previous)) = swapped {
        // Restore first, delete second. Deleting a font that is still selected
        // into a DC is the documented way to leak it: GDI refuses, returns
        // FALSE, and the handle stays in the process table until the daemon
        // exits -- which for a long-running tray app means every repaint of
        // every Cyrillic row costs one handle forever.
        unsafe {
            SelectObject(hdc, previous);
            let _ = DeleteObject(cyrillic);
        }
    }
    height
}

/// How one button is painted. Three colours and a radius, so a new kind of
/// button is a new constructor here rather than new drawing code at a call
/// site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonSkin {
    pub fill: COLORREF,
    pub text: COLORREF,
    pub border: Option<COLORREF>,
    /// The fill to use when the pointer is over the button. Carried as a
    /// field set by each constructor so `hovered()` never needs to know
    /// which kind of skin it was called on.
    hover_fill: COLORREF,
}

impl ButtonSkin {
    /// The blue call-to-action.
    pub fn primary() -> Self {
        Self {
            fill: rgb(crate::theme::BLUE),
            text: rgb(crate::theme::CARD),
            border: None,
            hover_fill: rgb(crate::theme::BLUE_BRIGHT),
        }
    }

    /// The quiet one beside it. **Bordered on purpose**: it is card-coloured
    /// on a card, so without an outline it does not read as a control.
    pub fn secondary() -> Self {
        Self {
            fill: rgb(crate::theme::CARD),
            text: rgb(crate::theme::INK),
            border: Some(rgb(crate::theme::BORDER)),
            hover_fill: rgb(crate::theme::CARD_TINT),
        }
    }

    /// Greyed, derived rather than hand-picked, so a palette change cannot
    /// leave the disabled variant behind.
    pub fn disabled(self) -> Self {
        Self { fill: rgb(crate::theme::TOGGLE_OFF), text: rgb(crate::theme::TEXT_GHOST), ..self }
    }

    /// Hovered, derived the same way `disabled()` is: the fill changes to
    /// whichever hover shade this skin's constructor picked, nothing else.
    /// Symmetric with `disabled()` so neither the primary nor the secondary
    /// button special-cases hover at the call site.
    pub fn hovered(self) -> Self {
        Self { fill: self.hover_fill, ..self }
    }
}

/// Paint one button into `hdc`. `radius` is the corner radius in device
/// pixels -- the caller's, since this module knows nothing about DPI
/// scaling; the unlock prompt already picks one for its buttons and passes
/// the scaled value in.
///
/// `RoundRect` does not antialias, so the corners are hard. That is the
/// accepted cost of GDI: Direct2D would smooth them and was measured at
/// 53.85 MB against this window's 1.79 MB.
///
/// Every GDI object created here (brush, pen, font selection) is restored
/// and deleted before returning -- a leaked `HBRUSH` in a repaint path
/// exhausts the handle table over a long-running daemon.
pub fn draw_button(hdc: HDC, rect: RECT, label: &str, font: HFONT, skin: ButtonSkin, radius: i32) {
    draw_button_with_shortcut(hdc, rect, label, font, skin, radius, None, 100);
}

/// Paint one button whose keyboard shortcut lives **inside** it.
///
/// `theme::toolbar_button_with_shortcut`'s idiom, in GDI: the design's markup
/// is one element containing both runs -- the label, then its shortcut in the
/// keyboard chip -- rather than a button with a chip floating beside it, so
/// the whole pill is the thing the shortcut acts on. The label stays centred
/// in what the chip leaves of the button, which is what keeps it from sliding
/// under the chip on a narrow one.
///
/// `hint` of `None` is exactly [`draw_button`], which is why that function is
/// this one with the argument left out rather than a second copy of the pill.
///
/// `RoundRect` does not antialias, so the corners are hard. That is the
/// accepted cost of GDI: Direct2D would smooth them and was measured at
/// 53.85 MB against this window's 1.79 MB.
///
/// Every GDI object created here (brush, pen, font selection) is restored
/// and deleted before returning -- a leaked `HBRUSH` in a repaint path
/// exhausts the handle table over a long-running daemon.
pub fn draw_button_with_shortcut(
    hdc: HDC,
    rect: RECT,
    label: &str,
    font: HFONT,
    skin: ButtonSkin,
    radius: i32,
    hint: Option<(&str, HFONT)>,
    scale: i32,
) {
    unsafe {
        let brush = CreateSolidBrush(skin.fill);
        let pen = match skin.border {
            Some(colour) => CreatePen(PS_SOLID, 1, colour),
            None => CreatePen(PS_SOLID, 1, skin.fill),
        };
        let old_brush = SelectObject(hdc, brush);
        let old_pen = SelectObject(hdc, pen);
        let _ = RoundRect(hdc, rect.left, rect.top, rect.right, rect.bottom, radius * 2, radius * 2);
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(brush);
        let _ = DeleteObject(pen);

        // The chip first, so the label below is centred in what is left.
        let hint_lane = match hint {
            Some((text, hint_font)) => draw_hint_chip(hdc, rect, text, hint_font, scale),
            None => 0,
        };

        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, skin.text);
        let old_font = SelectObject(hdc, font);
        let mut rc = RECT { right: rect.right - hint_lane, ..rect };
        draw_text(hdc, label, &mut rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(hdc, old_font);
    }
}

/// How many candidate rows to draw, and whether candidates were dropped.
///
/// **`cap` is the candidate cap, not the row cap.** The *Search the vault*
/// row is drawn under every populated card -- the matcher that produced the
/// candidates is loose on purpose, so a card whose two guesses are both wrong
/// is an ordinary state, and this window cannot scroll to a way out -- but it
/// is a row the card is *additionally* tall for, never one that competes with
/// the candidates for a slot. Spending a slot on it meant a list of exactly
/// `cap` candidates showed `cap - 1` of them and reported a truncation that
/// had not happened; see `picker_prompt::LIST_ROWS`, which is `ROW_CAP + 1`
/// precisely so this function can hand back the full `cap`.
///
/// **A cap that hides candidates without saying so is the defect this project
/// keeps finding**, so the returned flag is still the truncation news -- it
/// decides what that row's second line says rather than whether the row is
/// there at all. See `picker_prompt::populated_rows`.
pub fn visible_rows(total: usize, cap: usize) -> (usize, bool) {
    if total <= cap {
        (total, false)
    } else {
        (cap, true)
    }
}

/// Where a hint chip sits inside the surface it belongs to, and what it costs
/// the label lane beside it. See [`hint_chip_lane`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChipLane {
    /// The chip's left edge, never left of the rect's own.
    pub left: i32,
    /// The chip's right edge, inset from the rect's by `TEXT_CLIP_INSET`.
    pub right: i32,
    /// What the caller must take off the label's right edge: the chip plus
    /// the gap that keeps a truncated name from touching it. Never wider than
    /// the rect.
    pub lane: i32,
}
/// Places the chip for a run of text `text_w` device pixels wide, right
/// aligned inside `rect`.
///
/// **Clamped against the rect it was given, in both directions.** The chip is
/// measured at runtime -- `GetTextExtentPoint32W` on the user's own hint font
/// at the user's own DPI -- so its width is not a number this module chose,
/// and nothing here bounds it against the button. Unclamped, a chip wider
/// than its surface would put `left` to the left of `rect.left` and, worse,
/// hand back a lane wider than the rect, which inverts the label rect
/// [`draw_button_with_shortcut`] and [`draw_row`] derive by subtracting it --
/// an inverted `RECT` is a `DrawTextW` that paints nothing, so a chip 1 px too
/// wide would silently cost the row its name.
///
/// Today's arithmetic does not reach that: `CTRL+ALT+N` measures ~73 px inside
/// a 168 px `SECONDARY_W`, and both sides scale linearly with DPI, so the
/// ratio holds at every scaling factor. The clamp is the guarantee that a
/// longer hint, a wider font or a narrower button degrades into a clipped chip
/// rather than into a row with no text at all.
pub fn hint_chip_lane(rect: RECT, text_w: i32, scale: i32) -> ChipLane {
    let px = |v: f32| ((v * scale as f32) / 100.0).round() as i32;
    let width = (rect.right - rect.left).max(0);
    let right = (rect.right - px(crate::theme::TEXT_CLIP_INSET)).max(rect.left);
    let w = (text_w.max(0) + 2 * px(crate::theme::CHIP_PAD_X)).min(right - rect.left);
    let gap = px(crate::theme::CHIP_PAD_X);
    ChipLane { left: right - w, right, lane: (w + gap).min(width) }
}

/// Paints one keyboard-hint chip, right-aligned inside `rect`, and answers how
/// much of the row's width it took -- the chip plus the gap that keeps a
/// truncated name from touching it.
///
/// **The design's chip, not a second one.** Every number and colour is
/// `crate::theme`'s own [`crate::theme::CHIP_HEIGHT`],
/// [`crate::theme::CHIP_PAD_X`], [`crate::theme::CHIP_RADIUS`] and
/// [`crate::theme::CHIP_TEXT_PX`] -- the same four `theme::kbd_chip` paints
/// with -- and it is *bordered* and drawn inside the row it belongs to rather
/// than floating beside it, which is `theme::toolbar_button_with_shortcut`'s
/// documented idiom: one surface carrying the label and its shortcut, so the
/// whole of it is the thing the shortcut acts on.
///
/// Every GDI object created here is restored and deleted before returning,
/// matching [`draw_button`] -- this runs in the daemon's repaint path.
pub fn draw_hint_chip(hdc: HDC, rect: RECT, hint: &str, font: HFONT, scale: i32) -> i32 {
    unsafe {
        let chars: Vec<u16> = hint.encode_utf16().collect();
        let old_font = SelectObject(hdc, font);
        let mut size = SIZE::default();
        let measured =
            GetTextExtentPoint32W(hdc, &chars, &mut size).as_bool();
        // A refusal is cosmetic, never a reason to lose the row: the chip is
        // simply not drawn, and the row keeps its full text lane.
        if !measured {
            SelectObject(hdc, old_font);
            return 0;
        }
        let px = |v: f32| ((v * scale as f32) / 100.0).round() as i32;
        let ChipLane { left, right, lane } = hint_chip_lane(rect, size.cx, scale);
        let h = px(crate::theme::CHIP_HEIGHT);
        let top = rect.top + ((rect.bottom - rect.top) - h) / 2;
        let radius = px(crate::theme::CHIP_RADIUS) * 2;

        let brush = CreateSolidBrush(rgb(crate::theme::CANVAS));
        let pen = CreatePen(PS_SOLID, 1, rgb(crate::theme::BORDER_STRONG));
        let old_brush = SelectObject(hdc, brush);
        let old_pen = SelectObject(hdc, pen);
        let _ = RoundRect(hdc, left, top, right, top + h, radius, radius);
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(brush);
        let _ = DeleteObject(pen);

        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, rgb(crate::theme::TEXT_FAINT));
        let mut rc = RECT { left, top, right, bottom: top + h };
        draw_text(hdc, hint, &mut rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(hdc, old_font);
        lane
    }
}

// ---------------------------------------------------------------------------
// The brand lockup
//
// **One mark painter for the whole crate.** `unlock_prompt` had the only GDI
// copy of the shield; the four cards ported after it started straight at their
// title and lost the brand entirely. Rather than a second painter per card,
// the one that existed moved here -- `unlock_prompt::win32::paint_mark` now
// calls straight into it -- so the daemon has exactly one place that knows
// what the mark looks like, and it reads its geometry and its four fills out
// of `theme` like everything else in this file.
// ---------------------------------------------------------------------------

/// **The card header lockup's logical geometry**, in the cards' own logical
/// pixels at 100%.
///
/// One table, because four cards lay the same lockup out and four copies of
/// "16, then 6, then 100" is four chances to disagree. `mark_h`, `word_px` and
/// `tracking` are `theme`'s [`crate::theme::CARD_HEADER_MARK_H`],
/// [`crate::theme::CARD_HEADER_WORD_PX`] and
/// [`crate::theme::CARD_HEADER_TRACKING`] rounded to whole pixels -- GDI's
/// `SetTextCharacterExtra` takes whole pixels and nothing finer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lockup {
    pub mark_h: i32,
    pub mark_w: i32,
    /// The optical gap between the shield and the word. Wider than it looks
    /// it should be because the artboard pads the shield's ink by two of its
    /// own units on every side -- see [`crate::theme::mark_ink_rect`].
    pub gap: i32,
    pub word_w: i32,
    pub word_px: i32,
    pub tracking: i32,
    /// The gap between the lockup's baseline box and whatever the card puts
    /// under it -- its own title, or its header rule.
    pub gap_below: i32,
}

/// [`Lockup`], for the card headers.
pub fn card_lockup() -> Lockup {
    let mark_h = crate::theme::CARD_HEADER_MARK_H.round() as i32;
    Lockup {
        mark_h,
        mark_w: mark_width(mark_h),
        gap: 6,
        // "DESKWARDEN" is ten capitals of a bold 11px Archivo with a pixel of
        // tracking on each. Measured rather than guessed would mean a DC, and
        // `layout()` is deliberately pure; this is the measured width rounded
        // up, and every card asserts the lane it leaves fits inside its own
        // margins.
        word_w: 100,
        word_px: crate::theme::CARD_HEADER_WORD_PX.round() as i32,
        tracking: crate::theme::CARD_HEADER_TRACKING.round() as i32,
        gap_below: 12,
    }
}

/// How wide the mark's box is at `height`, in the design's artboard ratio.
///
/// [`draw_mark`] letterboxes inside whatever box it is given, so a box of the
/// wrong ratio would leave the shield floating in dead space and push the
/// wordmark away from it. Every caller sizes its mark box through this.
pub fn mark_width(height: i32) -> i32 {
    let (aw, ah) = crate::theme::MARK_ARTBOARD;
    ((height as f32) * aw / ah).round() as i32
}

/// **The Deskwarden shield**, fitted into `rect` in DEVICE pixels.
///
/// The design's four quadrants from [`crate::theme::quadrant_outlines`], in
/// [`crate::theme::QUADRANT_FILLS`]' checkerboard tone order, scaled to fit
/// `rect` without distorting the artboard and centred in whatever room is
/// left over.
///
/// Every brush and pen is restored and deleted before returning, including on
/// the path where a quadrant is degenerate: this runs in the daemon's repaint
/// path.
pub fn draw_mark(hdc: HDC, rect: RECT) {
    let (aw, ah) = crate::theme::MARK_ARTBOARD;
    let box_w = (rect.right - rect.left) as f32;
    let box_h = (rect.bottom - rect.top) as f32;
    if box_w <= 0.0 || box_h <= 0.0 {
        return;
    }
    let s = (box_w / aw).min(box_h / ah);
    let ox = rect.left as f32 + (box_w - aw * s) / 2.0;
    let oy = rect.top as f32 + (box_h - ah * s) / 2.0;

    unsafe {
        for (outline, fill_colour) in
            crate::theme::quadrant_outlines().iter().zip(crate::theme::QUADRANT_FILLS)
        {
            let points: Vec<POINT> = outline
                .iter()
                .map(|p| POINT {
                    x: (ox + p.x * s).round() as i32,
                    y: (oy + p.y * s).round() as i32,
                })
                .collect();
            let brush = CreateSolidBrush(rgb(fill_colour));
            // A `NULL_PEN` would leave a hairline gap between quadrants; a pen
            // of the quadrant's own colour makes the four shapes meet exactly
            // as they do in the vector original.
            let pen = CreatePen(PS_SOLID, 1, rgb(fill_colour));
            let old_brush = SelectObject(hdc, brush);
            let old_pen = SelectObject(hdc, pen);
            let _ = Polygon(hdc, &points);
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(brush);
            let _ = DeleteObject(pen);
        }
    }
}

/// **The card header's lockup**: the shield, and [`crate::theme::WORDMARK_CAPS`]
/// set beside it in `font` with `tracking` whole pixels of letterspacing.
///
/// Both rects are DEVICE pixels and `tracking` is already scaled by the
/// caller, because each card owns its own DPI factor.
///
/// This is the compact lockup the design's card headers carry
/// ([`crate::theme::card_header`]) -- **not** the login window's, which sets
/// "Deskwarden" at 25px over a tagline and is far too tall for a 380px card
/// that also has to fit a list and a footer. `unlock_prompt` keeps that one
/// because it is the one surface with the room for it.
pub fn draw_card_lockup(hdc: HDC, mark: RECT, word: RECT, font: HFONT, tracking: i32) {
    draw_mark(hdc, mark);
    unsafe {
        let old = SelectObject(hdc, font);
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, rgb(crate::theme::TEXT_SECONDARY));
        SetTextCharacterExtra(hdc, tracking);
        let mut rc = word;
        draw_text(
            hdc,
            crate::theme::WORDMARK_CAPS,
            &mut rc,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
        );
        SetTextCharacterExtra(hdc, 0);
        SelectObject(hdc, old);
    }
}

/// **One field mark, drawn into a row's square gutter.**
///
/// The geometry is [`crate::theme::field_mark_paths`]'s and none of this
/// function's: the marks are strokes in one artboard, and what happens here is
/// the artboard-to-device conversion and the GDI calls, exactly as
/// [`draw_mark`] does for the shield. That is what keeps the picker's second
/// step reading the same palette file the rest of the app does.
///
/// `gutter` is the row's icon column in DEVICE pixels -- the same square
/// `crate::picker_prompt` blends a favicon into on the step before, so the two
/// lists line up -- and `scale` is the card's DPI percentage.
///
/// Every pen and brush it makes is selected out and deleted before it returns,
/// including on the path where a mark has no paths at all: this runs on every
/// hover of every row.
pub fn draw_field_mark(hdc: HDC, gutter: RECT, mark: crate::theme::FieldMark, scale: i32) {
    use crate::theme::{MarkPathKind, FIELD_MARK_ARTBOARD, FIELD_MARK_SIDE, FIELD_MARK_STROKE};
    unsafe {
        let px = |v: f32| ((v * scale as f32) / 100.0).round() as i32;
        let side = px(FIELD_MARK_SIDE);
        let unit = side as f32 / FIELD_MARK_ARTBOARD;
        let left = gutter.left + ((gutter.right - gutter.left) - side) / 2;
        let top = gutter.top + ((gutter.bottom - gutter.top) - side) / 2;
        let at = |q: eframe::egui::Pos2| POINT {
            x: left + (q.x * unit).round() as i32,
            y: top + (q.y * unit).round() as i32,
        };

        let ink = rgb(crate::theme::TEXT_SECONDARY);
        // At least one pixel: a stroke that rounded to zero is a mark that
        // simply is not there, which is worse than a heavy one.
        let pen = CreatePen(PS_SOLID, (FIELD_MARK_STROKE * unit).round().max(1.0) as i32, ink);
        let brush = CreateSolidBrush(ink);
        let old_pen = SelectObject(hdc, pen);
        let old_brush = SelectObject(hdc, brush);

        for shape in crate::theme::field_mark_shapes(mark) {
            match shape {
                // A real ellipse, not a sampled ring: a circle flattened to
                // points and rounded to whole pixels at a 3-unit radius comes
                // out an octagon, which is what the first render of these
                // marks showed. `Ellipse` fills with the selected brush, so a
                // stroked ring selects the stock hollow one for the call and
                // puts the ink brush straight back -- there is nothing to
                // delete, because a stock object is not ours.
                crate::theme::MarkShape::Circle { centre, radius, filled } => {
                    let c = at(*centre);
                    let r = (radius * unit).round().max(1.0) as i32;
                    if *filled {
                        let _ = Ellipse(hdc, c.x - r, c.y - r, c.x + r, c.y + r);
                    } else {
                        let hollow = HBRUSH(GetStockObject(NULL_BRUSH).0);
                        let previous = SelectObject(hdc, hollow);
                        let _ = Ellipse(hdc, c.x - r, c.y - r, c.x + r, c.y + r);
                        SelectObject(hdc, previous);
                    }
                }
                crate::theme::MarkShape::Path { points, kind } => {
                    let mut device: Vec<POINT> = points.iter().map(|q| at(*q)).collect();
                    if device.len() < 2 {
                        continue;
                    }
                    match kind {
                        MarkPathKind::Filled => {
                            let _ = Polygon(hdc, &device);
                        }
                        MarkPathKind::Closed => {
                            // Stroked and closed, rather than `Polygon`,
                            // which would FILL it.
                            device.push(device[0]);
                            let _ = Polyline(hdc, &device);
                        }
                        MarkPathKind::Open => {
                            let _ = Polyline(hdc, &device);
                        }
                    }
                }
            }
        }

        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(brush);
        let _ = DeleteObject(pen);
    }
}

// ---------------------------------------------------------------------------
// The frameless cards' hit test.
//
// **Every card in this crate is frameless and is dragged by its background**,
// so each one answers `WM_NCHITTEST` by turning `HTCLIENT` into `HTCAPTION`.
// That is what made the close glyph unclickable on all of them: the
// glyph is PAINTED BY THE PARENT rather than being a child control, so once
// the whole client area reports itself as a title bar, a press on it starts a
// window drag and `WM_LBUTTONDOWN` never fires there at all. The rows and the
// footer buttons kept working only because they are child windows, with hit
// tests of their own that this arm never sees.
//
// The decision lives here once rather than once per card, and the half that
// decides is PURE: it takes `DefWindowProcW`'s answer, a point in CLIENT
// pixels and the glyph's rect in the same pixels, and returns the code to
// answer with. That is what lets the pin decide a hit test without opening a
// window.
// ---------------------------------------------------------------------------

/// Whether a point in client pixels falls on a card's close glyph.
///
/// Half-open on the right and the bottom, which is the convention every
/// card's own `in_close_glyph` already used: a rect's `right` column and
/// `bottom` row belong to whatever is next to it, never to it.
pub fn on_close_glyph(x: i32, y: i32, glyph: RECT) -> bool {
    x >= glyph.left && x < glyph.right && y >= glyph.top && y < glyph.bottom
}

/// **What a frameless card answers to `WM_NCHITTEST`.**
///
/// `HTCAPTION` everywhere `DefWindowProcW` said `HTCLIENT` -- so the card is
/// dragged by its background -- **except on the close glyph**, which stays
/// `HTCLIENT` so that the press on it arrives as `WM_LBUTTONDOWN` and the
/// card's own `in_close_glyph` gets to see it.
///
/// Anything that was not `HTCLIENT` to begin with is passed through untouched:
/// a border, a corner or `HTNOWHERE` is the system's answer about a part of
/// the window this card does not paint.
pub fn frameless_hit(default_hit: isize, x: i32, y: i32, glyph: RECT) -> isize {
    if default_hit == HTCLIENT as isize && !on_close_glyph(x, y, glyph) {
        HTCAPTION as isize
    } else {
        default_hit
    }
}

/// [`frameless_hit`] against a live window.
///
/// **`WM_NCHITTEST`'s `lparam` is in SCREEN pixels** and the glyph's rect is
/// in client pixels, so the point is converted before the two are compared. A
/// card that compared the screen point directly would appear to work with its
/// window at the top left of a monitor and nowhere else, which is the reason
/// this wrapper exists rather than each arm doing its own arithmetic.
///
/// A refusal from `ScreenToClient` leaves the point where it was -- a screen
/// point, which answers `HTCAPTION` like any other background pixel. Dragging
/// still works and the glyph simply does not answer, which is exactly the
/// behaviour these cards already had.
///
/// # Safety
///
/// `window` must be a live window handle: this calls `ScreenToClient` on it.
pub unsafe fn frameless_hit_test(
    window: HWND,
    default_hit: LRESULT,
    screen: LPARAM,
    glyph: RECT,
) -> LRESULT {
    let mut point = POINT {
        x: (screen.0 & 0xffff) as i16 as i32,
        y: ((screen.0 >> 16) & 0xffff) as i16 as i32,
    };
    let _ = ScreenToClient(window, &mut point);
    LRESULT(frameless_hit(default_hit.0, point.x, point.y, glyph))
}

/// Whether a row is under the pointer, selected, both or neither.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RowState {
    pub selected: bool,
    pub hovered: bool,
}

/// Paint one candidate row into `hdc`.
///
/// The background fills the entire `rect` -- edge to edge, including the icon
/// gutter -- before any text is drawn, so hover and selection never hug just
/// the text; that half-width highlight was reported as a defect on the vault
/// window's menu and is not to be repeated here.
///
/// A square gutter the height of the row is left blank on the left for
/// Task 5's icon; this function does not draw into it.
///
/// Every GDI object created here is restored and deleted before returning,
/// matching [`draw_button`] -- this also runs in the daemon's repaint path.
pub fn draw_row(
    hdc: HDC,
    rect: RECT,
    candidate: &Candidate,
    state: RowState,
    name_font: HFONT,
    user_font: HFONT,
    hint: Option<(&str, HFONT)>,
    scale: i32,
) {
    unsafe {
        let fill = if state.selected {
            rgb(crate::theme::BLUE_WASH)
        } else if state.hovered {
            rgb(crate::theme::CARD_TINT)
        } else {
            rgb(crate::theme::CARD)
        };
        let brush = CreateSolidBrush(fill);
        let old_brush = SelectObject(hdc, brush);
        let pen = CreatePen(PS_SOLID, 1, fill);
        let old_pen = SelectObject(hdc, pen);
        let _ = RoundRect(hdc, rect.left, rect.top, rect.right, rect.bottom, 0, 0);
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(brush);
        let _ = DeleteObject(pen);

        // The chip first, so the text lane below can stop short of it: a name
        // drawn to the row's own edge would run underneath the hint, and
        // `DT_END_ELLIPSIS` truncates against the rect it is given.
        let hint_lane = match hint {
            Some((text, font)) => draw_hint_chip(hdc, rect, text, font, scale),
            None => 0,
        };

        let gutter = rect.bottom - rect.top;
        let text_left = rect.left + gutter;
        // `DT_END_ELLIPSIS` truncates against the rect's right edge, so the
        // rect stops short of the row's: an ellipsis flush against the card's
        // edge reads as a cut rather than as "there is more". The left gutter
        // the icon lives in is untouched.
        let text_right = rect.right - crate::theme::TEXT_CLIP_INSET as i32 - hint_lane;

        SetBkMode(hdc, TRANSPARENT);

        SetTextColor(hdc, rgb(crate::theme::INK));
        let old_font = SelectObject(hdc, name_font);
        let mut name_rc = RECT {
            left: text_left,
            top: rect.top,
            right: text_right,
            bottom: rect.top + gutter / 2,
        };
        draw_text(
            hdc,
            &candidate.name,
            &mut name_rc,
            DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        SelectObject(hdc, old_font);

        SetTextColor(hdc, rgb(crate::theme::TEXT_FAINT));
        let old_font = SelectObject(hdc, user_font);
        let mut user_rc = RECT {
            left: text_left,
            top: rect.top + gutter / 2,
            right: text_right,
            bottom: rect.bottom,
        };
        draw_text(
            hdc,
            &candidate.username,
            &mut user_rc,
            DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        SelectObject(hdc, old_font);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The close glyph is the one part of a frameless card's background
    /// that is not a title bar.**
    ///
    /// The reported defect, stated as the three answers a hit test has to
    /// give: a point inside the glyph is `HTCLIENT` -- which is what lets the
    /// press reach `WM_LBUTTONDOWN` and cancel the card -- and every other
    /// background point, including one a single pixel outside the glyph, is
    /// `HTCAPTION`, which is what still drags a window with no title bar.
    ///
    /// Decidable without a window, which is why it is a test at all: all
    /// three answers come from `DefWindowProcW`'s code, a client point and a
    /// rect, and none of them from a live `HWND`. Against the code this
    /// replaced -- `if hit.0 == 1 { HTCAPTION } else { hit }`, with no glyph
    /// anywhere in it -- the first assertion fails, because that arm answered
    /// `HTCAPTION` for the glyph too and swallowed every click on the ✕.
    #[test]
    fn the_close_glyph_is_the_one_part_of_a_frameless_cards_background_that_is_not_a_title_bar() {
        const CLIENT: isize = HTCLIENT as isize;
        const CAPTION: isize = HTCAPTION as isize;
        // A card's glyph, at the size every one of them lays out: a 20x20 box
        // inset from the header's top right corner.
        let glyph = RECT { left: 344, top: 14, right: 364, bottom: 34 };

        for (x, y, what) in [
            (glyph.left, glyph.top, "its top left corner"),
            (glyph.right - 1, glyph.bottom - 1, "its bottom right corner"),
            ((glyph.left + glyph.right) / 2, (glyph.top + glyph.bottom) / 2, "its centre"),
        ] {
            assert_eq!(
                frameless_hit(CLIENT, x, y, glyph),
                CLIENT,
                "{what} of the close glyph answered the hit test as a title bar, so a press \
                 there starts a window drag and `WM_LBUTTONDOWN` never fires -- the ✕ does \
                 nothing, which is exactly what was reported"
            );
        }

        // The background still drags the window: that is the property the
        // `HTCAPTION` answer exists for, and fixing the glyph must not cost
        // it.
        for (x, y, what) in [
            (190, 200, "the middle of the card"),
            (16, 16, "the brand lockup"),
            (190, 400, "the footer"),
        ] {
            assert_eq!(
                frameless_hit(CLIENT, x, y, glyph),
                CAPTION,
                "{what} no longer drags the window, so a frameless card cannot be moved at all"
            );
        }

        // One pixel outside the glyph on each side, which is the boundary the
        // half-open rect draws. A rect read inclusively would answer `client`
        // for the first two of these and leave a one-pixel dead strip along
        // the card's edge.
        for (x, y, what) in [
            (glyph.left - 1, glyph.top, "just left of"),
            (glyph.left, glyph.top - 1, "just above"),
            (glyph.right, glyph.top, "just right of"),
            (glyph.left, glyph.bottom, "just below"),
        ] {
            assert_eq!(
                frameless_hit(CLIENT, x, y, glyph),
                CAPTION,
                "the point {what} the close glyph answered `HTCLIENT`, so the glyph's hit \
                 target is not the rect it is painted into"
            );
        }

        // CONTROL: an answer that was never `HTCLIENT` is the system's about
        // a part of the window this card does not paint, and is passed
        // through untouched -- on the glyph as anywhere else. A rewrite that
        // returned `HTCLIENT` for the glyph unconditionally would pass every
        // assertion above and fail this one.
        use windows::Win32::UI::WindowsAndMessaging::{HTBOTTOMRIGHT, HTNOWHERE};
        for other in [HTNOWHERE as isize, HTBOTTOMRIGHT as isize] {
            assert_eq!(
                frameless_hit(other, 190, 200, glyph),
                other,
                "control: a non-client hit code was rewritten by a card's hit test"
            );
            assert_eq!(
                frameless_hit(other, glyph.left, glyph.top, glyph),
                other,
                "control: a non-client hit code over the glyph was rewritten by a card's hit \
                 test"
            );
        }
        // CONTROL: the two codes this decides between are not the same
        // number, so the assertions above are distinguishing something.
        assert_ne!(CLIENT, CAPTION, "control: `HTCLIENT` and `HTCAPTION` are the same value");
    }

    /// **Every frameless card in this crate answers its hit test through
    /// [`frameless_hit_test`].**
    ///
    /// A defect class, not an instance. All the cards were built from one
    /// pattern -- `if hit.0 == 1 { HTCAPTION } else { hit }` -- and all of
    /// them paint their ✕ on the parent, so all of them swallowed every click
    /// on it. A source pin because the alternative is six live windows; what
    /// it buys is that the next card copied from any of them cannot quietly
    /// reintroduce the arm.
    ///
    /// **Six, and it was seven.** `preflight_card.rs` -- design 4b's send
    /// confirmation -- was the seventh and has been removed outright; see
    /// `vault_window::preflight`'s module doc for why. The rule is unchanged
    /// and so is every card still in the list.
    #[test]
    fn no_frameless_card_answers_its_whole_client_area_as_a_title_bar() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        // Normalised for the CRLF checkout, before anything is sliced out of
        // it: a scan that matched against `\r\n`-terminated lines would find
        // nothing at all and report every card as broken.
        for card in [
            "picker_prompt.rs",
            "unlock_prompt.rs",
            "generate_prompt.rs",
            "prompt_card.rs",
            "locked_card.rs",
            "save_login_card.rs",
        ] {
            let raw = std::fs::read_to_string(src.join(card)).unwrap().replace("\r\n", "\n");
            let production = raw.split(concat!("\n#[cfg(", "test)]\n")).next().unwrap();
            let code: String = production
                .lines()
                .map(|line| line.split("//").next().unwrap_or(""))
                .collect::<Vec<_>>()
                .join("\n");
            let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");

            assert!(
                flat.contains("WM_NCHITTEST"),
                "control: {card} has no `WM_NCHITTEST` arm at all, so this scan is reading the \
                 wrong file and the rules below could not fail"
            );
            assert!(
                flat.contains("frameless_hit_test("),
                "{card} answers `WM_NCHITTEST` without `win32_draw::frameless_hit_test`, so its \
                 close glyph is inside the region it reports as a title bar and clicking the ✕ \
                 does nothing"
            );
            assert!(
                flat.contains("fn close_glyph_rect()"),
                "control: {card} no longer derives its close glyph's rect once, so the rect the \
                 hit test excuses and the rect `WM_LBUTTONDOWN` answers on can drift apart"
            );
            assert!(
                !flat.contains("if hit.0 == 1 { LRESULT(HTCAPTION as isize) }"),
                "{card} is back to answering `HTCAPTION` for its entire client area -- the \
                 defect that made every card's ✕ unclickable"
            );
        }
    }

    /// **An oversized chip cannot invert the lane it leaves behind.**
    ///
    /// The chip's width comes from `GetTextExtentPoint32W` at runtime, so it
    /// is not a number this module chose. Both callers derive the label's
    /// rect by subtracting the returned lane from the surface's right edge,
    /// and a lane wider than the surface makes `right < left` -- a `RECT`
    /// `DrawTextW` paints nothing into, so the row would lose its name
    /// entirely rather than show a clipped chip.
    ///
    /// A hint measured at ten times the button's width is not a string this
    /// card has; it is the bound stated as a value, so the clamp cannot be
    /// removed and still pass.
    #[test]
    fn an_oversized_hint_chip_cannot_invert_the_label_lane() {
        let button = RECT { left: 0, top: 0, right: 168, bottom: 32 };
        for scale in [100, 125, 150, 200, 300] {
            let huge = hint_chip_lane(button, 1680, scale);
            assert!(
                huge.left >= button.left,
                "at {scale}% the chip starts at {} px, left of the button's own {} px",
                huge.left,
                button.left
            );
            assert!(huge.right >= huge.left, "at {scale}% the chip's own rect is inverted");
            assert!(
                button.right - huge.lane >= button.left,
                "at {scale}% the chip took a {} px lane out of a {} px button, so the label rect \
                 the callers build from it is inverted and draws nothing",
                huge.lane,
                button.right - button.left
            );
        }
        // CONTROL: the clamp is a bound on the bad case, not a flattening of
        // the ordinary one. The real hints still get a lane proportional to
        // their own width, and a wider run still costs more of the label.
        let narrow = hint_chip_lane(button, 24, 100);
        let wide = hint_chip_lane(button, 73, 100);
        assert!(
            narrow.lane < wide.lane && wide.lane < button.right - button.left,
            "the clamp has eaten the ordinary case: `ESC` took {} px and `CTRL+ALT+N` {} px of a \
             {} px button",
            narrow.lane,
            wide.lane,
            button.right - button.left
        );
        // And a button with no room at all degrades rather than inverting.
        let nothing = hint_chip_lane(RECT { left: 40, top: 0, right: 40, bottom: 32 }, 60, 100);
        assert_eq!(nothing.left, 40);
        assert_eq!(nothing.lane, 0);
    }

    /// **Both of a row's lines end in an ellipsis rather than mid-glyph.**
    ///
    /// A source pin, because `DT_END_ELLIPSIS` is a painting flag: it changes
    /// what `DrawTextW` puts on a device context, and nothing this crate can
    /// drive in a test reads pixels back off the daemon's card. What is
    /// decidable is that the flag is in the call, and that the rect it
    /// truncates against stops short of the row's right edge -- without the
    /// inset the "..." sits hard against the card's border.
    ///
    /// The shape is this crate's established one -- read the file, normalise
    /// line endings (this is a CRLF checkout), cut at the first column-0
    /// `#[cfg(test)]` and scan the production half -- with controls so a scan
    /// that read nothing cannot pass.
    #[test]
    fn both_of_a_rows_lines_are_drawn_with_an_end_ellipsis() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let raw =
            std::fs::read_to_string(src.join("win32_draw.rs")).unwrap().replace("\r\n", "\n");
        let production = raw.split(concat!("\n#[cfg(", "test)]\n")).next().unwrap();
        // Comments stripped, so the prose above `draw_row` -- which names both
        // the flag and the inset -- cannot satisfy a rule about CODE.
        let code: String = production
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");

        // CONTROLS, so a pin that scanned nothing cannot pass: the cut must
        // have thrown something away, and the half it kept must be the half
        // that carries the row painter this rule is about.
        assert!(
            production.len() < raw.len(),
            "control: the `#[cfg(test)]` cut marker was not found in win32_draw.rs, so this scan \r
             is reading the test module as production and the rules below are meaningless"
        );
        assert!(
            code.contains(concat!("pub fn draw_", "row(")),
            "control: the production cut of win32_draw.rs does not contain `draw_row`, so the \r
             cut is in the wrong place and this pin is scanning the wrong text"
        );
        // The needle is `draw_text(` and no longer `DrawTextW(`: this file's
        // five runs now go through the wrapper, and the raw call survives in
        // exactly one place -- inside that wrapper -- which is what
        // `the_crate_calls_draw_text_w_in_exactly_one_place` pins.
        let drawn = code.matches(concat!("draw_", "text(")).count();
        assert_eq!(
            drawn, 6,
            "control: win32_draw.rs draws text in five places -- a button label, a keyboard-hint chip, a row's two lines, and the brand lockup's wordmark -- and declares the wrapper itself, which is the sixth match. It now has {drawn}, so the counts below no longer mean what this pin says they mean"
        );

        assert_eq!(
            code.matches(concat!("| DT_END_", "ELLIPSIS")).count(),
            2,
            "a row's name and username are drawn into fixed-width rects, and `DrawTextW` with no \r
             `DT_END_ELLIPSIS` CLIPS: a long item name is cut through the middle of a letter \r
             with nothing to say it was truncated. Both lines need the flag; the button label \r
             does not, because a button is sized to its own text. The needle carries the \r
             leading `|` so the import list is not counted as a third use"
        );
        assert!(
            code.contains(concat!("TEXT_CLIP_", "INSET as i32 - hint_lane")),
            "a row's text lane no longer stops short of its keyboard-hint chip. The chip is 
             drawn inside the row, so a name measured against the row's own right edge runs 
             underneath it -- and `DT_END_ELLIPSIS` would truncate against the wrong edge, so 
             nothing would even mark it"
        );
        assert!(
            code.contains(concat!("rect.right - crate::theme::TEXT_CLIP_", "INSET as i32")),
            "the row's text rect no longer stops short of the row's right edge. \r
             `DT_END_ELLIPSIS` truncates against the rect it is given, so a rect flush with the \r
             card's edge puts the \"...\" hard against the border, where it reads as a cut \r
             rather than as \"there is more\""
        );
        assert!(
            code.contains("let text_left = rect.left + gutter;"),
            "the row's left gutter is gone. It is the square the favicon is drawn into, and the \r
             text starting at `rect.left` would run underneath it"
        );
    }

    /// **The empty-run guard comes BEFORE the pointer is taken.**
    ///
    /// The property that makes [`draw_text`] safe, stated the only way it can
    /// be from a test: as an ordering in the source. It cannot be exercised,
    /// because exercising it means a real `HDC` and a real window procedure,
    /// and the failure mode is not a panic a test could catch -- it is
    /// STATUS_FATAL_USER_CALLBACK_EXCEPTION, which terminates the test runner
    /// without unwinding. A test that *called* the unguarded function would
    /// not fail; it would take `cargo test` down with it and report nothing.
    /// So the assertion is the source order, which is exactly the register
    /// this crate uses for untestable Win32 wiring -- see the pins in
    /// `foreground.rs` and `unlock_prompt.rs`.
    ///
    /// Both entry points are checked, and both directions of the ordering
    /// matter: in [`draw_text`] the `return` must precede `encode_utf16`, so
    /// that on the empty path no buffer and therefore no dangling pointer ever
    /// comes into existence; in [`draw_text_utf16`] it must precede the
    /// `DrawTextW` call itself, because that entry point is reachable with a
    /// caller's own empty buffer.
    #[test]
    fn the_empty_run_guard_precedes_the_pointer_in_both_text_wrappers() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let raw =
            std::fs::read_to_string(src.join("win32_draw.rs")).unwrap().replace("\r\n", "\n");
        let production = raw.split(concat!("\n#[cfg(", "test)]\n")).next().unwrap();
        // Comments stripped: the long block comment above the wrappers spells
        // out `if text.is_empty()` and `DrawTextW` in prose, and prose must not
        // be able to satisfy a rule about the ORDER OF CODE.
        let code: String = production
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");

        // CONTROL: the cut threw something away, and what it kept is the half
        // holding both wrappers. Without this a mis-placed cut marker would
        // make every `find` below return `None` and the test would fail
        // loudly rather than pass vacuously -- but it would fail for the wrong
        // reason, and the message would send the reader to the wrong place.
        assert!(
            production.len() < raw.len(),
            "control: the `#[cfg(test)]` cut marker was not found in win32_draw.rs, so this pin \
             is reading its own test module as production"
        );

        // `draw_text`: the early return, then the encode.
        let guard = code
            .find("if text.is_empty() {")
            .expect("`draw_text` no longer guards on an empty `&str` at all -- an empty run now \
                     reaches `DrawTextW` through a dangling pointer at address 0x2 and kills the \
                     daemon inside its window procedure, with no panic and no log line");
        let encode = code
            .find(concat!("text.encode_", "utf16()"))
            .expect("control: `draw_text` no longer encodes its `&str` to UTF-16, so this pin is \
                     not reading the function it names");
        assert!(
            guard < encode,
            "`draw_text` encodes before it checks for the empty run. The `Vec<u16>` an empty \
             string collects into never allocates, so it carries Rust's dangling sentinel -- the \
             `u16` alignment, literally address 2 -- and the guard must run before that value \
             exists rather than after it"
        );

        // `draw_text_utf16`: the early return, then the one real call.
        let slice_guard = code
            .find("if chars.is_empty() {")
            .expect("`draw_text_utf16` no longer guards on an empty buffer. It is reachable \
                     directly -- `generate_prompt` passes its `Zeroizing<Vec<u16>>` straight in \
                     -- so a zero-length password would reach `DrawTextW` at address 0x2");
        let call = code
            .find(concat!("Draw", "TextW("))
            .expect("control: win32_draw.rs no longer calls `DrawTextW` at all, so this pin is \
                     asserting an ordering between two things that are not both there");
        assert!(
            slice_guard < call,
            "`draw_text_utf16` calls `DrawTextW` before it checks whether the slice is empty. \
             That is the crash, verbatim: `DrawTextExWorker` dereferences the buffer pointer \
             before it consults the length, so a zero-length slice faults on its first read"
        );

        // CONTROL: the guards return rather than falling through. A rewrite
        // that kept `if text.is_empty()` but dropped the `return` would
        // satisfy every ordering above and still crash.
        for (needle, which) in
            [("if text.is_empty() {\n        return 0;", "draw_text"),
             ("if chars.is_empty() {\n        return 0;", "draw_text_utf16")]
        {
            assert!(
                code.contains(needle),
                "control: {which}'s empty-run check no longer returns immediately, so the guard \
                 is present in the source but does not actually stop the call"
            );
        }
    }

    /// **`DrawTextW` is called in exactly ONE place in this crate.**
    ///
    /// The assertion that actually prevents the recurrence. The crash --
    /// an empty `Vec<u16>`'s dangling `2` read by `DrawTextExWorker`, faulting
    /// inside a window procedure and taking the whole daemon with it, with no
    /// panic and no log -- was fixed once at the two cards that had met it,
    /// each under its own copy of the explanation. Seven other cards were
    /// still drawing text without the guard, and the next one written brought
    /// the crash straight back. Fixing the eight remaining sites one at a time
    /// would leave the ninth free to do it again; what closes the class is
    /// that the raw call has exactly one home.
    ///
    /// **A census, and it fails in BOTH directions**, modelled on
    /// `main.rs`'s `every_window_the_daemon_can_still_draw_is_named_here`. A
    /// tenth card calling `DrawTextW` directly fails it, which is the obvious
    /// half. The wrapper's own call disappearing ALSO fails it, which is the
    /// half that matters here: this crate's standing defect class is "a test
    /// that passes because it never reached the thing it names", and a rule
    /// that only ever counted down would quietly become a rule about nothing.
    ///
    /// Every needle is split with `concat!` so this pin cannot match itself,
    /// and every file's production half is cut at its own `#[cfg(test)]` --
    /// a test may name the API freely.
    #[test]
    fn the_crate_calls_draw_text_w_in_exactly_one_place() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let needle = concat!("Draw", "TextW(");
        let import = concat!("Draw", "TextW,");

        let mut calls: Vec<(String, usize)> = Vec::new();
        let mut imports: Vec<String> = Vec::new();
        let mut scanned = 0usize;
        for entry in std::fs::read_dir(&src).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            // Normalised for the CRLF checkout before anything is sliced: a
            // scan matching against `\r\n`-terminated lines finds nothing and
            // would report the whole crate clean.
            let raw = std::fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
            let production = raw.split(concat!("\n#[cfg(", "test)]\n")).next().unwrap();
            let code: String = production
                .lines()
                .map(|line| line.split("//").next().unwrap_or(""))
                .collect::<Vec<_>>()
                .join("\n");
            scanned += 1;
            let n = code.matches(needle).count();
            if n > 0 {
                calls.push((name.clone(), n));
            }
            if code.contains(import) {
                imports.push(name);
            }
        }

        // CONTROL: the scan reached the crate. A `read_dir` that found nothing
        // -- a wrong `CARGO_MANIFEST_DIR`, a moved `src` -- would otherwise
        // make every assertion below pass on an empty set.
        assert!(
            scanned > 20,
            "control: only {scanned} `.rs` files were scanned under {}, so this census is not \
             reading the crate and could not fail",
            src.display()
        );

        assert_eq!(
            calls,
            vec![("win32_draw.rs".to_string(), 1usize)],
            "`{needle}` must appear exactly once in production across `src/`, inside \
             `win32_draw::draw_text_utf16`. Found: {calls:?}.\n\
             \n\
             MORE than one, or one somewhere else: a card is calling `DrawTextW` directly again. \
             An empty run through the raw call reads Rust's dangling `Vec<u16>` sentinel -- \
             address 2 -- inside `DrawTextExWorker`, faults in a window procedure, and Windows \
             kills the process with STATUS_FATAL_USER_CALLBACK_EXCEPTION: no panic hook, no log \
             line, the tray and the vault window simply gone. That has now happened twice, and \
             the second time only a minidump found it.\n\
             \n\
             NONE, or the wrapper's own call gone: the daemon draws no text at all, or the \
             wrapper has been hollowed out and every card is silently blank."
        );

        // The import follows the call: a module that still pulls `DrawTextW`
        // into scope is a module one line away from using it, and the unused
        // import would not even warn if the module names it in a doc link.
        assert_eq!(
            imports,
            vec!["win32_draw.rs".to_string()],
            "`{import}` is imported outside `win32_draw.rs`, by: {imports:?}. Every card's text \
             now goes through `win32_draw::draw_text`, so no other module needs the raw call in \
             scope -- and leaving it imported is an invitation to the crash above"
        );
    }

    #[test]
    fn a_list_that_fits_shows_everything_and_reports_no_truncation() {
        assert_eq!(visible_rows(3, 5), (3, false));
        assert_eq!(visible_rows(4, 5), (4, false));
    }

    /// **A list of exactly the cap is shown whole, and is not a truncation.**
    ///
    /// The *Search the vault* row used to take one of the cap's slots, so a
    /// user with exactly five matches saw four of them and was told the card
    /// had cut the list. Nothing had been cut: the card is simply one row
    /// taller than the candidate cap, because that row is additional to the
    /// candidates rather than in competition with them.
    #[test]
    fn a_list_of_exactly_the_cap_is_shown_whole_and_is_not_a_truncation() {
        assert_eq!(
            visible_rows(5, 5),
            (5, false),
            "five candidates against a cap of five is five candidates, and a card that dropped one \
             of them and reported an overflow was lying about both"
        );
    }

    #[test]
    fn a_list_that_overflows_gives_up_no_candidate_it_could_have_shown() {
        assert_eq!(
            visible_rows(6, 5),
            (5, true),
            "the sixth candidate is the first that genuinely does not fit"
        );
        let (shown, overflow) = visible_rows(9, 5);
        assert!(overflow, "the user must be told the list was cut");
        assert_eq!(shown, 5, "the cap is the candidate cap; the search row has a slot of its own");
    }

    #[test]
    fn a_cap_of_one_shows_that_one_and_says_there_is_more() {
        assert_eq!(visible_rows(4, 1), (1, true));
    }

    #[test]
    fn the_primary_and_secondary_skins_differ_in_every_channel_that_matters() {
        let primary = ButtonSkin::primary();
        let secondary = ButtonSkin::secondary();
        assert_ne!(primary.fill, secondary.fill, "a secondary button must not look primary");
        assert_ne!(primary.text, secondary.text, "white-on-white would be invisible");
        assert!(
            secondary.border.is_some(),
            "the secondary button has no fill contrast against the card, so it needs a border to \
             read as a button at all -- this is the defect the stock Cancel had"
        );
        assert!(primary.border.is_none(), "a filled button does not need one");
    }

    #[test]
    fn primary_hover_fill_differs_from_its_resting_fill() {
        let resting = ButtonSkin::primary();
        let hovered = resting.hovered();
        assert_ne!(
            resting.fill, hovered.fill,
            "a button whose hover looks identical to its resting state gives the user no \
             feedback that it is clickable"
        );
    }

    #[test]
    fn secondary_hover_fill_differs_from_its_resting_fill() {
        let resting = ButtonSkin::secondary();
        let hovered = resting.hovered();
        assert_ne!(
            resting.fill, hovered.fill,
            "a button whose hover looks identical to its resting state gives the user no \
             feedback that it is clickable"
        );
    }

    #[test]
    fn a_disabled_skin_is_derived_and_not_a_fourth_hand_picked_palette() {
        let disabled = ButtonSkin::primary().disabled();
        assert_ne!(disabled.fill, ButtonSkin::primary().fill);
        assert_eq!(
            disabled.border,
            ButtonSkin::primary().border,
            "disabling changes colour, not shape"
        );
    }

    // -----------------------------------------------------------------------
    // Cyrillic on the GDI cards.
    //
    // **What can and cannot be proven from here.** These are Win32 windows, so
    // the crate's paint harnesses -- which drive egui -- do not reach them,
    // and nothing below opens a card or rasterises a glyph. What IS decidable
    // is everything that decides the outcome *before* GDI is involved: the
    // face names are a pure function of the run and the design weight, the
    // registration list is a constant, and both of those can be checked
    // against the shipped font files' own `name`, `OS/2` and `cmap` tables
    // rather than against a second copy of this author's beliefs about them.
    // That is the whole of the fix except the `SelectObject` call itself.
    // -----------------------------------------------------------------------

    /// A word in Cyrillic: "Sberbank", the bank whose vault entry is the one
    /// the owner's report was looking at. Every character is inside the
    /// bundled subset.
    const CYRILLIC_ITEM: &str = "Сбербанк";

    /// Big-endian `u16` at `at`. The font tables below are all big-endian and
    /// this is spelled once rather than at every field.
    fn be16(bytes: &[u8], at: usize) -> u16 {
        u16::from_be_bytes([bytes[at], bytes[at + 1]])
    }

    /// The offset of one TrueType table in `bytes`, by its four-byte tag.
    fn table_at(bytes: &[u8], tag: &[u8; 4]) -> usize {
        let count = be16(bytes, 4) as usize;
        for i in 0..count {
            let entry = 12 + 16 * i;
            if &bytes[entry..entry + 4] == tag {
                return u32::from_be_bytes(bytes[entry + 8..entry + 12].try_into().unwrap())
                    as usize;
            }
        }
        panic!("the bundled face has no `{}` table", String::from_utf8_lossy(tag));
    }

    /// One Windows/Unicode/en-US `name` record, by name ID. ID 1 is the legacy
    /// family and ID 2 the legacy subfamily -- the two records GDI's font
    /// mapper actually matches `lfFaceName` against.
    fn name_record(bytes: &[u8], want: u16) -> String {
        let name = table_at(bytes, b"name");
        let count = be16(bytes, name + 2) as usize;
        let strings = name + be16(bytes, name + 4) as usize;
        for i in 0..count {
            let rec = name + 6 + 12 * i;
            let (platform, encoding, language, id) =
                (be16(bytes, rec), be16(bytes, rec + 2), be16(bytes, rec + 4), be16(bytes, rec + 6));
            if (platform, encoding, language, id) != (3, 1, 0x409, want) {
                continue;
            }
            let len = be16(bytes, rec + 8) as usize;
            let at = strings + be16(bytes, rec + 10) as usize;
            let units: Vec<u16> = (0..len / 2).map(|u| be16(bytes, at + 2 * u)).collect();
            return String::from_utf16_lossy(&units);
        }
        panic!("the bundled face has no Windows/en-US `name` record {want}");
    }

    /// Every codepoint the font's `cmap` maps, read from its format-4
    /// Windows/Unicode subtable.
    fn cmap_coverage(bytes: &[u8]) -> std::collections::BTreeSet<u16> {
        let cmap = table_at(bytes, b"cmap");
        let count = be16(bytes, cmap + 2) as usize;
        let mut subtable = None;
        for i in 0..count {
            let rec = cmap + 4 + 8 * i;
            if (be16(bytes, rec), be16(bytes, rec + 2)) == (3, 1) {
                subtable = Some(
                    cmap + u32::from_be_bytes(bytes[rec + 4..rec + 8].try_into().unwrap()) as usize,
                );
            }
        }
        let subtable = subtable.expect("the bundled face has no (3, 1) `cmap` subtable");
        assert_eq!(be16(bytes, subtable), 4, "the subtable is not format 4");
        let seg_x2 = be16(bytes, subtable + 6) as usize;
        let segments = seg_x2 / 2;
        let mut covered = std::collections::BTreeSet::new();
        for s in 0..segments {
            let end = be16(bytes, subtable + 14 + 2 * s);
            let start = be16(bytes, subtable + 16 + seg_x2 + 2 * s);
            if start == 0xFFFF {
                continue;
            }
            for unit in start..=end.min(0xFFFE) {
                covered.insert(unit);
            }
        }
        covered
    }

    /// **The GDI names in `CYRILLIC_GDI_FACES` are read out of the files, not
    /// guessed.**
    ///
    /// This is the pin that matters most, because guessing here is exactly how
    /// the defect could be "fixed" and still be broken. `theme`'s table calls
    /// these faces `NotoSans-Cyrillic-Regular` and so on; those are egui
    /// `font_data` keys and GDI has never heard of them. Asking GDI for a face
    /// name no font declares does not fail loudly -- the mapper silently
    /// returns its best other guess, which is precisely the substitution this
    /// whole change exists to stop, and the card would look no different.
    ///
    /// So every row is checked against the file's own legacy `name` records,
    /// and the weight against the legacy four-styles-per-family rule those
    /// records encode: `lfWeight` is bold only when the file says its
    /// subfamily is `Bold`. Noto's cuts spell themselves the same way
    /// Archivo's do -- Regular and Bold sharing the family `Noto Sans` and
    /// told apart by weight, SemiBold and ExtraBold each `Regular` inside a
    /// family of their own -- and a re-subset that changed that would fail
    /// here rather than on the owner's screen.
    #[test]
    fn the_gdi_names_are_the_font_files_own_name_records() {
        for (family, gdi, weight, bytes) in CYRILLIC_GDI_FACES {
            let legacy_family = name_record(bytes, 1);
            let legacy_subfamily = name_record(bytes, 2);
            assert_eq!(
                gdi, legacy_family,
                "`{family}` is handed to GDI as `lfFaceName = {gdi:?}`, but the file's own legacy \
                 family record says {legacy_family:?}. GDI matches on that record, and a name no \
                 font declares does not fail -- the mapper substitutes, which is the defect this \
                 table exists to fix"
            );
            let expected = if legacy_subfamily == "Bold" { 700 } else { 400 };
            assert_eq!(
                weight, expected,
                "`{family}` is asked for at weight {weight}, but {gdi:?}'s legacy subfamily is \
                 {legacy_subfamily:?}. The legacy `name` records hold only four styles per \
                 family, so weight is the ONLY thing that tells Regular from Bold inside one \
                 family -- and is meaningless for a cut that carries its own family and is \
                 `Regular` within it. Asking for ({gdi:?}, 600) returns a synthesised-looking \
                 Regular, not SemiBold"
            );
        }
    }

    /// **Each Archivo cut is paired with the Noto cut of the same weight.**
    ///
    /// The `usWeightClass` in the file, not the legacy `lfWeight`: the whole
    /// point of the Cyrillic faces is that a Cyrillic name set in the app's
    /// semibold is semibold, and that is the property egui's stacks already
    /// have. A pairing that put Regular behind SemiBold would compile, draw
    /// real glyphs, and reintroduce the lighter-than-its-neighbours look the
    /// Noto faces were bundled to cure.
    #[test]
    fn each_archivo_cut_is_paired_with_the_noto_cut_of_the_same_weight() {
        for (family, _, _, archivo) in crate::theme::ARCHIVO_FACES {
            let (_, _, _, noto) = CYRILLIC_GDI_FACES
                .iter()
                .find(|(paired, ..)| *paired == family)
                .unwrap_or_else(|| panic!("`{family}` has no row in CYRILLIC_GDI_FACES"));
            let os2 = |bytes: &[u8]| be16(bytes, table_at(bytes, b"OS/2") + 4);
            assert_eq!(
                os2(archivo),
                os2(noto),
                "the Cyrillic face paired with `{family}` has `usWeightClass` {} against \
                 Archivo's {}. A Cyrillic item name would then render at a different weight from \
                 the Latin one beside it -- a quieter version of the same defect",
                os2(noto),
                os2(archivo)
            );
        }
    }

    /// **`cyrillic_subset_covers` is the shipped subset's own `cmap`.**
    ///
    /// The face-selection rule is "switch only if the whole run is drawable",
    /// so it is only as honest as this table. A codepoint claimed here that
    /// the file does not carry is a run drawn in Noto with a hole in it; one
    /// the file carries but this omits is a Cyrillic string left in the
    /// fallback face for no reason.
    ///
    /// `U+0000` and `U+000D` are the two deliberate exclusions and are named
    /// as such: the notdef mapping and a carriage return are not text.
    #[test]
    fn the_coverage_table_is_the_bundled_subsets_own_cmap() {
        let deliberately_excluded = [0x0000u16, 0x000D];
        for (family, _, _, bytes) in CYRILLIC_GDI_FACES {
            let covered = cmap_coverage(bytes);
            assert!(
                covered.len() > 90,
                "control: {family}'s Cyrillic face parsed to only {} codepoints, so this pin is \
                 reading the file wrongly and would pass against nothing",
                covered.len()
            );
            for unit in 0u16..=0xFFFE {
                if deliberately_excluded.contains(&unit) {
                    continue;
                }
                assert_eq!(
                    cyrillic_subset_covers(unit),
                    covered.contains(&unit),
                    "`cyrillic_subset_covers` and the shipped {family} face disagree about \
                     U+{unit:04X}: the table says {}, the file's `cmap` says {}",
                    cyrillic_subset_covers(unit),
                    covered.contains(&unit)
                );
            }
        }
    }

    /// **All four cuts cover the same codepoints**, which is what lets one
    /// coverage table stand for the whole family. If a re-subset ever shipped
    /// a Bold that was missing a letter its Regular had, the "whole run is
    /// drawable" test would be true at one weight and false at another, and a
    /// heading would silently lose a character its body copy kept.
    #[test]
    fn every_cyrillic_cut_covers_the_same_codepoints() {
        let regular = cmap_coverage(CYRILLIC_GDI_FACES[0].3);
        for (family, _, _, bytes) in CYRILLIC_GDI_FACES {
            assert_eq!(
                cmap_coverage(bytes),
                regular,
                "{family}'s Cyrillic cut covers a different set of codepoints from Regular's, so \
                 one coverage table can no longer stand for all four"
            );
        }
    }

    /// **A pure Cyrillic item name takes the paired face, at its own weight.**
    /// The defect, stated as the thing that must now be true.
    #[test]
    fn a_cyrillic_item_name_takes_the_paired_face_at_its_own_weight() {
        for (family, archivo, _, _) in crate::theme::ARCHIVO_FACES {
            let (face, _) = gdi_face_for_text(family, CYRILLIC_ITEM);
            assert_ne!(
                face, archivo,
                "{CYRILLIC_ITEM:?} is still asked of {archivo:?}, which carries no codepoint in \
                 U+0400-04FF at all. GDI does not draw blanks: it font-links the run to whatever \
                 the system offers, so the name renders in Segoe UI beside Archivo Latin -- which \
                 is the report"
            );
            assert_eq!(
                (face, gdi_face_for_text(family, CYRILLIC_ITEM).1),
                CYRILLIC_GDI_FACES
                    .iter()
                    .find(|(paired, ..)| *paired == family)
                    .map(|(_, gdi, weight, _)| (*gdi, *weight))
                    .unwrap(),
                "the face chosen for {CYRILLIC_ITEM:?} at `{family}` is not the one this weight \
                 is paired with"
            );
        }
    }

    /// **Latin is untouched, at every weight.** The first thing a font change
    /// has to promise: not one existing measurement moves. The Noto subset
    /// carries no Latin codepoint at all, so a Latin run reaching it would not
    /// merely look different -- it would be drawn entirely by GDI's fallback.
    #[test]
    fn a_latin_run_is_asked_of_exactly_the_face_it_always_was() {
        for (family, ..) in crate::theme::ARCHIVO_FACES {
            for run in ["Netflix", "1Password", "DESKWARDEN", "user@example.com", ""] {
                assert_eq!(
                    gdi_face_for_text(family, run),
                    crate::theme::gdi_face_for(family),
                    "{run:?} at `{family}` no longer resolves to the face it did before the \
                     Cyrillic pairing existed"
                );
            }
        }
    }

    /// **A mixed run keeps Archivo, and the reason is arithmetic rather than
    /// taste.**
    ///
    /// `DrawTextW` takes one font per call, so the choice is one face for the
    /// whole string. The bundled subset has no Latin, no digits and no
    /// punctuation, so choosing it for a mixed string sends the *majority* of
    /// the characters into GDI's fallback to rescue the minority: "Netflix RU
    /// — Иван" would lose seventeen characters to save four, and "Почта 2"
    /// would lose the digit that distinguishes it from "Почта".
    ///
    /// So the rule fires only on a fully covered run, and this pin is the
    /// statement that it cannot make any string *worse* than it was: every
    /// case here resolves to exactly what `theme::gdi_face_for` alone would
    /// have returned.
    #[test]
    fn a_mixed_run_keeps_archivo_because_the_subset_has_no_latin() {
        for run in [
            "Netflix RU — Иван",
            "Почта 2",
            "Сбербанк (осн.)",
            "Сбербанк, личный",
            "Яндекс-Почта",
            "ivan@почта.рф",
        ] {
            assert_eq!(
                gdi_face_for_text(crate::theme::SEMIBOLD, run),
                crate::theme::gdi_face_for(crate::theme::SEMIBOLD),
                "{run:?} was switched to the Cyrillic subset, which carries no Latin letter, no \
                 digit and no ASCII punctuation. Every character outside U+0400-04FF in it would \
                 then be drawn by GDI's fallback -- more of the string wrong, not less"
            );
        }
    }

    /// **A run with no Cyrillic letter in it keeps Archivo even when every
    /// character is technically drawable.**
    ///
    /// The subset carries the space, the no-break space and `№`, so " " and
    /// " " are "fully covered" runs. Switching face for them would change the
    /// width of a blank label for no visible reason, and a layout measured
    /// against one face would be drawn in another.
    #[test]
    fn a_run_with_no_cyrillic_letter_keeps_archivo() {
        for run in [" ", "  ", "\u{00A0}", "№", "№ №"] {
            assert_eq!(
                gdi_face_for_text(crate::theme::REGULAR, run),
                crate::theme::gdi_face_for(crate::theme::REGULAR),
                "{run:?} contains no Cyrillic letter, so there is nothing for the paired face to \
                 fix and switching to it can only move a measurement"
            );
        }
    }

    /// **The backwards lookup maps every `LOGFONTW` the cards actually
    /// build.**
    ///
    /// `draw_text_utf16` only ever sees a realised font, so
    /// `cyrillic_pair_for_realised` is what decides the swap in practice --
    /// and it is fed `FW_BOLD`/`FW_NORMAL`, 700 and 400, rather than the
    /// file's own `usWeightClass`. This walks the same table the cards' `font`
    /// helpers read and checks every row survives the round trip.
    #[test]
    fn the_realised_font_lookup_maps_every_face_the_cards_build() {
        for (family, ..) in crate::theme::ARCHIVO_FACES {
            let (face, weight) = crate::theme::gdi_face_for(family);
            let as_created = if weight >= 700 { 700 } else { 400 };
            assert_eq!(
                cyrillic_pair_for_realised(face, as_created),
                Some(gdi_face_for_text(family, CYRILLIC_ITEM)),
                "a card's `{family}` font -- `lfFaceName = {face:?}`, `lfWeight = {as_created}` \
                 -- is not recognised as one of ours when it comes back out of the DC, so a \
                 Cyrillic run drawn with it would never be swapped"
            );
        }
        assert_eq!(
            cyrillic_pair_for_realised("archivo semibold", 400),
            cyrillic_pair_for_realised("Archivo SemiBold", 400),
            "the lookup is case sensitive. GDI's own face matching is not, and a `LOGFONTW` that \
             came back spelled differently would silently stop being ours"
        );
    }

    /// **Nothing that is not ours is second-guessed.**
    ///
    /// `Consolas` is the generated password's face and the keyboard chips',
    /// and it covers U+0400-04FF itself; the stock shell font covers it too.
    /// A swap there would be this module overriding a perfectly good face on a
    /// surface it does not own.
    #[test]
    fn a_face_that_is_not_ours_is_left_alone() {
        for face in [crate::theme::GDI_MONO_FACE, "Segoe UI", "MS Shell Dlg", ""] {
            assert_eq!(
                cyrillic_pair_for_realised(face, 400),
                None,
                "{face:?} is not one of the app's Archivo cuts, so nothing here has any business \
                 replacing it"
            );
        }
    }

    /// **An unknown design family falls back rather than panicking.**
    ///
    /// `theme::gdi_face_for` makes exactly this promise, and for exactly this
    /// reason: these are the surfaces whose whole reason for existing is that
    /// they must open when the heavier machinery cannot, so a prompt in the
    /// wrong weight is a cosmetic defect and a prompt that fails to draw is a
    /// locked-out user. The Cyrillic lookup must not be the thing that turns
    /// the first into the second.
    #[test]
    fn an_unknown_family_falls_back_instead_of_panicking() {
        assert_eq!(
            gdi_face_for_text("Archivo-Nonexistent", CYRILLIC_ITEM),
            crate::theme::gdi_face_for("Archivo-Nonexistent"),
            "an unknown design family must land on whatever `gdi_face_for` would have returned, \
             not on a panic inside a window procedure"
        );
    }

    /// **The GDI table and `theme`'s egui table bundle the same four files.**
    ///
    /// The one real cost of this module carrying its own `include_bytes!`:
    /// `theme::CYRILLIC_FACES` is private, so the bytes cannot be shared, and
    /// two tables naming font files by path is two tables that can drift onto
    /// different ones. A drift would be invisible -- both renderers would draw
    /// real Cyrillic glyphs, in two different cuts of Noto -- which is a
    /// subtler version of the defect being fixed.
    ///
    /// So the paths are read out of this file's production half and each one
    /// checked against `theme.rs`'s source. Nothing is hardcoded in the
    /// assertion, so adding a fifth weight to one table and not the other
    /// fails here.
    #[test]
    fn the_cyrillic_assets_are_the_ones_theme_bundles() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let raw =
            std::fs::read_to_string(src.join("win32_draw.rs")).unwrap().replace("\r\n", "\n");
        let production = raw.split(concat!("\n#[cfg(", "test)]\n")).next().unwrap();
        assert!(
            production.len() < raw.len(),
            "control: the `#[cfg(test)]` cut marker was not found in win32_draw.rs, so this pin \
             would be reading its own assertions as production"
        );

        let marker = concat!("include_", "bytes!(\"../assets/fonts/");
        let mut paths: Vec<&str> = Vec::new();
        for piece in production.split(marker).skip(1) {
            paths.push(piece.split('"').next().unwrap());
        }
        assert_eq!(
            paths.len(),
            CYRILLIC_GDI_FACES.len(),
            "win32_draw.rs bundles {} font files but CYRILLIC_GDI_FACES has {} rows: {paths:?}",
            paths.len(),
            CYRILLIC_GDI_FACES.len()
        );

        let theme = std::fs::read_to_string(src.join("theme.rs")).unwrap();
        assert!(
            theme.contains("CYRILLIC_FACES"),
            "control: theme.rs no longer has a CYRILLIC_FACES table, so this pin is comparing \
             against nothing"
        );
        for path in paths {
            assert!(
                theme.contains(path),
                "win32_draw.rs hands GDI `{path}`, which theme.rs does not bundle for egui. The \
                 two renderers are now drawing Cyrillic from different files -- both will look \
                 plausible and they will not match"
            );
        }
    }

    /// **The bundled faces are registered with GDI in ONE place.**
    ///
    /// A census in the shape this crate already uses for `DrawTextW`, and for
    /// a related reason. `AddFontMemResourceEx` copies the font data into the
    /// process font table; it does not refcount a shared buffer. Every card
    /// used to carry its own copy of the registration loop behind its own
    /// `OnceLock`, so a session that opened the picker and then the unlock
    /// prompt paid for two private copies of all four Archivo cuts -- and
    /// doubling the table to eight faces would have doubled that.
    ///
    /// The assertion is one-directional on purpose. `preflight_card.rs` is
    /// named as a known exception because it belongs to another workstream;
    /// it registers Archivo a second time, which costs memory and nothing
    /// else, and its Cyrillic runs are fixed anyway because it draws through
    /// `draw_text` like every other card. A tenth card growing its own loop
    /// fails this; `preflight_card` losing its one passes, which is the
    /// direction that is an improvement.
    #[test]
    fn the_bundled_faces_are_registered_in_one_place() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let needle = concat!("AddFontMem", "ResourceEx(");
        let allowed = ["win32_draw.rs", "preflight_card.rs"];
        let mut scanned = 0usize;
        let mut callers: Vec<String> = Vec::new();

        for entry in std::fs::read_dir(&src).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let raw = std::fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
            let production = raw.split(concat!("\n#[cfg(", "test)]\n")).next().unwrap();
            let code: String = production
                .lines()
                .map(|line| line.split("//").next().unwrap_or(""))
                .collect::<Vec<_>>()
                .join("\n");
            scanned += 1;
            if code.contains(needle) {
                callers.push(path.file_name().unwrap().to_string_lossy().into_owned());
            }
        }

        assert!(
            scanned > 20,
            "control: only {scanned} `.rs` files were scanned under {}, so this census is not \
             reading the crate and could not fail",
            src.display()
        );
        assert!(
            callers.contains(&"win32_draw.rs".to_string()),
            "control: win32_draw.rs no longer registers the bundled faces at all, so no card \
             gets Archivo or its Cyrillic pair and every surface falls back to the shell font. \
             Found: {callers:?}"
        );
        for caller in &callers {
            assert!(
                allowed.contains(&caller.as_str()),
                "`{caller}` registers fonts with GDI itself. `AddFontMemResourceEx` COPIES the \
                 data into the process font table rather than refcounting it, so that is another \
                 private ~750 KB of Archivo plus 64 KB of Noto for every card opened in a \
                 session. Call `win32_draw::register_fonts()` instead -- it is behind a \
                 process-wide `OnceLock`. Found: {callers:?}"
            );
        }
    }

    /// **The swap restores the DC before it deletes the font it created.**
    ///
    /// Source order again, for the same reason the empty-run guard is pinned
    /// that way: the failure is not a panic a test could catch. `DeleteObject`
    /// on a font that is still selected into a DC does not delete it -- GDI
    /// returns FALSE and the handle stays in the process table for the life of
    /// the daemon. On a tray app that repaints a list of Cyrillic rows all
    /// day, that is a handle per row per repaint, forever, ending in a GDI
    /// object limit and a window that stops drawing.
    #[test]
    fn the_cyrillic_swap_restores_before_it_deletes() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let raw =
            std::fs::read_to_string(src.join("win32_draw.rs")).unwrap().replace("\r\n", "\n");
        let production = raw.split(concat!("\n#[cfg(", "test)]\n")).next().unwrap();
        let code: String = production
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            production.len() < raw.len(),
            "control: the `#[cfg(test)]` cut marker was not found, so this pin is reading its \
             own test module"
        );
        let restore = code.find("SelectObject(hdc, previous);").expect(
            "the Cyrillic swap no longer puts the card's own font back into the DC. Everything \
             painted after the swapped run would then be drawn in a Cyrillic-only subset face",
        );
        let delete = code
            .find("DeleteObject(cyrillic)")
            .expect("control: the swapped font is never deleted, which leaks it outright");
        assert!(
            restore < delete,
            "the swapped font is deleted while it is still selected into the DC. GDI refuses \
             that and returns FALSE, so the handle leaks for the life of the daemon -- one per \
             Cyrillic run per repaint"
        );
    }
}

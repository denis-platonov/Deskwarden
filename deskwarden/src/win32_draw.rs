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
use windows::Win32::Foundation::{COLORREF, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreatePen, CreateSolidBrush, DeleteObject, DrawTextW, GetTextExtentPoint32W, Polygon, RoundRect,
    SelectObject, SetBkMode, SetTextCharacterExtra, SetTextColor, DT_CENTER, DT_END_ELLIPSIS,
    DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, HDC, HFONT, PS_SOLID,
    TRANSPARENT,
};

/// `theme`'s `Color32` as GDI's BGR `COLORREF`.
///
/// One conversion, used everywhere, so that no hex value in this file is a
/// palette entry written out a second time. Moved out of `unlock_prompt`'s
/// `win32` module, which had the only copy.
pub(crate) fn rgb(c: eframe::egui::Color32) -> COLORREF {
    COLORREF((c.r() as u32) | ((c.g() as u32) << 8) | ((c.b() as u32) << 16))
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
        let mut chars: Vec<u16> = label.encode_utf16().collect();
        let mut rc = RECT { right: rect.right - hint_lane, ..rect };
        DrawTextW(
            hdc,
            &mut chars,
            &mut rc,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
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
        let mut chars = chars;
        let mut rc = RECT { left, top, right, bottom: top + h };
        DrawTextW(hdc, &mut chars, &mut rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
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
        let mut chars: Vec<u16> = crate::theme::WORDMARK_CAPS.encode_utf16().collect();
        let mut rc = word;
        DrawTextW(hdc, &mut chars, &mut rc, DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
        SetTextCharacterExtra(hdc, 0);
        SelectObject(hdc, old);
    }
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
        let mut name_chars: Vec<u16> = candidate.name.encode_utf16().collect();
        let mut name_rc = RECT {
            left: text_left,
            top: rect.top,
            right: text_right,
            bottom: rect.top + gutter / 2,
        };
        DrawTextW(
            hdc,
            &mut name_chars,
            &mut name_rc,
            DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        SelectObject(hdc, old_font);

        SetTextColor(hdc, rgb(crate::theme::TEXT_FAINT));
        let old_font = SelectObject(hdc, user_font);
        let mut user_chars: Vec<u16> = candidate.username.encode_utf16().collect();
        let mut user_rc = RECT {
            left: text_left,
            top: rect.top + gutter / 2,
            right: text_right,
            bottom: rect.bottom,
        };
        DrawTextW(
            hdc,
            &mut user_chars,
            &mut user_rc,
            DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        SelectObject(hdc, old_font);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let drawn = code.matches(concat!("Draw", "TextW(")).count();
        assert_eq!(
            drawn, 5,
            "control: win32_draw.rs draws text in five places -- a button label, a keyboard-hint chip, a row's two lines, and the brand lockup's wordmark. It now draws it in {drawn}, so the counts below no longer mean what this pin says they mean"
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
            "five candidates against a cap of five is five candidates, and a card that dropped              one of them and reported an overflow was lying about both"
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
}

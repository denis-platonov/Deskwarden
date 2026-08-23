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
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::{
    CreatePen, CreateSolidBrush, DeleteObject, DrawTextW, RoundRect, SelectObject, SetBkMode,
    SetTextColor, DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, HDC,
    HFONT, PS_SOLID,
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

        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, skin.text);
        let old_font = SelectObject(hdc, font);
        let mut chars: Vec<u16> = label.encode_utf16().collect();
        let mut rc = rect;
        DrawTextW(
            hdc,
            &mut chars,
            &mut rc,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(hdc, old_font);
    }
}

/// How many candidate rows to draw, and whether an overflow row is needed.
///
/// **A cap that hides candidates without saying so is the defect this project
/// keeps finding.** When there are more candidates than fit, one slot is spent
/// on a *Search vault* row so the truncation is visible; that is why the
/// overflowing case shows `cap - 1` and not `cap`.
pub fn visible_rows(total: usize, cap: usize) -> (usize, bool) {
    if total <= cap {
        (total, false)
    } else {
        (cap.saturating_sub(1), true)
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
pub fn draw_row(hdc: HDC, rect: RECT, candidate: &Candidate, state: RowState, name_font: HFONT, user_font: HFONT) {
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

        let gutter = rect.bottom - rect.top;
        let text_left = rect.left + gutter;
        // `DT_END_ELLIPSIS` truncates against the rect's right edge, so the
        // rect stops short of the row's: an ellipsis flush against the card's
        // edge reads as a cut rather than as "there is more". The left gutter
        // the icon lives in is untouched.
        let text_right = rect.right - crate::theme::TEXT_CLIP_INSET as i32;

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
            drawn, 3,
            "control: win32_draw.rs draws text in three places -- a button label and a row's two \r
             lines. It now draws it in {drawn}, so the counts below no longer mean what this \r
             pin says they mean"
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
    fn a_list_that_fits_shows_everything_and_offers_no_overflow_row() {
        assert_eq!(visible_rows(3, 5), (3, false));
        assert_eq!(visible_rows(5, 5), (5, false), "exactly full is not overflowing");
    }

    #[test]
    fn a_list_that_overflows_gives_up_a_row_to_say_so() {
        let (shown, overflow) = visible_rows(9, 5);
        assert!(overflow, "the user must be told the list was cut");
        assert_eq!(
            shown, 4,
            "the overflow row occupies one of the cap's slots -- showing 5 candidates AND an \
             overflow row would be 6 rows in a window sized for 5, and the last one is unreachable"
        );
    }

    #[test]
    fn a_cap_of_one_still_leaves_room_to_say_there_is_more() {
        assert_eq!(visible_rows(4, 1), (0, true));
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

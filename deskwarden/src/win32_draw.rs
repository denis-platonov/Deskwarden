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

use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::{
    CreatePen, CreateSolidBrush, DeleteObject, DrawTextW, RoundRect, SelectObject, SetBkMode,
    SetTextColor, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, HDC, HFONT, PS_SOLID,
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
}

impl ButtonSkin {
    /// The blue call-to-action.
    pub fn primary() -> Self {
        Self { fill: rgb(crate::theme::BLUE_BRIGHT), text: rgb(crate::theme::CARD), border: None }
    }

    /// The quiet one beside it. **Bordered on purpose**: it is card-coloured
    /// on a card, so without an outline it does not read as a control.
    pub fn secondary() -> Self {
        Self {
            fill: rgb(crate::theme::CARD),
            text: rgb(crate::theme::INK),
            border: Some(rgb(crate::theme::BORDER)),
        }
    }

    /// Greyed, derived rather than hand-picked, so a palette change cannot
    /// leave the disabled variant behind.
    pub fn disabled(self) -> Self {
        Self { fill: rgb(crate::theme::TOGGLE_OFF), text: rgb(crate::theme::TEXT_GHOST), ..self }
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

#[cfg(test)]
mod tests {
    use super::*;

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

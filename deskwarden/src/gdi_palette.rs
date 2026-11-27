//! The palette the bare-Win32 cards paint with, so a card follows the
//! Light / Dark / System theme setting like the egui windows do.
//!
//! The cards are GDI windows: they cannot read an egui context, and each runs
//! on a thread of its own. So the palette is **per thread**: a card calls
//! [`open_for_card`] on its window's thread when it opens, and every painter
//! it reaches (`panel_draw`, `win32_draw`, `card_shell`, the card's own
//! `WM_PAINT`) asks the functions here for its colours. A thread that never
//! chose paints [`Palette::LIGHT`], which is also what every test sees.
//!
//! [`crate::win32_draw::rgb`] is the one place a `Color32` becomes a
//! `COLORREF`.

use crate::kit::tokens::{Palette, ThemeChoice};
use eframe::egui::Color32;
use std::cell::Cell;

thread_local! {
    static CURRENT: Cell<Palette> = const { Cell::new(Palette::LIGHT) };
    static DARK: Cell<bool> = const { Cell::new(false) };
}

/// Whether `choice` paints dark when Windows' app theme is `system_dark`.
/// Pure.
#[must_use]
pub fn is_dark_for(choice: ThemeChoice, system_dark: bool) -> bool {
    match choice {
        ThemeChoice::Light => false,
        ThemeChoice::Dark => true,
        ThemeChoice::System => system_dark,
    }
}

/// Makes this thread paint dark (or light).
pub fn set_dark(dark: bool) {
    DARK.with(|d| d.set(dark));
    CURRENT.with(|c| c.set(if dark { Palette::DARK } else { Palette::LIGHT }));
}

/// Chooses this thread's palette for `choice`, asking `system_dark` only when
/// the choice is `System`.
pub fn choose_with(choice: ThemeChoice, system_dark: impl FnOnce() -> bool) {
    let system = choice == ThemeChoice::System && system_dark();
    set_dark(is_dark_for(choice, system));
}

/// The saved theme setting, from `settings.json` (light when there is none).
fn saved_choice() -> ThemeChoice {
    crate::settings::default_path().as_deref().map(|p| crate::settings::Settings::load(p).theme).unwrap_or_default()
}

/// The previews' seam: `--dark` / `--light` on an example's command line make
/// every card opened afterwards paint that theme, whatever `settings.json`
/// says. 0 is "not forced".
static FORCED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Forces the theme every card opens with (the examples' `--dark`/`--light`).
pub fn force_for_preview(dark: bool) {
    FORCED.store(if dark { 2 } else { 1 }, std::sync::atomic::Ordering::SeqCst);
}

/// **Every card calls this once, on its window's thread, before it creates
/// its window**: the saved `Settings.theme`, with `System` following
/// Windows' app theme ([`crate::system_theme`]).
pub fn open_for_card() {
    match FORCED.load(std::sync::atomic::Ordering::SeqCst) {
        1 => set_dark(false),
        2 => set_dark(true),
        _ => choose_with(saved_choice(), crate::system_theme::apps_use_dark),
    }
}

/// Whether this thread paints dark.
#[must_use]
pub fn is_dark() -> bool {
    DARK.with(Cell::get)
}

/// This thread's palette.
#[must_use]
pub fn current() -> Palette {
    CURRENT.with(Cell::get)
}

/// The palette's `hover` (the close button's wash).
#[must_use]
pub fn hover() -> Color32 {
    current().hover
}

/// The palette's `text_label` (the close glyph's ink).
#[must_use]
pub fn text_label() -> Color32 {
    current().text_label
}

/// The palette's `danger_edge`.
#[must_use]
pub fn danger_edge() -> Color32 {
    current().danger_edge
}

/// The palette's `caution_dot`.
#[must_use]
pub fn caution_dot() -> Color32 {
    current().caution_dot
}

/// The palette's `badge_bg`.
#[must_use]
pub fn badge_bg() -> Color32 {
    current().badge_bg
}

/// What sits on a blue fill (the primary button's label, a ticked box's
/// tick): white on both themes.
#[must_use]
pub fn on_blue() -> Color32 {
    Color32::WHITE
}

/// The primary button's hover fill: the brighter blue on light, and on dark
/// (where the button already is the bright blue) the lifted one.
#[must_use]
pub fn primary_hover() -> Color32 {
    if is_dark() {
        current().blue_bright_hover
    } else {
        current().blue_bright
    }
}

/// The palette's `ink`.
#[must_use]
pub fn ink() -> Color32 {
    current().ink
}

/// The palette's `text_nav`.
#[must_use]
pub fn text_nav() -> Color32 {
    current().text_nav
}

/// The palette's `text_muted`.
#[must_use]
pub fn text_muted() -> Color32 {
    current().text_muted
}

/// The palette's `text_faint`.
#[must_use]
pub fn text_faint() -> Color32 {
    current().text_faint
}

/// The palette's `text_ghost`.
#[must_use]
pub fn text_ghost() -> Color32 {
    current().text_ghost
}

/// The palette's `card`.
#[must_use]
pub fn card() -> Color32 {
    current().card
}

/// The palette's `list_bg`.
#[must_use]
pub fn list_bg() -> Color32 {
    current().list_bg
}

/// The palette's `window_bg`.
#[must_use]
pub fn window_bg() -> Color32 {
    current().window_bg
}

/// The palette's `field_bg`.
#[must_use]
pub fn field_bg() -> Color32 {
    current().field_bg
}

/// The palette's `secondary_hover`.
#[must_use]
pub fn secondary_hover() -> Color32 {
    current().secondary_hover
}

/// The palette's `hairline`.
#[must_use]
pub fn hairline() -> Color32 {
    current().hairline
}

/// The palette's `border`.
#[must_use]
pub fn border() -> Color32 {
    current().border
}

/// The palette's `kbd_border`.
#[must_use]
pub fn kbd_border() -> Color32 {
    current().kbd_border
}

/// The palette's `switch_off`.
#[must_use]
pub fn switch_off() -> Color32 {
    current().switch_off
}

/// The palette's `blue_deep`.
#[must_use]
pub fn blue_deep() -> Color32 {
    current().blue_deep
}

/// The palette's `blue`.
#[must_use]
pub fn blue() -> Color32 {
    current().blue
}

/// The palette's `blue_bright`.
#[must_use]
pub fn blue_bright() -> Color32 {
    current().blue_bright
}

/// The palette's `blue_wash`.
#[must_use]
pub fn blue_wash() -> Color32 {
    current().blue_wash
}

/// The palette's `focus`.
#[must_use]
pub fn focus() -> Color32 {
    current().focus
}

/// The palette's `error`.
#[must_use]
pub fn error() -> Color32 {
    current().error
}

/// The palette's `danger_wash`.
#[must_use]
pub fn danger_wash() -> Color32 {
    current().danger_wash
}

/// The palette's `caution_wash`.
#[must_use]
pub fn caution_wash() -> Color32 {
    current().caution_wash
}

/// The palette's `caution_ink`.
#[must_use]
pub fn caution_ink() -> Color32 {
    current().caution_ink
}

/// The palette's `done_wash`.
#[must_use]
pub fn done_wash() -> Color32 {
    current().done_wash
}

/// The palette's `done_ink`.
#[must_use]
pub fn done_ink() -> Color32 {
    current().done_ink
}

/// The palette's `done_mark`.
#[must_use]
pub fn done_mark() -> Color32 {
    current().done_mark
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_that_never_chose_paints_light() {
        {
            assert!(!is_dark());
            assert_eq!(current(), Palette::LIGHT);
        }
    }

    #[test]
    fn the_choice_and_windows_decide_dark() {
        assert!(!is_dark_for(ThemeChoice::Light, true));
        assert!(is_dark_for(ThemeChoice::Dark, false));
        assert!(is_dark_for(ThemeChoice::System, true));
        assert!(!is_dark_for(ThemeChoice::System, false));
    }

    #[test]
    fn windows_is_asked_only_for_system() {
        {
            choose_with(ThemeChoice::Dark, || panic!("asked"));
            assert_eq!(current(), Palette::DARK);
            choose_with(ThemeChoice::Light, || panic!("asked"));
            assert_eq!(current(), Palette::LIGHT);
            choose_with(ThemeChoice::System, || true);
            assert!(is_dark());
            assert_eq!(card(), Palette::DARK.card);
        }
    }

    #[test]
    fn the_primary_button_stays_blue_on_both_themes() {
        {
            for dark in [false, true] {
                set_dark(dark);
                assert_eq!(blue(), if dark { Palette::DARK.blue } else { Palette::LIGHT.blue });
                assert_eq!(on_blue(), Color32::WHITE);
                assert_ne!(primary_hover(), blue());
            }
        }
    }
}

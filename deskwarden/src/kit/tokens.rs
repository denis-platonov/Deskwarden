//! The v2 design's tokens: its colours, its two sidebars, its type roles, its
//! radii and the card's ring-and-shadow.
//!
//! **Colours are a value, not constants.** Slice 8 gives the app a dark theme,
//! and a palette stored in the egui context lets it swap every colour at once
//! without a widget knowing. Widgets read [`palette`] once per frame.

use eframe::egui;
use egui::{Color32, CornerRadius, FontFamily, FontId, Shadow, Stroke};

use crate::theme::{BOLD, EXTRABOLD, REGULAR, SEMIBOLD};

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Every colour the v2 design uses, named by role. See the spec's part A
/// section 1.1 for where each one appears.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub ink: Color32,
    pub text_nav: Color32,
    pub text_muted: Color32,
    pub text_icon: Color32,
    pub text_faint: Color32,
    pub text_label: Color32,
    pub text_ghost: Color32,
    pub card: Color32,
    pub list_bg: Color32,
    pub window_bg: Color32,
    pub field_bg: Color32,
    pub hover: Color32,
    pub secondary_hover: Color32,
    pub badge_bg: Color32,
    pub hairline: Color32,
    pub card_ring: Color32,
    pub row_rule: Color32,
    pub border: Color32,
    pub kbd_border: Color32,
    pub switch_off: Color32,
    pub meter_track: Color32,
    pub tag_neutral_bg: Color32,
    pub tag_neutral_ink: Color32,
    pub star: Color32,
    pub blue_deep: Color32,
    pub blue: Color32,
    pub blue_bright: Color32,
    pub blue_bright_hover: Color32,
    pub blue_soft: Color32,
    pub blue_wash: Color32,
    pub blue_edge: Color32,
    pub focus: Color32,
    pub chip_on: Color32,
    pub row_hover_blue: Color32,
    pub done_mark: Color32,
    pub done_ink: Color32,
    pub done_wash: Color32,
    pub tag_2fa_bg: Color32,
    pub caution_wash: Color32,
    pub caution_mark: Color32,
    pub caution_ink: Color32,
    pub caution_edge: Color32,
    pub caution_dot: Color32,
    pub error: Color32,
    pub danger_edge: Color32,
    pub danger_wash: Color32,
    pub danger_ink: Color32,
    pub close_hover: Color32,
}

impl Palette {
    /// The design as drawn.
    pub const LIGHT: Palette = Palette {
        ink: rgb(0x201e1d),
        text_nav: rgb(0x4a4745),
        text_muted: rgb(0x605d5d),
        text_icon: rgb(0x6b6866),
        text_faint: rgb(0x7d7979),
        text_label: rgb(0x8a8785),
        text_ghost: rgb(0x9b9797),
        card: rgb(0xffffff),
        list_bg: rgb(0xfaf9f8),
        window_bg: rgb(0xf5f4f2),
        field_bg: rgb(0xf3f2f0),
        hover: rgb(0xf1f0ee),
        secondary_hover: rgb(0xf7f6f5),
        badge_bg: rgb(0xeceae7),
        hairline: rgb(0xe8e6e3),
        card_ring: rgb(0xebe9e6),
        row_rule: rgb(0xf1efec),
        border: rgb(0xe0deda),
        kbd_border: rgb(0xdcdad7),
        switch_off: rgb(0xd9d6d2),
        meter_track: rgb(0xeeece9),
        tag_neutral_bg: rgb(0xe4e1dd),
        tag_neutral_ink: rgb(0x2e2b29),
        star: rgb(0xf5b301),
        blue_deep: rgb(0x14307a),
        blue: rgb(0x1b3fa0),
        blue_bright: rgb(0x3b74e8),
        blue_bright_hover: rgb(0x2f63cf),
        blue_soft: rgb(0x7fa4ef),
        blue_wash: rgb(0xeef2fc),
        blue_edge: rgb(0xb8c7ea),
        focus: rgb(0xdbe4f7),
        chip_on: rgb(0xf5f7fd),
        row_hover_blue: rgb(0xf7f8fc),
        done_mark: rgb(0x1b7a3f),
        done_ink: rgb(0x17673a),
        done_wash: rgb(0xe8f3ec),
        tag_2fa_bg: rgb(0xdff0e5),
        caution_wash: rgb(0xfef6e7),
        caution_mark: rgb(0x8a5a06),
        caution_ink: rgb(0x7a4f05),
        caution_edge: rgb(0xf2d99b),
        caution_dot: rgb(0xb7791f),
        error: rgb(0xb42318),
        danger_edge: rgb(0xf0cfca),
        danger_wash: rgb(0xfdf3f2),
        danger_ink: rgb(0x8c3c33),
        close_hover: rgb(0xc42b1c),
    };
}

/// The palette this context's widgets paint with; [`Palette::LIGHT`] until
/// something sets another.
pub fn palette(ctx: &egui::Context) -> Palette {
    ctx.data(|d| d.get_temp::<Palette>(egui::Id::NULL)).unwrap_or(Palette::LIGHT)
}

/// Swaps the palette for every widget in this context, from the next frame.
pub fn set_palette(ctx: &egui::Context, palette: Palette) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::NULL, palette));
}

/// The design's two sidebars, chosen by a setting (General > Look).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SidebarStyle {
    #[default]
    Navy,
    Light,
}

/// One sidebar's colours (spec part A section 2.2, the `sb` table).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SidebarPalette {
    pub bg: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub nav_fg: Color32,
    pub hover: Color32,
    pub active: Color32,
    pub active_fg: Color32,
    pub new_bg: Color32,
    pub new_fg: Color32,
    /// The mark's top-left quadrant, which must contrast with `bg`.
    pub q1: Color32,
}

impl SidebarStyle {
    pub fn palette(self) -> SidebarPalette {
        let white_at = |alpha| Color32::from_rgba_unmultiplied(255, 255, 255, alpha);
        match self {
            SidebarStyle::Navy => SidebarPalette {
                bg: rgb(0x14307a),
                text: rgb(0xffffff),
                // rgba(255,255,255,.5 / .82 / .08 / .16)
                muted: white_at(128),
                nav_fg: white_at(209),
                hover: white_at(20),
                active: white_at(41),
                active_fg: rgb(0xffffff),
                new_bg: rgb(0xffffff),
                new_fg: rgb(0x14307a),
                q1: rgb(0xffffff),
            },
            SidebarStyle::Light => SidebarPalette {
                bg: rgb(0xffffff),
                text: rgb(0x201e1d),
                muted: rgb(0x9b9797),
                nav_fg: rgb(0x4a4745),
                hover: rgb(0xf3f2f0),
                active: rgb(0xeef2fc),
                active_fg: rgb(0x14307a),
                new_bg: rgb(0x1b3fa0),
                new_fg: rgb(0xffffff),
                q1: rgb(0x14307a),
            },
        }
    }
}

pub const R_WINDOW: u8 = 18;
pub const R_CARD: u8 = 18;
pub const R_CARD_SETTINGS: u8 = 20;
pub const R_HERO: u8 = 22;
pub const R_CONTROL: u8 = 14;
pub const R_NAV: u8 = 12;
pub const R_SMALL: u8 = 11;
pub const R_TAG_LIST: u8 = 6;
pub const R_TAG_DETAIL: u8 = 7;
pub const R_PILL: u8 = 255;

/// One step of the design's type scale. `face` is one of `theme`'s Archivo
/// faces: the design's 500 is Regular and its 900 is ExtraBold, because the
/// app ships no Medium or Black cut. `tracking_em` is CSS `letter-spacing` in
/// em.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Role {
    pub size: f32,
    pub face: &'static str,
    pub tracking_em: f32,
}

const fn role(size: f32, face: &'static str, tracking_em: f32) -> Role {
    Role { size, face, tracking_em }
}

pub const PAGE_TITLE: Role = role(32.0, EXTRABOLD, -0.03);
pub const LIST_HEADER: Role = role(26.0, EXTRABOLD, -0.02);
pub const WORDMARK: Role = role(20.0, EXTRABOLD, -0.02);
pub const FIELD_VALUE: Role = role(18.0, REGULAR, 0.0);
pub const TOTP: Role = role(26.0, BOLD, 0.12);
pub const BUTTON: Role = role(16.0, BOLD, 0.0);
pub const ROW_TITLE: Role = role(16.0, BOLD, 0.0);
pub const NAV: Role = role(15.0, SEMIBOLD, 0.0);
pub const BODY: Role = role(15.0, REGULAR, 0.0);
pub const SUBTITLE: Role = role(14.0, REGULAR, 0.0);
pub const LABEL: Role = role(13.0, SEMIBOLD, 0.0);
/// The sidebar's VAULT / TOOLS / FOLDERS: always set in capitals.
pub const SECTION: Role = role(12.0, BOLD, 0.12);
pub const TAG_LIST: Role = role(11.0, BOLD, 0.0);
pub const TAG_DETAIL: Role = role(12.0, BOLD, 0.0);
pub const TOOLTIP: Role = role(14.0, REGULAR, 0.0);
pub const HINT: Role = role(13.0, SEMIBOLD, 0.0);
pub const PILL: Role = role(13.0, SEMIBOLD, 0.0);
pub const SEGMENT: Role = role(14.0, BOLD, 0.0);

/// The font a role is set in. Regular is egui's proportional family itself;
/// the heavier cuts are registered by name in `theme`.
pub fn font_id(role: Role) -> FontId {
    let family = if role.face == REGULAR {
        FontFamily::Proportional
    } else {
        FontFamily::Name(role.face.into())
    };
    FontId::new(role.size, family)
}

/// `text` set in `role`, tracked as the design tracks it.
pub fn job(role: Role, text: &str, color: Color32) -> egui::text::LayoutJob {
    let text = if role == SECTION { text.to_uppercase() } else { text.to_owned() };
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &text,
        0.0,
        egui::TextFormat {
            font_id: font_id(role),
            color,
            extra_letter_spacing: role.size * role.tracking_em,
            ..Default::default()
        },
    );
    job
}

/// The height of one line set in `role`, as laid out in `ctx`.
pub fn font_height(ctx: &egui::Context, role: Role) -> f32 {
    ctx.fonts_mut(|f| f.layout_job(job(role, "Ag", Color32::BLACK)).size().y)
}

/// A card's soft drop shadow: `0 1px 3px rgba(32,30,29,.06)`.
pub fn card_shadow() -> Shadow {
    Shadow {
        offset: [0, 1],
        blur: 3,
        spread: 0,
        color: Color32::from_rgba_unmultiplied(32, 30, 29, 15),
    }
}

/// A white card with the design's 1 px ring (`0 0 0 1px #ebe9e6`) and soft
/// shadow, at `radius`.
pub fn card_frame(palette: &Palette, radius: u8) -> egui::Frame {
    egui::Frame::new()
        .fill(palette.card)
        .corner_radius(CornerRadius::same(radius))
        .stroke(Stroke::new(1.0, palette.card_ring))
        .shadow(card_shadow())
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Color32, CornerRadius, Stroke};

    fn rgb(hex: u32) -> Color32 {
        Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
    }

    fn white_at(alpha: u8) -> Color32 {
        Color32::from_rgba_unmultiplied(255, 255, 255, alpha)
    }

    #[test]
    fn light_palette_matches_the_design() {
        let p = Palette::LIGHT;
        assert_eq!(p.ink, rgb(0x201e1d));
        assert_eq!(p.window_bg, rgb(0xf5f4f2));
        assert_eq!(p.list_bg, rgb(0xfaf9f8));
        assert_eq!(p.border, rgb(0xe0deda));
        assert_eq!(p.card_ring, rgb(0xebe9e6));
        assert_eq!(p.row_rule, rgb(0xf1efec));
        assert_eq!(p.switch_off, rgb(0xd9d6d2));
        assert_eq!(p.close_hover, rgb(0xc42b1c));
    }

    #[test]
    fn navy_and_light_sidebars_match_the_design_table() {
        let navy = SidebarStyle::Navy.palette();
        assert_eq!(navy.bg, rgb(0x14307a));
        assert_eq!(navy.text, rgb(0xffffff));
        assert_eq!(navy.muted, white_at(128));
        assert_eq!(navy.nav_fg, white_at(209));
        assert_eq!(navy.hover, white_at(20));
        assert_eq!(navy.active, white_at(41));
        assert_eq!(navy.active_fg, rgb(0xffffff));
        assert_eq!(navy.new_bg, rgb(0xffffff));
        assert_eq!(navy.new_fg, rgb(0x14307a));
        assert_eq!(navy.q1, rgb(0xffffff));

        let light = SidebarStyle::Light.palette();
        assert_eq!(light.bg, rgb(0xffffff));
        assert_eq!(light.text, rgb(0x201e1d));
        assert_eq!(light.muted, rgb(0x9b9797));
        assert_eq!(light.nav_fg, rgb(0x4a4745));
        assert_eq!(light.hover, rgb(0xf3f2f0));
        assert_eq!(light.active, rgb(0xeef2fc));
        assert_eq!(light.active_fg, rgb(0x14307a));
        assert_eq!(light.new_bg, rgb(0x1b3fa0));
        assert_eq!(light.new_fg, rgb(0xffffff));
        assert_eq!(light.q1, rgb(0x14307a));

        assert_eq!(SidebarStyle::default(), SidebarStyle::Navy);
    }

    #[test]
    fn the_palette_defaults_to_light_and_can_be_swapped() {
        let ctx = egui::Context::default();
        assert_eq!(palette(&ctx), Palette::LIGHT);
        let changed = Palette { ink: rgb(0x010203), ..Palette::LIGHT };
        set_palette(&ctx, changed);
        assert_eq!(palette(&ctx), changed);
    }

    #[test]
    fn section_role_text_is_uppercased_and_tracked() {
        let job = job(SECTION, "Vault", rgb(0x9b9797));
        assert_eq!(job.text, "VAULT");
        let format = &job.sections[0].format;
        assert!((format.extra_letter_spacing - 12.0 * 0.12).abs() < 1e-4);
        assert_eq!(format.font_id.size, 12.0);
    }

    #[test]
    fn regular_roles_use_the_proportional_family() {
        let job = job(BODY, "Search your vault", rgb(0x201e1d));
        assert_eq!(job.sections[0].format.font_id.family, egui::FontFamily::Proportional);
        let bold = job_family(BUTTON);
        assert_eq!(bold, egui::FontFamily::Name(crate::theme::BOLD.into()));
    }

    fn job_family(role: Role) -> egui::FontFamily {
        job(role, "x", rgb(0)).sections[0].format.font_id.family.clone()
    }

    #[test]
    fn cards_carry_the_ring_and_the_soft_shadow() {
        let frame = card_frame(&Palette::LIGHT, R_CARD);
        assert_eq!(frame.fill, Color32::WHITE);
        assert_eq!(frame.stroke, Stroke::new(1.0, rgb(0xebe9e6)));
        assert_eq!(frame.corner_radius, CornerRadius::same(18));
        assert_eq!(frame.shadow.offset, [0, 1]);
        assert_eq!(frame.shadow.blur, 3);
        assert_eq!(frame.shadow.color, Color32::from_rgba_unmultiplied(32, 30, 29, 15));
    }
}

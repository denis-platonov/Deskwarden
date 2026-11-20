//! The v2 design's widget kit ("Friendlier, bigger, alive").
//!
//! Everything the v2 look needs lives here: tokens, motion, icons, controls,
//! cards, help bubbles, hints and toasts. `theme.rs` stays the v1 look, and
//! each screen moves onto this kit in its own slice of the 1.0 work (see
//! `docs/superpowers/specs/2026-11-18-deskwarden-1-0-redesign-design.md`).
//! The spec these values come from is `docs/design/deskwarden-v2-design-spec.md`.

pub mod auth;
pub mod avatar;
pub mod card;
pub mod controls;
pub mod field;
pub mod icon;
pub mod input;
pub mod logo_paths;
pub mod logos;
pub mod menu;
pub mod modal;
pub mod motion;
#[cfg(test)]
pub(crate) mod testing;
pub mod tips;
pub mod toast;
pub mod tokens;

//! The v2 design's widget kit ("Friendlier, bigger, alive").
//!
//! Everything the v2 look needs lives here: tokens, motion, icons, controls,
//! cards, help bubbles, hints and toasts. `theme.rs` stays the v1 look, and
//! each screen moves onto this kit in its own slice of the 1.0 work (see
//! `docs/superpowers/specs/2026-11-18-deskwarden-1-0-redesign-design.md`).
//! The spec these values come from is `docs/design/deskwarden-v2-design-spec.md`.

pub mod controls;
pub mod icon;
pub mod motion;
#[cfg(test)]
pub(crate) mod testing;
pub mod tokens;

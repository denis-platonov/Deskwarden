//! `nodewarden-native` — a Windows background app that watches the foreground
//! window, matches it against Bitwarden vault items (via `bw serve`), and fills
//! credentials into the matched app.
//!
//! This is the single, authoritative module list for the crate. The `main.rs`
//! binary target declares **no** modules of its own -- it only `use`s items
//! from this library. Declaring modules in both targets would compile every
//! shared module twice into two unrelated type universes, run every `#[test]`
//! twice, and leave bin-only modules unreachable from examples and
//! integration tests.

pub mod app;
pub mod app_match;
pub mod bw_serve;
pub mod dispatch;
pub mod hotkey;
pub mod injector;
pub mod job_object;
pub mod logging;
pub mod login_ui;
pub mod match_engine;
pub mod overlay_ui;
pub mod picker_ui;
pub mod process_list;
pub mod session_store;
pub mod tray;
pub mod vault_bridge;
pub mod window_watch;

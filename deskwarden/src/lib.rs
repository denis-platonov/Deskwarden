//! `deskwarden` — a Windows background app that watches the foreground
//! window, matches it against Bitwarden vault items (via `bw serve`), and fills
//! credentials into the matched app.
//!
//! This is the single, authoritative module list for the crate. The `main.rs`
//! binary target declares **no** modules of its own -- it only `use`s items
//! from this library. Declaring modules in both targets would compile every
//! shared module twice into two unrelated type universes, run every `#[test]`
//! twice, and leave bin-only modules unreachable from examples and
//! integration tests.

pub mod accounts;
pub mod app;
pub mod app_identity;
pub mod app_match;
pub mod app_window;
pub mod backend_policy;
pub mod bw_path;
pub mod bw_serve;
pub mod dispatch;
pub mod favicon;
pub mod file_picker;
pub mod fill_stats;
pub mod foreground;
pub mod hello;
pub mod hotkey;
pub mod http_agent;
pub mod icon;
pub mod injector;
pub mod job_object;
pub mod key_sequence;
pub mod loading_ui;
pub mod logging;
pub mod login_ui;
pub mod match_engine;
pub mod overlay_ui;
pub mod password_strength;
pub mod picker_ui;
pub mod prefs_ui;
pub mod session_store;
pub mod settings;
pub mod signature;
pub mod theme;
pub mod tray;
pub mod updater;
pub mod vault_bridge;
pub mod vault_cache;
pub mod vault_window;
pub mod window_list;
pub mod window_watch;

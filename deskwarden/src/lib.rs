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
pub mod app_candidates;
pub mod app_identity;
pub mod app_match;
/// The named Windows mutex the installer looks for before replacing this
/// app's files. Held for the life of the process; see its own docs for why
/// setup asks the user to close Deskwarden rather than force-closing it.
pub mod app_mutex;
pub mod app_window;
/// Locking the vault when Windows says the user walked away -- Win+L, a
/// session switch, or a suspend. It creates NO window: the notifications are
/// registered on the helper window the tray already owns, and are classified
/// in the message pump `main` already runs.
pub mod away_lock;
pub mod backend_policy;
/// The one brace matcher every below-the-cut source walk uses.
/// Test-only at the declaration, so nothing in it can ship.
#[cfg(test)]
pub mod below_cut;
/// Brand logo files on disk: where a card network's mark is looked for, what
/// is refused, and how one is normalised. It draws nothing -- [`card_mark`]
/// does that, and falls back to the wordmark whenever this module has no
/// answer for a brand.
pub mod brand_mark;
pub mod breach;
/// The whole-vault breach scan the Preferences page drives: one lookup per
/// distinct password, bounded concurrency, failures counted out loud, and
/// nothing that starts on its own.
pub mod breach_scan;
/// `#[cfg(test)]` for [`below_cut`]'s reason: it reads this crate's own
/// source text to guard it, and nothing in it can ship.
#[cfg(test)]
pub mod debug_leak_guard;
pub mod bw_path;
pub mod bw_serve;
pub mod card_brand;
pub mod card_mark;
pub mod clipboard;
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
/// Civil dates, and the one place a stored UTC instant becomes the wall clock
/// the user is reading. Store UTC, display local, never print "UTC".
pub mod local_time;
pub mod logging;
pub mod login_ui;
pub mod match_engine;
/// Parses and renders `otpauth://totp` URIs. Pure, and no I/O at all.
pub mod otpauth;
pub mod overlay_ui;
pub mod password_strength;
pub mod picker_prompt;
pub mod picker_ui;
pub mod preflight_host;
pub mod prefs_ui;
pub mod qr;
pub mod record;
/// The 6b dimmed full-screen surface a QR code is dragged out of. **At the
/// crate root and not under `vault_window`**, which carries a guard requiring
/// every UI module there to have a production caller; this one has none until
/// the picker is wired to it, exactly as `screen_capture` below has none yet.
pub mod region_overlay;
pub mod reprompt;
/// `scan_history.json`: what the last twenty breach scans counted. Counts and
/// timestamps only -- never a password, an item, or anything derived from one.
pub mod scan_history;
pub mod scratch_window;
/// Copies one rectangle of the screen into a self-wiping buffer of pixels.
/// The only OS-touching part of "scan a region of my screen"; everything
/// downstream of it is a pure function.
pub mod screen_capture;
pub mod send;
pub mod session_store;
pub mod settings;
pub mod signature;
pub mod single_instance;
/// Seeding a [`vault_cache::VaultCache`] for a test with no backend at all --
/// no `mockito` server, no port, no round-trip. Test-only at the declaration,
/// exactly like [`below_cut`], so nothing in it can ship.
#[cfg(test)]
pub mod test_vault;
pub mod theme;
pub mod tray;
pub mod update_panel;
pub mod updater;
/// The daemon's bare-Win32 unlock prompt: the app asking for the master
/// password without launching the app. Opens a window and never a GL context,
/// which is the whole point -- see the module doc.
pub mod unlock_prompt;
pub mod vault_bridge;
pub mod vault_cache;
pub mod vault_disk_cache;
pub mod vault_export;
pub mod vault_window;
pub mod win32_draw;
pub mod window_list;
pub mod window_watch;

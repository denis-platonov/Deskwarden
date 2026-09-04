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
pub mod api_key_ui;
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
/// Healing a logon entry written before the `--autostart` flag existed. An
/// install that predates it draws a window at every sign-in and holds the
/// graphics driver for the session; no update can fix it, because updates
/// deliberately never rewrite that value.
pub mod autostart_repair;
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
pub mod bw_acquire;
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
pub mod generate_prompt;
pub mod hello;
pub mod hotkey;
pub mod http_agent;
pub mod icon;
pub mod injector;
pub mod job_object;
pub mod key_sequence;
pub mod kind_mark;
pub mod loading_ui;
/// Civil dates, and the one place a stored UTC instant becomes the wall clock
/// the user is reading. Store UTC, display local, never print "UTC".
pub mod local_time;
pub mod logging;
/// Design 3b: the locked-vault card, in bare Win32.
pub mod locked_card;
pub mod login_ui;
pub mod match_engine;
/// Parses and renders `otpauth://totp` URIs. Pure, and no I/O at all.
pub mod otpauth;
/// This crate's own password generator -- the one vault operation no server
/// has an endpoint for. Beside the backends rather than inside one, because
/// every backend that is not `bw serve` needs it. Passphrases are refused by
/// name until a wordlist is a decision the owner has taken.
pub mod password_gen;
pub mod password_strength;
pub mod picker_prompt;
pub mod picker_ui;
/// Design 4b: the send preflight, in bare Win32. **The card that took the
/// daemon's fill path to zero GL contexts** -- see the module doc.
pub mod preflight_card;
pub mod prefs_ui;
/// Design 2a: the matched-item autofill prompt, in bare Win32.
pub mod prompt_card;
pub mod qr;
pub mod record;
/// The 6b dimmed full-screen surface a QR code is dragged out of. **At the
/// crate root and not under `vault_window`**, which carries a guard requiring
/// every UI module there to have a production caller; this one has none until
/// the picker is wired to it, exactly as `screen_capture` below has none yet.
pub mod region_overlay;
pub mod reprompt;
/// The direct-REST vault backend, without the `bw` CLI. Only its client-side
/// cryptography exists so far and nothing in the running app reaches it: no
/// HTTP, no API client, no login flow, no wiring. It changes what this
/// process holds -- see `rest::crypto`'s module docs, which is where that is
/// written down and where `PRIVACY.md` is flagged for revisiting.
pub mod rest;
/// `scan_history.json`: what the last twenty breach scans counted. Counts and
/// timestamps only -- never a password, an item, or anything derived from one.
pub mod save_login_card;
pub mod scan_history;
pub mod scratch_window;
/// Copies one rectangle of the screen into a self-wiping buffer of pixels.
/// The only OS-touching part of "scan a region of my screen"; everything
/// downstream of it is a pure function.
pub mod screen_capture;
/// The second-factor prompt: the stage between the sign-in card and the
/// spinner. Its state, its copy and its pure decisions -- and never a
/// `Challenge`, which stays on the sign-in worker thread.
pub mod second_factor_ui;
pub mod send;
/// Who may call what, for the local vault service: one pure function over a
/// method, a path and a credential. Binds nothing, and speaks `bw serve`'s
/// API rather than a new one.
pub mod service_api;
/// The socket the local vault service owns, and the few decisions that come
/// with owning one: loopback always, which lifetime it is running under, and
/// what status each decided answer carries.
pub mod service_host;
/// What the local vault service answers with, and what a key is allowed to
/// see of it. `bw serve`'s envelope, and a list filtered to the asking key
/// rather than refused outright.
pub mod service_body;
/// Named API keys for the local vault service: what one may do, and until
/// when. Default deny throughout, including for a subject this build has
/// never heard of -- see the module doc for why that direction is not a
/// matter of taste.
pub mod service_keys;
/// The bearer token the local vault service requires. Read its module doc
/// for what it stops and -- more importantly -- what it does not: a program
/// already running as the owner is not kept out by it.
pub mod service_token;
pub mod session_store;
pub mod settings;
pub mod signature;
pub mod single_instance;
/// The one place in this repository a test HTTP server is stood up -- the
/// hand-rolled listener that replaced `mockito`, whose 1.7.2 server resets
/// accepted connections (`os error 10054`). Test-only at the declaration,
/// exactly like [`below_cut`], so nothing in it can ship.
///
/// The `test-support` half of the gate is what `main.rs` reaches it through.
/// `main.rs` is a different crate and links this library built WITHOUT
/// `cfg(test)`, so a bare `#[cfg(test)]` here left it with no mock server at
/// all and it stayed on `mockito` -- and flaked. `Cargo.toml`'s
/// `[dev-dependencies]` turns the feature on for test targets only; a
/// `cargo build` resolves no dev-dependencies, so a shipped binary still has
/// no `test_http` in it.
#[cfg(any(test, feature = "test-support"))]
pub mod test_http;
/// The one scratch directory every test in this crate writes into, and the
/// `Drop` guard that removes it -- including when the test panics, which is
/// the case every hand-rolled tidy-up at the end of a test body missed.
/// Test-only at the declaration, and gated like [`test_http`] above for
/// exactly its reason: `main.rs` is a separate crate linking this library
/// built WITHOUT `cfg(test)`, and it has scratch directories of its own.
#[cfg(any(test, feature = "test-support"))]
pub mod test_scratch;
/// Seeding a [`vault_cache::VaultCache`] for a test with no backend at all --
/// no mock server, no port, no round-trip. Test-only at the declaration,
/// exactly like [`below_cut`], so nothing in it can ship; gated like
/// [`test_http`] above, and for the same reason.
#[cfg(any(test, feature = "test-support"))]
pub mod test_vault;
pub mod theme;
pub mod tray;
pub mod update_panel;
pub mod updater;
/// The daemon's bare-Win32 unlock prompt: the app asking for the master
/// password without launching the app. Opens a window and never a GL context,
/// which is the whole point -- see the module doc.
pub mod unlock_prompt;
/// The daemon/UI process boundary: what goes out on a `--ui` command line
/// (a mode and a surface, never a secret) and what comes back (a small
/// non-secret result file, plus the exit code that carries `locked` even if
/// the file is lost).
pub mod ui_process;
/// One account's master key and refresh token, DPAPI-wrapped, at a path its
/// caller chooses -- `session_store`'s idiom applied to a stronger secret.
/// See its own docs for what a caller owes the user when a stored record
/// stops working.
pub mod user_key_store;
/// The seam between the app and whatever is holding the vault: the twenty-one
/// vault operations as a trait, implemented by `bw serve`'s client and by
/// the direct-REST backend. Nothing about what any call does over the wire
/// lives here.
pub mod vault_backend;
pub mod vault_bridge;
pub mod ui_show;
pub mod vault_cache;
pub mod vault_disk_cache;
pub mod vault_export;
/// Who is currently using the vault, as a fact the kernel keeps rather than
/// a count this app maintains -- so an app that crashes stops counting
/// without having to say so.
pub mod vault_service;
pub mod vault_window;
pub mod win32_draw;
pub mod window_list;
pub mod window_watch;

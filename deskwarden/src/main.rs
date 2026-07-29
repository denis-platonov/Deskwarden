// Builds this binary against the Windows GUI subsystem instead of the console
// subsystem (the default). Without it, every launch -- including the
// autostart-on-login the installer registers -- pops a black console window
// next to the tray icon, and closing that window kills the whole app.
//
// This attribute applies to the crate it appears in, and a Cargo binary
// target is its own crate rooted at this file: it belongs here, in main.rs,
// and specifically NOT in lib.rs (where it would do nothing for this binary
// and would apply to `cargo test`'s harness and the examples, which *do* want
// a console). Verified on the built artifact by reading the PE optional
// header's Subsystem field (2 = GUI, 3 = console).
//
// Consequence: `println!`/`eprintln!` from this binary go nowhere. There is
// exactly one such call left (the logging-init failure below, which by
// definition can't use the log file), and it is a fallback for a case the
// user cannot act on anyway. Everything user-facing goes through the tray,
// the log file, or -- for the startup failures that happen before the tray
// exists -- a native message box; see `message_box`/`fatal_startup_error`.
#![windows_subsystem = "windows"]

//! Binary entry point.
//!
//! Declares no modules of its own: every module lives in `lib.rs` (see the
//! note there). This file is only `fn main()` and the startup sequence.

use deskwarden::app::{
    fill_from_vault, handle_match, match_entries, pump_windows_messages, refresh_match_engine,
};
use deskwarden::bw_serve::{
    self, readiness_schedule, wait_for_vault_ready, BW_SERVE_URL, READINESS_DEADLINE,
};
use deskwarden::dispatch;
use deskwarden::injector::{Injector, RealSendInput, RealUiAutomation};
use deskwarden::match_engine::MatchEngine;
use deskwarden::updater::{self, ReleaseInfo};
use deskwarden::vault_bridge::VaultBridge;
use deskwarden::{
    hotkey, job_object, logging, login_ui, picker_ui, session_store, tray, window_watch,
};
use semver::Version;
use std::process::Child;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use windows::core::HSTRING;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONERROR, MB_ICONWARNING, MB_OK,
    MB_SETFOREGROUND, MB_SYSTEMMODAL, MB_YESNO, MESSAGEBOX_RESULT, MESSAGEBOX_STYLE,
};

/// How often the match engine is rebuilt from the vault while running, so
/// matches added via the picker (or synced from another device) take effect
/// without restarting the app.
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// How often to poll GitHub for a newer release. Checked on startup and then
/// on this cadence from the main loop, same pattern as `REFRESH_INTERVAL`.
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Real GitHub REST API base, passed to `updater::check_for_update`. Not
/// `github.com` itself -- that's the web UI host; `api.github.com` is the
/// API host the releases endpoint actually lives on.
const GITHUB_API_BASE: &str = "https://api.github.com";

/// Authenticode thumbprint an update's signature must match before it's
/// trusted and applied.
///
/// TODO: set once SignPath cert is issued (Task 5's manual prerequisite).
/// The real certificate deskwarden's release builds will be signed with does
/// not exist yet at this point in the project, so there is no genuine value
/// to put here. This placeholder is intentionally not a plausible-looking
/// thumbprint: it can never match a real signature, so `is_trusted_signer`
/// (and therefore `download_and_verify`) fails closed -- refusing every
/// update -- until this constant is replaced with the real one.
const EXPECTED_SIGNER_THUMBPRINT: &str = "PLACEHOLDER_SET_ONCE_SIGNPATH_CERT_ISSUED";

/// Organization (`O=`) values accepted as proof that the resolved `bw.exe`
/// (see `bw_path::resolve_bw_exe`) really is Bitwarden's own CLI.
///
/// Mirrors `$BitwardenSignerOrganizations` in `installer/bootstrap-bw.ps1` --
/// kept in sync there by hand, not shared code, since one runs at install
/// time (PowerShell) and this runs at every app startup (Rust). Pinning the
/// *path* `bw.exe` is resolved from isn't enough on its own: the installer's
/// `bin` directory is itself inside deskwarden's user-writable install tree,
/// so anything able to plant a file beside `deskwarden.exe` can just as
/// easily overwrite `bin\bw.exe`. This is the check that actually matters --
/// whatever ends up at that path must be signed by Bitwarden before it's
/// handed the user's master password (`login_ui::run_bw_with_password`) or
/// session token.
///
/// TODO (verify before shipping): this list has not yet been confirmed
/// against a real Bitwarden-signed `bw.exe` -- see the identical TODO on
/// `$BitwardenSignerOrganizations` in `installer/bootstrap-bw.ps1` for the
/// verification step.
///
/// Because the list is *known to be unverified*, a mismatch is deliberately
/// **not** treated the same way `EXPECTED_SIGNER_THUMBPRINT` treats a bad
/// update signature. See `check_bw_signature` below for the graded response
/// and the reasoning behind it: an unsigned or tamper-detected binary is
/// still refused outright, but "validly signed by an organization this
/// unverified list doesn't happen to name" asks the user instead of killing a
/// tray app with no console and no explanation.
const TRUSTED_BW_SIGNER_ORGANIZATIONS: &[&str] = &[
    "Bitwarden Inc.",
    "Bitwarden, Inc.",
    "Bitwarden Inc",
    "Bitwarden",
    "8bit Solutions LLC",
];

fn main() {
    let project_dirs = directories::ProjectDirs::from("dev", "Deskwarden", "Deskwarden")
        .expect("could not resolve config directory");
    let config_dir = project_dirs.config_dir().to_path_buf();
    std::fs::create_dir_all(&config_dir).expect("failed to create config directory");

    // Downloaded installers go under the *cache* directory, not next to
    // `session.bin` and the log in the config directory: they are large,
    // disposable, and regenerable, which is exactly what a cache directory is
    // for -- and keeping multi-megabyte attacker-supplied-until-verified
    // downloads out of the directory holding the encrypted session token is
    // worth the one extra path. Created lazily by `download_and_verify`.
    let update_download_dir = project_dirs.cache_dir().join("updates");

    // Logging first: a background tray app has no console, so without a log
    // file every failure below is invisible to whoever has to diagnose it.
    match logging::init(&config_dir) {
        Ok(path) => log::info!("deskwarden starting; logging to {}", path.display()),
        Err(e) => eprintln!("warning: {e}"),
    }

    // Building with windows_subsystem = "windows" (no console) means a
    // panic's default stderr backtrace goes nowhere -- the process just
    // vanishes with zero trace, which is exactly the invisibility logging
    // was added to eliminate. Route panics into the same log file instead.
    std::panic::set_hook(Box::new(|info| {
        log::error!("panicked: {info}");
    }));

    // Verified once, up front, before anything below spawns the CLI or shows
    // the login window: `bw_serve`/`login_ui` hand this binary the user's
    // master password and, afterwards, their live session token. Refusing to
    // proceed with an unsigned or wrongly-signed `bw.exe` is the whole point
    // of resolving it to a specific path in the first place -- see
    // `bw_path::resolve_bw_exe` and `TRUSTED_BW_SIGNER_ORGANIZATIONS` above.
    let Some(bw_exe) = deskwarden::bw_path::resolve_bw_exe() else {
        fatal_startup_error(
            "Deskwarden could not work out its own install directory, so it cannot tell which \
             bw.exe is the real Bitwarden CLI.\n\nRather than guess -- and risk handing your \
             master password to the wrong program -- it is stopping here.\n\nReinstalling \
             Deskwarden should fix this.",
        );
    };
    if !bw_exe.exists() {
        fatal_startup_error(&format!(
            "Deskwarden needs the Bitwarden CLI (bw.exe) and could not find it.\n\nExpected it \
             at:\n{}\n\nInstall the Bitwarden CLI, or reinstall Deskwarden (its installer \
             downloads a signed copy for you).",
            bw_exe.display()
        ));
    }
    check_bw_signature(&bw_exe);

    // Resolved and verified exactly once, here. Everything that later spawns
    // the CLI -- `bw_serve`, `login_ui`, including the one call that hands
    // over the master password -- reads this single recorded result instead of
    // re-resolving, so a `bw.exe` appearing on disk *after* this point can
    // never be the one that gets the secrets.
    deskwarden::bw_path::remember_verified_bw_exe(bw_exe);

    // Any installer still sitting in the download directory is spent by now:
    // either it was applied (and this process is the result) or its attempt
    // failed. Deleting them here rather than after applying one is not a
    // stylistic choice -- `apply_update` launches the installer and this
    // process exits immediately after, so at that moment the file is a
    // running process image and cannot be deleted.
    match updater::cleanup_stale_downloads(&update_download_dir) {
        Ok(0) => {}
        Ok(n) => log::info!("cleaned up {n} stale update download(s)"),
        Err(e) => log::warn!("could not clean up stale update downloads: {e}"),
    }

    let session_path = config_dir.join("session.bin");
    let store = session_store::SessionStore::new(session_path);

    // Every child process we spawn joins this job object, which is configured
    // to kill its members when the last handle closes. Our handles close when
    // this process dies for *any* reason -- clean quit, panic, Ctrl+C, Task
    // Manager -- so `bw serve` can no longer be orphaned holding an unlocked
    // vault open on localhost. This must outlive the whole run, hence the
    // binding here rather than inside the spawn helper.
    let job = match job_object::KillOnCloseJob::new() {
        Ok(job) => Some(job),
        Err(e) => {
            log::error!(
                "could not create a kill-on-close job object ({e}); `bw serve` will only be \
                 cleaned up on a clean quit"
            );
            None
        }
    };

    // A cached session token is worthless if it has since been invalidated
    // (manual `bw lock`, password change, reboot). Trusting it unconditionally
    // is how the app used to proceed "unlocked" with no recovery path.
    let mut session_token = match store.load() {
        Some(token) => match login_ui::check_bw_status_with_session(Some(&token)) {
            login_ui::BwStatus::Unlocked => {
                log::info!("cached session token verified as unlocked");
                token
            }
            other => {
                log::warn!("cached session token reports {other:?}; re-authenticating");
                reauthenticate(&store)
            }
        },
        None => {
            log::info!("no cached session token; showing login flow");
            reauthenticate(&store)
        }
    };

    let vault = VaultBridge::new(BW_SERVE_URL);
    let mut engine = MatchEngine::new();

    let mut bw_serve_child = start_backend(&session_token, job.as_ref());

    // `bw serve` is a bundled Node binary: its cold start regularly takes
    // several seconds, far longer than the fixed 500ms sleep this replaces.
    // Losing that race used to leave the match engine permanently empty with
    // no diagnostic, so the app silently did nothing forever.
    let schedule = readiness_schedule(READINESS_DEADLINE);
    let items = match wait_for_vault_ready(&vault, &schedule) {
        Ok(items) => items,
        Err(e) => {
            // A rejected session is indistinguishable from a slow start at
            // this level, so give the user one chance to re-authenticate
            // before giving up rather than exiting on a recoverable problem.
            log::error!("{e}");
            log::warn!("retrying once after a fresh login, in case the session was rejected");
            bw_serve::stop_bw_serve(&mut bw_serve_child);
            session_token = reauthenticate(&store);
            // The longer grace: we just killed our own `bw serve`, and the
            // user just retyped their master password. Give the socket real
            // time to come free rather than aborting on them.
            bw_serve_child = match try_start_backend(
                &session_token,
                job.as_ref(),
                bw_serve::PORT_RELEASE_GRACE_RESTART,
            ) {
                Ok(child) => child,
                Err(e) => {
                    log::error!("{e}");
                    fatal_startup_error(&format!(
                        "Deskwarden could not start its Bitwarden backend after you signed \
                         in.\n\n{e}\n\nFull details are in:\n{}",
                        logging::log_file_path(&config_dir).display()
                    ));
                }
            };

            match wait_for_vault_ready(&vault, &schedule) {
                Ok(items) => items,
                Err(e) => {
                    log::error!("{e}");
                    bw_serve::stop_bw_serve(&mut bw_serve_child);
                    fatal_startup_error(&format!(
                        "Deskwarden's Bitwarden backend started but never became usable, so \
                         there is nothing to match your apps against.\n\n{e}\n\nFull details \
                         are in:\n{}",
                        logging::log_file_path(&config_dir).display()
                    ));
                }
            }
        }
    };

    let entries = match_entries(&items);
    log::info!("match engine loaded with {} app match(es)", entries.len());
    engine.rebuild(&entries);

    let injector = Injector {
        ui: RealUiAutomation,
        fallback: RealSendInput,
    };

    // The tray icon and the global hotkey manager each create a hidden
    // Win32 window on the thread that builds them (here, the main thread)
    // and rely on that thread pumping its message queue: tray clicks arrive
    // as WM_COMMAND/WM_MENUCOMMAND and the hotkey arrives as WM_HOTKEY, both
    // delivered only via GetMessage/PeekMessage + DispatchMessage on the
    // owning thread. That's why both are built here rather than on the
    // window-watch thread below (which runs its own, unrelated message
    // loop), and why the main loop calls `pump_windows_messages()` every
    // iteration -- without it, tray clicks and hotkey presses would sit
    // undelivered in the queue forever and `tray::next_menu_event()` /
    // `hotkey::fill_hotkey_pressed()` would never see anything.
    let fill_hotkey = hotkey::register_fill_hotkey();
    let tray = tray::build_tray();

    let current_version =
        Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is not valid semver");

    // Bounded connect/read timeouts, same "don't trust an external dependency
    // to answer promptly" reasoning as `bw_serve::READINESS_DEADLINE` -- just
    // applied to `api.github.com` instead of localhost. `ureq::Agent` is a
    // cheap `Arc`-backed handle, so it's fine to clone into the background
    // threads below.
    let http_agent = updater::build_agent();

    // The update check talks to an external host and, prior to this fix, ran
    // synchronously here -- before the tray, hotkey, and window-watch thread
    // even existed -- so a stalled `api.github.com` connection hung the
    // *entire app* on every launch before it became interactive at all. It's
    // now kicked off on its own background thread and reported back over
    // `update_rx`, polled non-blockingly from the main loop below, so a slow
    // or hung check can never delay startup. Same shape as the
    // `window_watch` thread just below.
    let mut available_update: Option<ReleaseInfo> = None;
    let (update_tx, update_rx) = mpsc::channel::<ReleaseInfo>();
    {
        let agent = http_agent.clone();
        let version = current_version.clone();
        let tx = update_tx.clone();
        std::thread::spawn(move || {
            if let Some(release) = check_for_update_logged(&version, &agent) {
                let _ = tx.send(release);
            }
        });
    }
    let mut last_update_check = Instant::now();

    // Outcome of a click-triggered update attempt. `Ok(())` means the
    // installer was downloaded, signature-verified, and launched, and this
    // process should now shut down for it; `Err` carries a message for the
    // log and the tray.
    //
    // The work behind this channel used to run inline in the tray-click
    // handler below, which streams a multi-megabyte download and then blocks
    // on a `powershell.exe` spawn for signature verification -- all while
    // `pump_windows_messages()` isn't running, so the tray, the global
    // hotkey, and window-watching were dead for the whole duration and
    // Windows would flag the app as not responding. It now runs on a
    // background thread and reports back here, polled non-blockingly from the
    // main loop, exactly like `update_rx` above.
    //
    // The *shutdown* deliberately stays on the main thread: `bw_serve_child`
    // is owned here, and the whole point of the shutdown path is that the
    // backend is killed before this process goes away.
    let (apply_tx, apply_rx) = mpsc::channel::<Result<(), String>>();

    // True from the moment a download starts until its outcome arrives, so a
    // second click can't start a second concurrent download of the same
    // installer into the same destination path.
    let mut update_in_progress = false;

    let (tx, rx) = mpsc::channel::<window_watch::ForegroundEvent>();
    std::thread::spawn(move || {
        if let Err(e) = window_watch::watch_foreground_windows(move |event| {
            let _ = tx.send(event);
        }) {
            log::error!("foreground window watcher stopped: {e}");
        }
    });

    // Set when a `Hotkey`-trigger match is seen: the item/window that's
    // eligible to be filled once the user presses the fill hotkey, rather
    // than being filled immediately from the window-match path.
    let mut pending_hotkey_fill: Option<(String, isize)> = None;

    // The hwnd of the last foreground window we acted on. See
    // `dispatch::should_dispatch` for why re-dispatching the same hwnd must be
    // suppressed (short version: closing our own overlay hands foreground back
    // to the target, which would otherwise re-match and re-show the overlay
    // forever, so "Dismiss" never dismissed).
    let mut last_dispatched_hwnd: Option<isize> = None;

    // Seed with whatever is already focused: `SetWinEventHook` only reports
    // foreground *changes*, so an app that was matched and already in front
    // when deskwarden started would otherwise be ignored until the next window
    // switch.
    if let Some(event) = window_watch::current_foreground_event() {
        log::info!(
            "seeding with current foreground window: {} (hwnd {})",
            event.exe_name,
            event.hwnd
        );
        process_foreground_event(
            &event,
            &vault,
            &injector,
            &engine,
            &mut pending_hotkey_fill,
            &mut last_dispatched_hwnd,
        );
    }

    let mut last_refresh = Instant::now();

    // How many periodic refreshes have failed in a row. A backend that still
    // holds its port but keeps failing is assumed busy at first and wedged
    // after a few tries; see `bw_serve::recovery_action`.
    let mut consecutive_refresh_failures: u32 = 0;

    loop {
        pump_windows_messages();

        if let Some(event) = tray::next_menu_event() {
            if event.id == tray.quit_id {
                // `bw serve` doesn't get killed on its own: `Child` doesn't
                // kill its process on `Drop`, and `process::exit` below
                // skips destructors anyway. Kill it explicitly, before
                // exiting -- nothing after `process::exit` ever runs -- so
                // it doesn't keep serving the unlocked vault over
                // `BW_SERVE_URL` after the user believes they've quit. The
                // process may already be gone (e.g. crashed, or killed
                // externally), so a `kill()` error is expected and ignored
                // rather than treated as fatal.
                log::info!("quit requested from tray; killing bw serve");
                bw_serve::stop_bw_serve(&mut bw_serve_child);
                std::process::exit(0);
            }

            if event.id == tray.add_app_id {
                // Two-step flow: choose the vault item the credentials come
                // from, then choose the process to attach to it.
                if let Some(item) = picker_ui::pick_vault_item(&vault) {
                    log::info!("adding an app match to vault item {}", item.id);
                    match picker_ui::run_picker(vault.clone(), item) {
                        Some(m) => {
                            log::info!("saved app match for {} ({:?})", m.process, m.trigger);
                            // Make the new match live immediately rather than
                            // waiting for the next periodic refresh.
                            match refresh_match_engine(&vault, &mut engine) {
                                Ok(count) => {
                                    log::info!("match engine refreshed: {count} app match(es)")
                                }
                                Err(e) => {
                                    log::warn!("refresh after saving app match failed: {e:?}")
                                }
                            }
                        }
                        None => log::info!("app-match picker cancelled (or save failed)"),
                    }
                }

                // Our own picker windows just stole and released foreground.
                // Forget the last-dispatched hwnd so the window the user
                // returns to is treated as a fresh switch rather than being
                // suppressed as a repeat.
                last_dispatched_hwnd = None;
            }

            if event.id == tray.update_id {
                // The item is disabled (and so shouldn't be clickable) until
                // `available_update` is `Some`, but the check is repeated
                // here defensively rather than trusting tray-icon's disabled
                // state to suppress the click event. Same reasoning for
                // re-checking `update_in_progress`.
                match (&available_update, update_in_progress) {
                    (Some(release), false) => {
                        log::info!(
                            "update requested from tray; downloading v{} in the background",
                            release.version
                        );
                        tray::set_update_in_progress(&tray, &release.version);
                        update_in_progress = true;

                        // Everything the thread needs is cloned in: the
                        // release (hence `ReleaseInfo: Clone`), the agent (a
                        // cheap `Arc` handle), the destination directory, and
                        // a sender. Nothing here is joined or waited on --
                        // the main loop keeps pumping messages and picks the
                        // outcome up from `apply_rx` whenever it lands.
                        let release = release.clone();
                        let agent = http_agent.clone();
                        let dest_dir = update_download_dir.clone();
                        let tx = apply_tx.clone();
                        std::thread::spawn(move || {
                            let outcome = updater::download_and_verify(
                                &release,
                                EXPECTED_SIGNER_THUMBPRINT,
                                &dest_dir,
                                &agent,
                            )
                            .and_then(|installer_path| updater::apply_update(&installer_path));
                            let _ = tx.send(outcome);
                        });
                    }
                    (Some(release), true) => log::info!(
                        "update to v{} is already being downloaded; ignoring repeat click",
                        release.version
                    ),
                    (None, _) => log::debug!("update item clicked with no update available"),
                }
            }
        }

        if hotkey::fill_hotkey_pressed(&fill_hotkey) {
            if let Some((item_id, hwnd)) = pending_hotkey_fill.take() {
                // Revalidate against the *actual* current foreground window
                // rather than trusting the stored value alone: even with
                // the invalidation below, there's a window between the
                // event that armed this and the hotkey press where focus
                // could have moved without us having processed a
                // `ForegroundEvent` for it yet.
                let current_fg = unsafe { GetForegroundWindow() }.0 as isize;
                if current_fg == hwnd {
                    fill_from_vault(&vault, &injector, &item_id, hwnd);
                } else {
                    log::info!("fill hotkey ignored: foreground window is no longer the match");
                }
            }
        }

        if last_refresh.elapsed() >= REFRESH_INTERVAL {
            match refresh_match_engine(&vault, &mut engine) {
                Ok(count) => {
                    consecutive_refresh_failures = 0;
                    log::debug!("match engine refreshed: {count} app match(es)");
                }
                Err(e) => {
                    // A failing refresh is the signal that something broke
                    // *while running*. Two independent things can cause it and
                    // they need different fixes, so both are probed:
                    //
                    // * the session went stale (`bw lock`, a server-side vault
                    //   timeout, a password change elsewhere) -- `bw status`
                    //   sees this, because it shells out to the CLI and does
                    //   not go through `bw serve` at all;
                    // * `bw serve` itself died or wedged while the session
                    //   stayed valid -- `bw status` is *blind* to this and
                    //   cheerfully reports `Unlocked`, so the port is probed
                    //   separately. Relying on the status check alone (as this
                    //   used to) meant a crashed backend logged this warning
                    //   every 60s forever and never restarted.
                    consecutive_refresh_failures += 1;
                    log::warn!(
                        "periodic match engine refresh failed \
                         (consecutive failures: {consecutive_refresh_failures}): {e:?}"
                    );

                    let status = login_ui::check_bw_status_with_session(Some(&session_token));
                    let port_listening = bw_serve::port_in_use(bw_serve::BW_SERVE_PORT);
                    let action = bw_serve::recovery_action(
                        status == login_ui::BwStatus::Unlocked,
                        port_listening,
                        consecutive_refresh_failures,
                    );

                    match action {
                        bw_serve::RecoveryAction::Wait => log::info!(
                            "session is {status:?} and `bw serve` is still listening on port \
                             {}; treating this as a transient failure and retrying next cycle",
                            bw_serve::BW_SERVE_PORT
                        ),
                        _ => {
                            if action == bw_serve::RecoveryAction::Reauthenticate {
                                log::warn!("session is now {status:?}; re-authenticating");
                                bw_serve::stop_bw_serve(&mut bw_serve_child);
                                session_token = reauthenticate(&store);
                                // The login window we just showed stole and
                                // released foreground, exactly like the tray
                                // picker above: forget the last-dispatched
                                // hwnd so the window the user returns to is
                                // treated as a fresh switch instead of being
                                // suppressed as a repeat.
                                last_dispatched_hwnd = None;
                            } else {
                                log::warn!(
                                    "session is still unlocked but `bw serve` is unusable \
                                     (port {} listening: {port_listening}); restarting the \
                                     backend",
                                    bw_serve::BW_SERVE_PORT
                                );
                                bw_serve::stop_bw_serve(&mut bw_serve_child);
                            }

                            // Deliberately *not* fatal, and deliberately more
                            // patient than the startup path. Exiting here --
                            // possibly seconds after the user retyped their
                            // master password -- over a socket that a `bw`
                            // grandchild has not released yet would punish
                            // them for a timing problem that usually resolves
                            // itself. Log it and try again next cycle.
                            match try_start_backend(
                                &session_token,
                                job.as_ref(),
                                bw_serve::PORT_RELEASE_GRACE_RESTART,
                            ) {
                                Ok(child) => {
                                    bw_serve_child = child;
                                    match wait_for_vault_ready(&vault, &schedule) {
                                        Ok(items) => {
                                            let entries = match_entries(&items);
                                            log::info!(
                                                "backend restarted; match engine reloaded with \
                                                 {} app match(es)",
                                                entries.len()
                                            );
                                            engine.rebuild(&entries);
                                            consecutive_refresh_failures = 0;
                                        }
                                        Err(e) => {
                                            log::error!("backend still unusable after restart: {e}")
                                        }
                                    }
                                }
                                Err(e) => log::error!(
                                    "could not restart `bw serve`: {e} -- staying up and \
                                     retrying on the next refresh cycle rather than exiting"
                                ),
                            }
                        }
                    }
                }
            }

            // Re-stamped *after* the work, not just before it. Recovery can
            // block for a long time -- a 30s port wait, a 30s readiness wait,
            // or a login window the user leaves open -- and measuring the next
            // interval from before all that would make the following cycle
            // fire immediately, turning a persistent failure into a tight
            // retry loop instead of one attempt per minute.
            last_refresh = Instant::now();
        }

        if last_update_check.elapsed() >= UPDATE_CHECK_INTERVAL {
            // Same off-thread treatment as the startup check above: this now
            // runs once a day from a live, interactive app, but it still
            // talks to an external host, so it's still kicked off on a
            // background thread rather than blocking the main loop (and
            // therefore tray/hotkey/window-watch responsiveness) for however
            // long `api.github.com` takes to answer.
            let agent = http_agent.clone();
            let version = current_version.clone();
            let tx = update_tx.clone();
            std::thread::spawn(move || {
                if let Some(release) = check_for_update_logged(&version, &agent) {
                    let _ = tx.send(release);
                }
            });
            last_update_check = Instant::now();
        }

        if let Ok(release) = update_rx.try_recv() {
            // Not while a download is in flight: relabelling the item back to
            // "Update available" mid-download would contradict what is
            // actually happening (the click would be rejected anyway, see the
            // handler above). Rare -- checks are 24h apart -- but the tray is
            // the only status this app shows, so it shouldn't lie.
            if !update_in_progress {
                tray::set_update_available(&tray, &release.version);
            }
            available_update = Some(release);
        }

        // Non-blocking, like the check above: the download thread reports here
        // when it's finished (or failed), and the main loop never waits on it.
        if let Ok(outcome) = apply_rx.try_recv() {
            update_in_progress = false;
            match outcome {
                Ok(()) => {
                    // Same shutdown path as the Quit handler above: kill
                    // `bw serve` explicitly before exiting so the installer
                    // (which replaces and relaunches this binary) doesn't
                    // leave an orphaned backend serving the unlocked vault
                    // behind.
                    log::info!("update installer launched; shutting down for update");
                    bw_serve::stop_bw_serve(&mut bw_serve_child);
                    std::process::exit(0);
                }
                Err(e) => {
                    // Surfaced, not just logged: a tray app with no window
                    // and no console has nowhere else to say this, and the
                    // user just asked for an update and is entitled to know
                    // it didn't happen.
                    log::error!("update failed: {e}");
                    if let Some(release) = &available_update {
                        tray::set_update_failed(&tray, &release.version);
                    }
                }
            }
        }

        if let Ok(event) = rx.recv_timeout(Duration::from_millis(200)) {
            process_foreground_event(
                &event,
                &vault,
                &injector,
                &engine,
                &mut pending_hotkey_fill,
                &mut last_dispatched_hwnd,
            );
        }
    }
}

/// Shows a native message box.
///
/// The only user-visible channel that exists this early. Everything in this
/// process is either a tray icon (not yet built at startup-check time) or an
/// egui window (which needs an event loop this code hasn't reached), and the
/// GUI subsystem means `eprintln!` goes nowhere at all -- so a plain
/// `MessageBoxW` is the one mechanism that can actually put words in front of
/// the user before anything else exists. `MB_SETFOREGROUND | MB_SYSTEMMODAL`
/// because this fires during login-time autostart, when whatever the shell is
/// doing would otherwise bury it.
fn message_box(title: &str, text: &str, style: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(text),
            &HSTRING::from(title),
            style | MB_SETFOREGROUND | MB_SYSTEMMODAL,
        )
    }
}

/// Logs `message`, shows it to the user, and exits.
///
/// The failure paths this replaces logged a line to a file nobody has open and
/// then called `exit(1)`: from the user's side, a double-clicked app that
/// simply never appeared, with no clue as to why.
fn fatal_startup_error(message: &str) -> ! {
    log::error!("refusing to start: {}", message.replace('\n', " "));
    message_box("Deskwarden cannot start", message, MB_ICONERROR | MB_OK);
    std::process::exit(1);
}

/// Checks that the resolved `bw.exe` is Bitwarden's, and decides what to do
/// when it can't be shown to be.
///
/// The response is graded rather than uniform, because the two failures are
/// not equally conclusive:
///
/// * **The signature itself is invalid** (unsigned, tampered with, expired,
///   or not chaining to a trusted root). That is a fact about the binary, not
///   an opinion of ours, and it is exactly the case this check exists to stop.
///   Refused outright -- with an explanation the user can actually see.
/// * **The signature is valid but the signer's `O=` isn't in
///   [`TRUSTED_BW_SIGNER_ORGANIZATIONS`]**, or the check couldn't be run at
///   all. Here the evidence points at *our* list as much as at the binary:
///   that list carries a standing "not yet confirmed against a real
///   Bitwarden-signed bw.exe" TODO, and `installer/bootstrap-bw.ps1` will
///   happily leave a Scoop- or Chocolatey-installed `bw` in place, whose
///   signer is legitimately somebody else. Hard-exiting on our own unverified
///   data would brick those installs with no recovery path -- the updater
///   can't help, this runs before it. So the user is told precisely what was
///   found and asked, with "no, quit" as the default button.
///
/// The judgment call, stated plainly: a *known-unverified* allowlist should
/// not be able to silently kill the app, but it also shouldn't be quietly
/// ignored, because the next thing to happen is the master password being
/// typed. Asking is the only option that is honest about both.
fn check_bw_signature(bw_exe: &std::path::Path) {
    let (headline, detail) = match deskwarden::signature::verify_authenticode(bw_exe) {
        Ok(info)
            if deskwarden::signature::is_trusted_organization(
                &info,
                TRUSTED_BW_SIGNER_ORGANIZATIONS,
            ) =>
        {
            log::info!(
                "bw CLI at {} verified as Bitwarden-signed",
                bw_exe.display()
            );
            return;
        }
        Ok(info) if !info.valid => {
            log::error!(
                "refusing to start: {} does not carry a valid Authenticode signature \
                 (subject: {:?})",
                bw_exe.display(),
                info.subject_dn
            );
            fatal_startup_error(&format!(
                "The Bitwarden CLI that Deskwarden found is not validly signed, so Deskwarden \
                 will not run it.\n\nFile:\n{}\n\nWindows could not confirm the file's \
                 signature. It may have been modified or replaced. Deskwarden hands this \
                 program your master password, so it is stopping instead.\n\nReinstall the \
                 Bitwarden CLI from bitwarden.com, or reinstall Deskwarden.",
                bw_exe.display()
            ));
        }
        Ok(info) => {
            log::warn!(
                "{} is validly signed, but by an organization not in the (still unverified) \
                 trusted list; subject: {:?}",
                bw_exe.display(),
                info.subject_dn
            );
            (
                "signed by an organization Deskwarden does not recognize",
                describe_signer(info.subject_dn.as_deref()),
            )
        }
        Err(e) => {
            log::warn!(
                "could not verify the signature of {}: {e}",
                bw_exe.display()
            );
            (
                "could not be signature-checked at all",
                format!("The check failed with: {e}"),
            )
        }
    };

    let answer = message_box(
        "Deskwarden: unrecognized Bitwarden CLI",
        &format!(
            "The Bitwarden CLI Deskwarden is about to use {headline}.\n\nFile:\n{}\n\n{detail}\n\n\
             Deskwarden gives this program your master password and vault session, so it should \
             only be Bitwarden's own CLI. This can also happen with a `bw` installed through \
             Scoop or Chocolatey, which are signed differently (or not at all).\n\nContinue \
             anyway?\n\nChoose No unless you know where this bw.exe came from.",
            bw_exe.display()
        ),
        MB_ICONWARNING | MB_YESNO | MB_DEFBUTTON2,
    );

    if answer == IDYES {
        log::warn!(
            "user chose to continue with an unrecognized bw.exe at {}",
            bw_exe.display()
        );
    } else {
        log::error!(
            "user declined to continue with an unrecognized bw.exe at {}; exiting",
            bw_exe.display()
        );
        std::process::exit(1);
    }
}

/// Turns a signer's subject DN into a sentence for the message box, since a
/// raw multi-line DN in a dialog is noise to everyone who isn't debugging.
fn describe_signer(subject_dn: Option<&str>) -> String {
    let Some(dn) = subject_dn else {
        return "It has no signer certificate.".to_string();
    };
    let orgs = deskwarden::signature::dn_component(dn, "O");
    match orgs.first() {
        Some(org) => format!("It is signed by: {org}"),
        None => "Its signer certificate names no organization.".to_string(),
    }
}

/// Applies the dispatch rules to one foreground event and, if it survives
/// them, matches and dispatches it.
fn process_foreground_event(
    event: &window_watch::ForegroundEvent,
    vault: &VaultBridge,
    injector: &Injector<RealUiAutomation, RealSendInput>,
    engine: &MatchEngine,
    pending_hotkey_fill: &mut Option<(String, isize)>,
    last_dispatched_hwnd: &mut Option<isize>,
) {
    // Our own windows (prompt overlay, process picker, login) are focused,
    // always-on-top windows, so showing one fires EVENT_SYSTEM_FOREGROUND for
    // this process. Those are not app switches: ignore them entirely, without
    // even invalidating a pending hotkey fill (the target hasn't changed --
    // we just temporarily covered it).
    if dispatch::is_own_process(event.pid) {
        return;
    }

    // Any foreground-window change invalidates a pending hotkey
    // fill unless it's the very window that armed it re-foregrounding
    // (same hwnd). Without this, arming the fill and then switching
    // away to an unrelated window -- without ever pressing the fill
    // hotkey -- would leave `pending_hotkey_fill` stale: a later,
    // unrelated Ctrl+Alt+B press would fire it against a `hwnd` that
    // may since have been recycled by the OS for a different window,
    // contradicting the guarantee that the hotkey does nothing when
    // no matching window is foregrounded.
    if let Some((_, armed_hwnd)) = pending_hotkey_fill.as_ref() {
        if *armed_hwnd != event.hwnd {
            *pending_hotkey_fill = None;
        }
    }

    if !dispatch::should_dispatch(event, *last_dispatched_hwnd) {
        log::debug!(
            "suppressing repeat foreground event for hwnd {} ({})",
            event.hwnd,
            event.exe_name
        );
        return;
    }
    *last_dispatched_hwnd = Some(event.hwnd);

    if let Some((item_id, m)) = engine.lookup(&event.exe_name) {
        log::info!(
            "matched {} to vault item {item_id} (trigger {:?})",
            event.exe_name,
            m.trigger
        );
        if let Some(armed) = handle_match(vault, injector, item_id, m, event.hwnd, &event.exe_name)
        {
            *pending_hotkey_fill = Some(armed);
        }
    }
}

/// Calls `updater::check_for_update` against the real GitHub API and logs the
/// outcome. Network failures, a malformed release, and "no update" are all
/// deliberately non-fatal here -- this runs on a background thread (see call
/// sites), so the worst case is that a check is skipped until the next
/// cycle, not that the app goes down (or hangs) over a transient GitHub API
/// problem.
fn check_for_update_logged(current_version: &Version, agent: &ureq::Agent) -> Option<ReleaseInfo> {
    match updater::check_for_update(GITHUB_API_BASE, current_version, agent) {
        Ok(Some(release)) => {
            log::info!(
                "update available: v{} (current: v{current_version})",
                release.version
            );
            Some(release)
        }
        Ok(None) => {
            log::debug!("no update available (current: v{current_version})");
            None
        }
        Err(e) => {
            log::warn!("update check failed: {e}");
            None
        }
    }
}

/// Runs the login/unlock UI and persists the resulting session token.
fn reauthenticate(store: &session_store::SessionStore) -> String {
    let token = login_ui::run_login_flow();
    if let Err(e) = store.save(&token) {
        log::error!("failed to persist session token: {e}");
    }
    token
}

/// Why `bw serve` could not be brought up.
///
/// Distinguished rather than collapsed into a string because the two cases
/// have very different prognoses: a held port frequently frees itself a moment
/// later, whereas a missing CLI never will.
enum BackendStartError {
    /// Something is still listening on the port after the grace period.
    PortHeld(Duration),
    /// No verified `bw.exe` is on record, so there is nothing safe to spawn.
    NoVerifiedCli(String),
    /// The `bw` process could not be spawned at all.
    Spawn(std::io::Error),
}

impl std::fmt::Display for BackendStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PortHeld(waited) => write!(
                f,
                "something is still listening on localhost:{} after waiting {waited:?} -- most \
                 likely an orphaned `bw serve` (or a grandchild of one we killed) that has not \
                 released the socket yet. Refusing to start rather than talking to an unknown \
                 process holding an unknown session.",
                bw_serve::BW_SERVE_PORT
            ),
            Self::NoVerifiedCli(e) => write!(f, "cannot start `bw serve`: {e}"),
            Self::Spawn(e) => write!(
                f,
                "failed to spawn `bw serve` from the verified Bitwarden CLI path (see \
                 bw_path::resolve_bw_exe for where that path comes from): {e}"
            ),
        }
    }
}

/// Syncs the vault, then spawns `bw serve` and attaches it to `job`.
///
/// Refuses to start if something is already listening on the `bw serve` port:
/// that is almost always an orphaned `bw serve` from a previous unclean exit,
/// and our newly spawned one would silently fail to bind while `VaultBridge`
/// happily talked to the *other* process -- a different, unknown session
/// serving an unknown vault.
///
/// Returns the failure instead of exiting, because on the restart paths (and
/// especially the one right after the user retyped their master password)
/// killing the whole app over a socket that needs another second to close is
/// far worse than logging and trying again on the next cycle. `port_grace` is
/// how long to give the port: short at startup, [`bw_serve::
/// PORT_RELEASE_GRACE_RESTART`] when we've just killed our own child.
fn try_start_backend(
    session_token: &str,
    job: Option<&job_object::KillOnCloseJob>,
    port_grace: Duration,
) -> Result<Child, BackendStartError> {
    if !bw_serve::wait_for_port_free(bw_serve::BW_SERVE_PORT, port_grace) {
        return Err(BackendStartError::PortHeld(port_grace));
    }

    // Pull the latest vault state down before the match engine is built, so a
    // match added on another device is live on first run rather than after the
    // next incidental sync.
    match bw_serve::run_bw_sync(session_token) {
        Ok(()) => log::info!("bw sync completed"),
        Err(e) => log::warn!("bw sync failed (continuing with cached vault): {e}"),
    }

    // Spawned suspended and assigned to the job before it runs a single
    // instruction, so there is no window in which a crash of *this* process
    // could orphan an unlocked-vault server. See `job_object::spawn_in_job`.
    let command =
        bw_serve::bw_serve_command(session_token).map_err(BackendStartError::NoVerifiedCli)?;
    job_object::spawn_in_job(job, command).map_err(BackendStartError::Spawn)
}

/// Startup variant of [`try_start_backend`]: there is nothing to fall back to
/// before the main loop exists, so a failure here is fatal.
fn start_backend(session_token: &str, job: Option<&job_object::KillOnCloseJob>) -> Child {
    match try_start_backend(session_token, job, bw_serve::PORT_RELEASE_GRACE) {
        Ok(child) => child,
        Err(e) => {
            log::error!("{e}");
            fatal_startup_error(&format!(
                "Deskwarden could not start its Bitwarden backend.\n\n{e}"
            ));
        }
    }
}

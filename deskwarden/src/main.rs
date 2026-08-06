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

use deskwarden::accounts::{self, account_label, Account};
use deskwarden::app::{fill_from_vault, handle_match, match_entries, pump_windows_messages};
use deskwarden::backend_policy;
use deskwarden::bw_path;
// `BACKEND_OP_TIMEOUT`: the upper bound on how long a legitimate backend
// start or sync may take before something is treated as having gone wrong.
// Used both by this file's own backend-op bookkeeping
// (`backend_task_in_progress`'s wedge deadline, `open_vault_window`'s
// lock-recovery wait) and by `picker_ui::run_picker`'s readiness probe (see
// its own doc for review 11's Important 2) -- defined in `bw_serve`, not
// here, so both sides share the exact same number rather than disagreeing.
use deskwarden::bw_serve::{
    self, readiness_schedule, wait_for_vault_ready, BACKEND_OP_TIMEOUT, BW_SERVE_URL,
    READINESS_DEADLINE,
};
use deskwarden::dispatch;
use deskwarden::injector::sequence::Notifier;
use deskwarden::injector::{
    Injector, RealSendInput, RealUiAutomation, SendInputFiller, UiAutomationFiller,
};
use deskwarden::match_engine::MatchEngine;
use deskwarden::updater::{self, ReleaseInfo};
use deskwarden::vault_bridge::VaultBridge;
use deskwarden::picker_ui::SavedAppMatch;
use deskwarden::vault_cache::{
    AppMatchWrite, PopulateOutcome, VaultCache, VaultEpoch, VaultEra, VaultSnapshot,
    VaultUnavailable,
};
use deskwarden::app_match::AppMatch;
use deskwarden::app_window;
use deskwarden::{
    fill_stats, hotkey, job_object, loading_ui, logging, login_ui, picker_ui,
    prefs_ui, session_store, settings, tray, vault_window, window_watch,
};
use semver::Version;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;
use std::rc::Rc;
use std::process::Child;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tray_icon::menu::{MenuEvent, MenuId};
use windows::core::HSTRING;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONERROR,
    MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MB_SYSTEMMODAL, MB_YESNO, MESSAGEBOX_RESULT,
    MESSAGEBOX_STYLE,
};

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

    // On-disk favicon cache, keyed by domain (see `favicon::write_cached_icon`).
    // Also a cache directory, for the same reason `update_download_dir` is:
    // disposable and regenerable, so it belongs alongside it rather than in
    // `config_dir`. Not created here -- `favicon::write_cached_icon` creates
    // it lazily on first write, same as `update_download_dir`'s directory is
    // created lazily by `download_and_verify`.
    let icon_cache_dir = project_dirs.cache_dir().join("icons");

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

    // User preferences (backend lifecycle, auto-lock timeout). A missing or
    // corrupt file falls back to defaults -- see `Settings::load` -- so this
    // is never a reason startup fails.
    //
    // `mut`, and the path kept around as `settings_path`: the preferences
    // window (`prefs_ui::run`) can change and save these mid-session, and
    // this binding -- not the file on disk -- is what every later read in
    // this loop (`settings.auto_lock_timeout()`, `settings.keep_backend_running`
    // in the idle reconciliation below) actually consults. Reassigning it in
    // the tray handler is what makes a change take effect immediately rather
    // than only on next launch.
    //
    // It is loaded once and never refreshed, so its `vault_window` goes stale
    // as soon as the vault window is closed (that geometry is written to the
    // file by `vault_window::run`, which this loop never hears about). Nothing
    // here reads that field -- `vault_window` re-reads the file when it opens
    // -- and the preferences save below goes through `persist_preferences`
    // precisely so a stale copy of it can never be written back.
    let settings_path = config_dir.join("settings.json");
    let mut settings = settings::Settings::load(&settings_path);

    // ------------------------------------------------------------------
    // Which account is this launch?
    //
    // The order below is the whole of it, and every step of it is load-
    // bearing:
    //
    //   1. resolve    -- which account this process runs as, given what
    //                    `settings.json` holds. Pure; see
    //                    `accounts::resolve_startup`. With no accounts stored
    //                    it MINTS one: there is nothing to import, because
    //                    the vault lives on Bitwarden's servers and
    //                    `%APPDATA%\Bitwarden CLI` is a local cache plus a
    //                    session token. That directory is left exactly where
    //                    it is -- never read, never copied, never deleted.
    //   2. point      -- `BITWARDENCLI_APPDATA_DIR` and the token store are
    //                    aimed at that account, BEFORE anything reads a token
    //                    or spawns `bw`. Otherwise a token is validated
    //                    against the wrong profile and silently
    //                    re-authenticated, which reads as "the update lost my
    //                    login".
    //
    // Nothing here may kill the app: there is no `fatal_startup_error`
    // anywhere in this block. The worst it can do is mint an account the user
    // then signs in to.
    //
    // `remember_verified_bw_exe` above is a prerequisite, not an ordering
    // nicety: `multi_account_availability` looks beside the *verified*
    // `bw.exe` for the `bitwarden-cli` directory that makes the CLI ignore
    // `BITWARDENCLI_APPDATA_DIR`.
    let availability = bw_path::multi_account_availability();
    // `settings.json` is there and could not be read (see
    // `Settings::accounts_unreadable`), so its account list parsed as EMPTY --
    // which is the same thing `resolve_startup` reads as "no accounts yet, mint
    // one". Minting on a list we know we cannot see would point the CLI at an
    // empty directory while the user's own account sat in `accounts/<id>/`,
    // and `Settings::save` refuses to write over the file, so the minted id
    // would not even be recorded: every launch would mint another one. So the
    // fact is handed to the resolution rather than being swallowed here.
    let unreadable_reason = settings.accounts_unreadable.then(|| {
        format!(
            "{} could not be read, so Deskwarden cannot tell which accounts are already set up",
            settings_path.display()
        )
    });
    if let Some(reason) = &unreadable_reason {
        log::error!("{reason}; running with no account of our own on this launch");
    }
    let startup = accounts::resolve_startup(
        &settings.accounts,
        settings.active_account.as_ref(),
        unreadable_reason.as_deref(),
    );
    // `accounts_state` is the one door for "may I switch accounts, and to
    // what" (Task 10). It is built HERE and nowhere else, and every later
    // reader asks it rather than recomputing the CLI availability for itself.
    // `mut`, both of them: an account switch re-points which account this
    // process is (`active_account`) and which account the switcher offers to
    // leave (`accounts_state`'s `active`). See `open_vault_window`.
    let (mut active_account, mut accounts_state, first_run_account) = match &startup {
        accounts::StartupAccounts::Ready {
            active,
            accounts,
            needs_persist,
            first_run,
        } => {
            if *needs_persist {
                if let Err(e) =
                    settings::Settings::persist_accounts(&settings_path, accounts, Some(&active.id))
                {
                    // Survivable. This launch is correctly pointed either way;
                    // the next one finds no account list, mints a second
                    // directory and asks for a sign-in again. That costs the
                    // user one sign-in and a few megabytes of stale cache --
                    // the vault itself is on Bitwarden's servers and is not
                    // involved.
                    log::warn!("could not persist the account list: {e}");
                }
            }
            let state =
                accounts::AccountsState::new(availability, accounts.clone(), active.id.clone());
            // Only the account this launch MINTED gets the first-run notice.
            // An id rather than a bool, because the same flag is handed to
            // every login window this process opens -- including the ones a
            // switch to some *other* account opens, which are not first runs
            // and must say nothing.
            let first_run_account = first_run.then(|| active.id.clone());
            (Some(active.clone()), state, first_run_account)
        }
        accounts::StartupAccounts::NoAccountList { reason } => {
            log::warn!(
                "{reason}; running with no account of our own: the CLI is left on whatever \
                 profile it resolves by itself"
            );
            // On SCREEN, and not only in the log. This is the app's worst
            // survivable startup: no switcher, no accounts submenu, and it
            // repeats identically every launch until `settings.json` is fixed
            // or removed -- so a user who never opens `deskwarden.log` has no
            // way to learn that anything is wrong, let alone what.
            //
            // Only in this arm. The other refusal -- a `bitwarden-cli`
            // directory beside `bw.exe` -- leaves a usable app and is reported
            // by `blocked_reason` where the switch is offered; a modal on every
            // launch for that would be nagging about something the user can
            // already see.
            //
            // The claim about deletion is absolute here and can be: this
            // launch resolved an account list and did nothing else. Deskwarden
            // has no code that deletes a Bitwarden profile it did not itself
            // create.
            message_box(
                "Deskwarden",
                &format!(
                    "Deskwarden could not read its account list, so it is running with no \
                     account of its own -- there is no account switcher and the Bitwarden CLI \
                     is signed out.\n\nNothing has been deleted. Fix or remove the file named \
                     below and start Deskwarden again.\n\n{reason}",
                ),
                MB_ICONWARNING | MB_OK,
            );
            (None, None, None)
        }
    };
    if let Some(why) = accounts_state
        .as_ref()
        .and_then(accounts::AccountsState::blocked_reason)
    {
        log::warn!("switching and adding accounts are unavailable on this machine: {why}");
    }

    // The two arms are "no account of our own" and "the account-aware app".
    // The first is a FALLBACK in the sense of "no override is set and nothing
    // offers a switch", not a second implementation of anything: the switch,
    // the resettle and the cache are untouched by it, and the
    // `NoAccountList` arm reaches none of them because `AccountsState` is
    // `None` there and so nothing offers a switch at all.
    //
    // The directory is created, not assumed. `resolve_startup` mints an
    // account without touching the disk, and `SessionStore` is explicit that
    // it will not create its own parent -- so without this the very first
    // `store.save` fails and the user retypes their master password on every
    // launch, forever, with nothing else to see.
    let (session_path, active_dir) = match &active_account {
        Some(a) => {
            if let Err(e) = accounts::ensure_account_dir(&config_dir, &a.id) {
                log::error!(
                    "could not create the data directory for the active account ({e}); its \
                     session token and Windows Hello enrolment cannot be stored"
                );
            }
            (
                accounts::session_path_for(&config_dir, &a.id),
                Some(accounts::data_dir_for(&config_dir, &a.id)),
            )
        }
        None => (config_dir.join("session.bin"), None),
    };
    bw_path::set_active_data_dir(active_dir);
    // `mut` for the same reason: `switch_to_account` re-points this at the
    // target account's own `session.bin` before it authenticates, so the token
    // the switch produces cannot land in the file of the account being left.
    let mut store = session_store::SessionStore::new(session_path);

    // What every login window this process opens is scoped to. Built through
    // the one constructor, so no call site can quietly go back to passing
    // `None` and losing quick unlock -- see `login_context`, and
    // `login_ui::run_login_flow_for`.
    //
    // A local for the startup paths below rather than for the whole run: it
    // borrows `active_account`, and an account switch inside the main loop
    // takes `&mut` to it. Its last use is the `recover_from_failed_vault_wait`
    // arms just below, and the borrow ends there.
    let login = login_context(
        config_dir.as_path(),
        active_account.as_ref(),
        first_run_account.as_ref(),
    );

    let fill_stats_path = config_dir.join("fill-stats.json");
    let fill_stats = fill_stats::FillStats::new(fill_stats_path);

    // Every child process we spawn joins this job object, which is configured
    // to kill its members when the last handle closes. Our handles close when
    // this process dies for *any* reason -- clean quit, panic, Ctrl+C, Task
    // Manager -- so `bw serve` can no longer be orphaned holding an unlocked
    // vault open on localhost. This must outlive the whole run, hence the
    // binding here rather than inside the spawn helper.
    // `Arc`-wrapped (rather than a plain `Option<KillOnCloseJob>` borrowed by
    // reference, as before) so a clone can be handed to the background
    // threads that now start `bw serve` off the main thread -- see
    // `spawn_backend_start` -- without them needing a `'static` borrow of a
    // stack local. `KillOnCloseJob` itself is `Send + Sync` with no `unsafe`
    // now that it's backed by `OwnedHandle` (see `job_object`), so this needs
    // no unsafe either.
    let job: Arc<Option<job_object::KillOnCloseJob>> = Arc::new(match job_object::KillOnCloseJob::new()
    {
        Ok(job) => Some(job),
        Err(e) => {
            log::error!(
                "could not create a kill-on-close job object ({e}); `bw serve` will only be \
                 cleaned up on a clean quit"
            );
            None
        }
    });

    // A cached session token is worthless if it has since been invalidated
    // (manual `bw lock`, password change, reboot). Trusting it unconditionally
    // is how the app used to proceed "unlocked" with no recovery path.
    // **Does this launch need a window at all?**
    //
    // A cached session that is still unlocked is the SILENT launch: nothing is
    // shown, the app goes to the tray, and that is correct. The user asked for
    // one window "when login (not lock) after entering" -- a launch that never
    // asks for a master password has nothing to show a card for, and giving it
    // a vault window it did not have before would be a window popping up on
    // every boot. So the answer here is not a token but a question, and only
    // the `None` arm below opens `app_window`.
    let cached_session = match store.load() {
        Some(token) => match login_ui::check_bw_status_with_session(Some(&token)) {
            login_ui::BwStatus::Unlocked => {
                log::info!("cached session token verified as unlocked");
                Some(token)
            }
            other => {
                log::warn!("cached session token reports {other:?}; re-authenticating");
                None
            }
        },
        None => {
            log::info!("no cached session token; showing the single startup window");
            None
        }
    };

    let vault = VaultBridge::new(BW_SERVE_URL);
    // The vault window's reads and writes, and now autofill's own reads (see
    // `app::fill_from_vault`), go through this in-memory snapshot rather than
    // straight to `bw serve` -- see `vault_cache`'s module doc. Built once,
    // here, wrapping the same bridge everything else in `main` still uses
    // directly: startup's readiness check, the picker's item list, and the
    // periodic match-engine refresh all still want the live server rather
    // than a snapshot that's deliberately not re-fetched on every read.
    let cache = Arc::new(VaultCache::new(vault.clone()));
    // Captured here, *before* the readiness probe below, because that probe's
    // own `list_items()` is the fetch whose result seeds the cache further
    // down via `populate_with` -- and the epoch guard can only cover the
    // window it is handed (review 14's Minor 3). Nothing between here and
    // there calls `cache.clear()` today, so this is inert; it is written this
    // way so it stays correct if any of that moves onto a background thread.
    let startup_epoch = cache.epoch();
    let mut engine = MatchEngine::new();

    // `bw serve` is a bundled Node binary: its cold start regularly takes
    // several seconds, far longer than the fixed 500ms sleep this replaces.
    // Losing that race used to leave the match engine permanently empty with
    // no diagnostic, so the app silently did nothing forever.
    let schedule = readiness_schedule(READINESS_DEADLINE);

    // Built here rather than after the cache is filled, because the single
    // startup window's VAULT STAGE needs it: that stage is `vault_window`'s own
    // frame, and it fills from the same injector every later window uses.
    let injector = Injector {
        ui: RealUiAutomation,
        fallback: RealSendInput,
    };

    // What the match engine is armed from, filled by whichever of the two
    // startup paths below runs. An `Rc<RefCell<_>>` because the single window's
    // `build_vault` closure is `'static` (it is moved into an eframe update
    // closure) and so cannot borrow anything on this stack -- the same handoff
    // every window in this crate uses to get an answer back, and safe for the
    // same reason: `app_window::run` blocks THIS thread for its whole life.
    let startup_entries: Rc<RefCell<Vec<(String, AppMatch)>>> =
        Rc::new(RefCell::new(Vec::new()));
    // The outcome of the vault the single window showed, if it showed one.
    // Dispatched further down, once the tray exists -- see the call to
    // `open_vault_window` just before the main loop, and its `first_result`
    // parameter for why it is dispatched THERE rather than handled here.
    let mut startup_vault: Option<vault_window::VaultWindowResult> = None;
    // `Option` rather than a plain `Child`: with `keep_backend_running`
    // turned off, the backend is only up while the vault window is open (see
    // `backend_policy::should_run`), so "not currently running" has to be
    // representable. Always `Some` by the end of both paths below -- something
    // has to answer the very first readiness probe regardless of the setting.
    //
    // Declared without a value, for the same reason `session_token` below is:
    // both arms now decide it outright (the single-window arm by claiming the
    // startup handoff -- see `StartupChildHandoff`), and a `= None` seed here
    // would let a future arm silently skip that decision and carry on with
    // "no backend running", which is precisely the false belief that made the
    // deadline path kill the app on its own orphan.
    let mut bw_serve_child: Option<Child>;
    // Declared without a value and assigned in BOTH arms below, rather than
    // seeded with an empty string: an empty token is a real value that would
    // compile everywhere and authenticate nothing, and the compiler is a better
    // check that both paths produce one than a reviewer is.
    let mut session_token: String;

    if let Some(token) = cached_session {
    session_token = token;
    bw_serve_child = Some(start_backend(&session_token, job_ref(&job)));
    let items = match wait_for_vault_ready_with_spinner(&vault, &schedule, SETUP_MESSAGE) {
        VaultReadyOutcome::Ready(items) => items,
        VaultReadyOutcome::Dismissed => {
            // Closing the "setting up" window is not, on its own, evidence
            // that the backend or session is broken -- unlike a genuine
            // timeout (the `Failed` arm below), there's no "maybe the
            // session was rejected" signal to act on here (review 12's
            // Important 2). Give the same, still-running backend one more
            // honest readiness probe -- no kill, no reauth -- before falling
            // back to the heavier recovery a real failure gets.
            log::info!(
                "setup window closed before the vault backend was confirmed ready; trying the \
                 readiness probe again before treating anything as actually broken"
            );
            match wait_for_vault_ready_with_spinner(&vault, &schedule, SETUP_RETRY_MESSAGE) {
                VaultReadyOutcome::Ready(items) => items,
                VaultReadyOutcome::Dismissed => recover_from_failed_vault_wait(
                    "setup window closed a second time without the vault backend becoming ready",
                    &vault,
                    &schedule,
                    &mut bw_serve_child,
                    &mut session_token,
                    &job,
                    &store,
                    &config_dir,
                    login,
                ),
                VaultReadyOutcome::Failed(e) => recover_from_failed_vault_wait(
                    &e,
                    &vault,
                    &schedule,
                    &mut bw_serve_child,
                    &mut session_token,
                    &job,
                    &store,
                    &config_dir,
                    login,
                ),
            }
        }
        VaultReadyOutcome::Failed(e) => recover_from_failed_vault_wait(
            &e,
            &vault,
            &schedule,
            &mut bw_serve_child,
            &mut session_token,
            &job,
            &store,
            &config_dir,
            login,
        ),
    };

    *startup_entries.borrow_mut() = match_entries(&items);
    seed_cache_at_startup(&cache, items, startup_epoch);
    } else {
    // **ONE WINDOW: the sign-in card, then the spinner, then the vault.**
    //
    // This is the launch the user reported. It used to be three windows -- the
    // card closed, a 320x150 spinner opened and closed, and the app arrived in
    // the tray having never shown the vault it had just unlocked. All three are
    // now stages of one `eframe` app in one OS window, at the vault window's
    // own geometry, which the user closes when they are done with it.
    //
    // The slow part between the stages runs on a worker thread (`prepare`), not
    // here and not on the frame closure: `try_start_backend` alone is regularly
    // several seconds, and any of it on the frame thread would freeze the window
    // exactly where it is meant to be showing a spinner.
    // Where the worker publishes the `bw serve` it starts, so that a
    // `WORKING_DEADLINE` expiry -- which throws the worker's whole result
    // away -- cannot also throw away the handle to a live backend. See
    // `StartupChildHandoff`.
    let startup_child = Arc::new(StartupChildHandoff::new());
    let handoff_for_work = startup_child.clone();
    let job_for_work = job.clone();
    let vault_for_work = vault.clone();
    let schedule_for_work = schedule.clone();

    let cache_for_vault = cache.clone();
    let fill_stats_for_vault = fill_stats.clone();
    let icon_cache_dir_for_vault = icon_cache_dir.clone();
    let auto_lock_for_vault = settings.auto_lock();
    let accounts_for_vault = accounts_state.clone();
    let entries_out = startup_entries.clone();

    let outcome = app_window::run(
        login.account,
        login.first_run,
        SETUP_MESSAGE,
        // ON A WORKER THREAD. `try_start_backend` rather than `start_backend`:
        // the fatal variant puts a modal message box up, and doing that from a
        // worker while an eframe window owns the main thread is a dialog behind
        // a window nobody can reach. The failure is carried back instead and is
        // just as fatal, one frame later, on the thread that can show it.
        move |token| StartupWork::produce(
            &token,
            &job_for_work,
            &vault_for_work,
            &schedule_for_work,
            &handoff_for_work,
        ),
        // ON THE MAIN THREAD, in the frame that drains the worker. The cache
        // has to be filled BEFORE the vault frame is built, or the window's
        // first vault frame paints an empty vault as data.
        move |token, work: &mut StartupWork| {
            // Annotated: without it, deref coercion picks `&mut [VaultItem]`
            // and `mem::take` asks for a `Default` slice.
            let items: Vec<deskwarden::vault_bridge::VaultItem> =
                std::mem::take(work.items.as_mut().ok()?);
            *entries_out.borrow_mut() = match_entries(&items);
            seed_cache_at_startup(&cache_for_vault, items, startup_epoch);
            let (_options, frame, handles) = vault_window::build_frame(
                cache_for_vault.clone(),
                fill_stats_for_vault,
                // `Ready`, not a fetch of its own: `StartupWork::produce`
                // already asked `bw status` on the worker thread, beside the
                // spinner this window was showing at the time. That is the
                // same "never on the frame thread" rule the tray path now
                // follows by fetching alongside its window -- one mechanism,
                // reached differently -- so the vault stage paints the avatar
                // on its first frame and nothing appears late here.
                vault_window::AccountDetails::Ready(work.details.clone()),
                token.to_string(),
                icon_cache_dir_for_vault,
                auto_lock_for_vault,
                // The readiness probe in `prepare` has just answered, so the
                // backend is up and the vault's own initial load may skip its
                // readiness wait -- the same exemption `open_vault_window`
                // makes when it finds the backend already running.
                true,
                accounts_for_vault,
                // This window's first frame already installed the fonts,
                // rounded the corners and raised it, three stages ago.
                true,
            );
            Some((frame, handles))
        },
    );

    // Closing the card is the same gesture, and costs the same thing, as
    // closing the old login window: every downstream operation needs a session.
    // `login_ui::run_login_flow` is where that decision lives for every OTHER
    // caller and it exits the process; this window cannot go through it (it
    // does not open a window of its own), so it says the same thing here.
    let Some(token) = outcome.token else {
        log::error!("the startup window was closed without producing a session token; exiting");
        std::process::exit(1);
    };
    if let Err(e) = store.save(&token) {
        log::error!("failed to persist session token: {e}");
    }
    session_token = token;
    startup_vault = outcome.vault;

    // **Before the arms split, not inside them.** Whether the worker finished
    // or `app_window::WORKING_DEADLINE` fired, this process may have a
    // `bw serve` listening on `BW_SERVE_PORT`; the two arms differ in what
    // they do next, not in whether that is possible. Asking here is what
    // stops the `None` arm from assuming the answer is no and then failing
    // fatally on its own orphan -- see `StartupChildHandoff`.
    bw_serve_child = startup_child.claim();

    match outcome.prepared {
        Some(work) => {
            // Fatal exactly where it was before this change -- `start_backend`
            // used to end the process itself -- and now on the main thread,
            // where the message box it puts up is in front of the user.
            if let Err(e) = work.child {
                fatal_startup_error(&format!(
                    "Deskwarden could not start its Bitwarden backend.\n\n{e}"
                ));
            }
            if bw_serve_child.is_none() {
                log::error!(
                    "the startup worker reported that `bw serve` started but parked no handle \
                     for it; the backend cannot be stopped or restarted from here"
                );
            }
            // `build_vault` above already armed `startup_entries` and filled the
            // cache on the happy path; this arm is the one where the readiness
            // probe failed, and it is the SAME recovery startup has always run.
            if let Err(e) = work.items {
                let items = recover_from_failed_vault_wait(
                    &e,
                    &vault,
                    &schedule,
                    &mut bw_serve_child,
                    &mut session_token,
                    &job,
                    &store,
                    &config_dir,
                    login,
                );
                *startup_entries.borrow_mut() = match_entries(&items);
                seed_cache_at_startup(&cache, items, startup_epoch);
            }
        }
        None => {
            // The window ended before the work landed -- the user closed it,
            // or `WORKING_DEADLINE` fired on a worker still blocked in the
            // readiness probe or in `check_bw_status_details`.
            //
            // **The worker may well have started a backend by then**, and
            // this arm used to state the opposite and pass `None` down. The
            // claim above has already adopted it if so, which is what lets
            // the recovery below actually kill it: recovery's first act is
            // `stop_bw_serve`, and with `None` it stopped nothing, re-asked
            // for the master password, then died on the port its own orphan
            // was holding.
            if bw_serve_child.is_some() {
                log::warn!(
                    "the startup window gave up while a `bw serve` this process started was \
                     already running; adopting it so the recovery below can stop it instead of \
                     colliding with it"
                );
            }
            let items = recover_from_failed_vault_wait(
                "the startup window closed before the vault backend was ready",
                &vault,
                &schedule,
                &mut bw_serve_child,
                &mut session_token,
                &job,
                &store,
                &config_dir,
                login,
            );
            *startup_entries.borrow_mut() = match_entries(&items);
            seed_cache_at_startup(&cache, items, startup_epoch);
        }
    }
    }

    log::info!(
        "match engine loaded with {} app match(es)",
        startup_entries.borrow().len()
    );
    engine.rebuild(&startup_entries.borrow());

    // The lifecycle this app promises: unlock -> start the backend -> fill
    // the cache once -> *then* obey the policy. The backend has had to be up
    // unconditionally until now, because nothing above this point could have
    // populated the cache without it -- but with the cache now filled, a
    // `keep_backend_running = false` setting means it should already be
    // torn back down again before the tray even appears, not "eventually,
    // the next time something happens to notice". Everything downstream (the
    // vault window opening, the tray's Sync item, another lock) restarts it
    // only for as long as it is actually needed and reconciles again
    // afterwards -- see `stop_backend_if_idle` and the main loop below.
    stop_backend_if_idle(&mut bw_serve_child, settings.keep_backend_running);

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
    // `mut`: the "Accounts" submenu is rebuilt in place after every add,
    // removal and switch, and rebuilding mints new `MenuId`s that the tray has
    // to remember.
    let mut tray = tray::build_tray();
    // Filled once here, so the submenu is correct before the user can open it,
    // and again after every account change below. `accounts_state` is `None`
    // for `StartupAccounts::NoAccountList` -- see `tray::accounts_menu_plan`,
    // which says so in the menu rather than leaving it empty.
    tray.rebuild_accounts_menu(accounts_state.as_ref());

    let current_version =
        Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is not valid semver");

    // Two agents, not one, because the two updater requests need two different
    // *kinds* of bound and ureq can carry only one per agent: the releases
    // check is a small JSON response bounded by total time, the installer
    // download is a ~6 MB stream bounded by time without progress. See
    // `deskwarden::http_agent`. Both are cheap `Arc`-backed handles, so it's
    // fine to clone them into the background threads below.
    let update_check_agent = updater::build_api_agent();
    let download_agent = updater::build_download_agent();

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
        let agent = update_check_agent.clone();
        let version = current_version.clone();
        let tx = update_tx.clone();
        std::thread::spawn(move || {
            if let Some(release) = check_for_update_logged(&version, &agent) {
                let _ = tx.send(release);
            }
        });
    }
    let mut last_update_check = Instant::now();

    // Prefetches the account email + server URL the vault window's toolbar
    // needs (see `open_vault_window`), on its own thread: `bw status`
    // regularly takes 1-3s to spawn on Windows, and `open_vault_window`
    // used to call it inline in the tray-click handler, so every "Open
    // Vault" -- including the very first one -- waited that long before the
    // window even appeared. Polled non-blockingly below, same shape as
    // `update_rx`; `open_vault_window` still falls back to a synchronous
    // call itself if a click lands before this has reported back.
    let mut cached_status_details: Option<login_ui::BwStatusDetails> = None;
    // **Which account the prefetch is ABOUT, carried with its answer.** `bw
    // status` reports on whatever profile the CLI is pointed at when it runs,
    // and this one is spawned before the startup window's outcome is dispatched
    // -- a window that can switch, add or remove an account. Its answer can
    // therefore land describing somebody else. Checked at the drain rather than
    // assumed, because what is done with it now is not only a toolbar label:
    // `learn_active_account_details` WRITES it into `settings.json`, and an
    // address learned into the wrong account is the hash bug with a worse
    // failure mode.
    let prefetch_for = active_account.as_ref().map(|a| a.id.clone());
    let (status_details_tx, status_details_rx) =
        mpsc::channel::<(Option<accounts::AccountId>, login_ui::BwStatusDetails)>();
    {
        let tx = status_details_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send((prefetch_for, login_ui::check_bw_status_details()));
        });
    }

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

    // The process id of the last real (not our own) foreground window, kept
    // up to date alongside every event below. "Add app..." defaults its
    // process picker to this -- the app the user was just in -- rather than
    // making them search for it every time.
    let mut last_active_pid: Option<u32> = None;
    let own_pid = std::process::id();

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
        if event.pid != own_pid {
            last_active_pid = Some(event.pid);
        }
        process_foreground_event(
            &event,
            &cache,
            &injector,
            &fill_stats,
            &engine,
            &mut pending_hotkey_fill,
            &mut last_dispatched_hwnd,
            &deskwarden::injector::sequence::REAL_NOTIFIER,
        );
    }

    // Outcome of a background operation that starts or restarts `bw serve`:
    // either `open_vault_window` making sure it's up before showing the
    // window, or the tray's "Sync" item forcing a resync. Reported back here
    // rather than joined inline on whichever thread kicked it off --
    // `try_start_backend` can take up to 30s (a port-release wait plus a
    // synchronous `bw sync`), and blocking on that before returning control
    // to the main loop used to freeze the tray, hotkey, and window-watching
    // for the whole wait -- see the fix note on `open_vault_window`.
    //
    // Both operations funnel through this one channel (rather than one each)
    // so `backend_task_in_progress` below can guarantee at most one is ever
    // in flight: two `try_start_backend` calls racing to bind the same port
    // would make one fail for a reason that has nothing to do with a real
    // problem, and it also means there is exactly one place -- not two -- a
    // lock event has to drain before it can safely stop/restart the backend
    // itself (see `open_vault_window`'s `locked` branch).
    let (backend_op_tx, backend_op_rx) = mpsc::channel::<BackendOp>();
    // `Some((started, kind))` while a background backend operation is in
    // flight, the instant it was set recording when -- rather than a plain
    // `bool` -- so the main loop below can tell a merely-slow operation
    // apart from one that has been outstanding so long it must be treated as
    // wedged (see the `BACKEND_OP_TIMEOUT` check right after the
    // non-blocking drain). `run_bw_sync` (`Command::output()`, no timeout of
    // its own) and `try_start_backend` (which calls it) have no bound on how
    // long they can take, so without this a stalled `bw sync` would leave
    // this flag `Some` forever: `stop_backend_if_idle` refuses to run while
    // it's set (save-memory mode never reclaims the backend's memory),
    // `open_vault_window` refuses to start a fresh attempt while it's set
    // (writes and TOTP stay dead), and the tray item is stuck disabled on
    // "Syncing...". `kind` records which of the two operations
    // (`BackendOpKind`) this is, so the wedge-deadline check can report a
    // stall in terms of what was actually requested (review Minor 4) instead
    // of always assuming a sync.
    let mut backend_task_in_progress: Option<(Instant, BackendOpKind)> = None;
    // Tray menu clicks that arrived while one of this app's blocking windows
    // was up and SURVIVED the sweep that runs when it closes -- i.e. requests
    // that window did not answer. Popped before the channel below, so they are
    // handled in the order the user made them rather than behind whatever has
    // arrived since. Empty on every ordinary iteration; see
    // `requests_outliving_a_window`.
    let mut pending_menu_events: VecDeque<MenuEvent> = VecDeque::new();

    // **The startup window's vault outcome, dispatched by the one dispatcher.**
    //
    // The single window showed the vault, and the user may have locked it,
    // asked for Preferences, or switched account from its titlebar before
    // closing it. Every one of those is a thing `open_vault_window` already
    // knows how to do -- and the lock/re-auth one is `resettle_session`, the one
    // hardened teardown-and-repopulate sequence in this app, which must keep
    // being reached from the code that already reaches it rather than from a
    // second copy written here.
    //
    // So the result is HANDED IN rather than acted on: `open_vault_window`'s
    // first pass dispatches it instead of opening a window, and every pass after
    // that opens a real one. That is what "the window closes and reopens" now
    // means for a switch or Preferences when the window that closed was the
    // startup window -- it closed, and the vault comes back in a window of its
    // own, exactly as it does from a tray click.
    //
    // HERE and not next to `app_window::run`, because all of that needs the tray
    // -- `resettle_session` takes `&tray`, and a switch rebuilds the accounts
    // submenu -- and the tray does not exist until a few lines above this one.
    // Nothing in between can act on the session: the update-check thread and the
    // hotkey registration do not touch it.
    if startup_vault.is_some() {
        open_vault_window(
            &cache,
            &fill_stats,
            &mut session_token,
            &mut bw_serve_child,
            &job,
            &mut store,
            &schedule,
            &mut engine,
            &icon_cache_dir,
            &mut cached_status_details,
            &config_dir,
            &mut active_account,
            &mut accounts_state,
            first_run_account.as_ref(),
            &mut settings,
            &settings_path,
            &tray,
            &backend_op_tx,
            &backend_op_rx,
            &mut backend_task_in_progress,
            // The one caller that hands an outcome in.
            startup_vault.take(),
        );
        rebuild_after_vault_window(&mut tray, accounts_state.as_ref());
        // The third door into the vault window, swept exactly like the other
        // two: a tray click made while the startup window was up is already
        // answered -- the window was open -- and must not reopen it.
        drop_vault_requests_queued_behind_the_window(
            &mut pending_menu_events,
            &tray.open_vault_id,
        );
        last_dispatched_hwnd = None;
    }

    loop {
        pump_windows_messages();

        if let Some(event) = pending_menu_events.pop_front().or_else(tray::next_menu_event) {
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
                //
                // The cache is cleared for the same reason, one level up:
                // decrypted vault contents shouldn't outlive the moment the
                // user asked to quit, even for the instant between here and
                // `process::exit` actually tearing the process down.
                log::info!("quit requested from tray; killing bw serve");
                cache.clear();
                if let Some(child) = bw_serve_child.as_mut() {
                    bw_serve::stop_bw_serve(child);
                }
                std::process::exit(0);
            }

            if event.id == tray.open_vault_id {
                open_vault_window(
                    &cache,
                    &fill_stats,
                    &mut session_token,
                    &mut bw_serve_child,
                    &job,
                    &mut store,
                    &schedule,
                    &mut engine,
                    &icon_cache_dir,
                    &mut cached_status_details,
                    &config_dir,
                    &mut active_account,
                    &mut accounts_state,
                    first_run_account.as_ref(),
                    &mut settings,
                    &settings_path,
                    &tray,
                    &backend_op_tx,
                    &backend_op_rx,
                    &mut backend_task_in_progress,
                    // This door opens its own window; there is no outcome to
                    // hand it.
                    None,
                );
                rebuild_after_vault_window(&mut tray, accounts_state.as_ref());
                // Everything the user clicked at the tray while the window was
                // up is queued behind this line. Asking for the vault window
                // again is already answered -- it was open, and raised -- so
                // those requests stop here instead of reopening it once per
                // click. Anything else queued behind them is kept and handled
                // on the next iterations. See `requests_outliving_a_window`.
                drop_vault_requests_queued_behind_the_window(
                    &mut pending_menu_events,
                    &tray.open_vault_id,
                );
                last_dispatched_hwnd = None;
            }

            if event.id == tray.preferences_id {
                // Blocks the main loop for as long as the window is open --
                // same as every other window here (`open_vault_window`,
                // `picker_ui::run_picker`). The idle backend reconciliation
                // a bit further down only runs once this returns, so a
                // changed `keep_backend_running` takes effect on the very
                // next iteration rather than waiting for the next launch.
                let edited = prefs_ui::run(settings.clone());
                if edited != settings {
                    settings = edited;
                    // `persist_preferences`, never a whole-struct save: this
                    // binding's `vault_window` is whatever was on disk at
                    // startup, and `vault_window::run` has been writing a
                    // fresh geometry straight to the file every time the
                    // window closed. Saving the struct here wrote that stale
                    // value back and silently reverted the saved geometry;
                    // see `Settings::persist_preferences`.
                    if let Err(e) = settings.persist_preferences(&settings_path) {
                        log::warn!("could not save settings: {e}");
                    }
                }
                // The vault window is not the only blocking one, and this is
                // the other one the TRAY can open: repeat clicks on
                // Preferences queue behind it exactly the same way and used to
                // reopen it once per click. Swept for the request it answers
                // and no other -- a tray-icon left click made while
                // Preferences was up is a genuine "open the vault" that
                // nothing has served, so that channel is deliberately left
                // alone here.
                drain_requests_queued_behind_a_window(
                    &mut pending_menu_events,
                    &tray.preferences_id,
                );
                last_dispatched_hwnd = None;
            }

            // ---------------------------------------------------------------
            // The "Accounts" submenu: switch, add, remove.
            //
            // Read out of the tray FIRST, and by value. `account_for_menu_id`
            // borrows the tray, the resettle below needs `&tray`, and the
            // rebuild afterwards needs `&mut tray` -- so nothing here may hold
            // a borrow across the work.
            // ---------------------------------------------------------------
            let switch_target = tray.accounts().account_for_menu_id(&event.id).cloned();
            let add_clicked = tray.accounts().is_add(&event.id);
            let remove_clicked = tray.accounts().is_remove(&event.id);
            if tray.accounts().owns(&event.id) {
                // **ONE resettle closure for all three actions**, handed to
                // each by `&mut`. Three copies of this would be three chances
                // to get the hardest sequence in this codebase subtly
                // different; `the_tray_settles_every_account_action_through_
                // the_one_sequence` pins that what is in here is the sequence
                // and not a fourth teardown path written inline.
                let mut resettle = |config_dir: &std::path::Path,
                                    to: &Account,
                                    store: &session_store::SessionStore|
                 -> ResettleReport {
                    // Built for the account being settled ONTO. A context for
                    // the account being left would put the master-password
                    // prompt up for the wrong account and seal the Windows
                    // Hello blob under the wrong id.
                    let login = login_context(config_dir, Some(to), first_run_account.as_ref());
                    let mut declined = false;
                    let outcome = resettle_session(
                        &cache,
                        &mut engine,
                        &mut bw_serve_child,
                        &job,
                        &schedule,
                        &tray,
                        &backend_op_rx,
                        &mut backend_task_in_progress,
                        &mut cached_status_details,
                        &mut session_token,
                        || {
                            let token = authenticate_for_switch(store, login);
                            declined = token.is_none();
                            token
                        },
                    );
                    // The three-way answer `ResettleOutcome`'s two variants
                    // cannot give: only whoever built the `authenticate`
                    // closure can tell "the user closed the prompt" from
                    // "nothing came up to serve it".
                    match outcome {
                        ResettleOutcome::BackendStarted => ResettleReport::Settled,
                        ResettleOutcome::BackendNotStarted if declined => ResettleReport::Declined,
                        ResettleOutcome::BackendNotStarted => ResettleReport::NotStarted,
                    }
                };

                // Whatever the click turned out to be, reported through one
                // match below: a switch and an add both land in a
                // `SwitchOutcome`, and reporting each in its own arm is how
                // one of them quietly stops raising the failure the other one
                // does.
                let reported: Option<(String, SwitchOutcome)> =
                    if let Some(target) = switch_target.as_ref() {
                    // **`switchable()`, not `all()`.** `all()` still reports
                    // the active account and still reports duplicate ids, and
                    // it is not emptied when switching is refused. The menu was
                    // built from `switchable()` too; re-asking here costs
                    // nothing and means a stale submenu -- one built before a
                    // state change and clicked after it -- cannot smuggle a
                    // target past the gate.
                    let picked = accounts_state
                        .as_ref()
                        .and_then(|state| state.switchable().iter().find(|a| &a.id == target))
                        .cloned();
                    match (picked, accounts_state.as_mut(), active_account.as_mut()) {
                        (Some(to), Some(state), Some(active)) => {
                            let from = active.clone();
                            let outcome = switch_to_account(
                                &config_dir,
                                &from,
                                &to,
                                active,
                                &mut store,
                                &mut resettle,
                            );
                            if outcome == SwitchOutcome::Switched {
                                state.adopt(to.clone());
                                // **After the switch has landed, never
                                // before.** A list written first and a switch
                                // that then failed would leave settings.json
                                // naming an account this process is not on,
                                // and the next launch would resume the wrong
                                // one -- indistinguishable from the
                                // `relativeDataDir` trap.
                                if let Err(e) = settings::Settings::persist_accounts(
                                    &settings_path,
                                    state.all(),
                                    Some(&to.id),
                                ) {
                                    log::warn!(
                                        "could not persist the active account after a switch: {e}"
                                    );
                                }
                            }
                            Some((format!("switch to {}", account_label(&to)), outcome))
                        }
                        _ => {
                            log::warn!(
                                "the account the tray offered is no longer one this app can \
                                 switch to; the submenu is being rebuilt"
                            );
                            None
                        }
                    }
                } else if add_clicked {
                    match (accounts_state.as_mut(), active_account.as_mut()) {
                        (Some(state), Some(active)) => {
                            let outcome = add_account(
                                &config_dir,
                                &settings_path,
                                state,
                                active,
                                &mut store,
                                // The sign-in window, run against whatever
                                // profile `add_account` has pointed the CLI at
                                // -- which is the NEW account's directory, and
                                // is the whole reason this is injected rather
                                // than called in there.
                                |prepared| {
                                    let login = login_context(
                                        &config_dir,
                                        Some(prepared),
                                        first_run_account.as_ref(),
                                    );
                                    login_ui::run_login_flow_for(login.account, login.first_run)
                                },
                                // Asked of a NAMED directory. The active-profile
                                // form would report the account being left.
                                login_ui::check_bw_status_details_in,
                                &mut resettle,
                            );
                            Some(("add an account".to_string(), outcome))
                        }
                        _ => {
                            log::warn!("cannot add an account: this app has none to add one to");
                            None
                        }
                    }
                } else if remove_clicked {
                    match (accounts_state.as_mut(), active_account.as_mut()) {
                        (Some(state), Some(active)) => {
                            let doomed = state.active().id.clone();
                            let label = account_label(state.active()).to_string();
                            if confirm_account_removal(&label) {
                                if let Err(reason) = remove_account(
                                    &config_dir,
                                    &settings_path,
                                    state,
                                    &doomed,
                                    active,
                                    &mut store,
                                    &mut resettle,
                                    // **The directory form, never the
                                    // active-profile one.** That one acts on
                                    // whatever this process is pointed at, and
                                    // by the time the logout runs the app has
                                    // already settled onto the SURVIVOR -- so
                                    // it would sign out the account the user is
                                    // keeping and leave the doomed one signed
                                    // in on the server. The name is spelled
                                    // once, here, because
                                    // `the_removal_logs_out_in_the_doomed_
                                    // accounts_own_directory` counts it.
                                    login_ui::bw_logout_in,
                                ) {
                                    log::warn!("could not remove {label}: {reason}");
                                    message_box("Deskwarden", &reason, MB_ICONERROR | MB_OK);
                                }
                            }
                            None
                        }
                        _ => {
                            log::warn!("cannot remove an account: this app has none");
                            None
                        }
                    }
                } else {
                    None
                };

                if let Some((what, outcome)) = reported {
                    report_account_action(&what, outcome);
                }
            }
            // **Unconditional, and outside the borrow above.** Every one of the
            // three actions can change what the submenu should say -- and so
            // can one that failed, since a rolled-back switch still leaves the
            // list it was built from intact but the ids it minted stale. A
            // submenu rebuilt only on success is one whose ids outlive the
            // state they name.
            if add_clicked || remove_clicked || switch_target.is_some() {
                tray.rebuild_accounts_menu(accounts_state.as_ref());
                last_dispatched_hwnd = None;
            }

            if event.id == tray.add_app_id {
                // Two-step flow: choose the vault item the credentials come
                // from, then choose the process to attach to it. Both
                // `pick_vault_item` and `run_picker`'s window/trigger choice
                // work purely from the cache and `window_list` -- but the
                // Save at the very end of `run_picker` calls
                // `cache.set_app_match`, a write, which needs `bw serve` up
                // *and answering*, not merely started (review 10's Important
                // 2 -- see `run_picker`'s own doc for how it now waits for
                // that itself rather than assuming a kicked-off start is
                // enough). Read once, before it might change, and reused for
                // `run_picker`'s own readiness wait below: whether `bw serve`
                // was already up at this exact moment is also exactly what
                // decides whether `run_picker` needs to wait for it at all
                // (same `backend_already_running` exemption `open_vault_window`
                // and `vault_window::run` already make).
                let backend_already_running = backend_is_running(&mut bw_serve_child);

                // Review 9's Important: in save-memory mode nothing here used
                // to start `bw serve` at all, so a save always failed after
                // two windows of user effort with nothing visible on screen.
                // Kick a start off now, the same non-blocking way
                // `open_vault_window` does. `run_picker` itself waits for it
                // to actually answer before letting Save fire.
                if needs_backend_start(&backend_task_in_progress, backend_already_running) {
                    backend_task_in_progress = Some((Instant::now(), BackendOpKind::EnsureRunning));
                    // A Sync click landing while this start is in flight
                    // would otherwise be silently dropped by the
                    // `backend_task_in_progress` guard below with nothing to
                    // show for it (review 10's Minor 6). Disabling the item
                    // here (not `set_sync_in_progress` -- this isn't a sync)
                    // means the click can't be issued in the first place;
                    // `apply_backend_op`'s `EnsureRunning` arms re-enable it
                    // once this completes.
                    tray::set_sync_busy_with_backend_op(&tray);
                    spawn_backend_start(session_token.clone(), job.clone(), backend_op_tx.clone());
                }

                // The vault session this "Add app..." belongs to is captured
                // INSIDE `AddAppFlow::begin`, before the first of its two
                // windows opens -- review 25's Minor 3 for why it is captured
                // there, review 30's Minor 5 for why the capture is no longer
                // a statement of its own that a later edit can slide below the
                // window it guards.
                if let Some((flow, item)) = AddAppFlow::begin(&cache) {
                    log::info!("adding an app match to vault item {}", item.id);
                    let target_item = item.clone();
                    match picker_ui::run_picker(cache.clone(), item, last_active_pid, backend_already_running) {
                        Some(saved) => {
                            log::info!(
                                "saved app match for {} ({:?}, {:?})",
                                saved.app_match.process,
                                saved.app_match.trigger,
                                saved.write
                            );
                            // Make the new match live immediately rather than
                            // waiting for the user to trigger a sync -- from
                            // the CACHE, which already holds the save
                            // (`run_picker`'s Save goes through
                            // `cache.set_app_match`, which updates the
                            // snapshot on success precisely so nothing has to
                            // re-fetch it).
                            //
                            // This used to be `app::refresh_match_engine`
                            // (deleted in review 23, once this was its last
                            // caller and it had none left), a THIRD
                            // live `list_items` against `bw serve` after the
                            // picker's own populate and the save's PUT
                            // (review 21's Minor). A transient 500 or a reset
                            // connection on that one request logged a warn and
                            // left the engine unarmed, so the match the user
                            // had just spent two windows creating did not go
                            // live until some later sync -- the exact failure
                            // mode review 16 removed from the unlock path, and
                            // the reason nothing in this app arms the engine
                            // from a request that has not already succeeded.
                            //
                            // ONE LOCK, not `is_populated()` then `items()`
                            // (review 23's fifth Minor): the two-lock spelling
                            // was sound only by an argument about which thread
                            // every `clear` runs on, and this is a place where
                            // "populated" and "the items" must be the same
                            // observation.
                            //
                            // AND ONE DOOR, not two (review 25's Minor 3, and
                            // for real since review 28's Important 1 deleted
                            // the items-only projection this used to go
                            // through). The era `flow` carries is not ceremony
                            // here, because the two windows this flow opens
                            // are a long user interaction and a `clear` -- the
                            // vault locking, or a re-auth into a possibly
                            // different ACCOUNT -- can land inside it. Arming
                            // the engine from whatever snapshot happens to
                            // exist afterwards would be arming it from a vault
                            // the user did not just edit.
                            //
                            // WHAT TO BUILD FROM is `add_app_rebuild_source`,
                            // reached through `flow` so the era it checks is
                            // the one captured before the FIRST window rather
                            // than whatever a later edit leaves nearest: on a
                            // `ServerOnly` save the snapshot does not hold the
                            // match, and rebuilding straight from it is review
                            // 28's Important 2. See both functions.
                            match flow.rebuild_source(&saved, &target_item) {
                                Some(items) => {
                                    let entries = match_entries(&items);
                                    log::info!(
                                        "match engine refreshed: {} app match(es)",
                                        entries.len()
                                    );
                                    engine.rebuild(&entries);
                                }
                                None => {
                                    // Not reachable from here today -- every
                                    // `clear` runs on this thread, which is
                                    // blocked inside the two picker windows for
                                    // the whole flow, and `pick_vault_item`
                                    // populates the cache and returns nothing to
                                    // pick if that fails -- and checked rather
                                    // than assumed, because rebuilding from an
                                    // empty or another account's snapshot would
                                    // DISARM autofill instead of merely failing to
                                    // arm the new match.
                                    //
                                    // WHAT THIS PROMISES IS CONDITIONED ON THE
                                    // WRITE (review 30's Minor 2). It used to
                                    // end "The match goes live at the next
                                    // sync or unlock" unconditionally, which
                                    // is the exact promise `server_only_notice`
                                    // has a test FORBIDDING it from making and
                                    // that both of `set_app_match`'s warns were
                                    // edited to stop making: for a `ServerOnly`
                                    // save's `position` miss the reachable path
                                    // is the item having stopped existing, and
                                    // no sync brings a deleted item's match
                                    // back.
                                    let outlook = match saved.write {
                                        AppMatchWrite::WroteThrough => {
                                            "The server has the match and so did this session's \
                                             snapshot, so the next sync or unlock brings it back"
                                        }
                                        // REVIEW 31'S MINOR 3. This used to end
                                        // flatly "so no sync is promised here",
                                        // which over-claims in the other
                                        // direction: `ServerOnly` has TWO
                                        // producers and its own doc says the
                                        // unpopulated-cache one IS cured by any
                                        // populate. Only the second -- an id
                                        // absent from a POPULATED snapshot,
                                        // i.e. the item is gone -- is beyond a
                                        // sync. Saying "no sync" for both is
                                        // the same class of over-claim that
                                        // review 30's claims 3 and 4 existed to
                                        // remove, merely inverted, and this
                                        // line cannot tell the two apart.
                                        AppMatchWrite::ServerOnly => {
                                            "The server has the match; which of \
                                             AppMatchWrite::ServerOnly's two misses this was \
                                             decides what follows -- an unpopulated cache is \
                                             cured by any populate, while an id missing from a \
                                             populated snapshot means the item is gone and no \
                                             sync brings it back. This line cannot tell them \
                                             apart, so it promises neither"
                                        }
                                    };
                                    log::warn!(
                                        "an app match was saved against a vault cache that is no \
                                         longer the session it was saved in (unpopulated, or \
                                         cleared and refilled meanwhile); leaving the match \
                                         engine as it is rather than rebuilding it from the wrong \
                                         snapshot. {outlook}"
                                    );
                                }
                            }
                        }
                        None => log::info!("app-match picker cancelled (or save failed)"),
                    }
                }

                // THE THIRD BLOCKING WINDOW THIS TRAY OPENS, and it opens
                // TWO of them back to back (`AddAppFlow::begin`'s item picker,
                // then `run_picker`) -- so this is the longest any tray click
                // can sit queued, and repeat clicks on "Add app..." ran the
                // whole two-window flow again once per click. Swept for the
                // request it answers and no other; see
                // `requests_outliving_a_window`.
                drain_requests_queued_behind_a_window(&mut pending_menu_events, &tray.add_app_id);
                // Our own picker windows just stole and released foreground.
                // Forget the last-dispatched hwnd so the window the user
                // returns to is treated as a fresh switch rather than being
                // suppressed as a repeat.
                last_dispatched_hwnd = None;
            }

            if event.id == tray.sync_id {
                // Defensive re-check, same reasoning as the update item just
                // below: the item is disabled while a sync (or a
                // window-open's own backend start) is in flight, but the
                // click event is handled the same way regardless of whether
                // tray-icon's disabled state actually suppressed the click.
                if backend_task_in_progress.is_some() {
                    log::info!("sync requested from tray but a backend operation is already in \
                                 progress; ignoring");
                } else {
                    log::info!("sync requested from tray");
                    tray::set_sync_in_progress(&tray);
                    backend_task_in_progress = Some((Instant::now(), BackendOpKind::Sync));

                    // Whether `bw serve` needs to be started first is decided
                    // here, on the main thread (the only place that owns
                    // `bw_serve_child`), and handed to the background thread
                    // as a plain bool -- see `backend_is_running`'s doc for
                    // why a `Some` child isn't automatically "running".
                    let currently_running = backend_is_running(&mut bw_serve_child);
                    spawn_sync(
                        session_token.clone(),
                        job.clone(),
                        cache.clone(),
                        currently_running,
                        backend_op_tx.clone(),
                    );
                }
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
                        let agent = download_agent.clone();
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

        // Left click opens the vault directly; right click still shows the
        // menu (built with `with_menu_on_left_click(false)` specifically so
        // the two aren't the same action). Same event, same recovery path as
        // the menu's "Open Vault" item above -- just a different trigger.
        if tray::next_left_click() == Some(true) {
            open_vault_window(
                &cache,
                &fill_stats,
                &mut session_token,
                &mut bw_serve_child,
                &job,
                &mut store,
                &schedule,
                &mut engine,
                &icon_cache_dir,
                &mut cached_status_details,
                &config_dir,
                &mut active_account,
                &mut accounts_state,
                first_run_account.as_ref(),
                &mut settings,
                &settings_path,
                &tray,
                &backend_op_tx,
                &backend_op_rx,
                &mut backend_task_in_progress,
                // This door opens its own window; there is no outcome to hand
                // it.
                None,
            );
            rebuild_after_vault_window(&mut tray, accounts_state.as_ref());
            // The second door into the same window, swept the same way and
            // through the same name -- see the tray menu's "Open Vault"
            // handler above. This is the door the report was made through:
            // three left clicks while the vault loaded became three
            // windows.
            drop_vault_requests_queued_behind_the_window(
                &mut pending_menu_events,
                &tray.open_vault_id,
            );
            last_dispatched_hwnd = None;
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
                    fill_from_vault(
                        &cache,
                        &injector,
                        &fill_stats,
                        &item_id,
                        hwnd,
                        &deskwarden::injector::sequence::REAL_NOTIFIER,
                    );
                } else {
                    log::info!("fill hotkey ignored: foreground window is no longer the match");
                }
            }
        }

        // Non-blocking: whenever a background backend operation
        // (`open_vault_window` making sure `bw serve` is up, or a
        // tray-triggered Sync) reports back, apply its outcome. This is also
        // where `backend_task_in_progress` is cleared, so the reconciliation
        // step right after it is never fighting a still-in-flight operation.
        if let Ok(op) = backend_op_rx.try_recv() {
            backend_task_in_progress = None;
            apply_backend_op(op, &mut bw_serve_child, &cache, &mut engine, &tray);
        }

        // A deadline on the FLAG itself, not just on any one `recv` -- see
        // `backend_task_in_progress`'s own doc for why a stalled `bw sync`
        // (or backend start) can otherwise wedge it `Some` forever with
        // nothing here ever noticing on its own. Reusing `BACKEND_OP_TIMEOUT`
        // rather than a second constant: it already means "how long a
        // legitimate backend operation can take before something is
        // genuinely wrong" for `open_vault_window`'s own bounded wait on this
        // same flag, and that reasoning applies here unchanged.
        if let Some((started, kind)) = backend_task_in_progress {
            if backend_task_is_wedged(started, BACKEND_OP_TIMEOUT) {
                backend_task_in_progress = None;
                // Report -- and, on the tray, only claim to have synced --
                // what actually stalled (review Minor 4). But *both* kinds
                // must still re-enable the tray's "Sync" item here (review
                // 11's Important 3): the comment this used to carry --
                // "an `EnsureRunning` wedge never touched the tray's Sync
                // item in the first place" -- stopped being true as soon as
                // the `tray.add_app_id` handler started disabling it before
                // kicking off its own `EnsureRunning` (see that call site).
                // Leaving it disabled here means a stalled "Add app..."
                // backend start (a hung `bw sync`, exactly the case this
                // wedge-deadline check exists for) permanently kills Sync
                // for the rest of the session: nothing else can re-enable
                // it, since `set_sync_idle`/`set_sync_failed` are only
                // reachable through paths that themselves require the item
                // to already be clickable.
                match kind {
                    BackendOpKind::Sync => {
                        log::error!(
                            "a background sync has been outstanding for over \
                             {BACKEND_OP_TIMEOUT:?} with no result; treating it as failed so the \
                             tray doesn't stay stuck on \"Syncing...\" forever"
                        );
                        tray::set_sync_failed(&tray);
                    }
                    BackendOpKind::EnsureRunning => {
                        log::error!(
                            "a background bw serve start (no sync requested) has been \
                             outstanding for over {BACKEND_OP_TIMEOUT:?} with no result; \
                             treating it as failed so the backend lifecycle doesn't stay wedged \
                             on it forever"
                        );
                        // Nothing is in flight any more, so the item goes all
                        // the way back to idle -- label, enabled state and
                        // tooltip together (review 18's Minor). Re-enabling
                        // alone used to be enough here only because the
                        // `tray.add_app_id` handler never changes the label;
                        // a wedged *sync* reaching a release like this one is
                        // what left the menu reading "Syncing..." on an idle
                        // item. Not `set_sync_failed`: no sync was requested,
                        // and claiming one failed would be untrue.
                        tray::set_sync_idle(&tray);
                    }
                }
            }
        }

        // The policy, reconciled here -- at idle, in the main loop -- rather
        // than only as a side effect of the vault window opening or closing.
        // This is what makes `keep_backend_running = false` actually save
        // memory in the common case (autofill-only, vault window never
        // opened this session): without it, `bw serve` -- started
        // unconditionally at startup so the cache could be populated --
        // would simply stay up forever, since nothing else was ever in a
        // position to notice and stop it. Only the "stop" half is evaluated
        // here; "start" is never something idle should initiate on its own
        // now that the periodic refresh that used to do that is gone (a
        // failed start would otherwise retry every ~200ms with nothing
        // throttling it) -- the three places that *do* need the backend
        // (startup, `open_vault_window`, the tray's Sync item) each ask for
        // it explicitly and this only ever tears it back down afterwards.
        if backend_task_in_progress.is_none() {
            stop_backend_if_idle(&mut bw_serve_child, settings.keep_backend_running);
        }

        if last_update_check.elapsed() >= UPDATE_CHECK_INTERVAL {
            // Same off-thread treatment as the startup check above: this now
            // runs once a day from a live, interactive app, but it still
            // talks to an external host, so it's still kicked off on a
            // background thread rather than blocking the main loop (and
            // therefore tray/hotkey/window-watch responsiveness) for however
            // long `api.github.com` takes to answer.
            let agent = update_check_agent.clone();
            let version = current_version.clone();
            let tx = update_tx.clone();
            std::thread::spawn(move || {
                if let Some(release) = check_for_update_logged(&version, &agent) {
                    let _ = tx.send(release);
                }
            });
            last_update_check = Instant::now();
        }

        // Non-blocking: whenever the prefetch thread (or a fallback
        // synchronous call inside `open_vault_window`) reports back, keep
        // the cache warm so the next "Open Vault" doesn't pay the `bw
        // status` spawn again.
        if let Ok((about, details)) = status_details_rx.try_recv() {
            // Everything this answer is allowed to DO -- the staleness guard,
            // the two copies it teaches, and the `settings.json` write -- lives
            // in `adopt_startup_prefetch`, which is a plain function the tests
            // call with account A's answer and account B active. What is left
            // here is the one thing that cannot move: the cache this loop owns,
            // filled from what that function hands back.
            //
            // The shape matters as much as the split. There is exactly ONE
            // account id passed in -- the one the prefetch was started FOR --
            // because the id this app is on NOW is read inside, off
            // `active_account`. So the guard cannot be handed the active
            // account twice, and the answer cannot be used without binding it.
            if let Some(adopted) = adopt_startup_prefetch(
                &settings_path,
                &mut accounts_state,
                &mut active_account,
                about.as_ref(),
                details,
            ) {
                cached_status_details = Some(adopted);
            }
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
                    // `bw serve` and clear the cache explicitly before
                    // exiting so the installer (which replaces and
                    // relaunches this binary) doesn't leave an orphaned
                    // backend serving the unlocked vault, or decrypted vault
                    // contents sitting in this process's memory a moment
                    // longer than it takes to tear down.
                    log::info!("update installer launched; shutting down for update");
                    cache.clear();
                    if let Some(child) = bw_serve_child.as_mut() {
                        bw_serve::stop_bw_serve(child);
                    }
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
            if event.pid != own_pid {
                last_active_pid = Some(event.pid);
            }
            process_foreground_event(
                &event,
                &cache,
                &injector,
                &fill_stats,
                &engine,
                &mut pending_hotkey_fill,
                &mut last_dispatched_hwnd,
                &deskwarden::injector::sequence::REAL_NOTIFIER,
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

/// How every account action that lands in a [`SwitchOutcome`] is reported —
/// `what` naming the request in the words the message box will use ("add an
/// account", "switch to ana@example.com").
///
/// One function since the vault window's account menu grew the same two actions
/// the tray's Accounts submenu has. Reporting each in its own arm is how one of
/// them quietly stops raising the failure the other one does — and the two
/// menus offer the same operations, so a user who found "Add account..." in the
/// window must not get a quieter failure than one who found it in the tray.
///
/// [`SwitchOutcome::Declined`] is deliberately not raised. Both menus gate
/// "Add account..." on [`AccountsState::can_add`](accounts::AccountsState::can_add),
/// so the only `Declined` this reporting can see is a sign-in the user
/// cancelled — `add_account` answers the same variant for the refused gate, and
/// a message box saying "cancelled" for a `relativeDataDir` block would be
/// naming something that never happened.
fn report_account_action(what: &str, outcome: SwitchOutcome) {
    match outcome {
        // Nothing to say: the app is where the user wanted it.
        SwitchOutcome::Switched => log::info!("{what}: done"),
        SwitchOutcome::Declined => {
            log::info!("the request to {what} was cancelled; nothing changed")
        }
        SwitchOutcome::RolledBack { reason } => {
            log::warn!("could not {what}: {reason}");
            message_box(
                "Deskwarden",
                &format!(
                    "Could not {what}.\n\n{reason}\n\nYou are still signed in to the account \
                     you were on.",
                ),
                MB_ICONERROR | MB_OK,
            );
        }
        SwitchOutcome::StoodDown { reason } => {
            log::error!("could not {what}, and could not go back: {reason}");
            message_box(
                "Deskwarden",
                &format!(
                    "Could not {what}.\n\n{reason}\n\nAutofill is stood down. Use the tray's \
                     \"Sync\" to bring it back.",
                ),
                MB_ICONERROR | MB_OK,
            );
        }
    }
}

/// **May the startup `bw status` prefetch be adopted?** — given the account it
/// was started FOR, and the account this app is on NOW.
///
/// A pure function on purpose. The decision used to live inline in `main`'s
/// frame loop, which no harness can run, and the only test over it read the
/// condition's source text: flipping its `&&` to `||` — which adopts a prefetch
/// for account A onto account B, and PERSISTS it — left the whole suite green.
///
/// `bw status` reports on whatever profile the CLI was pointed at when it ran,
/// and this one is spawned before the startup window's outcome is dispatched —
/// a window that can switch, add or remove an account. So the answer can land
/// describing somebody else.
///
/// Both halves must be a *known* account and they must be the SAME one:
///
/// * no `started_for` — the app had no account when the prefetch was spawned,
///   so `bw status` was pointed at whatever the CLI's default profile is, and
///   nothing says that is the account now active;
/// * no `active_now` — there is nothing to learn the answer onto;
/// * different ids — the startup window moved the app to another account, and
///   writing this address into that account's entry is the "random hash" bug
///   with a worse failure mode: a confident, persisted, WRONG address.
fn prefetch_still_describes_the_active_account(
    started_for: Option<&accounts::AccountId>,
    active_now: Option<&accounts::AccountId>,
) -> bool {
    match (started_for, active_now) {
        (Some(started), Some(now)) => started == now,
        _ => false,
    }
}

/// **The whole of what `main`'s frame loop does with a startup `bw status`
/// answer** — the staleness guard, the two copies it teaches, the
/// `settings.json` write, and the decision to cache — in one function a test
/// can call.
///
/// This body used to sit inline in that loop, which no harness can run, so the
/// only thing anyone could say about it was that its SOURCE TEXT looked right.
/// A reviewer killed it three ways with the entire suite green:
///
/// * negating the guard, so the answer is adopted exactly when it describes
///   somebody else;
/// * passing the ACTIVE account's id as BOTH of the guard's arguments, so it
///   compares the active account with itself and is `true` for any `Some`
///   (`about` becomes unused, warnings are not denied here, and it compiles);
/// * calling the guard, discarding its `bool`, and branching on `if true` —
///   this repo's signature defect verbatim.
///
/// Under all three, account A's email and server URL are written into account
/// B's entry in `settings.json`, and NOTHING DOWNSTREAM CAN NOTICE:
/// `learn_active_account_details`' own guard compares `active_account` against
/// `accounts_state` — two copies `adopt` keeps in step, both naming B — never
/// against the id the prefetch was started for. So the check has to happen
/// here, and something has to evaluate it.
///
/// **`started_for` is the only account id this takes.** The one the app is on
/// NOW is read here, off `active_account`, so there is no second id argument
/// for a caller to get wrong, and no way to hand the guard the active account
/// twice. And the answer is RETURNED rather than written into the loop's cache
/// from in here, so a caller that ignores the result has nothing to cache and
/// does not compile.
///
/// Returns what the caller should cache: `Some(details)` when the answer really
/// is the active account's own, `None` when it was dropped. Dropped rather than
/// cached, because a stale email under a new account is worse than a blank one
/// — it is the same claim the window makes about whose vault is on screen, made
/// about the wrong person — and the next open pays one `bw status` instead.
fn adopt_startup_prefetch(
    settings_path: &Path,
    accounts_state: &mut Option<accounts::AccountsState>,
    active_account: &mut Option<Account>,
    started_for: Option<&accounts::AccountId>,
    details: login_ui::BwStatusDetails,
) -> Option<login_ui::BwStatusDetails> {
    if !prefetch_still_describes_the_active_account(
        started_for,
        active_account.as_ref().map(|a| &a.id),
    ) {
        log::info!(
            "discarding the startup `bw status` prefetch: it describes the account this app \
             started on, not the one it is on now"
        );
        return None;
    }
    learn_active_account_details(settings_path, accounts_state, active_account, &details);
    Some(details)
}

/// **The one place a `bw status` answer becomes something this app remembers
/// about the account it is on**, and writes to `settings.json`.
///
/// The hole it closes: `resolve_startup` mints a first-install account with an
/// EMPTY email — nobody has signed in, so there is nothing to record — and
/// until now nothing filled it in afterwards. `accounts::account_label` falls
/// back to the account id for an empty email, so every menu naming that account
/// named a 32-character hash: the tray's Accounts submenu, and now the vault
/// window's account menu, which is where the user met it ("not some random
/// hash"). The sign-in that follows the mint is when the address becomes
/// knowable, and `bw status` is the only thing that knows it.
///
/// **The caller must already have established that `details` describes the
/// ACTIVE account.** This function cannot check: `BwStatusDetails` carries a
/// status, an email and a server URL, and no directory. Both call sites do —
/// `open_vault_window` refills from a window's own session before any switch
/// runs, and `main`'s drain compares the id the prefetch was started for.
///
/// The two copies are checked to be the same account BEFORE either of them is
/// touched, and then both are updated. `active_account` is what `login_context`
/// and
/// `switch_to_account` name the account by; `AccountsState` is what every menu
/// reads and what gets persisted. They are kept in step by `adopt`, so a
/// disagreement here is a bug elsewhere — and writing one account's address
/// into the other is not the way to find out.
///
/// Persists only on a change, so an app sitting idle does not rewrite
/// `settings.json` every time a window opens. A failed write is survivable: the
/// address is right in memory for this session and is relearned on the next
/// launch, which is the same cost the mint itself already carries.
fn learn_active_account_details(
    settings_path: &Path,
    accounts_state: &mut Option<accounts::AccountsState>,
    active_account: &mut Option<Account>,
    details: &login_ui::BwStatusDetails,
) {
    let (Some(state), Some(active)) = (accounts_state.as_mut(), active_account.as_mut()) else {
        // `StartupAccounts::NoAccountList`: this app has no `Account` of its
        // own, so there is nothing to record an address against and
        // `Settings::save` refuses to write over the file anyway.
        return;
    };
    let (email, server_url) = (details.user_email.as_deref(), details.server_url.as_deref());
    if active.id != state.active().id {
        // NEITHER copy is touched, and nothing is persisted. This used to
        // write `AccountsState` -- the copy `state.all()` persists -- BEFORE
        // reaching this check, so the answer landed in `settings.json` under
        // the other account's id while this very line claimed the address had
        // been left alone. The log was the only thing that was wrong, and it
        // was the only thing anyone would have read.
        log::error!(
            "the active account ({}) is not the one `AccountsState` is on ({}); this `bw \
             status` answer was discarded rather than written into either of them",
            active.id,
            state.active().id
        );
        return;
    }
    let changed = state.learn_active_details(email, server_url);
    accounts::learn_account_details(active, email, server_url);
    if !changed {
        return;
    }

    let active_id = state.active().id.clone();
    log::info!(
        "recording that account {active_id} is {}",
        account_label(state.active())
    );
    if let Err(e) =
        settings::Settings::persist_accounts(settings_path, state.all(), Some(&active_id))
    {
        log::warn!(
            "could not record that {} is {}: {e}; it will be asked for again on the next \
             launch",
            active_id,
            account_label(state.active())
        );
    }
}

/// The text of the confirmation shown before an account's profile is deleted.
///
/// A pure function, so what the user is actually told can be asserted: the
/// dialog itself is a `MessageBoxW` no test can drive, and a confirmation whose
/// wording drifted into "Remove this account?" would be asking about something
/// far less final than what happens next.
fn account_removal_warning(label: &str) -> String {
    format!(
        "Remove {label} from Deskwarden?\n\nThis signs that account out and DELETES its local \
         Bitwarden profile from this computer, including its saved session and its Windows \
         Hello quick unlock. Your vault on the server is not touched, and you can sign in to \
         this account again afterwards.\n\nThis cannot be undone."
    )
}

/// Asks before deleting an account's profile. `true` only on an explicit Yes.
///
/// Defaulted to No (`MB_DEFBUTTON2`), the same way the unrecognized-CLI prompt
/// is and for the same reason: this is the second most destructive thing this
/// app does, and a stray Return should not be what does it.
fn confirm_account_removal(label: &str) -> bool {
    let answer = message_box(
        "Deskwarden: remove account",
        &account_removal_warning(label),
        MB_ICONWARNING | MB_YESNO | MB_DEFBUTTON2,
    );
    if answer != IDYES {
        log::info!("the removal of {label} was cancelled; nothing has been deleted");
    }
    answer == IDYES
}

/// Logs `message`, shows it to the user, and exits.
///
/// The failure paths this replaces logged a line to a file nobody has open and
/// then called `exit(1)`: from the user's side, a double-clicked app that
/// simply never appeared, with no clue as to why.
///
/// **RESERVED FOR THE GENUINELY PRE-TRAY STARTUP PATH -- do not call it from
/// anywhere the tray already exists.** Every caller left is above `main`'s
/// loop, where there is no running app to preserve and no affordance the user
/// could recover through, so refusing to start is both true and the only
/// option. Past the tray it is neither: the text says "Deskwarden cannot
/// start" about an app that has been running for hours, and the exit takes
/// the tray, the global hotkey, autofill and window-watching down with it
/// over conditions that are usually transient. Three consecutive reviews
/// (12, 17, 18) each removed one such call from `open_vault_window`'s lock
/// recovery; the answer there is [`stand_down_after_unlock`], which leaves
/// the app running and locked and names a recovery that works.
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
///
/// Takes the cache, not the bridge -- `handle_match` (and the `fill_from_vault`
/// it may call) reads the vault from `VaultCache`'s snapshot rather than
/// hitting `bw serve` directly, which is what lets autofill keep working with
/// the backend stopped (see `backend_policy`).
///
/// **Generic over the two fillers**, rather than nailed to
/// `Injector<RealUiAutomation, RealSendInput>`, purely so `mod tests` can drive
/// this whole function with a recording injector. That is what pins the
/// `handle_match` call below: with a concrete `Injector` nothing could call
/// this, and replacing that call with `None::<(String, isize)>` -- autofill
/// switched off for every trigger mode, the app's entire purpose -- compiled
/// and left all 1217 lib and 113 bin tests green. See
/// `tests::the_dispatch_that_actually_fills`.
fn process_foreground_event<A: UiAutomationFiller, B: SendInputFiller>(
    event: &window_watch::ForegroundEvent,
    cache: &VaultCache,
    injector: &Injector<A, B>,
    fill_stats: &fill_stats::FillStats,
    engine: &MatchEngine,
    pending_hotkey_fill: &mut Option<(String, isize)>,
    last_dispatched_hwnd: &mut Option<isize>,
    notifier: &dyn Notifier,
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

    if let Some((item_id, m)) = engine.lookup(event) {
        log::info!(
            "matched {} to vault item {item_id} (trigger {:?})",
            deskwarden::app::window_label(&event.exe_name, &event.title),
            m.trigger
        );
        if let Some(armed) =
            handle_match(cache, injector, fill_stats, item_id, m, event, notifier)
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
fn check_for_update_logged(
    current_version: &Version,
    agent: &deskwarden::http_agent::TotalBounded,
) -> Option<ReleaseInfo> {
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

/// Rebuilds both halves of the post-unlock state -- the vault cache and the
/// match engine -- once the readiness wait has confirmed `bw serve` is
/// answering with the *new* session.
///
/// Takes `cache`, not a separate `vault: &VaultBridge`: `cache.bridge()` is
/// that same bridge, so a second parameter would just be another name for it,
/// and since review 16 nothing in here needs the bridge directly anyway.
///
/// Both halves are rebuilt from `items` -- the vault the readiness probe
/// ITSELF listed, a fetch already known to have succeeded -- exactly as
/// startup does (`match_entries` + `engine.rebuild`, then `populate_with`).
/// Nothing here re-fetches the item list, and that is the whole point.
///
/// The history is worth keeping, because the same defect was fixed twice at
/// the wrong depth. Between 128000c and review 15's Important, the engine's
/// refresh was tied to `cache.populate()`'s outcome and the engine cleared
/// otherwise; `populate()` is two requests (`list_items` then `list_folders`)
/// and atomic over both, so a 500 on the folders half -- a failure
/// `picker_ui::load_items_for_picker`'s doc records as something that
/// actually happens -- cleared the engine even though the vault read fine.
/// e83ef03 made the two independent, which fixed the folders case and left
/// the engine depending on the since-deleted `app::refresh_match_engine`'s
/// own, THIRD `list_items`
/// (review 16's Important): a transient 500 or a connection reset on that one
/// request cleared the engine just the same. With no periodic match-engine
/// refresh left in this app, either version disarmed autofill silently for
/// the whole session -- nothing matches, nothing prompts, nothing arms the
/// hotkey, and the app looks perfectly alive.
///
/// Building from `items` removes the failure mode rather than moving it: the
/// engine's arming now depends on a fetch that has ALREADY SUCCEEDED, so
/// there is no request left whose failure could disarm it. It also drops two
/// full-vault round-trips (~1.1s / 1.08 MB each on a 1657-item vault, measured
/// in this repo) from a recovery that blocks the main thread.
///
/// There is consequently no `engine.clear()` here at all, and there must not
/// be one. The invariant that motivated the old coupling -- "an empty cache
/// beside a populated engine is inconsistent" -- is only true when the
/// ENGINE'S CONTENTS might belong to a different account. That is the
/// `Dismissed` arm's situation (no usable backend, nothing re-fetched, the
/// entries are whatever the pre-lock account left behind), and clearing there
/// is right and stays. Here the engine is rebuilt outright from the CURRENT
/// account's items, so pre-lock entries cannot survive even when the new
/// account has no app matches at all (`rebuild` replaces, it does not merge);
/// and an empty cache paired with those entries is a pairing this codebase
/// deliberately supports -- `app::fill_from_vault` falls back to the bridge on
/// a cache miss precisely so a fill still works in it.
fn repopulate_and_refresh_after_unlock(
    cache: &VaultCache,
    engine: &mut MatchEngine,
    items: Vec<deskwarden::vault_bridge::VaultItem>,
    // Captured by the caller BEFORE the readiness probe that produced
    // `items`, for the reason `VaultCache::epoch`'s doc gives: the guard can
    // only cover the window it is handed, and a `clear` landing between that
    // probe's fetch and this write is invisible to an epoch captured any
    // later. Same contract, and the same reason, as startup's
    // `startup_epoch`.
    epoch: VaultEpoch,
) {
    // Engine first, and unconditionally: it is pure, it cannot fail, and
    // doing it before the move into `populate_with` is what lets `items` be
    // handed to the cache rather than cloned.
    let entries = match_entries(&items);
    engine.rebuild(&entries);
    log::info!(
        "match engine rebuilt after unlock: {} app match(es)",
        entries.len()
    );

    // Seeds the cache with the same already-fetched items instead of listing
    // them again; still fetches folders, since nothing has. A failure here
    // leaves the engine armed on purpose (see this function's doc).
    match cache.populate_with(items, epoch) {
        Ok(PopulateOutcome::Populated) => {}
        Ok(PopulateOutcome::DiscardedStale) => log::warn!(
            "the vault cache was cleared again while repopulating after unlock; it stays empty"
        ),
        Err(e) => log::warn!(
            "could not repopulate the vault cache after unlock ({e:?}); autofill will fall back \
             to bw serve per fill until the next successful populate"
        ),
    }
}

/// Restarts `bw serve` for the lock recovery, standing autofill down instead
/// of exiting when it cannot be started. `None` means the recovery is over:
/// the caller has no child to track and nothing left to probe.
///
/// **Why it does not exit** (review 18's Important). This was the last
/// `fatal_startup_error` left in `open_vault_window`, and every sibling arm
/// around it had already been made survivable -- `Ready` survives an
/// all-500 backend, `Dismissed` survives by review 12's design, and review
/// 17 made the readiness TIMEOUT stand down for the reason that applies here
/// with a higher base rate still: there is an already-running app -- tray,
/// hotkey, autofill, window-watching -- to preserve, and killing it costs
/// the user far more than the transient it is reacting to.
///
/// Transient is the operative word. The dominant failure here is
/// [`BackendStartError::PortHeld`], and this call site killed *its own*
/// `bw serve` a few lines earlier, so a socket that has not been released
/// yet is the EXPECTED case rather than an exceptional one -- which is what
/// [`bw_serve::PORT_RELEASE_GRACE_RESTART`] exists for, and what
/// `try_start_backend`'s own doc already said in as many words ("returns the
/// failure instead of exiting, because on the restart paths ... killing the
/// whole app over a socket that needs another second to close is far
/// worse"). Only the caller disagreed, and it fired immediately after the
/// user had retyped their master password.
///
/// Standing down reuses [`stand_down_after_unlock`] rather than inventing a
/// second mechanism, so ONE place decides what "we could not get the vault
/// back" looks like and says so to the user. The state the caller is left in
/// is the one the `Dismissed` path already produces and has been shipping:
/// cache cleared, `bw_serve_child` `None` (the old child was stopped and the
/// new one never existed, so nothing is orphaned that this process was
/// tracking), a freshly re-authenticated `session_token`,
/// `cached_status_details` `None` so the next open re-fetches,
/// `backend_task_in_progress` `None`, the engine cleared, and the tray's
/// "Sync" item idle and clickable -- the recovery `stand_down_after_unlock`'s
/// message names. The one difference from the readiness-timeout stand-down is
/// that no backend is left running to come up on its own; a tray Sync starts
/// one itself (`spawn_sync` takes `currently_running: false`), so the named
/// recovery still works.
fn restart_backend_after_unlock(
    engine: &mut MatchEngine,
    start: impl FnOnce() -> Result<Child, BackendStartError>,
) -> Option<Child> {
    match start() {
        Ok(child) => Some(child),
        Err(e) => {
            stand_down_after_unlock(
                engine,
                &format!("the Bitwarden backend could not be restarted after unlocking ({e})"),
            );
            None
        }
    }
}

/// Decides what the lock recovery does with the readiness probe's outcome:
/// repopulate, retry once, or stand autofill down.
///
/// Split out of `open_vault_window` -- which takes seventeen parameters and
/// blocks the main thread on real windows -- so the composition itself (first
/// probe -> optional retry -> repopulate or stand down) is what the tests
/// drive, rather than a reimplementation of it beside the live one. `probe`
/// is the readiness wait, taking the message its spinner should show; the
/// only caller passes `wait_for_vault_ready_with_spinner`.
///
/// **Review 17's Critical.** Before this, a `Dismissed` here went straight to
/// `engine.clear()`, and the warn it logged advised the user to "open it
/// again from the tray to retry" -- advice that is false. The engine is only
/// ever rebuilt at four places (startup, this function's `Ready` path, a
/// completed tray `Sync`, and the "Add app..." save's rebuild from the cache),
/// and `open_vault_window` reaches the recovery ONLY when the window
/// reports `locked || needs_reauth`. A normal open/close never touches the
/// engine at all, so reopening the vault window repopulates the CACHE and
/// leaves the ENGINE empty: the user sees all their items and autofill is
/// still dead. The scenario is one impatient click -- the vault auto-locks,
/// the master password is accepted, `bw serve` restarts fine, the spinner
/// appears and the user closes it -- and review 12 already ruled that
/// gesture must not be destructive.
///
/// So a dismissal now buys ONE free readiness probe before anything
/// destructive happens, exactly as startup's own dismissal does
/// (`SETUP_RETRY_MESSAGE`). That matters beyond politeness:
/// `wait_for_vault_ready_with_spinner`'s worker is DETACHED and still
/// running at that moment, so the vault is very likely ready a second later
/// and the retry simply takes the ordinary `Ready` path -- engine armed from
/// the probe's own items, cache seeded from the same ones. It is bounded the
/// same way startup's is, and structurally rather than by a counter: two
/// `probe` calls appear in this function and there is no loop.
///
/// A `Failed` -- the ~30s readiness deadline expiring -- does not retry. It
/// has already spent that deadline, and startup's `Failed` arm does not
/// retry either. It does now STAND DOWN rather than exit: see
/// `stand_down_after_unlock`.
fn settle_vault_after_unlock(
    cache: &VaultCache,
    engine: &mut MatchEngine,
    epoch: VaultEpoch,
    mut probe: impl FnMut(&'static str) -> VaultReadyOutcome,
) {
    match probe(SETUP_MESSAGE) {
        VaultReadyOutcome::Ready(items) => {
            repopulate_and_refresh_after_unlock(cache, engine, items, epoch)
        }
        VaultReadyOutcome::Dismissed => {
            // Closing this window is not, on its own, evidence that anything
            // is broken -- the same reasoning startup's dismissal retry is
            // built on. Nothing is killed, nothing is re-authenticated: the
            // still-running backend gets one more honest look.
            log::info!(
                "setup window closed before the vault backend was confirmed ready after \
                 unlocking; probing readiness once more before standing autofill down"
            );
            match probe(SETUP_RETRY_MESSAGE) {
                VaultReadyOutcome::Ready(items) => {
                    repopulate_and_refresh_after_unlock(cache, engine, items, epoch)
                }
                VaultReadyOutcome::Dismissed => stand_down_after_unlock(
                    engine,
                    "the setup window was closed a second time without the vault backend \
                     becoming ready after unlocking",
                ),
                VaultReadyOutcome::Failed(e) => stand_down_after_unlock(
                    engine,
                    &format!("the vault backend did not become ready after unlocking ({e})"),
                ),
            }
        }
        VaultReadyOutcome::Failed(e) => stand_down_after_unlock(
            engine,
            &format!("the vault backend did not become ready after unlocking ({e})"),
        ),
    }
}

/// Leaves the app running with the vault effectively still locked: cache
/// empty (the recovery's own `cache.clear()` emptied it), engine empty, tray
/// and hotkey and window-watching all still alive.
///
/// **Why the engine is cleared** (review 13's Minor 3, unchanged): nothing
/// on this path confirmed that `bw serve` is answering under the new
/// session, so nothing re-fetched, so the engine can only be holding the
/// PRE-lock account's matches. Left armed beside an empty cache, a matched
/// process still raises the autofill prompt and the fill then misses in the
/// cache and falls through to a `get_item` with an id from an account this
/// session is no longer signed into -- a prompt that can only ever end in an
/// error log. This is deliberately NOT what the `Ready` path does, and the
/// difference is the backend: there the probe itself listed the vault, so
/// the engine is rebuilt from THOSE items and an empty cache beside them is
/// a supported pairing (see `repopulate_and_refresh_after_unlock`).
///
/// **Why the message names Sync** (review 17's Critical): the warn this
/// replaces told the user to "open it again from the tray to retry", and
/// reopening the vault window provably does not rebuild the engine -- see
/// `settle_vault_after_unlock`'s doc. The recoveries that actually do are
/// the tray's "Sync", an "Add app..." save, and another lock/unlock cycle
/// whose readiness probe is allowed to finish. A message that names a
/// recovery which does not work is worse than no message: it costs the user
/// the one chance they had of finding a working one.
///
/// **Why it does not exit** (review 17's Minor): this used to be two
/// different answers to two transient conditions -- a dismissal survived,
/// while a readiness TIMEOUT called `fatal_startup_error` and took the whole
/// process down with it. Review 12's justification for making the dismissal
/// survivable ("there is an already-running app to preserve") applies
/// identically to a timeout, the error text came from a function named for
/// STARTUP at a call site that is not startup, and a probe that timed out is
/// weaker evidence of unrecoverable breakage than the all-requests-500 state
/// the `Ready` path now deliberately survives. `fatal_startup_error` is
/// reserved for the genuinely pre-tray path.
///
/// The freshly (re)started `bw serve` from just above is left running rather
/// than killed: it may still come up on its own, in which case a tray Sync
/// works immediately. `main`'s idle reconciliation tears it back down if
/// `keep_backend_running` says to, exactly as it does after startup. The
/// third caller ([`restart_backend_after_unlock`], review 18) is the one case
/// where no backend is running at all, because starting it is what failed --
/// `bw_serve_child` is `None` there and a tray Sync starts one itself, so the
/// recovery this message names still works.
fn stand_down_after_unlock(engine: &mut MatchEngine, reason: &str) {
    engine.clear();
    log::warn!(
        "{reason}; leaving Deskwarden running with the vault effectively still locked. The app \
         matches are cleared too, so nothing can prompt to autofill until they are rebuilt: use \
         \"Sync\" in the tray menu to rebuild them. Reopening the vault window refills the item \
         cache but does NOT rebuild the app matches."
    );
}

/// Rebuilds the tray's "Accounts" submenu after the vault window has closed.
///
/// **`open_vault_window` can switch accounts and cannot rebuild the menu
/// itself.** Its titlebar switcher runs the same `switch_to_account` the tray
/// does and calls `state.adopt(...)` on the way out, but it takes `&tray` --
/// `resettle_session` needs the tray to set the "Sync" item's state -- so it
/// has no `&mut` to rebuild with. Both of its call sites therefore rebuild on
/// the way out, through this one name.
///
/// Without it the submenu still holds the rows built while A was active. Two
/// things are then wrong at once: it offers a switch to the account the app is
/// now ON, and its "Remove a@example.com..." row does not remove A. That row
/// carries no id -- the removal reads `state.active().id`, which is B by now --
/// so the label names one vault and the deletion takes another. The
/// confirmation dialog does name B, so it takes one unread dialog rather than
/// none, which is why this is not the only guard on that path; it is the one
/// that keeps the label honest.
///
/// Unconditional rather than gated on "did the active account change?", for
/// the reason the submenu action's own rebuild is: every `MenuId` is minted
/// with its item, so a menu rebuilt only on the paths that changed something
/// is a menu whose ids sometimes outlive the state they name -- and
/// "sometimes" is the shape nothing catches.
fn rebuild_after_vault_window(tray: &mut tray::AppTray, state: Option<&accounts::AccountsState>) {
    tray.rebuild_accounts_menu(state);
}

/// Which of the tray requests that piled up behind a blocking window still
/// need handling.
///
/// **The bug.** Every window this app opens from the tray owns the main thread
/// for as long as it is up -- `open_vault_window`, `prefs_ui::run`,
/// `picker_ui::run_picker` all block `main`'s loop. The tray's event channels
/// do not stop during that: `muda` and `tray-icon` deliver into unbounded
/// `crossbeam` channels from the window-message pump, and the pump keeps
/// running because eframe/winit is pumping the very same thread. So a user who
/// clicks the tray icon three times while waiting for the vault to load queues
/// three requests, and the loop faithfully replays all three the moment the
/// window closes -- opening it again, and again. That is the report: "if user
/// clicks 3 times on tray icon (while waiting for a window) - if you close it,
/// it will launch 2 more times once closed".
///
/// **The rule is not "discard whatever is queued".** A click asking for the
/// window that was ALREADY on screen has been served -- it was shown, and
/// `foreground::raise_window` brought it to the front on its first frame, so
/// "show me the window" is precisely what happened. A click asking for
/// anything else -- Quit, Preferences, Sync, an account switch -- was served by
/// nothing and is still owed an answer; dropping those would trade one report
/// for a worse one (a Quit clicked at the tray while the vault window was up
/// would be silently ignored). So `satisfied` names the request the window
/// answers, and only that request is dropped.
///
/// **"While it was running" is a boundary, not a guess.** The caller drains the
/// channel ONCE, immediately after the blocking call returns, so what it hands
/// here is exactly what was already queued at that instant -- which is to say,
/// what arrived while the window was up, since the loop had drained everything
/// before it opened. A click a moment after the close lands after that drain
/// and is handled normally on the next iteration; it is never seen by this
/// function at all. Nothing here compares timestamps, and none of the two
/// event types carries one.
///
/// Order is preserved: the survivors are replayed as if the window had never
/// been in the way.
fn requests_outliving_a_window(queued: Vec<MenuEvent>, satisfied: &MenuId) -> Vec<MenuEvent> {
    queued
        .into_iter()
        .filter(|event| {
            if &event.id == satisfied {
                log::info!(
                    "ignoring a tray request for `{}` queued while that window was already open",
                    event.id.0
                );
                false
            } else {
                true
            }
        })
        .collect()
}

/// [`requests_outliving_a_window`] against the live tray channel: drains
/// whatever is queued right now, drops the requests the window that just
/// closed already satisfied, and appends the rest to `pending` for `main`'s
/// loop to handle on its next iterations.
///
/// `pending` is appended to rather than replaced because a previous window can
/// have left survivors of its own that have not been handled yet -- the tray's
/// Preferences item opens a window from inside the same block that would
/// otherwise be popping them.
/// [`drain_requests_queued_behind_a_window`] for the vault window
/// specifically, which is the one window this app has TWO doors into: the
/// tray menu's "Open Vault" item and a left click on the tray icon itself.
/// Both doors queue, so both have to be swept, and one name at both call sites
/// is what stops a future third door from sweeping only half of it.
fn drop_vault_requests_queued_behind_the_window(
    pending: &mut VecDeque<MenuEvent>,
    open_vault_id: &MenuId,
) {
    drain_requests_queued_behind_a_window(pending, open_vault_id);
    let clicks = tray::discard_queued_icon_events();
    if clicks > 0 {
        log::info!(
            "ignoring {clicks} tray-icon click(s) queued while the vault window was already open"
        );
    }
}

fn drain_requests_queued_behind_a_window(pending: &mut VecDeque<MenuEvent>, satisfied: &MenuId) {
    let mut queued = Vec::new();
    while let Some(event) = tray::next_menu_event() {
        queued.push(event);
    }
    pending.extend(requests_outliving_a_window(queued, satisfied));
}

/// Opens the vault window and handles it locking itself before returning.
/// Shared by both ways of asking for it -- the tray menu's "Open Vault" item
/// and a left click on the tray icon -- so the recovery sequence (mirroring
/// the startup retry path: `stop_bw_serve` on the old child ->
/// `reauthenticate` -> `try_start_backend` -> `wait_for_vault_ready_with_spinner`
/// -> rebuild the match engine) exists in exactly one place.
///
/// Does **not** decide whether `bw serve` should keep running once the
/// window closes. That decision used to live here, as an `else if
/// !backend_policy::should_run(..)` right after this function's old body --
/// which is exactly what review Critical 2 flagged: the *only* place the
/// policy was ever reconciled was a side effect of calling this function, so
/// a session that never opens the vault window (the normal autofill-only
/// case) held `bw serve` up forever under `keep_backend_running = false`.
/// The policy is now reconciled every idle iteration of `main`'s own loop
/// instead (see `stop_backend_if_idle`), which runs whether or not this
/// function was ever called. That also fixes review Important 4 as a direct
/// consequence: the old `locked` branch below never rechecked the policy (it
/// was the `if` half of an `if`/`else`, and the policy check was only in the
/// `else`), so locking the vault window in save-memory mode used to leave
/// the backend up indefinitely. Now both branches just return, and the
/// caller's next loop iteration reconciles either way.
///
/// Starting the backend for the window is also no longer awaited inline
/// (review Important 5): that used to be a scoped background thread joined
/// right after `vault_window::run` returned, which blocked the tray, the
/// global hotkey, and window-watching for up to ~30s (a port-release wait
/// plus a synchronous `bw sync`) on a window that may have been open for all
/// of two seconds -- and then immediately killed the child it just waited
/// for, if the policy said to. It's now a detached background operation
/// reported back through `backend_op_tx`/`backend_op_rx` and applied by
/// `main`'s own loop, same non-blocking shape as the update-download flow.
#[allow(clippy::too_many_arguments)]
fn open_vault_window(
    cache: &Arc<VaultCache>,
    fill_stats: &deskwarden::fill_stats::FillStats,
    session_token: &mut String,
    bw_serve_child: &mut Option<Child>,
    job: &Arc<Option<job_object::KillOnCloseJob>>,
    // `&mut` since Task 14: an account switch re-points this at the target
    // account's `session.bin` (inside `switch_to_account`) before the login
    // window it may raise produces a token to save.
    store: &mut session_store::SessionStore,
    schedule: &[Duration],
    engine: &mut MatchEngine,
    icon_cache_dir: &std::path::Path,
    // Warmed by a background thread at startup (see `main`'s
    // `status_details_rx`) and reused across opens, so the common case pays
    // no `bw status` spawn at all here. `None` only on a genuine cache miss
    // (a click landing before the prefetch reports back, or right after the
    // invalidation below) -- that path still falls back to the same
    // synchronous call this function always made, just no longer on every
    // single open.
    cached_status_details: &mut Option<login_ui::BwStatusDetails>,
    // The live preferences, and where they are stored. Taken by `&mut` and
    // as a path -- rather than the single pre-computed `auto_lock: Duration`
    // this used to take -- because the titlebar's gear now opens the
    // preferences window from *inside* a vault session (see
    // `VaultWindowResult::open_preferences`). Serving that needs both: the
    // struct, to hand to `prefs_ui::run` and to write back into so `main`'s
    // own later reads (`settings.keep_backend_running` in its idle
    // reconciliation) see the change, and the path to persist to.
    //
    // The auto-lock timeout is now derived per iteration of the loop below
    // instead of being computed once by the caller, which is what lets a
    // timeout edited in that window apply to the very next vault window
    // rather than only to the next app launch.
    // What the lock/re-auth prompt this window can raise is scoped to: the
    // account this process is signed into. The pieces rather than a built
    // `LoginContext`, because this function can now CHANGE which account that
    // is (the titlebar switcher below) and a context built by the caller would
    // still name the account the user just left -- so the master-password
    // prompt after a switch-then-lock would be for the wrong account. Every
    // context here goes through the one `login_context` constructor, which is
    // what keeps "no window is opened without an account" a single decision.
    config_dir: &std::path::Path,
    active_account: &mut Option<Account>,
    // The one door for "may I switch, and to what" (Task 10), and `&mut`
    // because a switch that lands moves its `active`.
    accounts: &mut Option<accounts::AccountsState>,
    // Passed through to every `login_context` this function builds; see that
    // function.
    first_run_account: Option<&accounts::AccountId>,
    settings: &mut settings::Settings,
    settings_path: &std::path::Path,
    tray: &tray::AppTray,
    backend_op_tx: &mpsc::Sender<BackendOp>,
    backend_op_rx: &mpsc::Receiver<BackendOp>,
    backend_task_in_progress: &mut Option<(Instant, BackendOpKind)>,
    // The outcome of a vault session THIS FUNCTION DID NOT OPEN, dispatched by
    // the loop below on its first pass instead of that pass opening a window.
    //
    // `Some` from exactly one caller: `main`, once, after the single startup
    // window has drawn the sign-in card, the spinner and then the vault in one
    // event loop and the user has closed it. That window can lock, ask for
    // Preferences or switch account exactly as any other vault session can, and
    // every one of those outcomes has a handler here already. Handing the
    // result in rather than growing a second dispatch next to the startup code
    // is what keeps `resettle_session` reached from one place and keeps the
    // switch/add/remove wiring the tray's guards pin the only wiring there is.
    mut first_result: Option<vault_window::VaultWindowResult>,
) {
    // Reopened, not merely opened once: the titlebar gear asks for the
    // preferences window, and `prefs_ui::run` is its own `eframe` window on
    // this same thread. eframe cannot nest one native event loop inside
    // another, so the vault window must be fully gone before that call and
    // has to come back afterwards -- which is a loop, not a straight line.
    // Every other outcome (closed, locked, needs re-auth) still leaves this
    // function exactly once, on its first pass.
    loop {
    // Opening this window is the app's slowest visible action and the one a
    // user times with their own patience, so each stage of it says how long
    // it took. Without this the only honest answer to "why did that take ten
    // seconds" is a guess: the stages have very different costs (a `bw`
    // spawn is seconds, a backend start is seconds, and eframe's own window
    // and GPU setup is not free either), and which one dominates depends on
    // what the backend was doing beforehand.
    // A vault session that ran SOMEWHERE ELSE: the single startup window drew
    // the vault in its own event loop, and its outcome has to be acted on by
    // THE CODE BELOW rather than by a second copy of it written next to the
    // call. Lock, re-auth, Preferences and an account switch each mean the
    // same thing however the window that reported them was opened, and the
    // lock/re-auth arm in particular runs `resettle_session` -- the one
    // hardened teardown-and-repopulate sequence in this app, which exists
    // exactly once and must keep existing exactly once.
    //
    // `take`, so this is the FIRST PASS ONLY. Everything that reopens the
    // window (Preferences, a switch) goes round the loop again and opens a
    // real one -- which is what "the window closes and reopens" now means when
    // the window that closed was the startup window: it closed, and the vault
    // comes back in a window of its own.
    let result = match first_result.take() {
        Some(result) => {
            log::info!(
                "dispatching the outcome of the vault session the startup window already ran"
            );
            result
        }
        None => {
        let opened_at = Instant::now();
        // **Never waited for.** This used to be a `bw status` spawn on a miss
        // -- 2.39s measured on the user's machine -- run BEFORE eframe was
        // asked for a window, so the click produced nothing on screen at all
        // for that long. What it fetches is the toolbar avatar's initials and
        // the host favicons come from; the window opens without them and they
        // fill in, exactly the way the item list already does.
        //
        // The cache is refilled from the RESULT now (just below the window),
        // not here: on a miss there is nothing to refill it with yet, and the
        // value the window ended up with is the one the next open should
        // reuse. See `account_details_source`.
        let details = account_details_source(cached_status_details.take(), |tx| {
            std::thread::spawn(move || {
                let _ = tx.send(login_ui::check_bw_status_details());
            });
        });

        // Read once, before the `if` below might short-circuit past it, and
        // reused for `vault_window::run`'s own `backend_already_running`
        // (review Minor 3): whether `bw serve` was already up at this exact
        // moment -- before this function might kick off a start of its own --
        // is also exactly the fact `spawn_vault_load` needs to know it can skip
        // its readiness wait. Nothing between here and `vault_window::run`
        // returning stops or restarts the backend out from under this snapshot
        // (the only paths that do -- lock/reauth recovery -- close the window
        // and return first), so it stays valid for the window's whole session.
        let backend_already_running = backend_is_running(bw_serve_child);

        // Reads don't need `bw serve` at all (`vault_window::run` paints
        // entirely from `cache`); writes and TOTP do. If save-memory mode tore
        // the backend down after the last close (or it crashed -- review Minor
        // 8: `backend_is_running` catches a `Some(dead child)` that a plain
        // `.is_none()` check would miss), kick a start off in the background and
        // move straight on to opening the window rather than waiting for it --
        // see this function's doc for why waiting here used to be a real freeze.
        if needs_backend_start(backend_task_in_progress, backend_already_running) {
            *backend_task_in_progress = Some((Instant::now(), BackendOpKind::EnsureRunning));
            spawn_backend_start(session_token.clone(), job.clone(), backend_op_tx.clone());
        }

        // The last thing before the window exists. Everything after this is
        // eframe's own startup -- creating the OS window, picking a graphics
        // backend, building the font atlas -- which this app does not control
        // and which is invisible to any timing inside the frame closure, since
        // the first closure call happens after all of it.
        //
        // The account details are named here rather than timed: on a hit the
        // toolbar has them on its first frame, and on a miss the fetch is
        // running beside this window instead of in front of it, so the only
        // honest number for it is the one the window itself logs when it
        // arrives ("account details arrived ... after the window was handed
        // to eframe").
        log::info!(
            "vault window: handing off to eframe after {:?} (backend was {}, account details \
             were {})",
            opened_at.elapsed(),
            if backend_already_running { "already up" } else { "being started" },
            match details {
                vault_window::AccountDetails::Ready(_) => "prefetched",
                vault_window::AccountDetails::Pending(_) =>
                    "not prefetched; fetching alongside the window",
            }
        );
        vault_window::run(
            cache.clone(),
            fill_stats.clone(),
            details,
            session_token.clone(),
            icon_cache_dir.to_path_buf(),
            // Read fresh on every pass, so a timeout changed in the preferences
            // window below governs the window this loop is about to reopen.
            settings.auto_lock(),
            backend_already_running,
            // Cloned per pass, not once outside the loop: a switch below replaces
            // the state, and the window reopened after it has to offer the account
            // the user just left rather than the one they are now on.
            accounts.clone(),
        )
        }
    };

    // **Refill the cache with what this window actually used, so the *next*
    // open pays no `bw status` spawn.** This is the same guarantee the
    // pre-fetch-or-block version made one line above the window; it moved
    // here because nothing blocks any more, so on a miss the details do not
    // exist until the window has drained them (see
    // `VaultWindowResult::account_details`). `None` -- a window closed before
    // its fetch reported back -- leaves the cache empty and the next open
    // starts another one, which is exactly what a miss has always cost.
    //
    // BEFORE the branches below, and that ordering is the whole safety
    // argument: an account switch and the lock/re-auth recovery both run
    // `resettle_session`, which sets this to `None` deliberately, and both are
    // reached from below this line -- so a switch still leaves the next window
    // to re-fetch rather than showing the account the user just left. Above
    // them, this would put a stale email in the toolbar of a window signed
    // into somebody else.
    if let Some(details) = result.account_details.clone() {
        // **And the account learns its own address from the same answer**, here
        // and for the same ordering reason. These details were fetched for the
        // profile this window was pointed at, which is the active account
        // *until one of the branches below changes it* -- so this is the last
        // moment at which "the active account" and "who this describes" are
        // provably the same account. Below the switch it would write one
        // account's address into another's entry.
        //
        // This is what fills the email in for an account `resolve_startup`
        // minted on a first install, which is why the account menu's identity
        // block and the switcher's rows stop naming a 32-character directory.
        learn_active_account_details(settings_path, accounts, active_account, &details);
        *cached_status_details = Some(details);
    }

    // Handled before the lock/re-auth branch and with its own `continue`,
    // never folded into it. `locked` and `needs_reauth` both mean the
    // session is gone and both run the full recovery below -- clear the
    // cache, stop `bw serve`, re-authenticate, restart, repopulate. Asking
    // for Preferences means none of that: the vault was never locked and the
    // backend is healthy, so reusing either flag would make a visit to the
    // gear demand the master password and needlessly restart `bw serve`.
    //
    // On `keep_backend_running` (the setting most likely to be changed
    // here): this deliberately does NOT call `stop_backend_if_idle` itself.
    // That function's whole contract is "stop the backend if the policy says
    // so AND no vault window is open", and this path is about to reopen the
    // vault window immediately -- which needs the backend for writes and
    // TOTP. `main`'s own loop reconciles the policy on its next idle
    // iteration, which is reached as soon as this function finally returns,
    // exactly as it is for the tray's preferences item. The change therefore
    // takes effect when the vault session actually ends, which is the
    // earliest moment at which it is meaningful. What DOES have to happen
    // here is writing the new value into `*settings`, because that binding
    // -- not the file -- is what that reconciliation reads.
    //
    // **No `continue`, and no window opened here any more.** The gear used to
    // close this window so `prefs_ui::run` could have the thread's event loop;
    // the form is now drawn as a modal over the vault window itself, so what
    // comes back is the answer rather than the request. It therefore arrives
    // alongside whatever actually ended the session -- an ordinary close, a
    // lock, a switch -- and is applied first, before any of those branches,
    // rather than instead of them.
    if let Some(edited) = result.edited_settings.clone() {
        if edited != *settings {
            *settings = edited;
            // `persist_preferences`, never a whole-struct `save`. This
            // struct's `vault_window` field is whatever was on disk when
            // `main` loaded it at startup, and `vault_window::run` has just
            // written a fresh geometry straight to the same file on its way
            // out of the call above. A whole-struct write here would put
            // that stale geometry back and silently revert the size and
            // position the user just left the window at -- the identical
            // trap the tray's preferences handler documents at its own call
            // site. `persist_preferences` re-reads the file and overwrites
            // only the two preference fields, so the geometry survives.
            if let Err(e) = settings.persist_preferences(settings_path) {
                log::warn!("could not save settings: {e}");
            }
        }
        continue;
    }

    // **The three things the titlebar's account menu can ask for, before the
    // lock/re-auth branch and with one `continue` between them.** None of them
    // is a lost session: the recovery below re-authenticates against the
    // account this process is ALREADY on, so any of them folded into it would
    // prompt for the master password of the account the user asked to leave and
    // then leave them on it. See `VaultWindowResult::switch_to`.
    //
    // **ONE resettle closure for all three**, exactly as the tray's Accounts
    // submenu has one for its three -- three copies would be three chances to
    // get the hardest sequence in this codebase subtly different, and
    // `the_vault_windows_account_actions_settle_through_the_one_sequence` pins
    // that what is in here is that sequence and not a fourth teardown path
    // written inline. Scoped to this block so the lock branch below, which
    // needs the same `&mut`s directly, still borrows them.
    if result.switch_to.is_some() || result.add_account || result.remove_account {
        // **The injected resettle, and it calls the one
        // teardown-and-repopulate sequence.** Nothing else may
        // live in here: `a_switch_reimplements_none_of_the_
        // sequence_it_is_supposed_to_reuse` pins that
        // `switch_to_account` did not do the work itself, and
        // `the_production_switch_resettles_through_the_one_
        // sequence` pins that this closure -- the only thing that
        // gets to decide what "resettle" means in production --
        // did not either.
        let mut resettle = |config_dir: &std::path::Path,
                            to: &Account,
                            store: &session_store::SessionStore|
         -> ResettleReport {
            // Built for the account being settled ONTO. A context
            // for the account being left would put the master-password prompt
            // up for the wrong account and seal the Hello blob
            // under the wrong id.
            let login = login_context(config_dir, Some(to), first_run_account);
            let mut declined = false;
            let outcome = resettle_session(
                cache,
                engine,
                bw_serve_child,
                job,
                schedule,
                tray,
                backend_op_rx,
                backend_task_in_progress,
                cached_status_details,
                session_token,
                || {
                    let token = authenticate_for_switch(store, login);
                    declined = token.is_none();
                    token
                },
            );
            // The three-way answer `ResettleOutcome`'s two
            // variants cannot give: only whoever built the
            // `authenticate` closure can tell "the user closed the
            // prompt" from "nothing came up to serve it", and a
            // switch that reported a backend failure because the
            // user pressed Cancel would be naming something that
            // never happened.
            match outcome {
                ResettleOutcome::BackendStarted => ResettleReport::Settled,
                ResettleOutcome::BackendNotStarted if declined => ResettleReport::Declined,
                ResettleOutcome::BackendNotStarted => ResettleReport::NotStarted,
            }
        };

    // Everything the switch itself does is `switch_to_account`'s -- the data
    // directory, the token store, the teardown, the authentication, the
    // restart, the rollback. This block is the caller's three jobs and no
    // fourth: name the target through the one gate, report the outcome, and
    // persist.
    if let Some(target) = result.switch_to.clone() {
        // **`switchable()`, not `all()`.** `all()` is every configured
        // account -- the active one included, and duplicate ids included --
        // and it is not emptied when switching is refused. Re-checking here
        // rather than trusting the id the window sent back costs nothing and
        // means the CLI-availability refusal is enforced on this side of the
        // window too.
        let picked = accounts
            .as_ref()
            .and_then(|state| state.switchable().iter().find(|a| a.id == target))
            .cloned();
        match (picked, accounts.as_mut(), active_account.as_mut()) {
            (Some(to), Some(state), Some(active)) => {
                let from = active.clone();
                let outcome =
                    switch_to_account(config_dir, &from, &to, active, store, &mut resettle);
                match outcome {
                    SwitchOutcome::Switched => {
                        // `adopt`, which moves this state's `active` and
                        // recomputes what it offers -- so the window reopened
                        // below offers the account just left.
                        state.adopt(to.clone());
                        // **After the switch has landed, never before.** A
                        // list written first and a switch that then failed
                        // would leave `settings.json` naming an account this
                        // process is not on, and the next launch would resume
                        // the wrong one. Written here, a switch that does not
                        // stick across a restart is impossible -- which
                        // matters because "switching that appears to work and
                        // then doesn't" is indistinguishable from the
                        // `relativeDataDir` trap and sends whoever debugs it
                        // down the wrong path entirely.
                        if let Err(e) = settings::Settings::persist_accounts(
                            settings_path,
                            state.all(),
                            Some(&to.id),
                        ) {
                            log::warn!("could not persist the active account after a switch: {e}");
                        }
                    }
                    // Not an error and not reported as one: the user closed
                    // the target account's master-password prompt and the
                    // account they were on is back.
                    SwitchOutcome::Declined => log::info!(
                        "the switch to {} was declined; staying on {}",
                        account_label(&to),
                        account_label(&from)
                    ),
                    SwitchOutcome::RolledBack { reason } => {
                        log::warn!("could not switch to {}: {reason}", account_label(&to));
                        message_box(
                            "Deskwarden",
                            &format!(
                                "Could not switch to {}.\n\n{reason}\n\nYou are still signed \
                                 in to {}.",
                                account_label(&to),
                                account_label(&from)
                            ),
                            MB_ICONERROR | MB_OK,
                        );
                    }
                    // `stand_down_after_unlock`'s state, which this app
                    // already ships and already tells the user to recover from
                    // with the tray's "Sync". Logged rather than raised: the
                    // stand-down has already said its piece.
                    SwitchOutcome::StoodDown { reason } => {
                        log::error!("the switch to {} stood autofill down: {reason}", account_label(&to))
                    }
                }
            }
            // The window offered an account this side will not switch to. Not
            // reachable through the switcher, which is built from the same
            // `switchable()`; reachable if the two ever disagree, and silence
            // there would be a click that does nothing forever.
            _ => log::warn!(
                "the vault window asked to switch to an account that is not one this app may \
                 switch to right now"
            ),
        }
    } else if result.add_account {
        // **Everything the add does is `add_account`'s** -- minting the
        // directory, running the sign-in inside it, asking `bw status` who
        // signed in, the two persists, and the rollback that leaves nothing
        // behind if any of it fails. This block is the caller's two jobs: hand
        // it the two windows it cannot open for itself, and report the outcome
        // through the same `SwitchOutcome` match the tray uses, so neither menu
        // can quietly stop raising a failure the other one raises.
        match (accounts.as_mut(), active_account.as_mut()) {
            (Some(state), Some(active)) => {
                let outcome = add_account(
                    config_dir,
                    settings_path,
                    state,
                    active,
                    store,
                    // The sign-in window, run against whatever profile
                    // `add_account` has pointed the CLI at -- which is the NEW
                    // account's directory, and is the whole reason this is
                    // injected rather than called in there.
                    |prepared| {
                        let login = login_context(config_dir, Some(prepared), first_run_account);
                        login_ui::run_login_flow_for(login.account, login.first_run)
                    },
                    // Asked of a NAMED directory. The active-profile form would
                    // report the account being left.
                    login_ui::check_bw_status_details_in,
                    &mut resettle,
                );
                report_account_action("add an account", outcome);
            }
            _ => log::warn!("cannot add an account: this app has none to add one to"),
        }
    } else if result.remove_account {
        // The active account, which is the one the menu's identity block names
        // and the one its row says "this account" about. Re-asked of
        // `can_remove_active` rather than trusted: the row is gated on it too,
        // and a state that changed between the two would otherwise reach a
        // removal that can only fail.
        match (accounts.as_mut(), active_account.as_mut()) {
            (Some(state), Some(active)) if state.can_remove_active() => {
                let doomed = state.active().id.clone();
                let label = account_label(state.active()).to_string();
                // **The existing confirmation, not a second one.** It defaults
                // to No and `the_removal_confirmation_defaults_to_no` pins that;
                // a copy written beside this call site would not have it.
                if confirm_account_removal(&label) {
                    if let Err(reason) = remove_account(
                        config_dir,
                        settings_path,
                        state,
                        &doomed,
                        active,
                        store,
                        &mut resettle,
                        // **The directory form, never the active-profile one.**
                        // That one acts on whatever this process is pointed at,
                        // and by the time the logout runs the app has already
                        // settled onto the SURVIVOR -- so it would sign out the
                        // account the user is keeping and leave the doomed one
                        // signed in on the server.
                        login_ui::bw_logout_in,
                    ) {
                        log::warn!("could not remove {label}: {reason}");
                        message_box("Deskwarden", &reason, MB_ICONERROR | MB_OK);
                    }
                }
            }
            _ => log::warn!(
                "the vault window asked to remove an account this app cannot remove right now"
            ),
        }
    }
    continue;
    }

    if result.locked || result.needs_reauth {
        // Two different triggers land here, both needing the exact same
        // recovery: the vault window locked itself (manual Lock button or
        // its own auto-lock timer), or a write inside it hit `bw serve`
        // returning 401 -- the session was invalidated out from under a
        // still-running backend (`bw lock` elsewhere, a server-side vault
        // timeout, a password change on another device). `backend_is_running`
        // only checks whether the *process* is alive, so that case would
        // otherwise go unnoticed forever: `bw serve` keeps answering, just
        // with 401s, and nothing before this fix ever re-authenticated (see
        // review Important 2). Both invalidate `bw serve`'s session exactly
        // the same way a rejected cached session does at startup, so both
        // get the same fix.
        if result.needs_reauth {
            log::warn!(
                "vault window write failed with an unauthorized session; re-authenticating"
            );
        } else {
            log::info!("vault window locked itself; re-authenticating");
        }

        // The recovery itself is `resettle_session`, which a lock/re-auth and
        // an account switch share whole rather than each spelling out (see
        // its doc). The only thing this caller contributes is where the new
        // session token comes from: the master-password prompt for the
        // account this app is already signed into.
        //
        // `reauthenticate` never returns `None` -- it exits the process
        // rather than hand back a failure -- so the `BackendNotStarted`
        // outcome is reached from here only by a backend that would not
        // start, which is why the outcome is not branched on. Both outcomes
        // leave this function the same way, through the `return` below; the
        // `BackendNotStarted` arm used to be an early `return` from inside
        // this block, which was only ever a jump to that same statement.
        resettle_session(
            cache,
            engine,
            bw_serve_child,
            job,
            schedule,
            tray,
            backend_op_rx,
            backend_task_in_progress,
            cached_status_details,
            session_token,
            // Built HERE rather than handed in, so it names whichever account
            // this process is on at this moment -- which the switch above may
            // have changed since the caller's own context was built.
            || {
                let login =
                    login_context(config_dir, active_account.as_ref(), first_run_account);
                Some(reauthenticate(store, login))
            },
        );
    }

    // The only way out that is not the `continue` above. Every non-
    // preferences outcome -- a plain close, a lock, a re-auth, and a
    // re-auth whose backend could not be restarted (which used to `return`
    // from inside the block above and now simply falls through to here) --
    // leaves the loop here, so this function still runs the window exactly
    // once for all of them.
    return;
    }
}

/// What a resettle left the app in, for a caller that has to decide whether
/// the thing it was resettling *for* actually happened.
///
/// Task 9's account switch is that caller: `BackendNotStarted` is what it
/// rolls back from. The lock/re-auth path has nothing to roll back to and
/// ignores this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResettleOutcome {
    /// The backend came up and the settle ran. The settle itself may still
    /// have stood autofill down -- a survivable outcome this app already
    /// ships (see [`settle_vault_after_unlock`]), and not a reason to undo
    /// whatever the resettle was for.
    BackendStarted,
    /// Nothing authenticated, or the backend would not start.
    /// [`stand_down_after_unlock`] has run: the cache is cleared,
    /// `bw_serve_child` is `None`, the engine is empty, and the tray's
    /// "Sync" is the recovery the user is told about.
    BackendNotStarted,
}

/// The one teardown-and-repopulate sequence in this crate: drain whatever
/// backend operation is in flight, stop `bw serve`, clear the cache,
/// authenticate, start a fresh backend, wait for it to answer, repopulate,
/// and rebuild the match engine.
///
/// This used to be inlined in `open_vault_window`'s `locked || needs_reauth`
/// block, and it is a function now because it has a second caller coming: an
/// account switch is *this* sequence with a different data directory, and the
/// spec is explicit that it must reuse this rather than grow a second
/// teardown path beside it. `authenticate` is the only thing that differs
/// between the two -- a lock/re-auth prompts for the password of the account
/// already signed in, a switch first points the CLI at the target account's
/// profile and then prompts. It runs AFTER the old backend is stopped and the
/// cache is cleared and BEFORE the new backend is started, which is the
/// ordering that makes a half-switched app unreachable, and returning `None`
/// from it (a declined switch) stands autofill down rather than starting a
/// backend for a session nobody authenticated.
///
/// The split with [`resettle_session_with`] is where the untestable parts
/// stop. Everything this function itself does needs a real tray icon (which
/// `tray::build_tray` can only make against a live message loop), a real
/// `bw` process, or a real spinner window; everything the sequence *decides*
/// lives in `resettle_session_with` behind injected `start` and `probe`
/// closures, the same shape `settle_vault_after_unlock`'s own `probe` already
/// proved. What is left here is wiring, and it is pinned by
/// `the_resettle_wiring_passes_the_real_backend_and_spinner`.
#[allow(clippy::too_many_arguments)]
fn resettle_session(
    cache: &Arc<VaultCache>,
    engine: &mut MatchEngine,
    bw_serve_child: &mut Option<Child>,
    job: &Arc<Option<job_object::KillOnCloseJob>>,
    schedule: &[Duration],
    tray: &tray::AppTray,
    backend_op_rx: &mpsc::Receiver<BackendOp>,
    backend_task_in_progress: &mut Option<(Instant, BackendOpKind)>,
    cached_status_details: &mut Option<login_ui::BwStatusDetails>,
    session_token: &mut String,
    authenticate: impl FnOnce() -> Option<String>,
) -> ResettleOutcome {
    // A backend operation kicked off above (or a tray Sync click that
    // landed while the window was open) may still be in flight. Unlike
    // `main`'s own non-blocking drain, this path is about to tear the
    // backend down and start a fresh one right now, so it has to wait
    // for that operation to actually finish first -- otherwise the two
    // attempts race to bind the same port. The user is already looking
    // at a blocking re-authentication flow at this point, so a few more
    // seconds here is not a new freeze, just a longer instance of one
    // that was already happening.
    //
    // Bounded, not a plain `recv()`: `backend_op_tx` lives in `main` for
    // the whole process, so the channel never disconnects on its own --
    // if the worker thread that owns the other end ever panicked before
    // sending, an unbounded `recv()` here would block this thread
    // forever, with no message pump running to keep the tray, hotkey, or
    // window-watching alive (review Minor). Giving up after
    // `BACKEND_OP_TIMEOUT` and proceeding anyway is strictly safer: the
    // worst case is racing a start/sync that eventually does land (see
    // `apply_backend_op`'s callers), not an unkillable app.
    if backend_task_in_progress.is_some() {
        log::info!("waiting for an in-flight backend operation before handling the lock");
        match backend_op_rx.recv_timeout(BACKEND_OP_TIMEOUT) {
            Ok(op) => apply_backend_op(op, bw_serve_child, cache, engine, tray),
            Err(_) => {
                log::warn!(
                    "in-flight backend operation did not report back within \
                     {BACKEND_OP_TIMEOUT:?}; proceeding with lock recovery anyway. If it \
                     later reports back late (see `apply_backend_op`'s child-adoption \
                     guard), its child is stopped rather than allowed to overwrite the one \
                     this recovery is about to start."
                );
                // Review 11's Important 3: whatever operation was in
                // flight may have disabled the tray's "Sync" item before
                // stalling (the `tray.add_app_id` handler does, for its
                // own `EnsureRunning`), and `apply_backend_op` -- the
                // only other place that re-enables it -- is never going
                // to run for an operation that gave up waiting on
                // instead of receiving. Left disabled, a hung `bw sync`
                // right here permanently kills Sync for the rest of the
                // session. A no-op if nothing had disabled it.
                //
                // Review 18's Minor: all the way back to idle, not just
                // re-enabled. THIS is the site where the two disagreed --
                // the operation being abandoned here is very often the
                // tray `Sync` that set the label to "Syncing...", and its
                // thread is by definition never going to report back and
                // relabel it. The stand-down message a few lines below
                // names "Sync"; leaving the item saying "Syncing..."
                // means the menu contains no item by that name, and the
                // one that is there reads as busy.
                tray::set_sync_idle(tray);
            }
        }
        *backend_task_in_progress = None;
    }

    resettle_session_with(
        cache,
        engine,
        bw_serve_child,
        cached_status_details,
        session_token,
        authenticate,
        |token| try_start_backend(token, job_ref(job), bw_serve::PORT_RELEASE_GRACE_RESTART),
        |message| wait_for_vault_ready_with_spinner(cache.bridge(), schedule, message),
    )
}

/// The sequence itself, with the two things that need a machine injected:
/// `start` is `try_start_backend` and `probe` is the readiness wait behind
/// its spinner window.
///
/// Nothing here is new -- it is `open_vault_window`'s lock/re-auth block,
/// moved -- with one arm added that the old code had no way to express: an
/// `authenticate` that answers `None`. That cannot happen on the lock/re-auth
/// path (`reauthenticate` exits instead of failing), and it is how a declined
/// account switch will arrive.
#[allow(clippy::too_many_arguments)]
fn resettle_session_with(
    cache: &VaultCache,
    engine: &mut MatchEngine,
    bw_serve_child: &mut Option<Child>,
    cached_status_details: &mut Option<login_ui::BwStatusDetails>,
    session_token: &mut String,
    authenticate: impl FnOnce() -> Option<String>,
    start: impl FnOnce(&str) -> Result<Child, BackendStartError>,
    probe: impl FnMut(&'static str) -> VaultReadyOutcome,
) -> ResettleOutcome {
    // The account the *next* unlock lands on may not be this one (a
    // "Log out" followed by a different sign-in), so the snapshot built
    // from this account must not survive into that one. Left populated,
    // the next window open -- or the next autofill, straight from
    // `cache.items()` -- would silently serve this account's items and
    // passwords under the new session, indefinitely if `bw sync` then
    // fails offline.
    cache.clear();
    if backend_is_running(bw_serve_child) {
        if let Some(child) = bw_serve_child.as_mut() {
            bw_serve::stop_bw_serve(child);
        }
    }
    *bw_serve_child = None;
    // Drop the cached email/server too, for the same reason: the *next*
    // open must re-fetch rather than show a stale account in the
    // toolbar. Unconditional, and therefore hoisted above the
    // authentication rather than left after it, so a declined
    // authentication does not leave the previous account's address in the
    // toolbar of a window that is no longer signed into it. Nothing can
    // observe the move: this function holds the only `&mut` to it, so
    // `authenticate` cannot read it.
    *cached_status_details = None;

    // Not reachable from the lock/re-auth path -- `reauthenticate` exits the
    // process rather than return a failure -- and the whole point of an
    // account switch, where it means "the user closed the master-password
    // prompt". There is no session, so there is nothing to start a backend
    // for; standing down is the same recovery the arms below use, and it is
    // what leaves the engine empty rather than armed with the matches of an
    // account this process is no longer talking to.
    let Some(token) = authenticate() else {
        stand_down_after_unlock(
            engine,
            "nothing was authenticated, so the Bitwarden backend was not restarted",
        );
        return ResettleOutcome::BackendNotStarted;
    };
    *session_token = token;

    // Same lifecycle as startup: the backend has to come up -- blocking,
    // with a spinner, since there is nothing useful to show without it
    // -- to re-populate the cache. `main`'s idle reconciliation tears it
    // back down afterwards if the policy says to, exactly as it does
    // after startup's own unconditional start; this function no longer
    // needs to know or care what the policy says.
    //
    // A failure to start it is survivable here, and standing down is the
    // whole of the recovery: see `restart_backend_after_unlock`. There is
    // nothing left to probe once no backend came up, so this returns
    // rather than spending the ~30s readiness deadline on a port nothing
    // is listening on.
    let Some(child) = restart_backend_after_unlock(engine, || start(session_token)) else {
        return ResettleOutcome::BackendNotStarted;
    };
    *bw_serve_child = Some(child);
    // Captured here, *before* the readiness probe below, for the same
    // reason startup captures `startup_epoch` before its own probe: that
    // probe's `list_items()` is the fetch whose result seeds the cache
    // via `populate_with`, and the epoch guard can only cover the window
    // it is handed (review 14's Minor 3). It has to be taken after the
    // `cache.clear()` further up -- that clear is the one this recovery
    // is repopulating from, not one to discard against. Nothing between
    // here and there clears the cache today (every `clear` site in the
    // crate runs on this same, currently blocked, main thread), so this
    // is inert; it is written this way so it stays correct if any of it
    // moves onto a background thread.
    let unlock_epoch = cache.epoch();
    settle_vault_after_unlock(cache, engine, unlock_epoch, probe);
    ResettleOutcome::BackendStarted
}

/// What one run of the resettle told the switch.
///
/// [`ResettleOutcome`] has two variants and this has three, because a
/// master-password prompt the user closed and a backend that would not start
/// both arrive at `BackendNotStarted`, and only whoever built the
/// `authenticate` closure can tell them apart. The switch has to: one is a
/// failure to report and the other is the user changing their mind, and a
/// switch that told the user "the backend did not start" because they pressed
/// Cancel would be reporting something that never happened.
///
/// The alternative was to authenticate before calling the sequence, where the
/// switch could see the answer directly. That is exactly the ordering Task 8's
/// `the_resettle_authenticates_after_the_teardown_and_settles_after_the_start`
/// exists to forbid: the master-password prompt for the *new* account would go
/// up with the *previous* account's items still live in the cache and its
/// matches still armed behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Constructed by the caller that owns the `authenticate` closure, which is the
// tray wiring in Task 11; until then only this file's tests build one.
enum ResettleReport {
    /// [`ResettleOutcome::BackendStarted`]: the account is live.
    Settled,
    /// `BackendNotStarted` because `authenticate` answered `None` -- the user
    /// closed the master-password prompt.
    Declined,
    /// `BackendNotStarted` with a session in hand: nothing came up to serve it.
    NotStarted,
}

/// Where an account switch left the app.
#[derive(Debug, Clone, PartialEq, Eq)]
// Returned to the tray wiring in Task 11; see `ResettleReport` above.
enum SwitchOutcome {
    /// The target account is live: its backend is up, its cache is populated
    /// from its own vault and its matches are armed from those items.
    Switched,
    /// The user closed the target account's master-password prompt, and the
    /// previous account is back. Not an error, and not to be reported as one.
    Declined,
    /// The switch failed and the previous account is back.
    RolledBack { reason: String },
    /// The switch failed and so did the rollback: [`stand_down_after_unlock`]'s
    /// state, which this app already ships and recovers from via the tray's
    /// "Sync".
    StoodDown { reason: String },
}

/// Removes a Bitwarden account: settles the app somewhere coherent, logs that
/// account's profile out, deletes it, and only then writes the shorter list.
///
/// This deletes a real encrypted vault profile from the user's disk, which is
/// the most destructive thing this app does. So the order is the whole of it,
/// and each step is chosen for what a crash *immediately after it* leaves
/// behind.
///
/// **1. The account being removed must not be the one the app is on.** If it
/// is, the app settles onto `accounts::next_active_after_removal` first,
/// through [`switch_to_account`] — the same switch every other account change
/// goes through, so there is no second teardown path here either. Deleting
/// first would run `remove_dir_all` over the profile `bw serve` is at that
/// moment serving, and leave the app pointed at a directory that no longer
/// exists. A switch that does not land is a removal that does not happen:
/// nothing is deleted and the account is still there to try again.
///
/// **The last account cannot be removed at all.** There is no coherent place
/// for the app to land: no profile to point the CLI at, no `session.bin` to
/// load and no account for the login window to enrol Windows Hello against, so
/// every window this app opens would be operating on a directory that is not
/// there. "Log out" already exists and is what emptying the app means; this
/// refuses, with a message that says so.
///
/// **2. The gate is `AccountsState`'s, not the machine's.** The account the app
/// has to reach — the survivor, or the target itself when it is not active —
/// has to be one `AccountsState::switchable` offers. That is one question
/// rather than a second reading of the CLI's availability, and it is the
/// *right* question: where multiple accounts are
/// unavailable, every account shares one profile, so `bw logout` in the doomed
/// account's directory would log out **the active account** and the deletion
/// would take a directory the CLI never used.
///
/// **3. `bw logout` runs in that account's OWN directory**, via the injected
/// closure, which takes the directory as an argument. Never a temporary
/// mutation of `bw_path::set_active_data_dir`: background threads spawn `bw`
/// against that global, so a window in which it names another account's profile
/// is a window in which a sync can land in the wrong vault.
///
/// A `bw logout` that fails does not stop the removal. The user asked for this
/// profile to be gone, and a CLI that cannot be run is not a reason to leave an
/// encrypted vault and a sealed master password on disk; it is logged and the
/// deletion goes ahead.
///
/// **4. The Hello blob goes first, then the whole directory.**
/// `accounts::delete_account_dir` takes `session.bin` and `hello.bin` with the
/// profile — the reasoning `login_ui`'s log-out handler already applies: a
/// sealed credential for an account the CLI no longer knows is a liability, not
/// a feature. `hello::unenroll_for` runs first anyway, because a directory that
/// *cannot* be deleted (a `bw` still holding `data.json` open) would otherwise
/// leave that sealed master password behind for an account this app has
/// forgotten. It touches only this account's file and rotates no Windows Hello
/// credential: one credential seals every account, separated by
/// `accounts::hello_kdf_suffix_for`, and replacing it would lock every *other*
/// account's quick unlock out.
///
/// **5. The list is written LAST**, for the reason Task 12 established: written
/// first, a removal that then fails leaves `settings.json` disagreeing with the
/// disk — an account the app has forgotten whose vault is still there, and no
/// way left to remove it. Written last, a crash leaves the directory orphaned
/// but intact, which is the survivable shape.
#[allow(clippy::too_many_arguments)]
// Live since Task 15: the tray's "Accounts" submenu offers "Remove <account>..."
// whenever `AccountsState` says there is a survivor to settle onto.
fn remove_account(
    config_dir: &Path,
    settings_path: &Path,
    state: &mut accounts::AccountsState,
    target: &accounts::AccountId,
    active_account: &mut Account,
    store: &mut session_store::SessionStore,
    resettle: impl FnMut(&Path, &Account, &session_store::SessionStore) -> ResettleReport,
    mut logout: impl FnMut(Option<&Path>) -> Result<(), String>,
) -> Result<(), String> {
    let Some(doomed) = accounts::account_for(state.all(), target).cloned() else {
        return Err(
            "that account is not one of this app's accounts, so there is nothing to remove"
                .to_string(),
        );
    };

    let survivor = if state.active().id == doomed.id {
        Some(
            accounts::next_active_after_removal(state.all(), target)
                .ok_or_else(|| {
                    format!(
                        "{} is the only account: removing it would leave this app with no \
                         profile to point the Bitwarden CLI at. Log out instead.",
                        account_label(&doomed)
                    )
                })?
                .clone(),
        )
    } else {
        None
    };

    // Asked of the state, once, for both cases: the account the app has to be
    // able to REACH is the survivor when the doomed one is active, and the
    // doomed one itself otherwise.
    let must_reach = survivor.as_ref().map_or(target, |s| &s.id);
    if !state.switchable().iter().any(|a| &a.id == must_reach) {
        return Err(format!(
            "{} cannot be removed right now: {}",
            account_label(&doomed),
            state.blocked_reason().unwrap_or(
                "this app cannot reach that account, and it will not delete a profile it \
                 cannot reach"
            )
        ));
    }

    if let Some(survivor) = &survivor {
        let from = state.active().clone();
        match switch_to_account(config_dir, &from, survivor, active_account, store, resettle) {
            SwitchOutcome::Switched => state.adopt(survivor.clone()),
            other => {
                return Err(format!(
                    "{} was not removed: the app could not settle onto {} first ({other:?}), \
                     and nothing has been deleted",
                    account_label(&doomed),
                    account_label(survivor)
                ))
            }
        }
    }

    let dir = accounts::data_dir_for(config_dir, &doomed.id);
    if let Err(e) = logout(Some(&dir)) {
        log::warn!(
            "`bw logout` for {} failed: {e}; its profile is being deleted anyway",
            account_label(&doomed)
        );
    }

    deskwarden::hello::unenroll_for(config_dir, &doomed.id);

    match accounts::delete_account_dir(config_dir, &doomed.id) {
        Ok(()) => log::info!(
            "removed {}: {} is gone",
            account_label(&doomed),
            dir.display()
        ),
        Err(reason) => log::error!(
            "{reason}; {} is being removed from the account list anyway, so that directory is \
             orphaned and can be deleted by hand",
            account_label(&doomed)
        ),
    }

    if !state.forget(&doomed.id) {
        // Unreachable: this function has already established that the account
        // is in the list and is not the active one, which are `forget`'s only
        // two refusals. Logged rather than ignored, because the state it would
        // mean -- a live entry naming a deleted profile -- is one the user
        // meets as an account that will not sign in.
        log::error!(
            "the account list still holds {} after its profile was deleted",
            account_label(&doomed)
        );
    }

    let active_id = state.active().id.clone();
    if let Err(e) =
        settings::Settings::persist_accounts(settings_path, state.all(), Some(&active_id))
    {
        log::error!(
            "{} was removed but the shorter account list could not be written to {}: {e}; it \
             will be back in the list on the next launch, naming a directory that is gone",
            account_label(&doomed),
            settings_path.display()
        );
    }

    Ok(())
}

/// Adds a Bitwarden account: mints one, signs in to it, and then settles onto
/// it with the **same** switch every other account change goes through — or
/// leaves the app exactly where it started, with nothing left behind.
///
/// **The gate is asked, not answered.** `state.can_add()` is the one answer to
/// whether multi-account is available at all (see [`accounts::AccountsState`]),
/// and an account added while it says no is a profile this app cannot reliably
/// reach again: the CLI ignores `BITWARDENCLI_APPDATA_DIR` there, so every
/// account it offers is served the one portable profile.
///
/// **The sign-in runs in the new account's own directory, and the way it gets
/// there is an argument, not a global.** `bw login` signs in to whatever
/// profile `BITWARDENCLI_APPDATA_DIR` names and *replaces* whatever was there,
/// so a sign-in aimed at the active account's directory would not add an
/// account — it would sign the existing one out and overwrite it. No end state
/// distinguishes the two, which is why
/// `the_sign_in_runs_with_the_cli_pointed_at_the_new_accounts_directory`
/// records what the sign-in could *see* rather than what it left behind.
///
/// Until review 13 this body got there by pointing
/// `bw_path::set_active_data_dir` at the new directory across the whole of
/// `sign_in` and restoring it after — a temporary mutation of exactly the
/// process-global [`remove_account`]'s doc bans, over a window as long as the
/// user takes to type a master password, while `bw_serve`'s background threads
/// read that global to spawn `bw`. `login_ui::profile_dir_for` now derives the
/// directory from the account the window was already handed and passes it to
/// `bw_command_in`, so the global never moves and there is no window to get
/// wrong. The invariant is one rule for both functions again.
///
/// **A sign-in that does not happen leaves nothing.** `sign_in` answers `None`
/// when the user closes the window ([`login_ui::run_login_flow_for`], the
/// cancellable form), and the whole of the response is
/// [`accounts::discard_prepared_account`]: no directory, no entry, no change of
/// active account. The state to avoid is an entry naming a directory with no
/// profile in it, which presents as an account that is permanently signed out.
///
/// **The account is written to `settings.json` before the switch, and the
/// active id after it.** Two writes, and the split is what makes a stranded
/// profile unrepresentable rather than merely unlikely.
///
/// The window to close is the one between "`bw login` has written
/// `accounts/<id>/data.json`" and "something on disk names that directory".
/// This function used to close it *after* the switch, so a failed
/// `persist_accounts` left a signed-in profile no launch could ever reach —
/// the state a whole scan-and-adopt path in the deleted `migration` module
/// existed to recover from, and the one it was found to be still reachable
/// through. Written before the switch instead, a failed write has nothing to
/// undo: the switch has not happened, so the response is
/// [`accounts::discard_prepared_account`] and a rollback, exactly as for a
/// closed sign-in window. It costs the user the sign-in they just did — the
/// directory is a local cache and a session token, and the vault itself is on
/// Bitwarden's servers — and it leaves nothing behind.
///
/// The active id stays on the account being *left* until the switch actually
/// lands, so the only thing a failed second write costs is that the next
/// launch resumes the old account with the new one listed beside it. Writing
/// the new id up front would instead point the next launch at a directory this
/// function may be about to delete.
///
/// Every remaining failure therefore leaves an entry whose directory is gone,
/// never a directory nothing names — and an entry whose directory is gone is
/// self-healing: `ensure_account_dir` recreates it and the login window signs
/// in to it, or the user removes it. That asymmetry is the whole reason this
/// ordering was chosen over keeping a path that scans for orphans.
///
/// `sign_in` and `account_details` are injected for the reason every other
/// window and `bw` spawn in this file is: neither can run in a test. That
/// injection is also what
/// `adding_an_account_opens_no_window_and_asks_the_cli_nothing_by_itself`
/// protects — a body that *also* called the real ones directly would make the
/// observations above a lie while every test stayed green.
#[allow(clippy::too_many_arguments)]
// Live since Task 15: the tray's "Accounts" submenu offers "Add account..."
// whenever `AccountsState::can_add` allows it.
fn add_account(
    config_dir: &Path,
    settings_path: &Path,
    state: &mut accounts::AccountsState,
    active_account: &mut Account,
    store: &mut session_store::SessionStore,
    mut sign_in: impl FnMut(&Account) -> Option<String>,
    mut account_details: impl FnMut(Option<&Path>) -> login_ui::BwStatusDetails,
    resettle: impl FnMut(&Path, &Account, &session_store::SessionStore) -> ResettleReport,
) -> SwitchOutcome {
    if !state.can_add() {
        log::warn!(
            "refusing to add an account: {}",
            state
                .blocked_reason()
                .unwrap_or("multiple accounts are unavailable on this machine")
        );
        return SwitchOutcome::Declined;
    }

    let from = state.active().clone();
    let prepared = match accounts::prepare_new_account(config_dir) {
        Ok(prepared) => prepared,
        Err(reason) => {
            log::error!("could not prepare a new account: {reason}");
            // Nothing was touched, so the app is already where it started.
            return SwitchOutcome::RolledBack { reason };
        }
    };
    let prepared_dir = accounts::data_dir_for(config_dir, &prepared.id);
    // Everything from here to the persist below has to end in either a write
    // that names `prepared_dir` or a delete of it. There is no third answer,
    // and that is the invariant `a_failed_persist_strands_no_signed_in_profile`
    // drives.

    // **No temporary mutation of `bw_path::set_active_data_dir`.** This used
    // to point the process-global at `prepared_dir`, run `sign_in` -- a
    // blocking, user-paced window -- and put it back, which is precisely what
    // `remove_account`'s doc bans four hundred lines up: `bw_serve.rs` reads
    // that global from background threads, so the window in which it names
    // another account's profile is a window in which a sync can land in the
    // wrong vault. The global does not move here at all now; the directory
    // reaches the CLI as an argument instead, through
    // `login_ui::profile_dir_for`, which derives it from the account the
    // window was already handed.
    let token = sign_in(&prepared);

    let Some(token) = token else {
        log::info!("the sign-in for the new account was closed; nothing was added");
        accounts::discard_prepared_account(config_dir, &prepared.id);
        return SwitchOutcome::Declined;
    };

    // The label the switcher shows, asked of the new account's OWN directory
    // rather than of whatever profile this process is pointed at — which is the
    // previous account's again by now. An account minted by
    // `prepare_new_account` carries an empty email until exactly here, and a
    // blank row is one the user cannot tell from any other.
    let details = account_details(Some(&prepared_dir));
    if details.user_email.is_none() {
        log::warn!(
            "the new account signed in but `bw status` named no address for it, so it will \
             appear in the account list with no address on it"
        );
    }
    let added = Account {
        id: prepared.id.clone(),
        email: details.user_email.unwrap_or_default(),
        server_url: details.server_url,
    };

    // Into the new account's own file, before the switch, because the switch's
    // sequence is what authenticates and it starts by looking for a token it
    // can still use. Without this the user types their master password, watches
    // the window close, and is asked for it again immediately.
    let prepared_store =
        session_store::SessionStore::new(accounts::session_path_for(config_dir, &added.id));
    if let Err(e) = prepared_store.save(&token) {
        log::warn!(
            "could not store the new account's session token at {}: {e}",
            prepared_store.path().display()
        );
    }

    // The claim, before the switch. `state.all()` plus the new account rather
    // than `state.adopt` first: adopting makes it ACTIVE, and it is not active
    // until the switch below says so.
    let mut claimed = state.all().to_vec();
    claimed.push(added.clone());
    if let Err(e) = settings::Settings::persist_accounts(settings_path, &claimed, Some(&from.id)) {
        // The signed-in profile in `prepared_dir` is now a directory nothing
        // will ever name, so it goes. That costs the sign-in that just
        // happened and nothing else -- the vault is on Bitwarden's servers.
        let reason = format!(
            "the new account signed in but could not be written to {}: {e}",
            settings_path.display()
        );
        log::error!("{reason}; discarding it rather than leaving a profile nothing names");
        accounts::discard_prepared_account(config_dir, &added.id);
        return SwitchOutcome::RolledBack { reason };
    }

    match switch_to_account(config_dir, &from, &added, active_account, store, resettle) {
        SwitchOutcome::Switched => {
            state.adopt(added.clone());
            if let Err(e) =
                settings::Settings::persist_accounts(settings_path, state.all(), Some(&added.id))
            {
                // The account itself is already recorded, so this costs only
                // which one the next launch resumes.
                log::error!(
                    "the new account is live but {} could not be updated: {e}; the next launch \
                     will resume {} instead, with the new account listed beside it",
                    settings_path.display(),
                    account_label(&from)
                );
            }
            SwitchOutcome::Switched
        }
        other => {
            log::warn!("the new account did not settle ({other:?}); discarding it");
            // The entry first, then the directory: the reverse order would
            // leave a moment in which the list names a directory that is gone,
            // and a crash inside it is the permanently signed-out account
            // above. If this write fails the directory is kept, because the
            // account it holds is real and listed -- the user can switch to it
            // or remove it.
            match settings::Settings::persist_accounts(settings_path, state.all(), Some(&from.id)) {
                Ok(()) => accounts::discard_prepared_account(config_dir, &added.id),
                Err(e) => log::error!(
                    "the new account did not settle and could not be removed from {}: {e}; it \
                     is signed in and will be offered on the next launch",
                    settings_path.display()
                ),
            }
            other
        }
    }
}

/// Switches the app from one account to another, or leaves it exactly where it
/// started.
///
/// The dominant risk the spec names is a switch that HALF lands -- the CLI
/// pointed at the new account's profile beside the old account's cache. That
/// state is not reachable from here, and the reason is that this function does
/// not perform a switch. It re-points the two values that answer "which
/// account is this process?" and then runs *the* existing
/// teardown-and-repopulate sequence, which is [`resettle_session`]. Every
/// thing that could half-land -- the cache, the match engine, `bw serve`, the
/// session token -- lives inside that sequence and is not a parameter here, so
/// this body has nothing to touch out of order and no second teardown path to
/// get wrong.
///
/// `resettle` is injected for exactly the reason `settle_vault_after_unlock`'s
/// `probe` is: `resettle_session`'s own body needs a real `tray::AppTray`, and
/// `tray::build_tray` makes a real Windows tray icon against a live message
/// loop. The live caller passes a closure that calls it;
/// `a_switch_reimplements_none_of_the_sequence_it_is_supposed_to_reuse` pins
/// that this body has not done the work itself instead.
///
/// **Order.** The data directory and the token store are re-pointed BEFORE the
/// sequence runs, because the sequence is what authenticates,
/// `login_ui::run_login_flow_for` spawns `bw`, and that spawn has to land in
/// the target account's profile -- and the token it produces has to be saved
/// into the target account's `session.bin`, not written over the account the
/// user is leaving. Re-pointing afterwards, or never, makes the whole feature
/// inert while every end-state assertion stays green, which is why the failed
/// -switch test records what each run of the sequence could SEE rather than
/// only what was left behind.
///
/// **Rollback.** A rollback has nothing to undo. Between the re-point and the
/// sequence this function changes nothing else: `active_account` is assigned
/// only once the target has settled, and the outgoing account's token file is
/// discarded only then too -- throwing it away up front would cost the user a
/// master-password prompt for an account they never asked to leave, on top of
/// the switch that just failed. So the rollback is the re-point in reverse
/// plus the same sequence again, run for the account whose session was never
/// invalidated.
///
/// **It may not kill the app.** `fatal_startup_error`, and startup's
/// `start_backend` wrapper that calls it, are banned from this body and the
/// ban is pinned by `no_switch_path_can_reach_the_fatal_startup_error`: the
/// *other* account's backend refusing to start is not a reason to take a
/// running app down. So is `run_login_flow`, the wrapper that exits when the
/// user declines -- a switch authenticates through `run_login_flow_for`, which
/// can answer `None`, and that `None` is this function's `Declined`.
// Called by the tray wiring in Task 11; see `ResettleReport` above.
fn switch_to_account(
    config_dir: &Path,
    from: &Account,
    to: &Account,
    active_account: &mut Account,
    store: &mut session_store::SessionStore,
    mut resettle: impl FnMut(&Path, &Account, &session_store::SessionStore) -> ResettleReport,
) -> SwitchOutcome {
    // Whatever it was, not `data_dir_for(from)`: an app that resolved
    // `StartupAccounts::NoAccountList` is on the CLI's own default directory,
    // and a rollback has to put it back there rather than invent an account
    // directory.
    let previous_dir = bw_path::active_data_dir();
    log::info!("switching accounts: {} -> {}", from.email, to.email);

    bw_path::set_active_data_dir(Some(accounts::data_dir_for(config_dir, &to.id)));
    *store = session_store::SessionStore::new(accounts::session_path_for(config_dir, &to.id));

    let report = resettle(config_dir, to, store);
    let failure = match report {
        ResettleReport::Settled => {
            *active_account = to.clone();
            // Only NOW. Until the switch has landed, the outgoing account is
            // still this app's account rather than an idle one.
            let outgoing = accounts::session_path_for(config_dir, &from.id);
            if let Err(e) = std::fs::remove_file(&outgoing) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!(
                        "could not discard {}'s session token at {}: {e}",
                        from.email,
                        outgoing.display()
                    );
                }
            }
            log::info!("switched to {}", to.email);
            return SwitchOutcome::Switched;
        }
        ResettleReport::Declined => {
            format!("the master password prompt for {} was closed", to.email)
        }
        ResettleReport::NotStarted => {
            format!("the Bitwarden backend for {} did not start", to.email)
        }
    };

    log::warn!("{failure}; returning to {}", from.email);
    bw_path::set_active_data_dir(previous_dir);
    *store = session_store::SessionStore::new(accounts::session_path_for(config_dir, &from.id));
    match resettle(config_dir, from, store) {
        // A decline is the user changing their mind, and the previous account
        // is back: there is nothing to report but the fact that nothing moved.
        ResettleReport::Settled if report == ResettleReport::Declined => SwitchOutcome::Declined,
        ResettleReport::Settled => SwitchOutcome::RolledBack { reason: failure },
        ResettleReport::Declined | ResettleReport::NotStarted => SwitchOutcome::StoodDown {
            reason: format!("{failure}, and {} could not be restored either", from.email),
        },
    }
}

/// Borrows the job object out of its `Arc` wrapper for a synchronous call.
///
/// The `Arc` only exists so a clone can be handed off to a background
/// thread (see `spawn_backend_start`/`spawn_sync`); every other call site
/// still just wants a plain `Option<&KillOnCloseJob>`, same as before that
/// wrapper existed.
fn job_ref(job: &Arc<Option<job_object::KillOnCloseJob>>) -> Option<&job_object::KillOnCloseJob> {
    job.as_ref().as_ref()
}

/// Whether `bw serve` is currently running, treating an already-exited child
/// the same as `None` rather than trusting `Option::is_some` alone.
///
/// `Child` has no way to notice its own process exiting on its own --
/// `bw_serve_child` stays `Some` even long after the process is gone unless
/// something calls `try_wait`. Review Minor 8: code that only checked
/// `.is_none()` to decide whether `bw serve` needed (re)starting would never
/// notice a `Some(dead child)` and so never restart it. Clears `*child` to
/// `None` on a detected exit, so callers can go back to the simpler
/// `is_none()` check afterwards.
fn backend_is_running(child: &mut Option<Child>) -> bool {
    let Some(c) = child.as_mut() else {
        return false;
    };
    match c.try_wait() {
        Ok(None) => true,
        Ok(Some(status)) => {
            log::warn!("bw serve exited on its own (status: {status}); treating it as stopped");
            *child = None;
            false
        }
        Err(e) => {
            // Can't tell either way. Assuming it's still running is the
            // safer failure mode: the alternative risks a second
            // `try_start_backend` racing the still-alive first one to bind
            // the same port.
            log::warn!("could not check whether bw serve is still running ({e}); assuming it is");
            true
        }
    }
}

/// Stops `bw serve` if it's running but [`backend_policy::should_run`] says
/// it shouldn't be, with no vault window open.
///
/// The other half of the policy -- starting the backend when it should be
/// running but isn't -- is deliberately not handled here as a symmetric
/// "else start it": with the periodic refresh removed (review Critical 1),
/// nothing throttles a repeated failure, and calling this every idle loop
/// iteration (as `main` does) would turn a backend that keeps failing to
/// start into a retry storm. The three places that genuinely need the
/// backend -- startup, `open_vault_window`, and the tray's Sync item -- each
/// ask for it explicitly instead; this function only ever tears it back
/// down again afterwards once the policy says it's no longer needed.
fn stop_backend_if_idle(bw_serve_child: &mut Option<Child>, keep_backend_running: bool) {
    if backend_policy::should_run(keep_backend_running) {
        return;
    }
    if backend_is_running(bw_serve_child) {
        log::info!("save-memory mode: nothing needs bw serve right now; stopping it");
        if let Some(child) = bw_serve_child.as_mut() {
            bw_serve::stop_bw_serve(child);
        }
        *bw_serve_child = None;
    }
}

/// Whether a backend operation marked in-flight since `started` has been
/// outstanding long enough to treat as wedged rather than merely slow.
///
/// A standalone predicate (rather than the `Duration` comparison inlined at
/// its one call site in `main`'s loop) purely so it can be unit tested --
/// `main` itself never returns, so nothing inside its loop is otherwise
/// reachable from a test. See `backend_task_in_progress`'s doc in `main` for
/// what not catching this leads to.
fn backend_task_is_wedged(started: Instant, deadline: Duration) -> bool {
    started.elapsed() >= deadline
}

/// Whether a background "ensure `bw serve` is running" kick should be
/// started right now.
///
/// Shared by `open_vault_window` and the tray's "Add app..." handler
/// (review 9's Important finding): both reach a step -- the vault window's
/// writes/TOTP, the picker's Save -- that needs `bw serve` up, and both
/// start it the same non-blocking way rather than waiting. Never start a
/// second attempt on top of one already in flight (`backend_task_in_progress`
/// racing itself to bind the same port), and never restart something that's
/// already running.
///
/// A standalone predicate for the same reason as `backend_task_is_wedged`:
/// testable without opening a window.
fn needs_backend_start(
    backend_task_in_progress: &Option<(Instant, BackendOpKind)>,
    backend_already_running: bool,
) -> bool {
    backend_task_in_progress.is_none() && !backend_already_running
}

/// Which kind of background backend operation `backend_task_in_progress` is
/// currently tracking. Recorded alongside the `Instant` so the wedge-deadline
/// check in `main`'s loop can say what actually stalled (review Minor 4):
/// `open_vault_window`'s `EnsureRunning` -- just making sure `bw serve` is up
/// before showing the window, no sync requested at all -- used to always be
/// reported (and shown on the tray) as a failed *sync* if it wedged, which
/// was simply untrue whenever no sync was involved.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BackendOpKind {
    EnsureRunning,
    Sync,
}

/// Outcome of a background operation that starts or restarts `bw serve`.
///
/// Both kinds -- `open_vault_window` making sure the backend is up, and the
/// tray's "Sync" item -- funnel through this one enum/channel rather than
/// one each, so `main`'s `backend_task_in_progress` flag can guarantee at
/// most one is ever in flight. Two concurrent `try_start_backend` calls
/// would race to bind the same port and make one fail for a reason that has
/// nothing to do with a real problem; sharing one channel also means there
/// is exactly one place -- not two -- a lock event has to drain before it
/// can safely stop and restart the backend itself (see `open_vault_window`'s
/// `locked` branch).
enum BackendOp {
    /// `open_vault_window` made sure the backend was up before showing the
    /// window. No sync/populate/rebuild attached -- reads already come from
    /// `cache` regardless of whether this succeeded.
    EnsureRunning(Result<Child, BackendStartError>),
    /// The tray's "Sync" item: ensure the backend is running (`child` is
    /// `Some` only if this operation itself had to start it), then run
    /// `bw sync` and repopulate the cache.
    Sync {
        child: Option<Result<Child, BackendStartError>>,
        outcome: SyncOutcome,
    },
}

/// What a tray-triggered sync actually achieved.
///
/// Three outcomes, not two: a `Result<(), String>` could not tell "the vault
/// was refreshed" apart from "the sync ran but its result was discarded
/// because the vault locked underneath it", so the latter took the success
/// path -- logging "sync complete", rebuilding the match engine from an
/// empty cache and returning the tray to its idle "Sync" label as though the
/// vault were freshly in sync (review 14's Minor).
///
/// This is what the WORKER observed. What the main thread should do about it
/// is [`SettledSync`], decided by `settle_sync_outcome`; both are matched
/// exhaustively, with no catch-all anywhere between here and the tray.
#[derive(Debug)]
enum SyncOutcome {
    /// `bw sync` succeeded and the refreshed vault landed in the cache.
    Refreshed {
        /// The vault era this sync was written in, captured by the worker
        /// BEFORE its own fetch. Re-checked by `settle_sync_outcome` when the
        /// outcome is finally applied on the main thread -- see that function
        /// and [`deskwarden::vault_cache::VaultEra`].
        ///
        /// The ERA, not the whole [`deskwarden::vault_cache::VaultEpoch`] the
        /// worker captured: the write half of that epoch is a writer's
        /// concern and was consumed by `populate_with` on the worker thread.
        /// What is left to decide here is only "is this still the same vault
        /// session?", and carrying a write position into it would invite
        /// exactly the "something changed, give up" reading review 18 removed.
        ///
        /// Deliberately the ONLY thing this variant carries. It used to carry
        /// the match entries this sync's own `list_items` produced as well,
        /// which is review 18's third finding: see `settle_sync_outcome`.
        era: VaultEra,
    },
    /// `bw sync` succeeded, but the cache was cleared while the repopulate
    /// was in flight, so nothing local was refreshed. Not a failure of the
    /// sync and not a success for the user's purposes either.
    DiscardedStale,
    /// Starting the backend, syncing, or repopulating failed.
    Failed(String),
}

/// What the main thread should DO about a completed sync, decided by
/// `settle_sync_outcome` the moment before it acts.
///
/// A separate type from [`SyncOutcome`] on purpose. `SyncOutcome` says what
/// happened on the worker thread, minutes ago and possibly for a vault
/// session that no longer exists; this says what is to be done here and now.
/// Collapsing the two -- `settle_sync_outcome` used to take a `SyncOutcome`
/// and return a `SyncOutcome` -- is what made it possible to write a
/// re-checked outcome that still carried the worker's own stale payload, and
/// for `apply_backend_op` to act on that payload believing the re-check had
/// blessed it. Here the only variant that means "go ahead" carries the data
/// to go ahead WITH, taken from the cache under the same check, so there is
/// nothing else in scope for the apply site to reach for.
enum SettledSync {
    /// The sync is still applicable -- same vault session, cache still
    /// populated -- and `items` is the snapshot AS IT STANDS NOW: this sync's
    /// refresh plus anything written since, which is newer truth than the
    /// sync and must survive it. Rebuild the match engine from these.
    Applicable {
        items: Vec<deskwarden::vault_bridge::VaultItem>,
    },
    /// The sync refreshed nothing that is still around to act on: either its
    /// own populate was discarded on the worker thread, or a `clear` (lock,
    /// re-auth, quit) started a new epoch before the main thread got here.
    /// Touch neither the engine nor the cache.
    NothingToApply,
    /// Starting the backend, syncing, or repopulating failed.
    Failed(String),
}

/// Applies a completed [`BackendOp`]: updates `bw_serve_child` and, for a
/// `Sync`, rebuilds the match engine and reflects the outcome on the tray.
///
/// Whether a sync's outcome is still applicable at all, and what to build the
/// engine from if it is, are both decided by `settle_sync_outcome` -- see
/// there for the contract. This function never reaches into `cache` itself
/// for that data, so there is no second, unchecked route to it.
fn apply_backend_op(
    op: BackendOp,
    bw_serve_child: &mut Option<Child>,
    cache: &VaultCache,
    engine: &mut MatchEngine,
    tray: &tray::AppTray,
) {
    match op {
        BackendOp::EnsureRunning(Ok(child)) => {
            if adopt_started_child(bw_serve_child, child) {
                log::info!("bw serve started for the vault window");
            }
            // Back to idle rather than merely re-enabled (review 18's
            // Minor): the "Add app..." handler that disabled it leaves the
            // label alone, but this arm also runs for an `EnsureRunning`
            // that a lock recovery abandoned, by which point the label may
            // be a stale "Syncing..." from an earlier wedged sync.
            tray::set_sync_idle(tray);
        }
        BackendOp::EnsureRunning(Err(e)) => {
            log::error!(
                "could not start bw serve for the vault window (writes and TOTP will fail until \
                 the next attempt; reads still work from the cache): {e}"
            );
            tray::set_sync_idle(tray);
        }
        BackendOp::Sync { child, outcome } => {
            match child {
                Some(Ok(c)) => {
                    adopt_started_child(bw_serve_child, c);
                }
                Some(Err(e)) => log::error!("sync could not start bw serve: {e}"),
                None => {}
            }
            match settle_sync_outcome(outcome, cache) {
                SettledSync::Applicable { items } => {
                    // From the snapshot `settle_sync_outcome` just checked and
                    // handed over, NOT from anything this sync froze earlier:
                    // a write that landed while the sync was in flight is
                    // newer truth and has to survive it (review 18's third
                    // finding). See `settle_sync_outcome`.
                    let entries = match_entries(&items);
                    log::info!(
                        "sync complete; match engine refreshed: {} app match(es)",
                        entries.len()
                    );
                    engine.rebuild(&entries);
                    tray::set_sync_idle(tray);
                }
                SettledSync::NothingToApply => {
                    // Deliberately touches neither the engine nor the cache:
                    // whatever cleared the cache (lock, re-auth) owns both,
                    // and by now it may already have repopulated them for a
                    // *different* account -- writing this sync's result here
                    // could just as easily wipe a freshly correct engine
                    // as clear a stale one. What must not happen is the tray
                    // reporting a completed sync for a sync that refreshed
                    // nothing locally; "click to retry" is the honest label,
                    // and retrying is exactly right once the vault is
                    // unlocked again.
                    log::warn!(
                        "sync ran, but the vault was locked while its result was being applied; \
                         nothing local was refreshed"
                    );
                    tray::set_sync_failed(tray);
                }
                SettledSync::Failed(e) => {
                    log::error!("sync failed: {e}");
                    tray::set_sync_failed(tray);
                }
            }
        }
    }
}

/// Adopts a freshly started `bw serve` child into `*bw_serve_child`, unless
/// one is already tracked there and still alive -- in which case the
/// incoming `child` is stopped instead. Returns whether it was adopted.
///
/// Exists for the race the final review's lock-recovery Minor flagged:
/// `open_vault_window`'s lock-recovery path gives up waiting on an in-flight
/// backend operation after `BACKEND_OP_TIMEOUT` and starts a fresh backend of
/// its own, synchronously, right there -- but giving up does not stop the
/// background thread it was waiting on. That thread can still complete
/// afterwards and send its own `Ok(child)` through the same channel, which
/// `main`'s ordinary non-blocking drain then hands to `apply_backend_op` like
/// any other result. Applying it unconditionally -- as this used to -- would
/// silently replace `*bw_serve_child` with that late, stale handle, orphaning
/// the newer process lock recovery is actually using: `Child`'s `Drop` does
/// not kill its process, so the replaced handle would simply be gone, with
/// nothing left able to stop or restart the process it pointed to on
/// purpose. Since at most one process can hold `BW_SERVE_PORT` at a time, a
/// late arrival landing while a live child is already tracked is by
/// definition redundant (or never got as far as actually binding the port),
/// so it's stopped outright rather than risking the swap.
fn adopt_started_child(bw_serve_child: &mut Option<Child>, mut child: Child) -> bool {
    if backend_is_running(bw_serve_child) {
        log::warn!(
            "a bw serve start reported back after a backend was already running (most likely \
             abandoned during lock recovery); stopping the redundant instance instead of \
             losing track of the one already in use"
        );
        bw_serve::stop_bw_serve(&mut child);
        return false;
    }
    *bw_serve_child = Some(child);
    true
}

/// What became of a child handed to [`StartupChildHandoff::deposit`].
///
/// Returned rather than swallowed so the worker can log honestly, and so the
/// decision itself can be asserted on directly instead of being inferred from
/// whether a process happens to still be alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HandoffDeposit {
    /// Parked in the slot. Whoever claims next gets it.
    Parked,
    /// The slot had already been claimed -- the startup window hit its
    /// `WORKING_DEADLINE` and `main` moved on without waiting for this
    /// worker. Nobody will ever look in the slot again, so the child was
    /// stopped here instead of being left listening on `BW_SERVE_PORT` with
    /// no handle anywhere in the process.
    ///
    /// **A belt, not the mechanism.** Reaching this needs the worker still
    /// inside `try_start_backend` when `WORKING_DEADLINE` fires, and that
    /// call is bounded by `PORT_RELEASE_GRACE` plus a spawn -- seconds, not
    /// the minutes the deadline allows -- so in production the deposit has
    /// essentially always landed before the claim. What actually stops the
    /// orphan is the ordering: the deposit is `produce`'s first act and the
    /// claim happens before `match outcome.prepared` splits, so the deadline
    /// arm inherits a real handle. This arm is what keeps the invariant
    /// ("there is no window in which a spawned `bw serve` is owned by
    /// nobody") total rather than merely likely, and it is exercised
    /// directly by tests because the schedule will not produce it.
    StoppedAfterClaim,
    /// A second child arrived while one was still parked. Not reachable from
    /// the one production depositor (`StartupWork::produce` deposits at most
    /// once), but stopping the newcomer is the only answer that cannot orphan
    /// anything: overwriting the slot would drop the parked handle, and
    /// `Child::drop` does not kill its process.
    StoppedAsRedundant,
}

/// The state of a [`StartupChildHandoff`]'s single slot.
enum HandoffSlot {
    /// Nothing deposited yet, and nobody has given up.
    Empty,
    /// A `bw serve` is waiting to be collected.
    Parked(Child),
    /// Somebody called `claim`. The handoff is closed for good.
    Claimed,
}

/// A one-shot channel for the `bw serve` the startup window's WORKER THREAD
/// starts, separate from the worker's *result*.
///
/// **The defect this exists for.** The single startup window's `Working`
/// stage has a wall-clock deadline (`app_window::WORKING_DEADLINE`). The
/// worker's result -- `StartupWork` -- only reaches `main` if the worker
/// finishes first; when the deadline fires instead, `main` takes the `None`
/// arm and the worker's `StartupWork`, including its `Child`, is eventually
/// dropped on the worker thread and forgotten. But `StartupWork::produce`
/// spawns `bw serve` as its FIRST act and can then block for a long time
/// (the readiness probe, and an untimed `check_bw_status_details` after it),
/// so by the time the deadline fires there is very often a `bw serve`
/// listening on `BW_SERVE_PORT` whose only handle is a local in a thread
/// nobody is waiting on. `recover_from_failed_vault_wait` then asks the user
/// for their master password again, tries to start a backend, finds the port
/// held by that very process, and kills the app with
/// "Deskwarden could not start its Bitwarden backend".
///
/// The `bw serve` is spawned into a `KillOnCloseJob`, so the orphan normally
/// dies with this process and the damage stops at one wasted re-auth. It does
/// NOT when `KillOnCloseJob::new()` failed and `job` is `None`: there the
/// orphan outlives the process and every subsequent launch hits the same
/// held port until the user kills it by hand. That case is why this is a slot
/// and not a comment.
///
/// So the child is published the moment it exists, before anything that can
/// block, and ownership passes through here rather than through the result:
/// whichever side gets to `claim` first owns it, and a deposit that loses the
/// race stops the child itself rather than leaking it. There is no window in
/// which a spawned `bw serve` is owned by nobody.
struct StartupChildHandoff {
    slot: Mutex<HandoffSlot>,
}

impl StartupChildHandoff {
    fn new() -> Self {
        Self { slot: Mutex::new(HandoffSlot::Empty) }
    }

    /// Publishes a freshly spawned `bw serve`. Called on the worker thread.
    fn deposit(&self, mut child: Child) -> HandoffDeposit {
        // `unwrap_or_else(PoisonError::into_inner)`: a panic elsewhere while
        // holding this lock must not turn "there is a live `bw serve` to
        // account for" into a second panic that loses it. The slot is three
        // states and a handle; there is no invariant a poisoned lock could
        // have left half-written.
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        match &*slot {
            HandoffSlot::Empty => {
                *slot = HandoffSlot::Parked(child);
                HandoffDeposit::Parked
            }
            HandoffSlot::Claimed => {
                bw_serve::stop_bw_serve(&mut child);
                HandoffDeposit::StoppedAfterClaim
            }
            HandoffSlot::Parked(_) => {
                bw_serve::stop_bw_serve(&mut child);
                HandoffDeposit::StoppedAsRedundant
            }
        }
    }

    /// Takes whatever is parked and closes the handoff for good, so a deposit
    /// that arrives afterwards stops its own child instead of parking it
    /// where nobody will look again.
    ///
    /// Called on the main thread, on BOTH arms of `match outcome.prepared`:
    /// the worker succeeding and the deadline firing are the same question
    /// here -- "is there a `bw serve` this process started?" -- and answering
    /// it in one place is what stops the `None` arm from assuming the answer
    /// is no.
    fn claim(&self) -> Option<Child> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        match std::mem::replace(&mut *slot, HandoffSlot::Claimed) {
            HandoffSlot::Parked(child) => Some(child),
            HandoffSlot::Empty | HandoffSlot::Claimed => None,
        }
    }
}

/// Kicks off a background attempt to make sure `bw serve` is running,
/// reporting the outcome through `tx` rather than being joined -- see
/// `BackendOp`'s doc for why this can't just be awaited inline.
fn spawn_backend_start(
    session_token: String,
    job: Arc<Option<job_object::KillOnCloseJob>>,
    tx: mpsc::Sender<BackendOp>,
) {
    std::thread::spawn(move || {
        let result = try_start_backend(
            &session_token,
            job_ref(&job),
            bw_serve::PORT_RELEASE_GRACE_RESTART,
        );
        let _ = tx.send(BackendOp::EnsureRunning(result));
    });
}

/// Kicks off the tray's "Sync" item in the background: ensure the backend is
/// running, `bw sync`, then repopulate the cache.
///
/// `currently_running` is decided by the caller -- on the main thread, the
/// only place that owns `bw_serve_child` -- before this thread starts, so
/// there is no race between this thread's own start attempt and the main
/// loop's idle `stop_backend_if_idle`.
///
/// `try_start_backend` already runs `bw sync` itself as part of coming up
/// (see its doc), so this only issues a separate, explicit sync when the
/// backend was already running and therefore never got that free one.
fn spawn_sync(
    session_token: String,
    job: Arc<Option<job_object::KillOnCloseJob>>,
    cache: Arc<VaultCache>,
    currently_running: bool,
    tx: mpsc::Sender<BackendOp>,
) {
    std::thread::spawn(move || {
        // Captured before ANY fetch below, for the reason
        // `VaultCache::epoch`'s doc gives: the guard only covers the window
        // it is handed. This is the one genuinely live `DiscardedStale`
        // producer in the crate -- it runs on a background thread while the
        // main thread can call `cache.clear()` -- so unlike the other epoch
        // captures in this file it is not inert.
        let sync_epoch = cache.epoch();
        let child = if currently_running {
            None
        } else {
            Some(try_start_backend(
                &session_token,
                job_ref(&job),
                bw_serve::PORT_RELEASE_GRACE_RESTART,
            ))
        };

        // `Ok(Some(items))` when the readiness wait below already listed the
        // vault: `sync_outcome_from` reuses those rather than paying for a
        // second full-vault `list_items` (~1.1s / 1.08 MB on a 1657-item
        // vault, measured in this repo), the same reuse review 16 made on
        // the unlock path. `Ok(None)` when nothing has listed it yet.
        let start_failed = matches!(&child, Some(Err(_)));
        let ready = if start_failed {
            Err("bw serve could not be started".to_string())
        } else if currently_running {
            bw_serve::run_bw_sync(&session_token).map(|()| None)
        } else {
            // We just started `bw serve` ourselves. `try_start_backend`
            // returns as soon as the child process is resumed -- it does
            // *not* wait for `bw serve` (a bundled Node binary whose cold
            // start regularly takes several seconds) to actually be
            // listening. That gap is exactly why `wait_for_vault_ready`
            // exists and why the startup path always calls it before its
            // first `populate()`. Without the same wait here, `populate()`
            // below would very often race a backend that isn't answering
            // requests yet, fail with a connection error, and report "sync
            // failed" even though `try_start_backend`'s own `bw sync` had
            // completed successfully and the cache was never actually
            // refreshed -- precisely the mode this tray item exists for
            // (`keep_backend_running = false`, backend stopped at idle).
            // The `currently_running` branch above needs no such wait: a
            // backend that was already running before this click is, by
            // definition, already past this race.
            let schedule = readiness_schedule(READINESS_DEADLINE);
            wait_for_vault_ready(cache.bridge(), &schedule).map(Some)
        };

        let outcome = match ready {
            Err(e) => SyncOutcome::Failed(e),
            Ok(probe_items) => sync_outcome_from(&cache, sync_epoch, probe_items),
        };

        let _ = tx.send(BackendOp::Sync { child, outcome });
    });
}

/// **The freshness contract for a completed backend operation, in one
/// place.** Re-checks a sync's result against the cache as it stands NOW, on
/// the main thread, and answers with what to do about it.
///
/// There are two windows between a sync's `list_items` and the engine rebuild
/// it eventually causes, and they need OPPOSITE answers. That is the thing
/// five consecutive reviews of this seam have each rediscovered from a
/// different door, so it is written out rather than left to be re-derived:
///
///  1. **A `clear` -- lock, re-auth into a possibly different account, quit.**
///    The sync's result must be thrown away. `VaultCache::populate_with`'s own
///    epoch guard covers the worker's fetch-to-write window; this covers the
///    one after it, between the worker sending its outcome and `main` draining
///    it, which is reachable because `open_vault_window` abandons an in-flight
///    backend operation after `BACKEND_OP_TIMEOUT` and carries on into a lock
///    recovery that calls `cache.clear()`. Applied unguarded, such a late
///    arrival writes the PREVIOUS account's app matches over an engine the
///    recovery has just armed correctly for the new one -- the same
///    cross-account pairing `stand_down_after_unlock` exists to prevent, from
///    the other direction. It also covers review 17's finding, where the
///    recovery's own populate failed and left the cache legitimately empty:
///    that recovery clears first, so the epoch has moved and nothing is
///    applied to the empty cache.
///  2. **A WRITE -- an "Add app..." save, or any vault-window edit.** The
///    sync's result must NOT be thrown away, and must not overwrite the write
///    either. A write is newer truth than a fetch that predates it, and the
///    "Add app..." handler does not block on `backend_task_in_progress`
///    while its two picker windows block the main thread, so a save landing
///    inside a queued sync's window is ordinary, not exotic.
///
/// Review 18's third finding is what happens when one mechanism is asked to
/// answer both: `Refreshed` carried the entries from the sync's own
/// `list_items`, and an unchanged epoch was read as proving they were still
/// the vault. It never proved that -- writes mutate the snapshot without
/// touching the epoch, deliberately (see [`VaultEpoch`]) -- so the app match
/// the user had just spent two windows creating was rebuilt away, with two
/// success log lines and no warning.
///
/// The fix is not a second counter to detect case 2. A counter would only let
/// this discard a sync that is perfectly applicable, trading a silently lost
/// match for a silently skipped refresh. What case 2 wants is the snapshot
/// itself, at apply time -- which is also, by construction, exactly what case
/// 1 wants when it is safe: `Refreshed` is only produced when the sync's own
/// populate SUCCEEDED, so at an unchanged epoch the cache holds that sync's
/// items plus any newer writes. One question ("is this vault session still
/// the one I saw, and if so what is in it?"), asked of the crate's one
/// era-checked door: [`VaultCache::snapshot_unless_superseded`].
///
/// **Why the door and not the projection that used to sit in front of it**
/// (review 28's Important 1). This called `items_unless_superseded`, and this
/// doc called that "the one type that can answer it under a single lock" --
/// so the recorded plan to get to one checked door pointed everyone at a
/// different call site and left this one, the highest-stakes era check in the
/// crate, behind. The projection folded `Superseded` and `Unpopulated` into
/// one `None`, and the log line below then asserted a clear for a refusal it
/// had not distinguished. That line was true only as a downstream consequence
/// of two facts held in other files -- `Refreshed{era}` is emitted solely
/// after a successful populate, and only `clear` can unpopulate -- neither of
/// which this function checks or could notice changing. See
/// `sync_discard_reason`.
fn settle_sync_outcome(outcome: SyncOutcome, cache: &VaultCache) -> SettledSync {
    match outcome {
        SyncOutcome::Refreshed { era } => match cache.snapshot_unless_superseded(era) {
            // The `folders` half is dropped here, on the UI thread. See
            // `snapshot_unless_superseded`'s own doc for why that clone is
            // the accepted price of there being exactly one checked door.
            Ok(snapshot) => SettledSync::Applicable {
                items: snapshot.items,
            },
            Err(reason) => {
                log::info!("{}", sync_discard_reason(reason, era, cache.epoch().era()));
                SettledSync::NothingToApply
            }
        },
        SyncOutcome::DiscardedStale => SettledSync::NothingToApply,
        SyncOutcome::Failed(e) => SettledSync::Failed(e),
    }
}

/// The post-mortem line for a completed sync whose result could not be
/// applied, in words that match the refusal that actually happened.
///
/// Separated out and directly tested (review 28's Important 1) because the
/// single line it replaced named a `clear` unconditionally. This is a
/// post-mortem -- the whole reason to read it is to find out what happened --
/// and one that guesses the cause sends the reader looking for a lock or a
/// re-auth that never occurred.
///
/// `current_era` is read by the caller AFTER the refusal, so in the
/// `Superseded` case it is where the era has got to by now, not necessarily
/// the era that did the superseding; the line says "era X -> Y" rather than
/// claiming Y is what cleared it.
///
/// **And the `Unpopulated` arm has to respect that too** (review 30's Minor
/// 3). It used to say "in this sync's own era ({sync_era}) -- nothing started
/// a new one" while discarding `current_era` entirely, which is a stronger,
/// present-tense claim than the `Superseded` arm makes and one this function
/// had not observed: a `clear` landing between the checked read and the
/// caller's `cache.epoch().era()` falsifies it. It now splits on the parameter
/// it already takes and, when the era HAS moved, says so and says explicitly
/// that the move came after the check -- which is sound rather than a guess,
/// because `Unpopulated` is only returned when the era still matched at the
/// read.
///
/// This is NOT a second instance of the concurrent-read seam. The decision --
/// which refusal happened -- comes entirely from the single checked read;
/// `current_era` feeds a `format!` and nothing else, and the point of the
/// split is precisely to stop the string asserting more than the read saw.
fn sync_discard_reason(
    reason: VaultUnavailable,
    sync_era: VaultEra,
    current_era: VaultEra,
) -> String {
    match reason {
        VaultUnavailable::Superseded => format!(
            "a completed sync's result reached the main thread after the vault was cleared \
             (era {sync_era} -> {current_era}); discarding it rather than writing it over \
             whatever cleared it"
        ),
        VaultUnavailable::Unpopulated if current_era == sync_era => format!(
            "a completed sync's result reached the main thread with the cache holding no \
             snapshot at all, in this sync's own era ({sync_era}) -- nothing has started a new \
             one. Discarding the result rather than arming anything from an empty vault"
        ),
        VaultUnavailable::Unpopulated => format!(
            "a completed sync's result reached the main thread with the cache holding no \
             snapshot at all, in this sync's own era ({sync_era}); the cache has since reached \
             era {current_era}, which can only be a clear and which therefore landed AFTER the \
             check -- so it is not what discarded this result. Discarding the result rather \
             than arming anything from an empty vault"
        ),
    }
}

/// What the "Add app..." flow should rebuild the match engine from once the
/// picker has returned a save, or `None` to leave the engine exactly as it is.
///
/// **The `ServerOnly` half is review 28's Important 2**, and it is why this is
/// a function rather than three lines inline. `run_picker` can report a save
/// that reached the server but not the snapshot; rebuilding from that
/// snapshot arms an engine WITHOUT the match the user just spent two windows
/// creating, with cheerful log lines and nothing on screen. The snapshot is
/// still the right base -- it carries every other match, and everything
/// written since -- so the fix is to apply the one match it is missing rather
/// than to re-fetch. (This crate does not arm the engine from a request that
/// has not already succeeded; see `app.rs`'s note on the deleted
/// `refresh_match_engine`, whose three call sites three separate reviews
/// removed for exactly that reason.)
///
/// Applied by id, and ONLY by id: the match replaces the snapshot's copy of
/// that item, and if the snapshot has no such item nothing is injected. It
/// replaces rather than appends because a populate landing between the save
/// and here can perfectly well have brought the item back, and two entries for
/// one vault item would make the engine's answer depend on iteration order.
///
/// **Why the miss arms nothing, rather than pushing the matched item**
/// (review 30's Important 1 -- it used to push). The push was justified with
/// "the match is what autofill needs, not the item's existence". That is not
/// how any consumer works: `app::handle_match` resolves the id against
/// `cache.items()`, and `fill_from_vault` falls through to
/// `bridge().get_item`. A pushed entry for an id nothing can resolve therefore
/// arms a match that can only FAIL -- Prompt mode raises the anonymous "fill
/// something?" overlay whose Fill button 404s into a `log::error!` with
/// nothing on screen, Auto mode spends the same round-trip silently on every
/// foregrounding (including in save-memory mode, against a dead port), and it
/// repeats until the next sync or unlock.
///
/// And it fires exactly when the best available evidence says the item is
/// GONE. `target_item` came from `pick_vault_item` -> `load_items_for_picker`
/// -> the snapshot, in this same era, so the item WAS in the snapshot at pick
/// time; for it to be absent from a POPULATED same-era snapshot now, a
/// populate's fetch dropped it, which means it stopped existing server-side.
/// A missing match is silent, correct and self-heals at the next sync that
/// finds the item again. A firing match that never fills teaches the user to
/// distrust autofill, and in Auto mode is invisible except in the log.
///
/// **`Err` outranks all of that, for either refusal.** A `clear` inside the
/// two picker windows means this snapshot belongs to a vault session the user
/// did not just edit, and an unpopulated one is not a vault at all; arming
/// from either -- even with this flow's own match spliced in -- would replace
/// a correctly armed engine with a nearly empty one, which DISARMS autofill
/// rather than merely failing to arm the new match.
fn add_app_rebuild_source(
    snapshot: Result<VaultSnapshot, VaultUnavailable>,
    saved: &SavedAppMatch,
    target_item: &deskwarden::vault_bridge::VaultItem,
) -> Option<Vec<deskwarden::vault_bridge::VaultItem>> {
    let mut items = snapshot.ok()?.items;
    match saved.write {
        AppMatchWrite::WroteThrough => {}
        AppMatchWrite::ServerOnly => {
            // `with_app_match` rather than a hand-rolled field push, so the
            // entry the engine sees is exactly what the server was told --
            // the same function `set_app_match`'s own write-through uses,
            // for exactly that reason.
            let matched = deskwarden::vault_bridge::with_app_match(target_item, &saved.app_match);
            match items.iter().position(|i| i.id == matched.id) {
                Some(at) => items[at] = matched,
                None => log::warn!(
                    "the app match was saved to the server for vault item {}, but that item is \
                     absent from a populated snapshot of the same vault session -- the best \
                     available evidence is that it no longer exists server-side. Arming the \
                     engine with it anyway would arm a match that can only fail to fill, so the \
                     engine is armed from the snapshot as it stands and this match is not in it",
                    matched.id
                ),
            }
        }
    }
    Some(items)
}

/// [`AddAppFlow`]'s privacy boundary, and the whole reason it is a module.
///
/// A `struct` at this file's top level has FILE-VISIBLE fields: `AddAppFlow {
/// era: cache.epoch().era() }` compiles anywhere in these ~4000 lines, so a
/// future edit could re-introduce the late capture the type exists to forbid
/// (review 30's Minor 5) and still compile. Review 31's Important 2. Inside a
/// module, `era` and `cache` are private to these few lines and there is no
/// spelling of this type outside them except [`AddAppFlow::begin`].
///
/// That is a COMPILER guarantee rather than a test, which is why this finding
/// has no test of its own: nothing in a `#[test]` can observe code that does
/// not compile. It replaces the source-text guard that the identical
/// `window_era` hazard needed in `vault_window`, where the value being
/// protected is a local rather than a field.
mod add_app {
    use super::{add_app_rebuild_source, picker_ui, SavedAppMatch};
    use deskwarden::vault_bridge::VaultItem;
    use deskwarden::vault_cache::{VaultCache, VaultEra};
    use std::sync::Arc;

    /// One "Add app..." click, from the era capture to the engine rebuild.
    /// **Not** the save: `run_picker`'s Save fires in between, goes to the
    /// server through `cache.set_app_match`, and is not era-checked at all --
    /// see the paragraph on `run_picker` below.
    ///
    /// **This type exists to bind the era capture to the flow it guards**
    /// (review 30's Minor 5). Those were four unbound statements in `main`'s
    /// event loop -- `let add_app_era = cache.epoch().era();`,
    /// `pick_vault_item`, `run_picker`, and the rebuild -- and moving the
    /// capture below `pick_vault_item` reads as an innocent tidy, since
    /// `pick_vault_item` captures its own era internally. It is not: it narrows
    /// the guard from "both windows" to "the second window", which is the half
    /// that matters least, because the long user interaction a `clear` can land
    /// inside starts with the FIRST one. No test noticed, and none could -- all
    /// four `add_app_rebuild_source` tests hand it a `Result` directly and never
    /// touch the capture site.
    ///
    /// **What is actually enforced, and by what.** Three things, all by the
    /// compiler:
    ///
    ///  * The fields are private to this module, so the only way to obtain an
    ///    `AddAppFlow` in `main.rs` is [`Self::begin`] -- which captures the
    ///    era ABOVE the `?` that opens the first window, in one expression.
    ///  * [`Self::rebuild_source`] takes no cache. It reads the `Arc` the
    ///    capture was taken from, so the era and the cache it describes cannot
    ///    be a mismatched pair; before review 31 the method re-accepted a
    ///    `&VaultCache` and `flow.rebuild_source(&some_other_cache, ..)`
    ///    type-checked.
    ///  * Holding that `Arc` also means the flow keeps the cache alive for its
    ///    own lifetime, so "the era's cache" cannot be dropped mid-flow.
    ///
    /// What is deliberately NOT here is `run_picker`: it takes an owned
    /// `VaultItem` and a couple of unrelated flags, and folding it in would make
    /// this a wrapper around the event loop rather than a binding of the two
    /// statements that actually drift. The consequence is stated plainly above:
    /// the PUT this flow's save performs is outside the era check.
    pub(super) struct AddAppFlow {
        /// The vault session this click belongs to -- see `VaultEra` for why an
        /// era rather than a bare "is it populated?", and
        /// `add_app_rebuild_source` for what it outranks.
        era: VaultEra,
        /// The cache that era was read from, and the only one this flow will
        /// ever ask. See the type doc's second bullet.
        cache: Arc<VaultCache>,
    }

    impl AddAppFlow {
        /// Captures the era, THEN asks the user which vault item to attach a
        /// match to. `None` means the user cancelled that first window.
        ///
        /// Not `#[cfg(test)]`-testable: `pick_vault_item` opens a real window.
        /// What it buys is that the ordering is unrepresentable wrong rather
        /// than tested -- see the type's doc.
        pub(super) fn begin(cache: &Arc<VaultCache>) -> Option<(Self, VaultItem)> {
            let flow = Self::capture(cache.clone());
            let item = picker_ui::pick_vault_item(&flow.cache)?;
            Some((flow, item))
        }

        /// The capture itself, in ONE place: the era is read from the very
        /// `Arc` that is stored beside it, so no caller can pair an era with a
        /// cache it did not come from. `begin` is its only production caller.
        fn capture(cache: Arc<VaultCache>) -> Self {
            Self { era: cache.epoch().era(), cache }
        }

        /// `begin` minus the window, for the one test that needs a flow and
        /// cannot open a picker. It goes through `capture`, so the test
        /// exercises the same binding production does rather than assembling
        /// its own -- which is precisely the hole this module closed.
        #[cfg(test)]
        pub(super) fn begin_without_the_picker(cache: Arc<VaultCache>) -> Self {
            Self::capture(cache)
        }

        /// The crate's one era-checked read, asked with THIS flow's era against
        /// THIS flow's cache, handed to `add_app_rebuild_source`. `None` leaves
        /// the match engine exactly as it is.
        pub(super) fn rebuild_source(
            &self,
            saved: &SavedAppMatch,
            target_item: &VaultItem,
        ) -> Option<Vec<VaultItem>> {
            add_app_rebuild_source(
                self.cache.snapshot_unless_superseded(self.era),
                saved,
                target_item,
            )
        }
    }
}

use add_app::AddAppFlow;

/// Refreshes the cache from `bw serve` after a completed `bw sync` and says
/// what that achieved.
///
/// `probe_items` lets a caller that has already listed the vault (the
/// readiness wait `spawn_sync` runs when it had to start the backend itself)
/// hand those items over instead of paying for a second full-vault
/// `list_items`.
///
/// `epoch` must be captured by the caller BEFORE the sync it is reporting on
/// -- see [`VaultCache::epoch`]. This is the one genuinely live
/// `DiscardedStale` producer in the crate.
///
/// **What this deliberately does NOT return** (review 18's third finding).
/// Between review 17 and review 18 it also returned `match_entries(&items)`,
/// so that the engine was rebuilt from the fetch that produced the outcome
/// rather than from a re-read of the cache on the main thread. That closed
/// review 17's case -- a late sync rebuilding the engine from a cache a lock
/// recovery had emptied -- but by freezing data on a background thread and
/// applying it minutes later, which is how it then erased writes that landed
/// in between. Review 17's case is closed by the epoch instead (that recovery
/// calls `cache.clear()`, which starts a new epoch, so the outcome is
/// discarded before it can be applied to anything), and the engine is once
/// again built from the cache at apply time -- see `settle_sync_outcome`,
/// which owns that decision and the reasoning behind it.
fn sync_outcome_from(
    cache: &VaultCache,
    epoch: VaultEpoch,
    probe_items: Option<Vec<deskwarden::vault_bridge::VaultItem>>,
) -> SyncOutcome {
    let items = match probe_items {
        Some(items) => Ok(items),
        None => cache.bridge().list_items(),
    };
    let items = match items {
        Ok(items) => items,
        Err(e) => return SyncOutcome::Failed(format!("{e:?}")),
    };

    match cache.populate_with(items, epoch) {
        Ok(PopulateOutcome::Populated) => SyncOutcome::Refreshed { era: epoch.era() },
        Ok(PopulateOutcome::DiscardedStale) => SyncOutcome::DiscardedStale,
        Err(e) => SyncOutcome::Failed(format!("{e:?}")),
    }
}

/// What the readiness spinner says the first time it is shown for a given
/// attempt at getting the vault ready.
const SETUP_MESSAGE: &str = "Setting up your vault...";

/// What it says when it comes back after the user closed it (review 13's
/// Minor 4). Closing the window used to bring an apparently identical one
/// straight back with nothing to distinguish it, so the retry read as the
/// app ignoring the click rather than as a deliberate second attempt. Kept
/// short: this is a 320px-wide window with one line of text.
const SETUP_RETRY_MESSAGE: &str = "Still not ready -- trying once more...";

/// What it says on the wait that follows a fresh master-password sign-in
/// (`recover_from_failed_vault_wait`).
///
/// Its own message rather than `SETUP_RETRY_MESSAGE` (review 14's nit):
/// "Still not ready -- trying once more..." describes a retry of something
/// the user watched fail, but from *this* window's point of view the user
/// has just typed their master password into a fresh login and nothing has
/// been tried since. What is actually happening is a backend that was just
/// restarted under a new session coming up.
const SETUP_AFTER_SIGN_IN_MESSAGE: &str = "Signed in -- starting your vault...";

/// Outcome of [`wait_for_vault_ready_with_spinner`].
///
/// Review 12's Critical: a user closing the "setting up" window and the
/// readiness probe itself genuinely failing used to both collapse into the
/// same `Err`, even though they call for very different responses --
/// dismissal is not evidence that anything is actually broken, while a
/// failure is. Kept as its own enum, not a sentinel string stuffed inside
/// `Err`, so that distinction is enforced by the compiler at every call site
/// (an exhaustive `match`, same discipline `TotpState` uses) rather than by
/// whoever remembers to check the message text.
enum VaultReadyOutcome {
    /// The vault became ready in time.
    Ready(Vec<deskwarden::vault_bridge::VaultItem>),
    /// The user closed the spinner (title-bar X / Alt+F4) before the probe
    /// reported back.
    Dismissed,
    /// The readiness probe itself failed or timed out.
    Failed(String),
}

/// Same as `wait_for_vault_ready`, but shows a spinner window for the
/// duration instead of blocking with nothing on screen.
///
/// The worker runs fully detached (`std::thread::spawn`, not a
/// `thread::scope`d one this function has to join before returning) --
/// review 12's Important 2. With a `thread::scope`d worker, closing the
/// spinner early (`show_while` returning `None`) still left this function's
/// caller blocked -- with no window on screen at all, the exact silence this
/// module's spinner exists to prevent -- until the probe finished on its
/// own, up to the rest of `schedule`'s ~30s deadline; the probe's own
/// eventual result then had nowhere to go (the receiver had already been
/// dropped) and was thrown away regardless of whether it was actually `Ok`.
/// `vault`/`schedule` are cloned into the worker rather than borrowed for
/// the same reason: a detached thread can't borrow the caller's stack.
/// `message` is what the spinner says. It is a parameter rather than a
/// constant because the retry after a dismissal has to look *different* from
/// the window the user just closed (review 13's Minor 4): re-running this
/// with the identical wording made closing the window pop an apparently
/// identical one straight back, with nothing to explain why, and closing
/// that one jumped to a master-password prompt with no explanation either.
/// The bounded retry itself is correct and stays; only its wording changes.
fn wait_for_vault_ready_with_spinner(
    vault: &VaultBridge,
    schedule: &[Duration],
    message: &str,
) -> VaultReadyOutcome {
    let (tx, rx) = mpsc::channel();
    let worker_vault = vault.clone();
    let worker_schedule = schedule.to_vec();
    std::thread::spawn(move || {
        let _ = tx.send(wait_for_vault_ready(&worker_vault, &worker_schedule));
    });
    match loading_ui::show_while(message, rx) {
        Some(Ok(items)) => VaultReadyOutcome::Ready(items),
        Some(Err(e)) => VaultReadyOutcome::Failed(e),
        None => VaultReadyOutcome::Dismissed,
    }
}

/// Recovers from a vault-readiness wait that didn't produce a ready vault at
/// startup -- either the probe genuinely failed, or the user dismissed the
/// spinner a second time in a row (see `main`'s own call sites for the free
/// first retry a mere dismissal gets before landing here). Kills the current
/// `bw serve`, sends the user through the login flow again (a rejected
/// session is indistinguishable from a slow start at this level, so this is
/// a reasonable guess even when the real cause turns out to be something
/// else), restarts the backend, and waits for it once more -- exiting
/// fatally if that second wait also doesn't produce a ready vault. There is
/// nothing left to fall back to at this point: the tray, hotkey, and
/// window-watch thread don't exist yet, so unlike the lock-recovery path in
/// `open_vault_window` (review 12's Critical), there is no already-running
/// app for a further dismissal here to preserve.
#[allow(clippy::too_many_arguments)]
fn recover_from_failed_vault_wait(
    reason: &str,
    vault: &VaultBridge,
    schedule: &[Duration],
    bw_serve_child: &mut Option<Child>,
    session_token: &mut String,
    job: &Arc<Option<job_object::KillOnCloseJob>>,
    store: &session_store::SessionStore,
    config_dir: &std::path::Path,
    login: LoginContext<'_>,
) -> Vec<deskwarden::vault_bridge::VaultItem> {
    log::error!("{reason}");
    log::warn!("retrying once after a fresh login, in case the session was rejected");
    if let Some(child) = bw_serve_child.as_mut() {
        bw_serve::stop_bw_serve(child);
    }
    *session_token = reauthenticate(store, login);
    // The longer grace: we just killed our own `bw serve`, and the user just
    // retyped their master password. Give the socket real time to come free
    // rather than aborting on them.
    *bw_serve_child = match try_start_backend(
        session_token.as_str(),
        job_ref(job),
        bw_serve::PORT_RELEASE_GRACE_RESTART,
    ) {
        Ok(child) => Some(child),
        Err(e) => {
            log::error!("{e}");
            fatal_startup_error(&format!(
                "Deskwarden could not start its Bitwarden backend after you signed \
                 in.\n\n{e}\n\nFull details are in:\n{}",
                logging::log_file_path(config_dir).display()
            ));
        }
    };

    match wait_for_vault_ready_with_spinner(vault, schedule, SETUP_AFTER_SIGN_IN_MESSAGE) {
        VaultReadyOutcome::Ready(items) => items,
        VaultReadyOutcome::Dismissed => {
            if let Some(child) = bw_serve_child.as_mut() {
                bw_serve::stop_bw_serve(child);
            }
            fatal_startup_error(
                "Deskwarden's Bitwarden backend restarted after you signed back in, but the \
                 setup window was closed again before it was confirmed ready.\n\nRelaunch \
                 Deskwarden and give the setup window a little longer to finish.",
            );
        }
        VaultReadyOutcome::Failed(e) => {
            log::error!("{e}");
            if let Some(child) = bw_serve_child.as_mut() {
                bw_serve::stop_bw_serve(child);
            }
            fatal_startup_error(&format!(
                "Deskwarden's Bitwarden backend started but never became usable, so \
                 there is nothing to match your apps against.\n\n{e}\n\nFull details \
                 are in:\n{}",
                logging::log_file_path(config_dir).display()
            ));
        }
    }
}

/// Runs the login/unlock UI and persists the resulting session token.
/// Everything a login window this process opens needs in order to be scoped to
/// an account, carried as one `Copy` value so it can be threaded through
/// `open_vault_window` and `recover_from_failed_vault_wait` without either of
/// them growing two more parameters it does nothing with.
///
/// `account` is `None` in exactly one state --
/// `accounts::StartupAccounts::NoAccountList`, where this app has no `Account`
/// at all -- and the window then offers no Windows Hello quick unlock. It never
/// falls back to an account-less enrolment: the only key derivation available
/// without an account id is the empty KDF suffix
/// (`accounts::hello_kdf_suffix_for`'s doc), and one account sealed under that
/// is one account's master password every other account can open.
#[derive(Clone, Copy)]
struct LoginContext<'a> {
    account: Option<(&'a Path, &'a Account)>,
    first_run: bool,
}

/// **The only place a [`LoginContext`] is built**, which is the property
/// `every_login_window_this_process_opens_is_scoped_to_an_account` pins by
/// counting the struct literal.
///
/// It used to be a single `let login = ...` in `main`, and it could not stay
/// one: that binding borrows the active account for as long as it lives, and
/// an account switch takes `&mut` to the very same value. A context built at
/// each point of use borrows only across the call, which is what lets the same
/// function open a login window and then change which account this process is.
///
/// The function is the thing that keeps the guarantee the single binding was
/// there for -- no call site can quietly go back to passing `None` for the
/// account and losing Windows Hello quick unlock, because none of them says
/// `None` at all.
fn login_context<'a>(
    config_dir: &'a Path,
    active_account: Option<&'a Account>,
    // The account `accounts::resolve_startup` MINTED on this launch, if it
    // minted one. An id rather than a bool because this same value reaches
    // every login window the process opens, including the ones a switch to
    // some other account opens -- and only the minted account has never been
    // signed in to. The comparison is what gates
    // `login_ui::FIRST_RUN_NOTICE`, and it is what makes
    // `the_first_run_notice_is_offered_to_exactly_one_account` able to fail.
    first_run_account: Option<&accounts::AccountId>,
) -> LoginContext<'a> {
    LoginContext {
        account: active_account.map(|account| (config_dir, account)),
        first_run: active_account
            .zip(first_run_account)
            .is_some_and(|(account, minted)| &account.id == minted),
    }
}

/// The cancellable half of [`reauthenticate`]: runs the login window for
/// whichever account `login` names and saves the token into whichever
/// `SessionStore` it is handed.
///
/// **Both of those are the switch's**, and that is the whole reason this
/// exists rather than `reauthenticate` being reused. `reauthenticate` calls
/// `login_ui::run_login_flow`, which *exits the process* when the user closes
/// the window -- correct at startup, where there is nothing to fall back to,
/// and catastrophic for a switch, where declining means "stay on the account I
/// was already on". `run_login_flow_for` answers `None` instead, and that
/// `None` becomes [`ResettleReport::Declined`].
///
/// The store is a parameter rather than read from anywhere because
/// [`switch_to_account`] has already re-pointed it at the target account's
/// `session.bin` before the sequence runs; saving into `main`'s original store
/// would write the new account's token over the file of the account being left.
fn authenticate_for_switch(
    store: &session_store::SessionStore,
    login: LoginContext<'_>,
) -> Option<String> {
    let token = login_ui::run_login_flow_for(login.account, login.first_run)?;
    if let Err(e) = store.save(&token) {
        log::error!("failed to persist the session token for the account switched to: {e}");
    }
    Some(token)
}

fn reauthenticate(store: &session_store::SessionStore, login: LoginContext<'_>) -> String {
    let token = login_ui::run_login_flow(login.account, login.first_run);
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
    // Said once per spawn, not once per launch. `job == None` (the job object
    // could not be created -- see `main`) is the case with no backstop at
    // all: nothing kills this process's `bw serve` if we lose the handle, so
    // an orphan survives the app and holds `BW_SERVE_PORT` against every
    // later launch. Every path that can drop a handle -- most of all the
    // startup window's `WORKING_DEADLINE`, see `StartupChildHandoff` -- has
    // to be exact about ownership, and when a report says "could not start
    // its Bitwarden backend" this line is how the log says whether the
    // process now holding the port could have been ours.
    if job.is_none() {
        log::warn!(
            "spawning `bw serve` with no kill-on-close job object: if this process loses track \
             of the child, nothing will stop it and it will hold localhost:{} after we exit",
            bw_serve::BW_SERVE_PORT
        );
    }
    job_object::spawn_in_job(job, command).map_err(BackendStartError::Spawn)
}

/// Startup variant of [`try_start_backend`]: there is nothing to fall back to
/// before the main loop exists, so a failure here is fatal.
/// Seeds the cache with the `items` a readiness probe just fetched
/// (`VaultCache::populate_with`), rather than a plain `populate()` listing them
/// all over again right after -- the same request, for data that cannot have
/// changed in the instant between the two calls. Still fetches folders, since
/// nothing above needed those. This also means `items` doesn't need a separate
/// `drop()`: it becomes the cache's own storage instead of a throwaway local
/// that would otherwise keep the entire deserialized vault (potentially
/// thousands of items, each carrying a serde_json::Map "other" catch-all)
/// resident for the rest of the process's life doing nothing -- this app spends
/// nearly all its runtime idle in the tray with no window open.
///
/// A function rather than the inline block it used to be, because startup now
/// has two shapes -- the silent cached-session launch and the single window --
/// and the second fills the cache from inside an eframe frame closure. Two
/// hand-written copies would be two chances to drop the `DiscardedStale` arm,
/// which is the one that stops a cache cleared mid-populate being painted as an
/// empty vault.
fn seed_cache_at_startup(
    cache: &VaultCache,
    items: Vec<deskwarden::vault_bridge::VaultItem>,
    epoch: VaultEpoch,
) {
    match cache.populate_with(items, epoch) {
        Ok(PopulateOutcome::Populated) => {}
        Ok(PopulateOutcome::DiscardedStale) => {
            log::warn!("the vault cache was cleared during startup's populate; it stays empty")
        }
        Err(e) => log::warn!("could not populate the vault cache at startup: {e:?}"),
    }
}

/// Everything the single startup window's WORKER THREAD produces: the backend
/// it started, the vault the readiness probe fetched, and who the CLI says is
/// signed in.
///
/// All three used to happen on the main thread between the login window closing
/// and the tray appearing, with nothing on screen. Inside one window none of it
/// may run on the frame thread -- `try_start_backend` alone is regularly several
/// seconds -- so it runs here, off-thread, and the frame closure drains the
/// result. Every field is a `Result` or plain data rather than something that
/// needs the main thread: that is what `Send` is enforcing, and it is also why
/// the resettle sequence cannot be expressed this way (it borrows a
/// thread-bound `tray::AppTray` and opens two eframe windows of its own).
struct StartupWork {
    /// Whether `bw serve` started. `Err` is fatal, but not HERE -- see
    /// `produce`.
    ///
    /// **Deliberately not the `Child` itself.** The handle travels through
    /// `StartupChildHandoff` instead, published the moment the process
    /// exists rather than carried here and only delivered if the worker beats
    /// `app_window::WORKING_DEADLINE` -- see that type for the orphan this
    /// closes. Keeping a `Child` in this struct as well would give the same
    /// process two owners and put back exactly the "which copy did we
    /// forget?" question the handoff removes.
    child: Result<(), String>,
    items: Result<Vec<deskwarden::vault_bridge::VaultItem>, String>,
    details: login_ui::BwStatusDetails,
}

impl StartupWork {
    /// Runs on the worker thread. Never exits the process and never puts a
    /// window up: `try_start_backend` rather than `start_backend`, because the
    /// fatal variant shows a modal message box, and a message box raised from a
    /// worker while an eframe window owns the main thread is a dialog behind a
    /// window the user cannot reach. The failure is carried back and is just as
    /// fatal one frame later, on the thread that can show it.
    fn produce(
        token: &str,
        job: &Arc<Option<job_object::KillOnCloseJob>>,
        vault: &VaultBridge,
        schedule: &[Duration],
        handoff: &StartupChildHandoff,
    ) -> Self {
        // Published BEFORE the readiness probe and before
        // `check_bw_status_details`, both of which can block past
        // `app_window::WORKING_DEADLINE`. From this line on the child is
        // reachable from the main thread whatever happens to this one.
        let child = match try_start_backend(token, job_ref(job), bw_serve::PORT_RELEASE_GRACE) {
            Ok(child) => {
                match handoff.deposit(child) {
                    HandoffDeposit::Parked => {}
                    outcome => log::warn!(
                        "the startup window had already given up on this worker ({outcome:?}); \
                         the `bw serve` it just started was stopped here rather than left \
                         holding the port"
                    ),
                }
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        };
        let items = match &child {
            Ok(()) => wait_for_vault_ready(vault, schedule),
            // No backend, so there is nothing to probe -- and spending the whole
            // ~30s readiness deadline on a port nothing is listening on is the
            // same waste `resettle_session_with` already refuses to make.
            Err(e) => Err(format!("the vault backend did not start: {e}")),
        };
        // Last, and unconditional: the toolbar's account name and server come
        // from here, and a window that shows the vault with no idea whose it is
        // is worse than one that waited another moment for the answer.
        //
        // The BOUNDED form, and that is the whole of review's Important here.
        // The unbounded one is a bare `Command::output()`, so the only limit on
        // this line used to be `app_window::WORKING_DEADLINE` -- which bounds
        // the window, not the child. When it fired the spawn was still running
        // and the user had sat in front of a frozen spinner for the entire
        // budget, a budget whose own doc named this phase as one of the three
        // it covered. `login_ui::STATUS_DEADLINE` is what covers it now, and
        // `WORKING_DEADLINE` charges this phase that number instead of guessing
        // at `BACKEND_OP_TIMEOUT`. "Another moment" above is 30s and no longer
        // a figure of speech.
        let details = login_ui::check_bw_status_details_bounded();
        Self { child, items, details }
    }
}

/// Where a vault window's account details come from: the warm cache if it has
/// them, otherwise a fetch that runs BESIDE the window rather than in front of
/// it.
///
/// **The blocking call this replaces is the whole point.** `open_vault_window`
/// began with a synchronous `bw status` on the miss path -- a `bw` CLI
/// spawn, measured at 2.39s on the user's machine -- and it ran before eframe
/// was asked for a window, so a tray click that missed the prefetch produced
/// nothing on screen for that long. The miss is not exotic: the prefetch is
/// only a thread started at launch, and the cache is deliberately emptied on
/// every re-auth and every account switch.
///
/// `spawn` is handed the `Sender` and is expected to start a thread; it is a
/// parameter so this decision can be tested without a `bw` CLI, and so the
/// hit path can be shown to never call it. The one production caller passes a
/// `std::thread::spawn` of `check_bw_status_details`.
fn account_details_source(
    cached: Option<login_ui::BwStatusDetails>,
    spawn: impl FnOnce(mpsc::Sender<login_ui::BwStatusDetails>),
) -> vault_window::AccountDetails {
    match cached {
        Some(details) => vault_window::AccountDetails::Ready(details),
        None => {
            log::info!(
                "vault window: account details were not prefetched; fetching them on a thread \
                 and opening the window without them"
            );
            let (tx, rx) = mpsc::channel();
            spawn(tx);
            vault_window::AccountDetails::Pending(rx)
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The foreground window these tests ask the match engine about.
    ///
    /// `MatchEngine::lookup` takes a whole `ForegroundEvent` rather than the
    /// strings it reads, so that `main`'s one production call site -- inside
    /// an event loop no test can drive -- has nothing to assemble and nothing
    /// to get wrong. That applies here too: these tests are about which
    /// matches a *rebuild* armed, so the window they ask about is an ordinary
    /// titleless one, and the title rule has its own tests in `match_engine`.
    fn foreground(exe_name: &str) -> deskwarden::window_watch::ForegroundEvent {
        deskwarden::window_watch::ForegroundEvent {
            hwnd: 0x1234,
            pid: 4242,
            exe_name: exe_name.to_string(),
            title: String::new(),
        }
    }

    /// A real, short-lived child process, for exercising `backend_is_running`
    /// and `stop_backend_if_idle` against an actual `Child` without needing a
    /// real `bw serve` -- neither function cares what the process is, only
    /// whether it's alive.
    ///
    /// **`ping` is spawned directly, not as `cmd /c ping`.** Through `cmd` the
    /// `Child` is the shell and `ping` is a GRANDCHILD, so every `kill()` in
    /// these tests stopped the shell and left the `ping` running: one leaked
    /// process per use, and these tests spawn one per assertion. Measured on
    /// the test machine mid-run: 7812 live `PING` processes out of 9037
    /// processes total.
    ///
    /// That debris is what made `tasklist` cost seconds -- it enumerates the
    /// process table, and the table was the leak. `ProcessWatch` already means
    /// no test has to ask `tasklist` anything, so this no longer decides
    /// whether the suite is fast; it decides whether running the suite quietly
    /// degrades the machine it runs on. Spawning `ping` itself makes the
    /// `Child` the process under test, so `kill()` stops it and `wait()` reaps
    /// it, and the suite leaves nothing behind.
    fn long_lived_command() -> std::process::Command {
        let mut cmd = std::process::Command::new("ping");
        cmd.args(["-n", "20", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        cmd
    }

    fn quick_exit_command() -> std::process::Command {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/c", "exit", "0"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        cmd
    }

    fn kill_and_reap(child: &mut Option<Child>) {
        if let Some(c) = child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// An OS handle held on a spawned process, so that "did this exact process
    /// stop?" stays answerable after its `Child` is gone.
    ///
    /// **This exists because `is_pid_running` cannot answer that question.** A
    /// pid is not an identity: `stop_bw_serve` kills and then `wait()`s, and
    /// `wait()` closes the last handle, at which point Windows is free to hand
    /// that number to the next process that asks. Measured on this machine:
    /// spawning, reaping and re-spawning 600 processes in a tight loop reused
    /// 80 of the freed pids -- 13%. A `tasklist` lookup that lands after a
    /// reuse reports the pid as running and the assertion reads it as "the
    /// child was not stopped". That is the whole of the flake in
    /// `a_second_deposit_stops_the_newcomer_and_keeps_the_parked_child`: it
    /// only ever fails under load, because load is what fills the window
    /// between `wait()` freeing the number and `tasklist` being scheduled to
    /// look it up.
    ///
    /// A held handle closes that window at the source rather than papering
    /// over it: Windows will not recycle a pid while any handle to the process
    /// object is open, so the number this watch was opened on continues to
    /// mean the process it was opened for, and nothing else, for as long as
    /// the watch lives. The wait is then the OS's own -- blocking until the
    /// process object is signalled -- so there is no poll to be slow and no
    /// `tasklist` output to parse.
    ///
    /// Note what this does NOT change: the tests still spawn real child
    /// processes and still put them through the real `stop_bw_serve`. Only the
    /// *observation* is made unambiguous.
    struct ProcessWatch {
        handle: windows::Win32::Foundation::HANDLE,
    }

    impl ProcessWatch {
        /// Opens the watch. Must be called while the process is still alive
        /// and its `Child` still held -- that is what guarantees the pid still
        /// names the process the caller means.
        fn on(pid: u32) -> Self {
            use windows::Win32::System::Threading::{
                OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            };
            let handle = unsafe {
                OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, false, pid)
            }
            .expect("the process must still be alive when its watch is opened");
            Self { handle }
        }

        /// Blocks up to `budget` for the watched process to exit. `true` means
        /// it has; `false` means it was still running when the budget ran out.
        ///
        /// A bounded blocking wait rather than a sleep-then-look: a sleep long
        /// enough to be reliable under load is slow every other time, and one
        /// short enough to be fast is the flake again.
        fn exited_within(&self, budget: Duration) -> bool {
            use windows::Win32::Foundation::WAIT_OBJECT_0;
            use windows::Win32::System::Threading::WaitForSingleObject;
            let ms = u32::try_from(budget.as_millis()).unwrap_or(u32::MAX);
            unsafe { WaitForSingleObject(self.handle, ms) == WAIT_OBJECT_0 }
        }

        /// Whether the process is still running *right now*, with no waiting.
        fn still_running(&self) -> bool {
            !self.exited_within(Duration::ZERO)
        }
    }

    impl Drop for ProcessWatch {
        fn drop(&mut self) {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(self.handle);
            }
        }
    }

    /// How long a watch waits for a process that `stop_bw_serve` has already
    /// killed AND reaped to show as exited.
    ///
    /// It should be signalled the instant the wait begins -- `stop_bw_serve`
    /// calls `wait()`, which does not return until the process has actually
    /// terminated. The budget is generous anyway so that a scheduler that
    /// simply has not run this thread yet cannot be mistaken for a process
    /// that was never stopped, and costs nothing in the passing case because
    /// the wait returns as soon as the object is signalled.
    const STOP_OBSERVATION_BUDGET: Duration = Duration::from_secs(10);

    #[test]
    fn backend_is_running_is_true_for_a_live_child() {
        let mut child = Some(long_lived_command().spawn().unwrap());
        assert!(backend_is_running(&mut child));
        assert!(child.is_some(), "a live child must not be cleared");
        kill_and_reap(&mut child);
    }

    #[test]
    fn backend_is_running_detects_an_already_exited_child_and_clears_it() {
        // Regression test for review Minor 8: code that only checked
        // `bw_serve_child.is_none()` never noticed a `Some(dead child)` and so
        // never restarted it. `wait()` blocks until the process has actually
        // exited (not just been asked to), so the `try_wait()` inside
        // `backend_is_running` is guaranteed to see it as gone rather than
        // racing a process that hasn't finished exiting yet.
        let mut c = quick_exit_command().spawn().unwrap();
        let _ = c.wait();
        let mut child = Some(c);

        assert!(!backend_is_running(&mut child));
        assert!(
            child.is_none(),
            "a dead child must be cleared to None, not left dangling as a stale Some"
        );
    }

    #[test]
    fn backend_is_running_is_false_with_nothing_running() {
        let mut child: Option<Child> = None;
        assert!(!backend_is_running(&mut child));
    }

    #[test]
    fn stop_backend_if_idle_leaves_a_running_backend_alone_when_keeping_it() {
        let mut child = Some(long_lived_command().spawn().unwrap());
        stop_backend_if_idle(&mut child, true);
        assert!(
            backend_is_running(&mut child),
            "keep_backend_running = true must never stop the backend"
        );
        kill_and_reap(&mut child);
    }

    #[test]
    fn stop_backend_if_idle_stops_a_running_backend_in_save_memory_mode() {
        // The core of review Critical 2's fix: with no vault window open and
        // `keep_backend_running = false`, idle reconciliation must actually
        // tear the backend down rather than leaving it running forever.
        let mut child = Some(long_lived_command().spawn().unwrap());
        stop_backend_if_idle(&mut child, false);
        assert!(
            child.is_none(),
            "save-memory mode must stop bw serve once nothing needs it"
        );
    }

    #[test]
    fn stop_backend_if_idle_is_a_no_op_with_nothing_running() {
        let mut child: Option<Child> = None;
        stop_backend_if_idle(&mut child, false);
        assert!(child.is_none());
    }

    #[test]
    fn stop_backend_if_idle_clears_an_already_dead_child_too() {
        // The `backend_is_running` fix applies here too: a dead child left in
        // `Some` must not be treated as "still needs stopping" (harmless) but
        // must at least end up cleared to `None` either way.
        let mut c = quick_exit_command().spawn().unwrap();
        let _ = c.wait();
        let mut child = Some(c);

        stop_backend_if_idle(&mut child, false);
        assert!(child.is_none());
    }

    #[test]
    fn backend_task_is_wedged_is_false_while_within_the_deadline() {
        let started = Instant::now();
        assert!(!backend_task_is_wedged(started, Duration::from_secs(60)));
    }

    #[test]
    fn backend_task_is_wedged_is_true_once_the_deadline_has_passed() {
        // Regression test for final review Important 2: `run_bw_sync` has no
        // timeout of its own, so nothing else ever notices a stalled
        // operation on its own -- this predicate is what `main`'s loop uses
        // to catch it. Backdating `started` rather than sleeping keeps the
        // test instant.
        let started = Instant::now() - Duration::from_secs(120);
        assert!(backend_task_is_wedged(started, Duration::from_secs(90)));
    }

    #[test]
    fn needs_backend_start_is_true_with_nothing_running_and_no_task_in_flight() {
        assert!(needs_backend_start(&None, false));
    }

    #[test]
    fn needs_backend_start_is_false_while_a_task_is_already_in_flight() {
        // Guards against two attempts racing to bind the same port -- see
        // this fn's doc.
        let in_progress = Some((Instant::now(), BackendOpKind::EnsureRunning));
        assert!(!needs_backend_start(&in_progress, false));
    }

    #[test]
    fn needs_backend_start_is_false_when_the_backend_is_already_running() {
        assert!(!needs_backend_start(&None, true));
    }

    #[test]
    fn adopt_started_child_adopts_into_an_empty_slot() {
        let mut bw_serve_child: Option<Child> = None;
        let child = long_lived_command().spawn().unwrap();

        assert!(adopt_started_child(&mut bw_serve_child, child));
        assert!(backend_is_running(&mut bw_serve_child));
        kill_and_reap(&mut bw_serve_child);
    }

    #[test]
    fn adopt_started_child_stops_a_late_arrival_instead_of_replacing_a_live_one() {
        // Regression test for the final review's lock-recovery Minor:
        // `open_vault_window`'s lock-recovery path can give up waiting on a
        // backend operation
        // (`BACKEND_OP_TIMEOUT` expiry) and start its own fresh backend
        // before the abandoned operation's own `Ok(child)` eventually arrives
        // through `apply_backend_op`. That late arrival must not overwrite
        // the handle to the backend actually in use -- doing so would orphan
        // it (`Child::drop` does not kill its process) with nothing left able
        // to stop or restart it on purpose.
        let mut bw_serve_child = Some(long_lived_command().spawn().unwrap());
        let current_pid = bw_serve_child.as_ref().unwrap().id();

        let late_arrival = long_lived_command().spawn().unwrap();
        let late_pid = late_arrival.id();
        // Watched from before the stop, for `ProcessWatch`'s reason -- and
        // because this test shares the handoff suite's root cause: it asked
        // `tasklist` (a whole extra process, ~6s on the developer's machine)
        // about a pid nothing was keeping reserved.
        let late_watch = ProcessWatch::on(late_pid);
        let current_watch = ProcessWatch::on(current_pid);
        assert!(
            late_watch.still_running(),
            "control: the late arrival was not alive to begin with"
        );
        assert!(!adopt_started_child(&mut bw_serve_child, late_arrival));

        // The originally tracked child must still be the one in place...
        assert_eq!(
            bw_serve_child.as_ref().unwrap().id(),
            current_pid,
            "the live, already-tracked child must not be replaced"
        );
        // ...and the redundant late arrival must actually have been stopped,
        // not merely dropped (which would leave it running, untracked).
        // `adopt_started_child` routes it through `stop_bw_serve`, which
        // calls `wait()` after `kill()`, so the wait below is signalled
        // immediately in the passing case.
        assert!(
            late_watch.exited_within(STOP_OBSERVATION_BUDGET),
            "the redundant late-arriving child must be stopped, not orphaned"
        );
        // The other half, which the pid form could not state safely: the
        // child that was KEPT is still alive. A version that stopped both
        // would have satisfied the identity check above.
        assert!(
            current_watch.still_running(),
            "the already-tracked child was stopped along with the late arrival, so the handle \
             left in place names a dead backend"
        );

        kill_and_reap(&mut bw_serve_child);
    }

    // ---- the startup window's `bw serve` handoff -------------------------
    //
    // The defect: `StartupWork::produce` spawns `bw serve` as its first act
    // and then blocks -- the readiness probe, then an untimed
    // `check_bw_status_details`. If that outlasts
    // `app_window::WORKING_DEADLINE` the window closes, `main` takes the
    // `None` arm, and the worker's `StartupWork` (and its `Child`) is dropped
    // on a thread nobody joins. `Child::drop` does not kill its process, so a
    // `bw serve` was left listening on `BW_SERVE_PORT` with no handle
    // anywhere -- and `recover_from_failed_vault_wait`, running with
    // `bw_serve_child: &mut None`, stopped nothing, re-asked for the master
    // password, and then died on `PortHeld` against its own orphan.
    //
    // These drive `StartupChildHandoff` directly with ordinary long-lived
    // child processes. NOTHING here spawns `bw serve`, binds
    // `BW_SERVE_PORT`, or touches the real profile: the handoff is about
    // ownership of a `Child`, and any `Child` proves it.

    #[test]
    fn a_deposited_startup_child_is_handed_to_whoever_claims() {
        let handoff = StartupChildHandoff::new();
        let child = long_lived_command().spawn().unwrap();
        let pid = child.id();
        let watch = ProcessWatch::on(pid);

        assert_eq!(handoff.deposit(child), HandoffDeposit::Parked);

        let mut claimed = handoff.claim();
        assert_eq!(
            claimed.as_ref().map(Child::id),
            Some(pid),
            "the claimer must receive the very child the worker started"
        );
        assert!(
            watch.still_running(),
            "control: parking and claiming must not have stopped the child -- the point is to \
             hand a LIVE backend over, so recovery can stop it deliberately"
        );
        kill_and_reap(&mut claimed);
    }

    /// **The orphan, exactly.** The startup window gives up first (`claim`),
    /// and only afterwards does the worker finish spawning and deposit. There
    /// is nobody left to collect it, so the handoff must stop it here rather
    /// than park it where no one will look again.
    #[test]
    fn a_child_that_arrives_after_the_deadline_gave_up_is_stopped_not_orphaned() {
        let handoff = StartupChildHandoff::new();

        assert!(
            handoff.claim().is_none(),
            "control: the deadline fired before anything was deposited, so there was nothing \
             to adopt at that moment"
        );

        let late = long_lived_command().spawn().unwrap();
        let pid = late.id();
        // Opened before the deposit, for `ProcessWatch`'s reason: after
        // `stop_bw_serve` reaps the process the pid stops naming it, and this
        // test carries the same reuse exposure the sibling below was actually
        // bitten by.
        let watch = ProcessWatch::on(pid);
        assert!(
            watch.still_running(),
            "control: the late arrival was not alive to begin with"
        );
        assert_eq!(handoff.deposit(late), HandoffDeposit::StoppedAfterClaim);

        // `stop_bw_serve` kills and then `wait()`s, so the process is already
        // reaped by the time this runs -- the wait below is signalled
        // immediately in the passing case and only spends its budget when the
        // child really was left running.
        assert!(
            watch.exited_within(STOP_OBSERVATION_BUDGET),
            "a `bw serve` deposited after the startup window gave up must be stopped; leaving \
             it is the orphan that holds localhost:8087 against the next launch"
        );
    }

    #[test]
    fn the_startup_child_can_only_be_claimed_once() {
        let handoff = StartupChildHandoff::new();
        let mut claimed = {
            let child = long_lived_command().spawn().unwrap();
            handoff.deposit(child);
            handoff.claim()
        };
        assert!(claimed.is_some(), "control: the first claim takes the child");
        assert!(
            handoff.claim().is_none(),
            "a second claim must not hand the same process out twice -- two owners is how a \
             kill lands on a handle the other side is still using"
        );
        kill_and_reap(&mut claimed);
    }

    #[test]
    fn a_second_deposit_stops_the_newcomer_and_keeps_the_parked_child() {
        let handoff = StartupChildHandoff::new();
        let first = long_lived_command().spawn().unwrap();
        let first_pid = first.id();
        // Both watches are opened while both processes are alive and their
        // `Child`s are still held, which is what makes each pid still name the
        // process this test means by it. See `ProcessWatch`: asking `tasklist`
        // after `stop_bw_serve` has reaped the process is asking about a number
        // Windows has already put back in the pool.
        let first_watch = ProcessWatch::on(first_pid);
        assert_eq!(handoff.deposit(first), HandoffDeposit::Parked);

        let second = long_lived_command().spawn().unwrap();
        let second_pid = second.id();
        let second_watch = ProcessWatch::on(second_pid);
        assert!(
            second_watch.still_running(),
            "control: the newcomer was not alive before it was deposited, so proving it stopped \
             afterwards would prove nothing about the deposit"
        );
        assert_eq!(handoff.deposit(second), HandoffDeposit::StoppedAsRedundant);

        let mut claimed = handoff.claim();
        assert_eq!(
            claimed.as_ref().map(Child::id),
            Some(first_pid),
            "the parked child must not be overwritten -- overwriting drops its handle, and a \
             dropped `Child` keeps running"
        );
        assert!(
            second_watch.exited_within(STOP_OBSERVATION_BUDGET),
            "the redundant newcomer must be stopped"
        );
        // The other half of the same claim, and the half a pid lookup could
        // never have made safely: the child that was KEPT is still running.
        // Without this, a `deposit` that stopped both would pass.
        assert!(
            first_watch.still_running(),
            "the parked child was stopped along with the newcomer -- the claimer is handed a \
             dead backend and starts over, which is the wasted re-auth this handoff exists to \
             prevent"
        );
        kill_and_reap(&mut claimed);
    }

    #[test]
    fn an_untouched_handoff_hands_over_nothing() {
        assert!(
            StartupChildHandoff::new().claim().is_none(),
            "control: no worker ever got as far as spawning, so there is genuinely nothing to \
             adopt and recovery is right to start from scratch"
        );
    }

    /// `StartupWork::produce`'s body, bounded by the function defined
    /// immediately after `impl StartupWork` rather than by a closing brace: a
    /// `"\n}"` needle passes on LF and fails on CRLF, and `main.rs` is CRLF.
    fn the_produce_body() -> &'static str {
        let production = production_half_of_this_file();
        let after = production
            .split_once(concat!("fn produce", "("))
            .expect("`StartupWork::produce` must still exist")
            .1;
        let body = after
            .split_once(concat!("fn account_details", "_source("))
            .expect("`account_details_source` must still be the function defined after it")
            .0;
        assert!(
            body.len() < after.len(),
            "control: the split isolated a region rather than keeping the rest of the file"
        );
        assert!(
            body.contains(concat!("try_start_", "backend(token,")),
            "control: the region really is `produce`'s own body"
        );
        body
    }

    /// The wiring half of the fix. Depositing the child is only worth
    /// anything if it happens BEFORE the calls that can outlast
    /// `WORKING_DEADLINE`; deposited afterwards it is exactly as unreachable
    /// as carrying it in the return value was.
    #[test]
    fn the_startup_worker_publishes_bw_serve_before_anything_that_can_block() {
        let body = the_produce_body();
        let deposit = body
            .find(concat!("handoff.dep", "osit(child)"))
            .expect("`produce` must publish the child it started through the handoff");
        for later in [
            concat!("wait_for_vault", "_ready(vault, schedule)"),
            concat!("login_ui::check_bw_", "status_details_bounded()"),
        ] {
            let at = body
                .find(later)
                .unwrap_or_else(|| panic!("`produce` must still call `{later}`"));
            assert!(
                deposit < at,
                "`{later}` can block past `app_window::WORKING_DEADLINE`; the child must be \
                 published before it, or the deadline still finds no handle to adopt"
            );
        }
    }

    /// **The third phase is bounded, and `WORKING_DEADLINE` may say so.**
    ///
    /// `login_ui::check_bw_status_details` is a bare `Command::output()`. Left
    /// on this line it made `app_window::WORKING_DEADLINE`'s third term -- a
    /// documented `BACKEND_OP_TIMEOUT` for "an untimed `bw` spawn" -- a
    /// statement about nothing: the deadline bounded the WINDOW, the spawn ran
    /// on regardless, and the number in the doc was a guess at how long a call
    /// nobody was timing might take. `produce` must call the form that carries
    /// `login_ui::STATUS_DEADLINE`, and must not call the unbounded one.
    ///
    /// A ban paired with the positive rather than only the positive: adding a
    /// second, unbounded status call beside the bounded one would leave the
    /// positive green while restoring the unbounded phase.
    #[test]
    fn the_startup_worker_bounds_the_status_call_the_deadline_charges_for() {
        let body = the_produce_body();
        let banned = concat!("check_bw_status_details", "();");
        assert!(
            body.contains(concat!("check_bw_status_details_bo", "unded();")),
            "`produce` no longer runs its `bw status` under a deadline of its own, so \
             `app_window::WORKING_DEADLINE`'s third term is a fiction again and the user can \
             watch a frozen spinner for the whole budget"
        );
        assert!(
            !body.contains(banned),
            "`produce` calls the UNBOUNDED `check_bw_status_details()`; it is a bare \
             `Command::output()` with no timeout, and the window deadline that appears to \
             cover it does not"
        );
        // Positive control for that negative: the banned needle really does
        // match the spelling it bans, and really does NOT match the bounded
        // form -- so `!contains` passing means the unbounded call is absent
        // rather than the needle being unmatchable or matching both.
        assert!(
            concat!("let d = login_ui::check_bw_status_details", "();").contains(banned),
            "control: the banned spelling does not match itself"
        );
        assert!(
            !concat!("let d = login_ui::check_bw_status_details_bo", "unded();").contains(banned),
            "control: the ban also matches the bounded form, so it could never distinguish them"
        );
    }

    /// `StartupWork` must not also carry the handle. Two owners for one
    /// process is the ambiguity the handoff exists to remove, and a `Child`
    /// back in this struct would restore the arm-only delivery that the
    /// deadline throws away.
    #[test]
    fn the_startup_work_result_carries_no_child_handle() {
        let production = production_half_of_this_file();
        let decl = production
            .split_once(concat!("struct Startup", "Work {"))
            .expect("`StartupWork` must still exist")
            .1;
        let fields = decl
            .split_once(concat!("impl Startup", "Work {"))
            .expect("`impl StartupWork` must still follow the struct")
            .0;
        assert!(
            fields.contains(concat!("child: Result<(), ", "String>")),
            "`StartupWork::child` must report only whether the backend started; the handle \
             travels through `StartupChildHandoff`"
        );
        assert!(
            !fields.contains(concat!("Result<Child", ",")),
            "a `Child` is back in `StartupWork`, so it is once again delivered only on the arm \
             the deadline discards"
        );
    }

    /// The single startup window's region of `main`, bounded at both ends by
    /// single-line anchors that cannot drift into a neighbouring branch.
    fn the_single_window_startup_region() -> &'static str {
        let production = production_half_of_this_file();
        let after = production
            .split_once(concat!("let handoff_for", "_work = startup_child.clone();"))
            .expect("the single startup window must still hand the worker a handoff clone")
            .1;
        let region = after
            .split_once(concat!("match engine loaded with ", "{} app match(es)"))
            .expect("the region must still end at the post-startup engine log")
            .0;
        assert!(
            region.len() < after.len(),
            "control: the split isolated a region rather than keeping the rest of the file"
        );
        assert!(
            region.contains(concat!("match outcome.pre", "pared {")),
            "control: the region really is the one that decides on the worker's result"
        );
        region
    }

    /// **The whole point of the fix, as wiring.** The claim has to happen
    /// before the `Some`/`None` split, because the `None` arm is the one that
    /// used to assume no backend had been started. If it moves inside the
    /// `Some` arm, the deadline path is orphaning again with every unit test
    /// above still green.
    #[test]
    fn the_single_window_claims_the_startup_child_before_the_arms_split() {
        let region = the_single_window_startup_region();
        let claim = region
            .find(concat!("bw_serve_child = startup_child.cl", "aim();"))
            .expect("`main` must claim the startup handoff into `bw_serve_child`");
        let split = region
            .find(concat!("match outcome.pre", "pared {"))
            .expect("control: the region must still contain the split");
        assert!(
            claim < split,
            "the claim must precede `match outcome.prepared`; inside the `Some` arm it never \
             runs on the deadline path, which is the path that orphans"
        );
        let recovery = region
            .find(concat!("recover_from_failed_vault", "_wait("))
            .expect("the region must still reach the startup recovery");
        assert!(
            claim < recovery,
            "recovery's first act is `stop_bw_serve` on whatever it is handed; claiming after \
             it would hand it `None` again"
        );
        assert_eq!(
            production_half_of_this_file()
                .matches(concat!("startup_child.cl", "aim()"))
                .count(),
            1,
            "exactly one place may take ownership of the startup child; a second claim would \
             silently get `None` and conclude no backend is running"
        );
        // Claiming and then throwing the answer away is the original bug
        // wearing the fix's clothes: the `None` arm's whole defect was that
        // it went into recovery believing `bw_serve_child` was `None`.
        assert!(
            !region[claim..].contains(concat!("bw_serve_child = ", "None")),
            "nothing after the claim may put `bw_serve_child` back to `None`; recovery would \
             then stop nothing and collide with the very backend just adopted"
        );
    }

    /// **The claim's VALUE, not merely its position.** The guard above proves
    /// only where `startup_child.claim()` sits relative to the split and to
    /// recovery. Substituting the *argument* -- `&mut None` in place of
    /// `&mut bw_serve_child` at either recovery call site -- reinstates the
    /// original defect verbatim and violates nothing it checks: the claim
    /// still happens first, the assignment still happens, `bw_serve_child` is
    /// still `Some(child)`, and `&mut None` contains no `bw_serve_child =
    /// None` spelling. Recovery's `stop_bw_serve` then stops nothing,
    /// `reauthenticate` runs, `try_start_backend` meets the live orphan on
    /// `BW_SERVE_PORT`, and the process dies fatally leaving it behind to
    /// block every later launch. Both arms are equally exposed, so both are
    /// checked here.
    ///
    /// **Why a needle and not a type.** The sibling fix in `app_window.rs`
    /// closed an identical class by making the bad state unwritable, and that
    /// worked because `Closing(true)` is never a legitimate value. Here it is
    /// the opposite: `None` is a *correct* value for this slot on the
    /// deadline path -- the worker may have been killed before it ever
    /// reached `try_start_backend` and deposited anything -- so no newtype can
    /// make the empty case unrepresentable, and no constructor can be
    /// restricted to "only the claim may produce this" when three further
    /// call sites in the cached-session arm legitimately pass a slot that
    /// `start_backend` filled instead. `&mut Option<Child>` is also the shape
    /// a dozen other functions in this file already take. What remains
    /// checkable is the one thing that actually matters: that the argument at
    /// these call sites is the binding the claim wrote into, spelled out.
    #[test]
    fn startup_recovery_is_handed_the_claimed_child_on_both_arms() {
        let region = the_single_window_startup_region();
        let call = concat!("recover_from_failed_vault", "_wait(");
        let claimed = concat!("&mut bw_serve_", "child,");
        // The last argument every call site passes, and the only bound in
        // this argument list that cannot drift: it is one token on one line,
        // and no reason string or earlier argument contains it.
        let last_arg = concat!("log", "in,");

        assert_eq!(
            region.matches(call).count(),
            2,
            "control: the single-window region must still hold exactly the two recoveries this \
             guard was written for -- the `Some` arm's failed `work.items` and the deadline arm"
        );
        assert_eq!(
            region.matches(claimed).count(),
            2,
            "the claimed child must be handed to both recoveries; a count of one means one arm \
             was switched to something else and stops nothing"
        );

        let mut checked = 0;
        let mut rest = region;
        while let Some(at) = rest.find(call) {
            let after = &rest[at + call.len()..];
            let args = after
                .split_once(last_arg)
                .expect("every startup recovery call must still end its argument list at `login`")
                .0;
            assert!(
                !args.contains(call),
                "control: the argument window swallowed the next call site, so the bound drifted"
            );
            assert!(
                !args.is_empty() && args.len() < 600,
                "control: the argument window must be a real, single argument list -- got {} bytes",
                args.len()
            );
            assert!(
                args.contains(claimed),
                "a startup recovery is called with something other than `&mut bw_serve_child`; \
                 its first act is `stop_bw_serve` on whatever it is handed, so anything else \
                 (`&mut None` above all) stops nothing and leaves the claimed `bw serve` alive \
                 to hold `BW_SERVE_PORT` against the restart two lines later"
            );
            checked += 1;
            rest = after;
        }
        assert_eq!(
            checked, 2,
            "control: both recovery call sites must have been inspected, not zero or one"
        );

        // A tripwire on the whole file, so a sixth call site cannot appear
        // outside this region and escape the check above: five calls plus the
        // one `fn` definition.
        assert_eq!(
            production_half_of_this_file().matches(call).count(),
            6,
            "the number of `recover_from_failed_vault_wait` sites changed; a new one is not \
             covered by the argument check above -- extend the guard to reach it"
        );
    }

    #[test]
    fn the_startup_worker_is_given_the_same_handoff_main_claims_from() {
        let production = production_half_of_this_file();
        assert_eq!(
            production
                .matches(concat!("let startup_child = Arc::new(StartupChildHan", "doff::new())"))
                .count(),
            1,
            "there must be exactly one startup handoff, or the worker deposits into one slot \
             while `main` claims from another and the orphan is back"
        );
        assert_eq!(
            production
                .matches(concat!("let handoff_for", "_work = startup_child.clone();"))
                .count(),
            1,
            "the worker's handoff must be a clone of that same one"
        );
        let region = the_single_window_startup_region();
        assert!(
            region.contains(concat!("&handoff_for", "_work,")),
            "the worker closure must actually be passed the handoff; without the argument \
             `produce` has nowhere to publish and the deposit is unreachable"
        );
    }

    fn vault_item_with_match(id: &str, process: &str) -> String {
        format!(
            r#"{{"id":"{id}","name":"{id}","type":1,"fields":[{{"name":"deskwarden:app-match","value":"{{\"process\":\"{process}\",\"trigger\":\"auto\"}}"}}]}}"#
        )
    }

    /// The items a readiness probe would have handed back.
    fn probe_items(specs: &[(&str, &str)]) -> Vec<deskwarden::vault_bridge::VaultItem> {
        specs
            .iter()
            .map(|(id, process)| {
                serde_json::from_str(&vault_item_with_match(id, process))
                    .expect("the test fixture must deserialize as a vault item")
            })
            .collect()
    }

    /// Review 15's Important: a transient `list_folders` failure on the
    /// post-unlock repopulate must NOT disarm autofill for the rest of the
    /// session. `populate_with` still fetches folders, so a 500 on that
    /// request fails the whole populate -- but the match engine is built
    /// from the readiness probe's OWN items, a fetch already known to have
    /// succeeded against a backend just restarted with the new session, so
    /// its entries are the current account's by construction and
    /// `fill_from_vault`'s documented bridge fallback serves the fill from
    /// an empty cache.
    #[test]
    fn a_folders_failure_after_unlock_leaves_the_match_engine_armed() {
        let mut server = mockito::Server::new();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(500)
            .with_body("nope")
            .create();

        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let mut engine = MatchEngine::new();
        let epoch = cache.epoch();

        repopulate_and_refresh_after_unlock(
            &cache,
            &mut engine,
            probe_items(&[("1", "notepad.exe")]),
            epoch,
        );

        assert!(
            !cache.is_populated(),
            "the populate genuinely failed; this test is about what happens *despite* that"
        );
        assert!(
            engine.lookup(&foreground("notepad.exe")).is_some(),
            "a transient list_folders failure must not disarm autofill for the whole session -- \
             there is no periodic match-engine refresh left, so nothing would ever re-arm it"
        );
    }

    /// Review 16's Important: the engine must be armed from the readiness
    /// probe's own items even if the backend answers 500 to absolutely
    /// everything afterwards. Before this fix the engine was rebuilt by
    /// `app::refresh_match_engine` (since deleted), i.e. by a THIRD
    /// `list_items` after the
    /// probe's and the populate's, and a transient failure of that one
    /// request cleared the engine and silently disarmed autofill for the
    /// whole session -- the exact blast radius of review 15's finding, one
    /// request over.
    #[test]
    fn the_engine_is_armed_from_the_probes_items_even_if_every_later_request_fails() {
        let mut server = mockito::Server::new();
        let _items = server
            .mock("GET", "/list/object/items")
            .with_status(500)
            .with_body("nope")
            .create();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(500)
            .with_body("nope")
            .create();

        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let mut engine = MatchEngine::new();
        let epoch = cache.epoch();

        repopulate_and_refresh_after_unlock(
            &cache,
            &mut engine,
            probe_items(&[("1", "notepad.exe")]),
            epoch,
        );

        assert!(
            engine.lookup(&foreground("notepad.exe")).is_some(),
            "the engine's arming must depend only on the fetch already known to have succeeded, \
             so no later backend failure can disarm autofill for the rest of the session"
        );
    }

    /// The other half of the same invariant: entries the engine is holding
    /// from the account this app was signed into BEFORE the unlock must not
    /// survive into a session that may be a different account. The probe's
    /// items are the new account's, so rebuilding from them replaces the old
    /// ones outright -- including when the new account has no app matches at
    /// all, which is the case that would otherwise leave stale ones armed.
    #[test]
    fn matches_from_the_pre_lock_account_do_not_survive_the_unlock() {
        let mut server = mockito::Server::new();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(500)
            .with_body("nope")
            .create();

        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let mut engine = MatchEngine::new();
        engine.rebuild(&[(
            "old".to_string(),
            deskwarden::app_match::AppMatch::for_process("notepad.exe", deskwarden::app_match::TriggerMode::Auto),
        )]);
        let epoch = cache.epoch();

        repopulate_and_refresh_after_unlock(&cache, &mut engine, probe_items(&[]), epoch);

        assert!(
            engine.lookup(&foreground("notepad.exe")).is_none(),
            "matches from the account this app was signed into before the unlock must not \
             survive an unlock whose own vault does not have them"
        );
    }

    /// The cache seeding reuses the probe's items too, so a successful
    /// populate needs only the folders request -- no second `list_items`.
    #[test]
    fn the_cache_is_seeded_from_the_probes_items_without_listing_them_again() {
        let mut server = mockito::Server::new();
        let items = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[]}}"#)
            .expect(0)
            .create();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[]}}"#)
            .create();

        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let mut engine = MatchEngine::new();
        let epoch = cache.epoch();

        repopulate_and_refresh_after_unlock(
            &cache,
            &mut engine,
            probe_items(&[("1", "notepad.exe")]),
            epoch,
        );

        assert!(cache.is_populated(), "the populate must have succeeded");
        assert_eq!(cache.items().len(), 1, "seeded from the probe's own items");
        items.assert();
    }

    /// A scripted stand-in for `wait_for_vault_ready_with_spinner`, recording
    /// the message each probe was asked to show so a test can assert both
    /// HOW MANY probes ran and that the retry looked different from the
    /// window the user just closed.
    fn scripted_probe<'a>(
        script: Vec<VaultReadyOutcome>,
        seen: &'a std::cell::RefCell<Vec<&'static str>>,
    ) -> impl FnMut(&'static str) -> VaultReadyOutcome + 'a {
        let mut remaining = script.into_iter();
        move |message| {
            seen.borrow_mut().push(message);
            remaining
                .next()
                .expect("the lock recovery must not probe more times than the script allows")
        }
    }

    /// Review 17's Critical: closing the post-unlock spinner is ONE CLICK,
    /// the gesture review 12 already ruled must not be destructive, and it
    /// used to disarm autofill for the rest of the session -- the detached
    /// readiness worker was very likely about to answer `Ok(items)`, and
    /// that answer was thrown away. Startup gives a dismissal one free probe
    /// (`SETUP_RETRY_MESSAGE`); this site now gives the same one, and a
    /// probe that then succeeds takes the ordinary `Ready` path.
    #[test]
    fn a_dismissed_spinner_after_unlock_gets_one_free_readiness_retry() {
        let mut server = mockito::Server::new();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[]}}"#)
            .create();

        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let mut engine = MatchEngine::new();
        let epoch = cache.epoch();
        let seen = std::cell::RefCell::new(Vec::new());

        settle_vault_after_unlock(
            &cache,
            &mut engine,
            epoch,
            scripted_probe(
                vec![
                    VaultReadyOutcome::Dismissed,
                    VaultReadyOutcome::Ready(probe_items(&[("1", "notepad.exe")])),
                ],
                &seen,
            ),
        );

        assert!(
            engine.lookup(&foreground("notepad.exe")).is_some(),
            "a dismissal followed by a successful retry must arm the engine exactly as a \
             first-probe success does -- otherwise one impatient click kills autofill for the \
             whole session with no recovery the user would ever think to try"
        );
        assert!(
            cache.is_populated(),
            "the retry's items must seed the cache too, i.e. the ordinary Ready path"
        );
        assert_eq!(
            *seen.borrow(),
            vec![SETUP_MESSAGE, SETUP_RETRY_MESSAGE],
            "the retry has to look different from the window the user just closed \
             (review 13's Minor 4)"
        );
    }

    /// The retry is bounded exactly as startup's is: a dismissal buys one
    /// more probe and nothing more. Two calls, structurally -- not a loop.
    #[test]
    fn a_second_dismissal_after_unlock_stands_autofill_down_without_looping() {
        let cache = VaultCache::new(VaultBridge::new("http://127.0.0.1:1".to_string()));
        let mut engine = MatchEngine::new();
        engine.rebuild(&[(
            "old".to_string(),
            deskwarden::app_match::AppMatch::for_process("notepad.exe", deskwarden::app_match::TriggerMode::Auto),
        )]);
        let epoch = cache.epoch();
        let seen = std::cell::RefCell::new(Vec::new());

        settle_vault_after_unlock(
            &cache,
            &mut engine,
            epoch,
            scripted_probe(
                vec![VaultReadyOutcome::Dismissed, VaultReadyOutcome::Dismissed],
                &seen,
            ),
        );

        assert_eq!(
            seen.borrow().len(),
            2,
            "exactly one retry -- the scripted probe panics if a third is asked for"
        );
        assert!(
            engine.lookup(&foreground("notepad.exe")).is_none(),
            "nothing confirmed the backend, so the engine can only be holding the PRE-lock \
             account's matches and a locked app must be inert (review 13's Minor 3)"
        );
    }

    /// Review 17's Minor: a readiness TIMEOUT is a transient condition, and
    /// it used to call `fatal_startup_error` -- killing the tray, the
    /// hotkey, autofill and window-watching over a ~30s probe that did not
    /// answer, at a call site that is not startup and has an already-running
    /// app to preserve. It now stands down exactly as a dismissal does. That
    /// this test can run at all is the assertion: the old arm called
    /// `std::process::exit(1)`.
    #[test]
    fn a_readiness_timeout_after_unlock_leaves_the_app_running() {
        let cache = VaultCache::new(VaultBridge::new("http://127.0.0.1:1".to_string()));
        let mut engine = MatchEngine::new();
        engine.rebuild(&[(
            "old".to_string(),
            deskwarden::app_match::AppMatch::for_process("notepad.exe", deskwarden::app_match::TriggerMode::Auto),
        )]);
        let epoch = cache.epoch();
        let seen = std::cell::RefCell::new(Vec::new());

        settle_vault_after_unlock(
            &cache,
            &mut engine,
            epoch,
            scripted_probe(
                vec![VaultReadyOutcome::Failed("timed out".to_string())],
                &seen,
            ),
        );

        assert_eq!(
            seen.borrow().len(),
            1,
            "a genuine timeout has already spent the whole readiness deadline; it does not buy \
             another one (startup's Failed arm does not retry either)"
        );
        assert!(
            engine.lookup(&foreground("notepad.exe")).is_none(),
            "same stand-down as a dismissal: empty cache, empty engine, app alive and locked"
        );
    }

    /// Review 18's Important, and the twin of the test above. Commit 7041360
    /// made the readiness TIMEOUT survivable on the argument that a transient
    /// failure must not kill tray, hotkey, autofill and window-watching when
    /// there is a running app to preserve -- and then left the
    /// `try_start_backend` failure twenty lines earlier calling
    /// `fatal_startup_error`. That failure is *more* likely to be transient,
    /// not less: its dominant shape is `PortHeld`, and this very call site
    /// killed its own `bw serve` moments before, so a port that has not been
    /// released yet is the EXPECTED case -- which is exactly why
    /// `PORT_RELEASE_GRACE_RESTART` exists and why `try_start_backend`'s own
    /// doc says it returns the failure "instead of exiting, because on the
    /// restart paths (and especially the one right after the user retyped
    /// their master password) killing the whole app over a socket that needs
    /// another second to close is far worse". Only the caller disagreed.
    ///
    /// It now stands down through the same `stand_down_after_unlock` the
    /// readiness arms use, so there is one place that decides what "we could
    /// not get the vault back" looks like. That this test runs at all is
    /// again part of the assertion: the old arm called `std::process::exit(1)`
    /// and would have taken the test runner with it.
    #[test]
    fn a_backend_that_cannot_be_restarted_after_unlock_leaves_the_app_running() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[(
            "old".to_string(),
            deskwarden::app_match::AppMatch::for_process("notepad.exe", deskwarden::app_match::TriggerMode::Auto),
        )]);

        let child = restart_backend_after_unlock(&mut engine, || {
            Err(BackendStartError::PortHeld(Duration::from_secs(1)))
        });

        assert!(
            child.is_none(),
            "there is no child to track -- and `bw_serve_child` must stay None so the next open \
             starts one rather than talking to a process nothing owns"
        );
        assert!(
            engine.lookup(&foreground("notepad.exe")).is_none(),
            "nothing confirmed the backend under the NEW session, so the engine can only hold \
             the pre-lock account's matches: same stand-down the readiness arms produce, not an \
             exit and not a silently armed engine"
        );
    }

    /// The other half, so the fix above cannot pass by standing down
    /// unconditionally: a start that succeeds hands the child straight back
    /// and touches nothing. The engine it leaves alone is about to be rebuilt
    /// by the readiness probe on the ordinary path.
    #[test]
    fn a_backend_that_does_restart_after_unlock_is_handed_back_untouched() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[(
            "old".to_string(),
            deskwarden::app_match::AppMatch::for_process("notepad.exe", deskwarden::app_match::TriggerMode::Auto),
        )]);

        let started = restart_backend_after_unlock(&mut engine, || {
            std::process::Command::new("cmd")
                .args(["/C", "exit"])
                .spawn()
                .map_err(BackendStartError::Spawn)
        });

        let mut child = started.expect("a successful start must hand its child back");
        let _ = child.wait();
        assert!(
            engine.lookup(&foreground("notepad.exe")).is_some(),
            "a successful restart must not stand autofill down"
        );
    }

    /// The state a resettle is handed: a cache populated from one account and
    /// an engine armed from it, plus the toolbar strings that account's
    /// `bw status` produced. Returns the mock server too, because dropping it
    /// unmounts the mocks the repopulate below will need.
    fn an_app_signed_into_one_account() -> (
        mockito::ServerGuard,
        VaultCache,
        MatchEngine,
        Option<login_ui::BwStatusDetails>,
    ) {
        let server = sync_server();
        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let mut engine = MatchEngine::new();
        let epoch = cache.epoch();
        repopulate_and_refresh_after_unlock(
            &cache,
            &mut engine,
            probe_items(&[("1", "prev.exe")]),
            epoch,
        );
        let details = Some(login_ui::BwStatusDetails {
            status: login_ui::BwStatus::Unlocked,
            user_email: Some("prev@example.com".to_string()),
            server_url: Some("https://prev.example.com".to_string()),
        });
        (server, cache, engine, details)
    }

    /// The ORDER inside the resettle is the whole safety property, and until
    /// this test nothing pinned it. The four tests the plan names as this
    /// refactor's safety net all drive the PIECES --
    /// `repopulate_and_refresh_after_unlock`, `settle_vault_after_unlock`,
    /// `restart_backend_after_unlock` -- and not one of them could tell
    /// "clear the cache, then authenticate" from "authenticate, then clear
    /// the cache". Inlined in `open_vault_window`, the sequence had nowhere
    /// to be tested from; that is exactly what the extraction buys.
    ///
    /// The difference the assertion below is protecting is not academic. Run
    /// the other way round, the user is retyping a master password -- for a
    /// DIFFERENT account, once switching exists -- while the outgoing
    /// account's items are still live in the cache behind the prompt, and
    /// autofill is still armed from them. And the backend must be started
    /// from the token the authentication just produced, not the one it
    /// replaced, or `bw serve` comes up under a session that was already
    /// gone.
    ///
    /// So each injected closure records the step it is and what it can see of
    /// the app at the moment it runs, and the whole sequence is one
    /// assertion.
    #[test]
    fn the_resettle_authenticates_after_the_teardown_and_settles_after_the_start() {
        let (_server, cache, mut engine, mut cached_status_details) =
            an_app_signed_into_one_account();
        assert!(
            cache.is_populated() && engine.lookup(&foreground("prev.exe")).is_some(),
            "control: the app really is holding the previous account's vault and matches, so \
             the assertions below are about them being torn down rather than never existing"
        );

        let mut bw_serve_child = None;
        let mut session_token = "old-token".to_string();
        let steps = std::cell::RefCell::new(Vec::new());

        let outcome = resettle_session_with(
            &cache,
            &mut engine,
            &mut bw_serve_child,
            &mut cached_status_details,
            &mut session_token,
            || {
                steps
                    .borrow_mut()
                    .push(format!("authenticate, cache populated: {}", cache.is_populated()));
                Some("new-token".to_string())
            },
            |token| {
                steps.borrow_mut().push(format!("start, token: {token}"));
                std::process::Command::new("cmd")
                    .args(["/C", "exit"])
                    .spawn()
                    .map_err(BackendStartError::Spawn)
            },
            |_message| {
                steps.borrow_mut().push("probe".to_string());
                VaultReadyOutcome::Ready(probe_items(&[("2", "next.exe")]))
            },
        );

        assert_eq!(outcome, ResettleOutcome::BackendStarted);
        assert_eq!(
            steps.into_inner(),
            vec![
                "authenticate, cache populated: false".to_string(),
                "start, token: new-token".to_string(),
                "probe".to_string(),
            ],
            "the sequence is teardown -> authenticate -> start -> settle, and each step must \
             see the previous one's work: nothing authenticates over a live cache, and nothing \
             starts a backend from the session the authentication just replaced"
        );
        assert_eq!(session_token, "new-token");
        assert!(
            cached_status_details.is_none(),
            "the toolbar's cached email and server belong to the account that just went away; \
             left behind, the next open shows it under the new session"
        );
        assert!(bw_serve_child.is_some(), "the started backend must be tracked");
        assert!(
            engine.lookup(&foreground("next.exe")).is_some() && engine.lookup(&foreground("prev.exe")).is_none(),
            "the engine is rebuilt from the probe's items, so the incoming account's matches \
             are armed and the outgoing account's are gone"
        );
        assert!(cache.is_populated(), "and the cache is refilled from the same items");

        kill_and_reap(&mut bw_serve_child);
    }

    /// The plan's Step 8.3. `authenticate` answering `None` is the one arm
    /// the inlined block had no way to express -- `reauthenticate` exits the
    /// process rather than return a failure -- and it is how a declined
    /// account switch arrives: the user closed the master-password prompt.
    ///
    /// Nothing is started for a session nobody authenticated, and the app is
    /// left in the state `stand_down_after_unlock` already ships and the tray
    /// already names a recovery for. The outgoing token is deliberately still
    /// here: it is what a switch rolls back with, and clearing it would cost
    /// the user a second password prompt for an account they never asked to
    /// leave.
    #[test]
    fn a_declined_authentication_starts_no_backend_and_leaves_the_cache_cleared() {
        let (_server, cache, mut engine, mut cached_status_details) =
            an_app_signed_into_one_account();
        assert!(
            cache.is_populated() && engine.lookup(&foreground("prev.exe")).is_some(),
            "control: the previous account's vault and matches are really here to be torn down"
        );

        let mut bw_serve_child = None;
        let mut session_token = "old-token".to_string();

        let outcome = resettle_session_with(
            &cache,
            &mut engine,
            &mut bw_serve_child,
            &mut cached_status_details,
            &mut session_token,
            || None,
            |_token| panic!("a declined authentication must not start a backend"),
            |_message| panic!("a declined authentication must not open a readiness spinner"),
        );

        assert_eq!(outcome, ResettleOutcome::BackendNotStarted);
        assert!(
            engine.lookup(&foreground("prev.exe")).is_none(),
            "the previous account's matches survived a declined authentication: a matched \
             process would still raise the prompt, and the fill could only ever end in an error"
        );
        assert!(!cache.is_populated());
        assert!(bw_serve_child.is_none());
        assert!(
            cached_status_details.is_none(),
            "a declined authentication left the previous account's address in the toolbar"
        );
        assert_eq!(
            session_token, "old-token",
            "the outgoing token is what a rollback re-authenticates from; a decline must not \
             spend it"
        );
    }

    /// The other failing arm, and the pair to the test above: the session WAS
    /// authenticated and the backend still would not come up. There is
    /// nothing left to probe once no backend came up, so the ~30s readiness
    /// deadline is not spent on a port nothing is listening on -- the probe
    /// closure below fails the test if it runs, and
    /// `the_resettle_authenticates_after_the_teardown_and_settles_after_the_start`
    /// is the control that it does run when a backend did start.
    #[test]
    fn a_backend_that_will_not_start_is_never_probed_and_reports_it() {
        let (_server, cache, mut engine, mut cached_status_details) =
            an_app_signed_into_one_account();
        let mut bw_serve_child = None;
        let mut session_token = "old-token".to_string();

        let outcome = resettle_session_with(
            &cache,
            &mut engine,
            &mut bw_serve_child,
            &mut cached_status_details,
            &mut session_token,
            || Some("new-token".to_string()),
            |_token| Err(BackendStartError::PortHeld(Duration::from_secs(1))),
            |_message| panic!("there is nothing to probe once no backend came up"),
        );

        assert_eq!(outcome, ResettleOutcome::BackendNotStarted);
        assert!(bw_serve_child.is_none());
        assert!(
            engine.lookup(&foreground("prev.exe")).is_none(),
            "nothing confirmed a backend under the new session, so the engine can only be \
             holding the previous account's matches"
        );
        assert_eq!(
            session_token, "new-token",
            "the re-authentication itself succeeded, and a later tray Sync starts a backend \
             from this token: throwing it away would make the named recovery fail"
        );
    }

    /// **What the vault window is opened WITH, now that it is never opened
    /// LATE.**
    ///
    /// The user's own two measurements of the same click: `account details
    /// ready in 300ns` (the prefetch had landed) and `account details ready in
    /// 2.3911946s` (it had not, so `open_vault_window` spawned the `bw` CLI
    /// and waited) -- the second of those 2.39 seconds spent before eframe was
    /// asked for a window, i.e. with nothing on screen at all. The window does
    /// not wait any more; these tests are what says so.
    mod the_account_details_a_window_opens_with {
        use super::*;

        fn some_details(email: &str) -> login_ui::BwStatusDetails {
            login_ui::BwStatusDetails {
                status: login_ui::BwStatus::Unlocked,
                user_email: Some(email.to_string()),
                server_url: Some("https://vault.example.eu".to_string()),
            }
        }

        /// The hit: the prefetch (or the last window's own result) already has
        /// them, so nothing is fetched and the toolbar is complete on the
        /// first frame.
        #[test]
        fn a_warm_cache_is_handed_to_the_window_ready_and_starts_no_fetch() {
            let mut fetches = 0;
            let source =
                account_details_source(Some(some_details("held@example.eu")), |_tx| fetches += 1);

            assert_eq!(
                fetches, 0,
                "a cache hit still started a `bw status` fetch -- the spawn this change was \
                 made to avoid, now paid on every open instead of only on a miss"
            );
            match source {
                vault_window::AccountDetails::Ready(details) => assert_eq!(
                    details.user_email.as_deref(),
                    Some("held@example.eu"),
                    "the window was handed details that are not the cached ones"
                ),
                vault_window::AccountDetails::Pending(_) => panic!(
                    "a cache HIT was handed to the window as pending: the toolbar would open \
                     blank and fill in later even though the answer was already in hand"
                ),
            }
        }

        /// The miss, which is the whole bug: it must produce a CHANNEL, not an
        /// answer. An answer here can only have been obtained by blocking.
        #[test]
        fn a_miss_hands_the_window_a_channel_and_leaves_the_fetch_to_a_thread() {
            let (started_tx, started_rx) = mpsc::channel();
            let answer = some_details("fetched@example.eu");
            let source = account_details_source(None, move |tx| {
                std::thread::spawn(move || {
                    let _ = started_tx.send(());
                    let _ = tx.send(answer);
                });
            });

            let rx = match source {
                vault_window::AccountDetails::Pending(rx) => rx,
                vault_window::AccountDetails::Ready(_) => panic!(
                    "the miss path answered with the details themselves, which it can only do \
                     by WAITING for the `bw` CLI -- the window is opened seconds late again, \
                     with nothing on screen while it waits"
                ),
            };
            assert!(
                started_rx.recv_timeout(Duration::from_secs(5)).is_ok(),
                "the miss path never started the fetch at all, so the toolbar would stay blank \
                 for the whole session and the next open would be cold too"
            );
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(5))
                    .expect("the fetch's answer never reached the window's channel")
                    .user_email
                    .as_deref(),
                Some("fetched@example.eu"),
                "the channel handed to the window does not carry what the fetch produced"
            );
        }

        /// `open_vault_window`'s own body, depth-counted from its signature to
        /// its closing brace.
        ///
        /// Sliced rather than checked against the whole file, because the
        /// whole file legitimately contains other synchronous status calls --
        /// the startup prefetch thread, and `StartupWork::produce`, which runs
        /// on the worker `app_window` spawns. Neither is on the path a window
        /// opens from; this function is.
        fn open_vault_window_body() -> &'static str {
            let production = production_half_of_this_file();
            let at = production
                .find(concat!("fn open_vault_", "window("))
                .expect("no `open_vault_window` in this file -- the vault window's one door");
            let after = &production[at..];
            let open = after.find('{').expect("`open_vault_window` has no body to slice");
            let after_open = &after[open + 1..];

            let mut depth = 1usize;
            for (offset, ch) in after_open.char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            let body = &after_open[..offset];
                            assert!(
                                body.contains("handing off to eframe"),
                                "the sliced body is not `open_vault_window`: every assertion \
                                 over it would be about the wrong code"
                            );
                            assert!(
                                !body.contains(concat!("impl Startup", "Work {")),
                                "the slice ran past the end of `open_vault_window` and swept \
                                 up the startup worker, whose own status call is legitimately \
                                 synchronous"
                            );
                            return body;
                        }
                    }
                    _ => {}
                }
            }
            panic!("`open_vault_window`'s body is never closed");
        }

        /// Every synchronous `bw status` call left on the path a window opens
        /// from is inside a thread spawn.
        ///
        /// The runtime tests above check the DECISION; this checks that no
        /// call site quietly makes it again on the thread that has not opened
        /// the window yet. Written as "every occurrence, nearest preceding
        /// spawn" rather than a count of call sites, because a second one
        /// would be just as acceptable -- on a thread.
        /// `()` in the needle, so `check_bw_status_details_in(` -- a different
        /// function, called from the account flows -- is not matched by the
        /// prefix.
        fn status_call() -> &'static str {
            concat!("check_bw_status", "_details()")
        }

        /// Whether the status call at `at` is inside a thread spawn.
        ///
        /// A 200-byte backward window, not "anywhere earlier in the file":
        /// anywhere-earlier is satisfied by any of the eight unrelated spawns
        /// in `main`, which says nothing about this call. Both call sites
        /// spell `spawn(move || { let _ = tx.send(` before reaching it, which
        /// is about 60 bytes. Exercised against a fabricated blocking snippet
        /// below, so the window is known to be able to answer `false`.
        fn is_inside_a_spawn(source: &str, at: usize) -> bool {
            let spawn = concat!("std::thread::", "spawn(move ||");
            let before = &source[..at];
            match before.rfind(spawn) {
                Some(nearest) => before.len() - nearest < 200,
                None => false,
            }
        }

        #[test]
        fn no_status_call_in_this_file_runs_on_the_thread_a_window_opens_from() {
            let body = open_vault_window_body();
            let call = status_call();

            let sites: Vec<usize> = body.match_indices(call).map(|(at, _)| at).collect();
            assert_eq!(
                sites.len(),
                1,
                "expected exactly the one on-open fetch to call {call:?} inside \
                 `open_vault_window`; found {} -- none at all means the miss path stopped \
                 fetching and the cache can never warm up again",
                sites.len()
            );
            for at in sites {
                assert!(
                    is_inside_a_spawn(body, at),
                    "a {call:?} call at byte {at} is not inside a thread spawn: it spawns the \
                     `bw` CLI on whichever thread reached it, which for `open_vault_window` is \
                     the thread that has not opened the window yet -- 2.4 seconds of nothing \
                     on screen"
                );
            }
        }

        /// The control for the test above, and the reason its window is 200
        /// bytes rather than "somewhere earlier": the same predicate, given
        /// the shape this change REMOVED, has to answer `false`.
        #[test]
        fn the_spawn_check_rejects_the_blocking_call_this_change_removed() {
            let call = status_call();
            let blocking = format!(
                "let status_details = match cached_status_details.take() {{ Some(d) => d, None \
                 => login_ui::{call} }};"
            );
            let at = blocking.find(call).expect("the fabricated snippet lost its call");
            assert!(
                !is_inside_a_spawn(&blocking, at),
                "control: the predicate accepts a plain synchronous call, so the test above \
                 would pass against exactly the code it exists to forbid"
            );

            // And the positive half: a spawned call is accepted, so a `false`
            // from the predicate means something.
            let spawned = format!(
                "std::thread::spawn(move || {{ let _ = tx.send(login_ui::{call}); }});"
            );
            let at = spawned.find(call).expect("the fabricated snippet lost its call");
            assert!(
                is_inside_a_spawn(&spawned, at),
                "control: the predicate rejects even a spawned call, so it can never be \
                 satisfied and asserts nothing"
            );
        }

        /// The cache is still refilled with what the open used -- and refilled
        /// EARLY ENOUGH that a switch or a lock can still empty it.
        ///
        /// Both of those run `resettle_session`, which sets
        /// `cached_status_details` to `None` on purpose so the next window
        /// re-fetches rather than showing the account the user just left.
        /// Refilling after them would put a stale email in the toolbar of a
        /// window signed into somebody else; refilling not at all would make
        /// every open pay the spawn this change moved off the critical path,
        /// which is a slower app rather than a wrong one and is therefore the
        /// second assertion, not the first.
        #[test]
        fn the_result_refills_the_cache_before_a_switch_or_a_lock_can_empty_it() {
            let production = production_half_of_this_file();
            let refill = concat!("if let Some(details) = result.account", "_details.clone() {");
            // Re-declared rather than shared with
            // `the_switch_the_vault_window_asks_for`: the same needle in two
            // modules is two chances to notice it moved, and a helper made
            // `pub(super)` for one caller is a wider door than this needs.
            let lock = concat!("if result.locked ", "|| result.needs_reauth {");

            let refill_at = production.find(refill).unwrap_or_else(|| {
                panic!(
                    "{refill:?} is not in the production code: nothing refills the cache from \
                     the window's own details, so every open pays the `bw status` spawn again"
                )
            });
            let lock_at = production.find(lock).unwrap_or_else(|| {
                panic!("{lock:?} is not in the production code -- this guard needs it to say \
                        which side of the recovery the refill is on")
            });
            assert_ne!(
                refill_at, lock_at,
                "positive control: the two needles are two distinct positions"
            );
            assert!(
                refill_at < lock_at,
                "the cache is refilled AFTER the lock/re-auth recovery, which deliberately \
                 empties it -- so the next window opens showing the account whose session was \
                 just torn down"
            );
        }
    }

    /// The `main.rs` half that is production code. `mod tests` drives
    /// `settle_vault_after_unlock` three times on purpose, so the plan's
    /// "two occurrences in `main.rs`" cannot be read off the whole file.
    fn production_half_of_this_file() -> &'static str {
        let source = include_str!("main.rs");
        let production = source
            .split_once(concat!("mod te", "sts {"))
            .expect("main.rs must still have its test module")
            .0;
        assert!(
            production.len() < source.len(),
            "control: the split really cut the test module off the end"
        );
        production
    }

    /// **The switcher's production wiring**, which is where every previous
    /// task in this feature stopped.
    ///
    /// `switch_to_account`, `add_account` and `remove_account` were each
    /// shipped complete, correct and `#[allow(dead_code)]`; four tasks in a row
    /// recorded that nothing proved the injected `resettle` closure would call
    /// `resettle_session`, because there was no production closure to look at.
    /// Task 14 wrote the first one, and it is in `open_vault_window` -- a
    /// function no test in this crate can call, since it opens a real eframe
    /// window. So it is sliced and read.
    ///
    /// The needles are `concat!`-split and single-line, for this file's usual
    /// two reasons: a whole literal matches its own declaration, and a needle
    /// carrying a newline passes on LF and fails on CRLF.
    mod the_switch_the_vault_window_asks_for {
        use super::production_half_of_this_file;

        /// Where `open_vault_window` reads the account menu's answers: the one
        /// `if` that all three of switch, add and remove sit inside, and that
        /// holds the single `resettle` closure they share.
        fn account_actions_request() -> String {
            concat!(
                "if result.switch_to.is_some() || result.add_account ",
                "|| result.remove_account {"
            )
            .to_string()
        }

        /// Where `open_vault_window` reads the switcher's answer.
        fn switch_request() -> String {
            concat!("if let Some(target) = result.switch", "_to.clone()").to_string()
        }

        fn add_request() -> String {
            concat!("} else if result.add", "_account {").to_string()
        }

        fn remove_request() -> String {
            concat!("} else if result.remove", "_account {").to_string()
        }

        fn lock_recovery() -> String {
            concat!("if result.locked ", "|| result.needs_reauth {").to_string()
        }

        /// The body of `head`'s block, depth-counted to its closing brace.
        ///
        /// Depth-counted rather than taken as a fixed number of bytes: a window
        /// long enough to hold one branch is long enough to overrun into the
        /// next, and this file has already watched that let one branch satisfy
        /// another's assertion.
        fn block_after(head: &str) -> &'static str {
            let production = production_half_of_this_file();
            let at = production.find(head).unwrap_or_else(|| {
                panic!(
                    "{head:?} is not in the production code -- `open_vault_window` never reads \
                     that answer, so the control that produces it is inert"
                )
            });
            let after_open = &production[at..];
            let open = after_open
                .find('{')
                .expect("the request has no block to slice");
            let after_open = &after_open[open + 1..];

            let mut depth = 1usize;
            for (offset, ch) in after_open.char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            let body = &after_open[..offset];
                            assert!(
                                body.len() > 200,
                                "the block sliced from {head:?} is {} bytes, which is not a \
                                 branch: every assertion over it would pass against nothing",
                                body.len()
                            );
                            assert!(
                                body.len() < production.len() / 4,
                                "control: the slice isolated a branch rather than running on \
                                 into the rest of the function"
                            );
                            return body;
                        }
                    }
                    _ => {}
                }
            }
            panic!("the block sliced from {head:?} is never closed");
        }

        fn account_actions_block() -> &'static str {
            block_after(&account_actions_request())
        }

        fn switch_block() -> &'static str {
            block_after(&switch_request())
        }

        /// The plan's own wiring pin. A `switch_to` that `open_vault_window`
        /// never reads means the switcher is 100% inert -- the exact shape of
        /// the Trash/Archive feature that shipped dead behind an early return
        /// with a green suite.
        ///
        /// **Before the lock recovery, and it reopens rather than returning.**
        /// That recovery re-authenticates against the account this process is
        /// already on, so a switch that fell into it would put up the master
        /// password prompt for the account the user asked to LEAVE and then
        /// leave them on it.
        #[test]
        fn open_vault_window_acts_on_a_switch_and_reopens_rather_than_running_the_lock_recovery()
        {
            let source = production_half_of_this_file();
            let (request, lock) = (switch_request(), lock_recovery());

            let switch_at = source.find(&request).unwrap_or_else(|| {
                panic!("{request:?} is not in the production code -- the switcher is inert")
            });
            let lock_at = source.find(&lock).unwrap_or_else(|| {
                panic!("{lock:?} is not in the production code -- this guard needs it to say \
                        which side of the recovery the switch is on")
            });
            assert_ne!(
                switch_at, lock_at,
                "positive control: the two needles are two distinct positions"
            );
            assert!(
                switch_at < lock_at,
                "the switch is handled AFTER the lock/re-auth branch, so it would be \
                 swallowed by a recovery for the wrong account"
            );

            assert!(
                account_actions_block().contains("continue;"),
                "the account actions do not reopen the window, so asking to switch closes the \
                 vault window for good and the user is left staring at the tray"
            );
        }

        /// **The other two rows of the same menu have to reach `main` too.**
        /// "Add account..." and "Remove this account..." are decided in
        /// `account_menu`, carried back through `VaultWindowResult`, and would
        /// then be dropped on the floor by an `open_vault_window` that never
        /// reads them -- complete, correct and unreachable, which is precisely
        /// the shape `switch_to_account`, `add_account` and `remove_account`
        /// each shipped in before Task 14/15.
        ///
        /// Each branch sliced by itself, and each watched calling the ONE
        /// function that does the work rather than a second copy of it.
        #[test]
        fn open_vault_window_acts_on_the_add_and_the_remove_the_menu_can_ask_for() {
            let source = production_half_of_this_file();
            let lock_at = source
                .find(&lock_recovery())
                .expect("the lock recovery must still exist");

            for (head, calls, why) in [
                (
                    add_request(),
                    concat!("add", "_account("),
                    "the menu's \"Add account...\" is inert, or adds an account by some other \
                     means than the one function with the rollback",
                ),
                (
                    remove_request(),
                    concat!("remove", "_account("),
                    "the menu's \"Remove this account...\" is inert, or deletes a profile by \
                     some other means than the one function that settles onto the survivor \
                     first",
                ),
            ] {
                let at = source
                    .find(&head)
                    .unwrap_or_else(|| panic!("{head:?} is not in the production code -- {why}"));
                assert!(
                    at < lock_at,
                    "{head:?} is handled AFTER the lock/re-auth branch, so it would be \
                     swallowed by a recovery that re-authenticates the account being left"
                );
                let block = block_after(&head);
                assert!(block.contains(calls), "{why}: {block:?}");
            }
        }

        /// **`learn_active_account_details` is reached, and reached in the one
        /// place where "the active account" and "who these details describe"
        /// are provably the same account.**
        ///
        /// Every test of that function injects its arguments, so all of them go
        /// on passing against an app that never calls it — and the symptom of
        /// that is exactly the bug this whole change is about: the account menu
        /// naming a 32-character directory forever. Its two call sites are both
        /// in code no harness can run (`open_vault_window` opens a real eframe
        /// window; the other is `main`'s own loop), so they are read.
        ///
        /// **Above the switch/add/remove branch, and that ordering is the whole
        /// safety argument.** These details were fetched for the profile this
        /// window was pointed at. Below those branches, the active account may
        /// be a different one — so the address of the account the user just
        /// left would be written into the entry of the account they are now on.
        #[test]
        fn the_window_teaches_the_active_account_its_own_address_before_anything_changes_it() {
            let production = production_half_of_this_file();
            let learn = concat!("learn_active_account", "_details(");
            let refill = concat!("if let Some(details) = result.account", "_details.clone() {");

            assert_eq!(
                production.matches(learn).count(),
                3,
                "expected the definition and exactly two call sites -- the vault window's own \
                 session, and `main`'s startup prefetch -- found {}",
                production.matches(learn).count()
            );

            let refill_at = production.find(refill).unwrap_or_else(|| {
                panic!("{refill:?} is gone -- the window's `bw status` answer is never read back")
            });
            let block = block_after(refill);
            assert!(
                block.contains(learn),
                "the window's `bw status` answer warms the cache and teaches the account \
                 nothing, so an account minted on a first install keeps its directory name in \
                 every menu forever: {block:?}"
            );

            let actions_at = production
                .find(&account_actions_request())
                .expect("the account actions must still exist");
            assert!(
                refill_at < actions_at,
                "the address is learned AFTER the switch/add/remove branch, so a switch writes \
                 the address of the account the user just left into the entry of the account \
                 they are now on"
            );
        }

        /// **The startup prefetch's drain hands the answer to the one function
        /// whose tests EVALUATE what happens to it.**
        ///
        /// What the drain does is `adopt_startup_prefetch`, and that is driven
        /// end to end by
        /// `a_prefetch_for_the_account_left_behind_is_neither_learned_nor_persisted`
        /// and `a_prefetch_that_names_nobody_is_dropped_by_the_drain` — real
        /// tests, over a scratch `settings.json`, with account A's answer and
        /// account B active. The guard's own truth table is
        /// `the_prefetch_is_adopted_only_by_the_account_it_was_started_for`.
        ///
        /// So this pins only what is left in `main`'s frame loop, which no
        /// harness can run: that the drain calls that function, hands it the
        /// PREFETCH's id (not the active account's), BINDS what comes back, and
        /// does neither half of the adoption out here where nothing evaluates
        /// it.
        ///
        /// Its predecessor pinned the guard's wiring by source text alone, and
        /// three one-line edits at this drain — negating the condition, passing
        /// the active account's id as both of the guard's arguments, and
        /// `let _ = guard(..); if true {` — each persisted account A's address
        /// under account B's id with all 1330 tests green. The shape asserted
        /// below is what makes two of those three not compile.
        #[test]
        fn the_startup_prefetch_is_drained_through_the_function_that_guards_it() {
            let production = production_half_of_this_file();
            let decision = concat!("prefetch_still_describes_the_active", "_account(");
            assert_eq!(
                production.matches(decision).count(),
                2,
                "expected the definition and exactly one call site -- inside \
                 `adopt_startup_prefetch`; if the call is gone the decision is a pure function \
                 nothing consults, and any answer is adopted onto whatever account is active"
            );

            let adopt = concat!("adopt_startup_", "prefetch(");
            assert_eq!(
                production.matches(adopt).count(),
                2,
                "expected the definition and exactly one call site -- `main`'s drain of the \
                 startup prefetch; a second caller is a second copy of this decision, and an \
                 absent one means the tested function is dead code"
            );

            let drain = concat!("if let Ok((about, details)) = status_details_rx.try_", "recv() {");
            let block = block_after(drain);
            // Bound, not discarded. `let _ = adopt_startup_prefetch(..);
            // if true {` -- the repo's signature defect -- has nothing to put
            // in the cache afterwards and does not compile against this shape,
            // but the spelling is pinned anyway so a future edit cannot quietly
            // reintroduce a discarded answer beside a cache filled from
            // somewhere else.
            let bound = concat!("if let Some(adopted) = adopt_startup_", "prefetch(");
            assert!(
                block.contains(bound),
                "the startup `bw status` answer is not drained through \
                 `adopt_startup_prefetch`, or its result is not bound -- so nothing the tests \
                 drive decides whether it is adopted: {block:?}"
            );
            // The id it is handed is the PREFETCH's. Handing it the active
            // account's instead is the mutation that makes the guard compare
            // the active account with itself and answer `true` for any `Some`.
            assert!(
                block.contains(concat!("about.as_", "ref(),")),
                "the id the prefetch was started FOR is not what the drain hands over, so the \
                 guard is deciding about some other pair of accounts: {block:?}"
            );
            assert!(
                !block.contains(concat!("active_account.as_ref().map(|a| &", "a.id)")),
                "the drain reads the active account's id itself instead of leaving that to the \
                 one function whose tests evaluate the comparison -- which is how the same id \
                 came to be passed as both sides of it: {block:?}"
            );
            // Neither half of the adoption may happen out here, where no test
            // can reach it: the persistence belongs to the guarded function,
            // and the cache must be filled from what that function returns
            // rather than from the raw answer.
            assert!(
                !block.contains(concat!("learn_active_account", "_details(")),
                "the drain persists the prefetch itself, outside the guarded function, so the \
                 guard's answer is not what decides whether it is written: {block:?}"
            );
            let cache = concat!("cached_status_details = Some(", "adopted);");
            assert_eq!(
                block.matches(cache).count(),
                1,
                "expected exactly one place the drain caches what it was handed back: {block:?}"
            );
            assert!(
                !block.contains(concat!("cached_status_details = Some(", "details);")),
                "the RAW prefetch is cached rather than what the guarded function handed back, \
                 so a dropped answer is cached anyway: {block:?}"
            );
        }

        /// **The removal reuses the one confirmation, and never writes a
        /// second.** `confirm_account_removal` defaults to No and
        /// `the_removal_confirmation_defaults_to_no` pins that; a dialog
        /// written beside this call site would not have it, and this is the
        /// second most destructive thing this app does.
        #[test]
        fn the_windows_removal_asks_before_it_deletes_and_reuses_the_one_dialog() {
            let block = block_after(&remove_request());
            let confirm = concat!("confirm_account", "_removal(");
            assert!(
                block.contains(confirm),
                "the vault window's removal deletes a profile without asking: {block:?}"
            );
            assert!(
                !block.contains(concat!("MB_YES", "NO")),
                "a second confirmation dialog was written beside the removal instead of \
                 reusing the one that defaults to No: {block:?}"
            );
            // The removal must be gated on the same door the row is drawn from,
            // so a state that changed between the two cannot reach a removal
            // that can only fail.
            assert!(
                block.contains(concat!("state.can_remove", "_active()")),
                "the removal is not re-checked against the door the menu row is gated on: \
                 {block:?}"
            );
            // And the logout runs in the DOOMED account's own directory. The
            // active-profile form would sign out the survivor the app has just
            // settled onto and leave the doomed account signed in on the server.
            assert!(
                block.contains(concat!("login_ui::bw_logout", "_in,")),
                "the removal does not hand `remove_account` the directory form of the logout: \
                 {block:?}"
            );
        }

        /// **One resettle for all three actions, not three.** Three copies
        /// would be three chances to get the hardest sequence in this codebase
        /// subtly different, and each copy would be a teardown path with none
        /// of `resettle_session_with`'s tests. The tray's submenu already has
        /// exactly one for its three; this is the same rule for this window.
        #[test]
        fn the_vault_windows_account_actions_settle_through_the_one_sequence() {
            let block = account_actions_block();
            let sequence = concat!("resettle_", "session(");
            assert_eq!(
                block.matches(sequence).count(),
                1,
                "expected exactly ONE resettle closure shared by the switch, the add and the \
                 remove; found {}: {block:?}",
                block.matches(sequence).count()
            );
            // And all three really are handed it -- one closure that only the
            // switch reaches is the same duplication with a longer fuse, since
            // the other two would each have to grow one.
            let handed = block.matches(concat!("&mut ", "resettle")).count();
            assert_eq!(
                handed, 3,
                "expected the one resettle closure to be handed to all three actions (the \
                 switch, the add and the remove); it reaches {handed}: {block:?}"
            );
        }

        /// **The hole four tasks recorded and none could close.**
        /// `switch_to_account`'s `resettle` is injected because its real body
        /// needs a live Windows tray icon, and every test of the switch passes
        /// a stub. Until this task there was no production closure at all, so
        /// nothing anywhere said that the real one runs the one
        /// teardown-and-repopulate sequence rather than a second, untested
        /// one written inline.
        #[test]
        fn the_production_switch_resettles_through_the_one_sequence() {
            let block = account_actions_block();
            let sequence = concat!("resettle_", "session(");
            assert!(
                block.contains(sequence),
                "the closure `open_vault_window` hands to `switch_to_account` does not call \
                 {sequence:?}. The switch would then tear down and repopulate by some other \
                 means -- a second implementation of the hardest code in this codebase, with \
                 none of its tests: {block:?}"
            );
            // Positive control on the same slice, so the assertion above
            // cannot be satisfied by a block this helper failed to find.
            assert!(
                block.contains(concat!("switch_to", "_account(")),
                "the sliced block does not call the switch at all: {block:?}"
            );
        }

        /// **The gate is `AccountsState`'s.** `all()` still reports the active
        /// account and still reports duplicate ids, and it is not emptied when
        /// switching is refused -- so a target resolved from it could be the
        /// account the user is already on, or one the CLI would ignore the
        /// profile of.
        #[test]
        fn the_target_is_resolved_through_switchable_and_not_the_raw_list() {
            let block = switch_block();
            let gate = concat!("state.switch", "able().iter().find(");
            assert!(
                block.contains(gate),
                "the switch does not resolve its target through {gate:?}: {block:?}"
            );
            assert!(
                !block.contains(concat!("state.all()", ".iter()")),
                "the switch picks its target out of the raw account list, which still holds \
                 the active account, still holds duplicate ids, and is NOT emptied when \
                 switching is refused: {block:?}"
            );
            // Positive control: `all()` really is spelled that way here, so
            // the negative above is about where it is used and not about a
            // name that no longer exists.
            assert!(
                block.contains(concat!("state.", "all()")),
                "the block never mentions `all()` at all, so the assertion above would pass \
                 against a block that had stopped persisting anything: {block:?}"
            );
        }

        /// **A switch that does not stick is worse than one that fails.** The
        /// app would resume the PREVIOUS account on the next launch, which is
        /// indistinguishable from the `relativeDataDir` trap and sends whoever
        /// debugs it down entirely the wrong path.
        ///
        /// Persisted only on `Switched`, and only after it: written first, a
        /// switch that then failed would leave `settings.json` naming an
        /// account this process is not on.
        #[test]
        fn a_successful_switch_persists_the_new_active_account() {
            let block = switch_block();
            let persist = concat!("Settings::persist", "_accounts(");
            let switched = concat!("SwitchOutcome::", "Switched => {");

            assert_eq!(
                block.matches(persist).count(),
                1,
                "expected exactly one persist in the switch block: {block:?}"
            );
            let landed = block
                .split_once(switched)
                .unwrap_or_else(|| {
                    panic!("no {switched:?} arm in the switch block: {block:?}")
                })
                .1;
            assert!(
                landed.contains(persist),
                "a successful switch does not persist the new active account, so the app \
                 resumes the previous one on the next launch: {block:?}"
            );
            // ...and the arms that did NOT land do not persist. The `Declined`
            // arm is the one that matters: the user is still on the account
            // they started on, and writing the target there would strand them.
            let before_landing = block.split_once(switched).unwrap().0;
            assert!(
                !before_landing.contains(persist),
                "something is persisted before the switch is known to have landed: {block:?}"
            );
        }

        /// Every outcome the switch can have is reported. A `RolledBack` the
        /// user never sees is a click that appears to do nothing; a `StoodDown`
        /// that is not logged is an app with no autofill and no record of why.
        ///
        /// **Each arm is cut at the next arm**, not at a fixed number of
        /// bytes. Task 15 watched the fixed window this used to use survive the
        /// mutation that deletes the `RolledBack` message box outright: the
        /// window overran into `StoodDown`, whose own `message_box` satisfied
        /// the assertion.
        #[test]
        fn every_outcome_of_the_switch_is_reported_somewhere() {
            let block = switch_block();
            let variant = concat!("SwitchOutcome", "::");
            for (arm, must_contain, why) in [
                (
                    concat!("SwitchOutcome::", "Declined =>"),
                    concat!("log::", "info!"),
                    "a decline is not an error and must not be raised as one, but it is \
                     still the record of a switch that did not happen",
                ),
                (
                    concat!("SwitchOutcome::", "RolledBack { reason } =>"),
                    concat!("message", "_box("),
                    "a failed switch the user is not told about is a click that did nothing",
                ),
                (
                    concat!("SwitchOutcome::", "StoodDown { reason } =>"),
                    concat!("log::", "error!"),
                    "autofill has been stood down and nothing records why",
                ),
            ] {
                let at = block
                    .find(arm)
                    .unwrap_or_else(|| panic!("no {arm:?} arm in the switch block: {block:?}"));
                let rest = &block[at + arm.len()..];
                let arm_body = match rest.find(variant) {
                    Some(next) => &rest[..next],
                    None => rest,
                };
                assert!(
                    !arm_body.is_empty(),
                    "control: the {arm:?} arm was cut down to nothing"
                );
                assert!(
                    arm_body.contains(must_contain),
                    "the {arm:?} arm does not {must_contain:?}: {why}. The arm reads: \
                     {arm_body:?}"
                );
            }
        }
    }

    /// **The tray's account wiring**, and the first production caller
    /// `add_account` and `remove_account` have ever had.
    ///
    /// Both shipped complete, correct and `#[allow(dead_code)]` in Tasks 12
    /// and 13; Task 14 gave `switch_to_account` a caller and left these two
    /// where they were. Everything below is a source guard for the same reason
    /// Task 14's is: this wiring lives in `main`'s loop, which owns a real
    /// tray icon, a real message pump and a real `bw serve`, and no test in
    /// this crate can enter it.
    ///
    /// The needles are `concat!`-split and single-line, for this file's usual
    /// two reasons: a whole literal matches its own declaration, and a needle
    /// carrying a newline passes on LF and fails on CRLF -- and this file is
    /// entirely CRLF.
    mod the_accounts_the_tray_offers {
        use super::production_half_of_this_file;

        /// Where the main loop decides a click belongs to the submenu.
        fn account_click() -> String {
            concat!("if tray.accounts().owns", "(&event.id) {").to_string()
        }

        /// That `if`'s body, depth-counted to its closing brace.
        fn tray_block() -> &'static str {
            let production = production_half_of_this_file();
            let needle = account_click();
            let at = production.find(&needle).unwrap_or_else(|| {
                panic!(
                    "{needle:?} is not in the production code -- the tray's account submenu is \
                     built and then never acted on, which is the whole feature inert"
                )
            });
            let after_open = &production[at..];
            let open = after_open
                .find('{')
                .expect("the account click has no block to slice");
            let after_open = &after_open[open + 1..];

            let mut depth = 1usize;
            for (offset, ch) in after_open.char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            let body = &after_open[..offset];
                            assert!(
                                body.len() > 500,
                                "the sliced tray block is {} bytes, which is not the wiring: \
                                 every assertion over it would pass against nothing",
                                body.len()
                            );
                            return body;
                        }
                    }
                    _ => {}
                }
            }
            panic!("the account click's block is never closed");
        }

        /// **The hole four tasks recorded and none could close.** A submenu
        /// that offers "Add account..." and "Remove ..." and then calls
        /// neither is exactly the shape this feature has shipped twice: a
        /// complete, correct, entirely inert piece of work behind a green
        /// suite.
        #[test]
        fn the_tray_calls_all_three_account_operations() {
            let block = tray_block();
            for (call, why) in [
                (
                    concat!("switch_to", "_account("),
                    "clicking an account row switches to nothing",
                ),
                (
                    concat!("add", "_account("),
                    "\"Add account...\" is a menu item that does nothing; `add_account` still \
                     has no production caller",
                ),
                (
                    concat!("remove", "_account("),
                    "\"Remove ...\" is a menu item that does nothing; `remove_account` still \
                     has no production caller",
                ),
            ] {
                assert!(
                    block.contains(call),
                    "the tray's account block never calls {call:?}: {why}"
                );
            }
        }

        /// **One resettle, and it is the one sequence.** `switch_to_account`,
        /// `add_account` and `remove_account` all take the teardown-and-
        /// repopulate as an injected closure because its real body needs a live
        /// tray icon, and every test of all three passes a stub. Nothing but
        /// this says the production closure runs `resettle_session` rather than
        /// a second, untested implementation written inline.
        ///
        /// Exactly once: three copies would be three chances to get the hardest
        /// code in this codebase subtly different, and the two that were not
        /// looked at would be the ones that drifted.
        #[test]
        fn the_tray_settles_every_account_action_through_the_one_sequence() {
            let block = tray_block();
            let sequence = concat!("resettle_", "session(");
            assert_eq!(
                block.matches(sequence).count(),
                1,
                "expected exactly one {sequence:?} in the tray's account block -- a second is a \
                 second teardown path with none of this codebase's tests, and none is a switch \
                 that tears down by some other means entirely: {block:?}"
            );
            // Positive control on the same slice, so the count above cannot be
            // satisfied by a block this helper failed to find.
            assert!(
                block.contains(concat!("switch_to", "_account(")),
                "the sliced block does not call the switch at all: {block:?}"
            );
        }

        /// **`bw_logout_in`, never `bw_logout`.** By the time the logout runs,
        /// `remove_account` has already settled the app onto the SURVIVOR, so
        /// the active-profile form would sign out the account the user is
        /// keeping and leave the one being deleted signed in on the server.
        /// Neither outcome is visible afterwards: both leave a deleted
        /// directory and both report success.
        ///
        /// Counted rather than matched against `bw_logout()`, so the guard
        /// cannot be fooled by a spelling with different whitespace -- and so
        /// it carries no paren the production code might not.
        #[test]
        fn the_removal_logs_out_in_the_doomed_accounts_own_directory() {
            let block = tray_block();
            let active_form = concat!("bw_", "logout");
            let directory_form = concat!("bw_logout", "_in");
            assert_eq!(
                block.matches(directory_form).count(),
                1,
                "the removal does not name {directory_form:?} once: {block:?}"
            );
            assert_eq!(
                block.matches(active_form).count(),
                block.matches(directory_form).count(),
                "the tray reaches a logout that is not the directory form -- which acts on \
                 whatever profile this process is pointed at, and by then that is the SURVIVOR: \
                 {block:?}"
            );
            // Positive control for the counter: it really can find two.
            assert_eq!(
                format!("{active_form} {active_form}")
                    .matches(active_form)
                    .count(),
                2
            );
        }

        /// **`switchable()`, not `all()`.** The submenu was built from
        /// `switchable()` too, so this is the second gate rather than the
        /// first: a submenu built before a state change and clicked after it
        /// must not be able to smuggle a target past the refusal.
        #[test]
        fn the_tray_resolves_its_target_through_switchable_and_not_the_raw_list() {
            let block = tray_block();
            let gate = concat!("state.switch", "able().iter().find(");
            assert!(
                block.contains(gate),
                "the tray does not resolve its target through {gate:?}: {block:?}"
            );
            assert!(
                !block.contains(concat!("state.all()", ".iter()")),
                "the tray picks its target out of the raw account list, which still holds the \
                 active account, still holds duplicate ids, and is NOT emptied when switching \
                 is refused: {block:?}"
            );
            // Positive control: `all()` really is spelled that way here, so the
            // negative above is about where it is used and not about a name
            // that no longer exists.
            assert!(
                block.contains(concat!("state.", "all()")),
                "the block never mentions `all()` at all, so the assertion above would pass \
                 against a block that had stopped persisting anything: {block:?}"
            );
        }

        /// **A switch that does not stick is worse than one that fails.** The
        /// app resumes the previous account on the next launch, which is
        /// indistinguishable from the `relativeDataDir` trap and sends whoever
        /// debugs it down entirely the wrong path.
        ///
        /// `add_account` and `remove_account` persist inside themselves, in the
        /// order Tasks 12 and 13 argued for; the switch is the one this block
        /// owns, so there is exactly one persist here.
        #[test]
        fn a_successful_tray_switch_persists_the_new_active_account() {
            let block = tray_block();
            let persist = concat!("Settings::persist", "_accounts(");
            let landed = concat!("== SwitchOutcome::", "Switched {");

            assert_eq!(
                block.matches(persist).count(),
                1,
                "expected exactly one persist in the tray's account block: {block:?}"
            );
            let after = block
                .split_once(landed)
                .unwrap_or_else(|| panic!("no {landed:?} guard in the tray block: {block:?}"))
                .1;
            assert!(
                after.contains(persist),
                "a successful tray switch does not persist the new active account, so the app \
                 resumes the previous one on the next launch: {block:?}"
            );
            let before = block.split_once(landed).unwrap().0;
            assert!(
                !before.contains(persist),
                "something is persisted before the switch is known to have landed: {block:?}"
            );
        }

        /// Every outcome is reported. A `RolledBack` the user never sees is a
        /// click that appears to do nothing; a `StoodDown` that is not logged
        /// is an app with no autofill and no record of why.
        ///
        /// `Declined` is `log::info!` and nothing louder, and it can only mean
        /// one thing here: the submenu gates "Add account..." on `can_add()`,
        /// so the refused-gate `Declined` is unreachable and the only one left
        /// is the sign-in the user closed.
        ///
        /// **Each arm is cut at the next arm, not at a fixed number of
        /// bytes.** A window that overran into the following arm would let the
        /// `RolledBack` assertion be satisfied by `StoodDown`'s message box --
        /// which is exactly what happened when this was written with a 900-byte
        /// window, and it survived the mutation that deletes the `RolledBack`
        /// message box entirely.
        ///
        /// **Sliced from `report_account_action`, which both menus now share.**
        /// The vault window's account menu grew the same add and remove, and
        /// two copies of this reporting is how one of them quietly stops
        /// raising the failure the other one raises -- so there is one, and
        /// this guards it for both.
        #[test]
        fn every_outcome_of_a_tray_account_action_is_reported() {
            let production = production_half_of_this_file();
            let head = concat!("fn report_account", "_action(what: &str, outcome: SwitchOutcome) {");
            let block = production
                .split_once(head)
                .unwrap_or_else(|| {
                    panic!("no {head:?} in production code -- neither menu reports its outcomes")
                })
                .1;
            // Ending-agnostic: a `"\r\n}"` needle stops matching the moment a
            // tool rewrites this file with LF endings.
            let block = block
                .split_once("\n}")
                .expect("its body must be closed")
                .0
                .trim_end_matches('\r');
            assert!(
                block.len() < production.len() / 10,
                "control: the split isolated a body rather than keeping the rest of the file"
            );
            // And both menus really do route through it, so this guard is about
            // what the user is told rather than about an unused function.
            assert_eq!(
                production.matches(concat!("report_account", "_action(")).count(),
                3,
                "expected the one reporting to be reached from both the tray's submenu and \
                 the vault window's account menu, beside its own definition"
            );
            let variant = concat!("SwitchOutcome", "::");
            for (arm, must_contain, why) in [
                (
                    concat!("SwitchOutcome::", "Declined => {"),
                    concat!("log::", "info!"),
                    "a cancellation is not an error and must not be raised as one, but it is \
                     still the record of an action that did not happen",
                ),
                (
                    concat!("SwitchOutcome::", "RolledBack { reason } => {"),
                    concat!("message", "_box("),
                    "a failed switch or add the user is not told about is a click that did \
                     nothing",
                ),
                (
                    concat!("SwitchOutcome::", "StoodDown { reason } => {"),
                    concat!("log::", "error!"),
                    "autofill has been stood down and nothing records why",
                ),
            ] {
                let at = block
                    .find(arm)
                    .unwrap_or_else(|| panic!("no {arm:?} arm in the reporting: {block:?}"));
                let rest = &block[at + arm.len()..];
                let arm_body = match rest.find(variant) {
                    Some(next) => &rest[..next],
                    None => rest,
                };
                assert!(
                    !arm_body.is_empty(),
                    "control: the {arm:?} arm was cut down to nothing, so the assertion below \
                     would be about an empty string"
                );
                assert!(
                    arm_body.contains(must_contain),
                    "the {arm:?} arm does not {must_contain:?}: {why}. The arm reads: \
                     {arm_body:?}"
                );
            }
        }

        /// **The submenu is rebuilt after the action, not inside its success
        /// arm.** `MenuId`s are minted with their items, so the map goes stale
        /// the moment the account list moves -- and it moves on a rolled-back
        /// switch too, because `switch_to_account` may have re-pointed and
        /// restored the store on the way. A rebuild only on success is a
        /// submenu whose ids outlive the state they name.
        #[test]
        fn the_submenu_is_rebuilt_outside_the_action_that_changed_it() {
            let production = production_half_of_this_file();
            let rebuild = concat!("rebuild_accounts", "_menu(");
            // NOT a count of the call sites. This asserted "exactly two" until
            // review 13 found the third one missing -- the vault window's own
            // titlebar switcher -- and a count is what made that omission look
            // deliberate: the only way to add the missing rebuild was to edit
            // the number the guard checked, which is how a guard stops
            // guarding. What is actually being protected is *where* the
            // rebuild sits relative to the action, and that is asserted
            // directly below and, for the vault window, in
            // `every_way_into_the_vault_window_rebuilds_the_submenu_on_the_
            // way_out`.
            assert!(
                !tray_block().contains(rebuild),
                "the rebuild is INSIDE the action's own block, so it is reachable only on the \
                 paths that reach it -- and a click on an account the state no longer holds \
                 reaches none of them"
            );
            // Positive control for that negative: the block really is a region
            // of this file, and the rebuild really is spelled that way.
            assert!(
                tray_block().contains(concat!("switch_to", "_account(")),
                "control: the sliced block is the account wiring"
            );
            assert!(
                production.contains(rebuild),
                "control: the rebuild is still called at all"
            );
        }

        /// **Every way into the vault window rebuilds the submenu on the way
        /// out**, because the window can switch accounts from its own titlebar
        /// (`state.adopt(...)` in `open_vault_window`) and takes `&tray`, so it
        /// cannot rebuild anything itself.
        ///
        /// The bug this closes: after a titlebar switch from A to B the tray
        /// still offered `Remove a@example.com...`, and the removal reads
        /// `state.active().id` -- which is B. The row named one vault and the
        /// deletion took another.
        ///
        /// Per call site, not a count. A count is what let the omission look
        /// deliberate for two reviews; a new `open_vault_window(` that forgets
        /// the rebuild fails this test with nothing to "update".
        #[test]
        fn every_way_into_the_vault_window_rebuilds_the_submenu_on_the_way_out() {
            let production = production_half_of_this_file();
            // `concat!`-split and single-line, this file's usual two reasons.
            // The call needle does not match `rebuild_after_vault_window(`,
            // and it does not match the DECLARATION either -- that is
            // `fn open_vault_window<A: UiAutomationFiller...>(`, whose `(`
            // comes after the generics. There was an `ends_with("fn ")` skip
            // here claiming to be what excluded it; it could never fire, so it
            // was a guard that read as protection and was not one.
            let call = concat!("open_vault", "_window(");
            let rebuild = concat!("rebuild_after_vault", "_window(");
            // The end of each call's statement. Both sites forget the last
            // dispatched window after any of our own windows steals focus, so
            // this is a real boundary in the code rather than a byte count --
            // the shape that already overran an arm in this file once.
            let end_of_statement = concat!("last_dispatched_hwnd", " = None;");

            let mut sites = 0usize;
            let mut from = 0usize;
            while let Some(offset) = production[from..].find(call) {
                let at = from + offset;
                from = at + call.len();
                // The DECLARATION now matches this needle too:
                // dropping the vault window's unused `Injector`
                // parameter took `open_vault_window`'s `<A, B>`
                // generics with it, so its `(` follows the name
                // directly, exactly as a call's does. Skipped
                // rather than respelled: a needle that matched only
                // calls would have to encode the argument list, and
                // would go quietly vacuous the next time one changed.
                if production[..at].ends_with("fn ") {
                    continue;
                }
                sites += 1;
                let rest = &production[from..];
                let end = rest.find(end_of_statement).unwrap_or_else(|| {
                    panic!(
                        "the vault-window call at byte {at} is not followed by \
                         {end_of_statement:?}, so this test cannot tell where its statement \
                         ends -- give it one, or reshape this boundary"
                    )
                });
                let statement = &rest[..end];
                assert!(
                    statement.contains(rebuild),
                    "the vault-window call at byte {at} does not rebuild the accounts submenu \
                     afterwards. The window can switch accounts from its titlebar, so the tray \
                     is left offering a switch to the account the app is now ON and a \
                     \"Remove <the account just left>...\" that removes the OTHER one. The \
                     statement reads: {statement:?}"
                );
            }

            // Controls for that loop: it examined real call sites, and the
            // needles are spelled the way the production code spells them.
            // `>=`, not `== 2`. A count is what let the omission look
            // deliberate for two reviews, and an equality here recreates it
            // from the other side: a legitimate THIRD way into the window,
            // one that does rebuild, fails a test whose only green is editing
            // the number -- which is the pressure `72dfc90` removed. This
            // cannot go vacuous: `sites == 0` fails here, and an empty slice
            // fails the `contains` control below.
            assert!(
                sites >= 2,
                "control: the two known ways of opening the vault window -- the tray menu's \
                 \"Open Vault\" item and a left click on the tray icon -- are what this walked; \
                 finding {sites} means the needle stopped matching"
            );
            assert!(
                production.contains(concat!("fn rebuild_after_vault", "_window(")),
                "control: the rebuild helper is still declared here"
            );
        }

        /// The plan's own gate test, widened to the three files that could
        /// bypass it. `settings.accounts` is the raw list off disk: it has not
        /// been through the `relativeDataDir` refusal, the active-account
        /// exclusion or the duplicate-id dedupe, and a UI reading it directly
        /// bypasses all three.
        #[test]
        fn nothing_offers_an_account_without_going_through_accounts_state() {
            let raw = concat!("settings.accounts", ".iter()");
            for (name, source) in [
                ("tray.rs", include_str!("tray.rs")),
                ("main.rs", include_str!("main.rs")),
                ("vault_window/mod.rs", include_str!("vault_window/mod.rs")),
            ] {
                assert!(
                    !source.contains(raw),
                    "{name} iterates the raw account list instead of asking AccountsState"
                );
                assert!(
                    source.len() > 1000,
                    "control: {name} was really read, so the assertion above is about its \
                     contents and not about an empty string"
                );
            }
            assert!(
                format!("x{raw}y").contains(raw),
                "control: the needle is findable in text that contains it"
            );
        }

        /// `tray.rs` is on the ban list `no_window_answers_may_i_switch_for_
        /// itself` enforces for `vault_window/mod.rs`, and for the same
        /// reason: a second reading of the CLI's availability is a second
        /// answer, and the two would first disagree exactly where the trap is.
        #[test]
        fn the_tray_does_not_answer_may_i_switch_for_itself() {
            let source = include_str!("tray.rs");
            for banned in [
                concat!("MultiAccount", "Availability"),
                concat!("multi_account", "_availability"),
            ] {
                assert!(
                    !source.contains(banned),
                    "tray.rs names {banned:?}: the submenu is deciding for itself whether a \
                     switch is allowed, instead of asking AccountsState"
                );
            }
            // Positive control: it does ask the door, so the negatives above
            // are about a second answer rather than about no answer at all.
            assert!(
                source.contains(concat!("Accounts", "State")),
                "tray.rs does not consult AccountsState at all"
            );
        }
    }

    /// The confirmation shown before an account's profile is deleted. The
    /// dialog itself is a `MessageBoxW` no test can drive, so the text is a
    /// pure function and this is what is actually asserted.
    #[test]
    fn the_removal_confirmation_names_the_account_and_says_what_is_deleted() {
        let warning = account_removal_warning("someone@example.com");
        assert!(
            warning.contains("someone@example.com"),
            "the confirmation does not say which account would go: {warning:?}"
        );
        assert!(
            warning.contains("DELETES"),
            "the confirmation reads as a tidy-up rather than as a deletion: {warning:?}"
        );
        assert!(
            warning.contains("cannot be undone"),
            "the confirmation does not say it is final: {warning:?}"
        );
        assert!(
            warning.contains("not touched"),
            "the confirmation does not say the server-side vault survives, which is the one \
             thing a user needs to know before answering: {warning:?}"
        );
        // Positive control: a different account gets a different warning, so
        // none of the above is passing against a constant string.
        assert!(!account_removal_warning("other@example.com").contains("someone@example.com"));
    }

    /// This is the second most destructive thing this app does, and a stray
    /// Return should not be what does it. Source-level: `MessageBoxW` cannot
    /// be driven from a test.
    #[test]
    fn the_removal_confirmation_defaults_to_no() {
        let production = production_half_of_this_file();
        let body = production
            .split_once(concat!("fn confirm_account", "_removal(label: &str) -> bool {"))
            .expect("the confirmation must still exist")
            .1;
        // Ending-agnostic: a `"\r\n}"` needle here silently stopped matching
        // the moment a tool rewrote this file with LF endings, and the test
        // then failed for a reason that had nothing to do with what it
        // guards. The repo has files in both states, so no needle in it may
        // carry a line ending.
        let body = body
            .split_once("\n}")
            .expect("its body must be closed")
            .0
            .trim_end_matches('\r');
        assert!(
            body.contains(concat!("MB_DEF", "BUTTON2")),
            "the removal prompt defaults to Yes: {body:?}"
        );
        assert!(
            body.contains(concat!("MB_", "YESNO")),
            "the removal prompt is not a question at all: {body:?}"
        );
        assert!(
            body.len() < production.len() / 10,
            "control: the split isolated a body rather than keeping the rest of the file"
        );
    }

    /// The spec's own warning, made mechanical: "if a task finds itself
    /// writing a second teardown-and-repopulate path, it has gone wrong".
    /// A second call site is a second implementation of the hardest code in
    /// this codebase, and it would not have the three tests above.
    ///
    /// Triangulated rather than counted in one lump, so the guard can tell a
    /// duplicated sequence from a settle that was quietly lifted back out
    /// into the wiring where nothing can drive it: the definition is before
    /// `resettle_session`, the wiring between the two functions has none, and
    /// the sequence has exactly one.
    #[test]
    fn there_is_exactly_one_teardown_and_repopulate_path() {
        let production = production_half_of_this_file();
        let settle = concat!("settle_vault_after", "_unlock(");
        assert_eq!(
            format!("{settle} {settle}").matches(settle).count(),
            2,
            "control: the counter can find a needle in text that has two"
        );
        assert!(
            include_str!("main.rs").matches(settle).count() > production.matches(settle).count(),
            "control: the test module really does call it, so `production` is not the whole file"
        );

        let (definition, from_resettle) = production
            .split_once(concat!("fn resettle", "_session("))
            .expect("`resettle_session` must still exist");
        let (wiring, sequence) = from_resettle
            .split_once(concat!("fn resettle_session", "_with("))
            .expect("`resettle_session_with` must still exist");

        assert_eq!(
            definition.matches(settle).count(),
            1,
            "control: the settle's own definition is where it always was"
        );
        assert_eq!(
            wiring.matches(settle).count(),
            0,
            "the settle moved into `resettle_session`'s own body, which needs a real tray icon \
             and so is reachable from no test"
        );
        assert_eq!(
            sequence.matches(settle).count(),
            1,
            "expected exactly ONE call site, inside `resettle_session_with`"
        );
        assert_eq!(
            production.matches(settle).count(),
            2,
            "the definition and one call site, and nothing else in production"
        );
    }

    /// `resettle_session`'s own body is the half no test can reach: the
    /// in-flight drain needs a real `tray::AppTray`, and `tray::build_tray`
    /// makes a real Windows tray icon against a live message loop. So what
    /// the sequence is actually GIVEN in production is pinned here instead --
    /// every test above injects stubs, and all three would go on passing
    /// against an app whose real wiring started nothing.
    #[test]
    fn the_resettle_wiring_passes_the_real_backend_the_real_spinner_and_the_drain() {
        let production = production_half_of_this_file();
        let from_resettle = production
            .split_once(concat!("fn resettle", "_session("))
            .expect("`resettle_session` must still exist")
            .1;
        let wiring = from_resettle
            .split_once(concat!("fn resettle_session", "_with("))
            .expect("`resettle_session_with` must still exist")
            .0;
        assert!(
            wiring.len() < from_resettle.len(),
            "control: the split isolated a region rather than keeping the rest of the file"
        );
        assert!(
            !wiring.contains(concat!("fn job_", "ref(")),
            "control: the region really ends at `resettle_session_with`, since `job_ref` is \
             defined after it"
        );

        for needle in [
            concat!("try_start_", "backend("),
            concat!("wait_for_vault_ready_", "with_spinner("),
            concat!("apply_backend_", "op("),
        ] {
            assert!(
                wiring.contains(needle),
                "`resettle_session` no longer reaches `{needle}`: the sequence's tests all \
                 inject stubs, so nothing else would notice"
            );
        }
    }

    // ---------------------------------------------------------------------
    // Task 9 -- the account switch.
    // ---------------------------------------------------------------------

    /// `bw_path::set_active_data_dir` writes a process-global, and these tests
    /// both set it and read it back. Same guard the `bw_path` tests use for
    /// the same static; a separate one because that one is `mod tests`-private
    /// to the library and this is the binary's own test process.
    static ACTIVE_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_active_dir() -> std::sync::MutexGuard<'static, ()> {
        ACTIVE_DIR_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    const ACCOUNT_A: &str = "0123456789abcdef0123456789abcdef";
    const ACCOUNT_B: &str = "fedcba9876543210fedcba9876543210";

    fn account(id: &str, email: &str) -> Account {
        Account {
            id: deskwarden::accounts::AccountId::parse(id)
                .unwrap_or_else(|| panic!("{id:?} should be a valid account id")),
            email: email.to_string(),
            server_url: None,
        }
    }

    /// A config directory with both accounts' directories already made, and a
    /// session token file already sitting in each -- the state a switch is
    /// actually run from.
    ///
    /// **Every path any switch test touches is under one of these.** Nothing
    /// here may reach the real `%APPDATA%` profile or the user's real config
    /// directory: these tests delete a `session.bin` by design.
    struct ScratchConfig(std::path::PathBuf);

    impl ScratchConfig {
        fn new(tag: &str) -> Self {
            Self::with_accounts(tag, &[ACCOUNT_A, ACCOUNT_B])
        }

        /// The same, for the tests where the *number* of account directories
        /// is the assertion: an add starts from one account and must end with
        /// either one or two, never one and a half.
        fn with_accounts(tag: &str, ids: &[&str]) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "deskwarden-switch-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            for id in ids {
                let account_dir =
                    accounts::data_dir_for(&dir, &deskwarden::accounts::AccountId::parse(id).unwrap());
                std::fs::create_dir_all(&account_dir).unwrap();
                std::fs::write(account_dir.join("session.bin"), b"a-wrapped-token").unwrap();
            }
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for ScratchConfig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// What the real sequence leaves behind when it settles, with the backend
    /// and the master-password prompt taken out: the cache cleared (which is
    /// what bumps the era), then repopulated and the engine rearmed from the
    /// account's *own* items, through the same `repopulate_and_refresh_after_unlock`
    /// production calls.
    fn settled_on(cache: &VaultCache, engine: &mut MatchEngine, items: &[(&str, &str)]) {
        cache.clear();
        let epoch = cache.epoch();
        repopulate_and_refresh_after_unlock(cache, engine, probe_items(items), epoch);
    }

    /// What the real sequence leaves behind when it does NOT settle: the
    /// teardown has already happened -- `resettle_session_with` clears the
    /// cache before it authenticates, and both failing arms end in
    /// `stand_down_after_unlock` -- and only then is the failure reported.
    ///
    /// Modelling that is the point. A stub that returned the failure without
    /// tearing anything down would make every "the previous account still
    /// works" assertion below pass on an app that had never been disturbed.
    fn torn_down(cache: &VaultCache, engine: &mut MatchEngine) {
        cache.clear();
        stand_down_after_unlock(engine, "a test's backend did not come up");
    }

    /// The spec's own test, with the worst failure mode it protects against: a
    /// match left armed from account A, under account B's session, raises an
    /// autofill prompt whose fill can only ever end in an error -- or, if the
    /// two vaults happen to share an item id, in a credential from the wrong
    /// vault.
    #[test]
    fn a_switch_rebuilds_the_engine_so_the_previous_accounts_matches_are_gone() {
        let _guard = lock_active_dir();
        let cfg = ScratchConfig::new("rebuild");
        let (a, b) = (
            account(ACCOUNT_A, "a@example.com"),
            account(ACCOUNT_B, "b@example.com"),
        );
        let server = sync_server();
        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let mut engine = MatchEngine::new();
        let mut active = a.clone();
        let mut store = session_store::SessionStore::new(accounts::session_path_for(
            cfg.path(),
            &a.id,
        ));
        bw_path::set_active_data_dir(Some(accounts::data_dir_for(cfg.path(), &a.id)));

        settled_on(&cache, &mut engine, &[("a-item", "notepad.exe")]);
        assert!(
            engine.lookup(&foreground("notepad.exe")).is_some() && cache.is_populated(),
            "precondition: the app really is live on A, so the assertions below are about \
             A being torn down rather than never having been there"
        );

        let outcome = switch_to_account(
            cfg.path(),
            &a,
            &b,
            &mut active,
            &mut store,
            |_cfg, account, _store| {
                assert_eq!(account.id, b.id, "only the switch itself should have run");
                settled_on(&cache, &mut engine, &[("b-item", "code.exe")]);
                ResettleReport::Settled
            },
        );

        assert_eq!(outcome, SwitchOutcome::Switched);
        assert_eq!(active.id, b.id, "the app is still reporting itself as A");
        assert!(
            engine.lookup(&foreground("notepad.exe")).is_none(),
            "account A's match is STILL ARMED under account B's session"
        );
        // The positive control, so this cannot pass on an engine that is
        // simply empty -- which is what a switch that stood autofill down and
        // never rearmed it would leave, and which every negative assertion
        // above would happily accept.
        assert!(
            engine.lookup(&foreground("code.exe")).is_some(),
            "account B's own match is not armed, so the switch disarmed autofill instead of \
             moving it"
        );
        let items = cache.items();
        assert!(
            items.iter().all(|i| i.id != "a-item"),
            "account A's items are still in the cache under account B's session"
        );
        assert!(
            items.iter().any(|i| i.id == "b-item"),
            "control: account B's own items are there, so the cache was refilled rather than \
             just emptied"
        );
        assert!(
            !accounts::session_path_for(cfg.path(), &a.id).exists(),
            "A's session token outlived the switch"
        );
        assert!(
            accounts::session_path_for(cfg.path(), &b.id).exists(),
            "control: the switch deleted a token file, and it deleted the right one"
        );
    }

    /// The era machinery is what makes a switch safe against a fetch that was
    /// already in the air, and this is the assertion that a switch is actually
    /// ROUTED THROUGH it rather than bypassing it.
    #[test]
    fn a_populate_from_the_previous_account_in_flight_across_a_switch_is_discarded() {
        let _guard = lock_active_dir();
        let cfg = ScratchConfig::new("era");
        let (a, b) = (
            account(ACCOUNT_A, "a@example.com"),
            account(ACCOUNT_B, "b@example.com"),
        );
        let server = sync_server();
        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let mut active = a.clone();
        let mut store = session_store::SessionStore::new(accounts::session_path_for(
            cfg.path(),
            &a.id,
        ));

        // Account A's sync, mid-flight: its epoch was captured before its
        // fetch, and its items are in hand but not yet written.
        let a_epoch = cache.epoch();
        let a_items = probe_items(&[("a-item", "notepad.exe")]);

        let outcome = switch_to_account(
            cfg.path(),
            &a,
            &b,
            &mut active,
            &mut store,
            |_cfg, _account, _store| {
                cache.clear();
                ResettleReport::Settled
            },
        );
        assert_eq!(outcome, SwitchOutcome::Switched);

        assert_eq!(
            cache.populate_with(a_items, a_epoch).unwrap(),
            PopulateOutcome::DiscardedStale,
            "account A's items were written into account B's cache"
        );
        assert!(cache.items().is_empty());

        // The positive control: an epoch captured AFTER the switch is not
        // discarded, so the assertion above cannot pass merely because
        // `populate_with` refuses everything from here on.
        let b_epoch = cache.epoch();
        assert_eq!(
            cache
                .populate_with(probe_items(&[("b-item", "code.exe")]), b_epoch)
                .unwrap(),
            PopulateOutcome::Populated,
            "nothing can repopulate the cache after a switch, so the switch broke the vault \
             rather than moving it"
        );
    }

    /// "A half-switched app -- new data directory, old cache -- is the one
    /// outcome that must not be reachable."
    ///
    /// The `seen` vector is the assertion this test exists for. Everything
    /// else here is end state, and a switch that re-pointed the CLI *after*
    /// the sequence ran -- or never -- leaves exactly the same end state while
    /// making the entire feature inert: `bw` would keep answering from the
    /// previous account's profile, so the "new" account's vault would simply
    /// be the old one. Recording what each run of the sequence could SEE is
    /// the only thing that can tell those apart.
    #[test]
    fn a_failed_switch_returns_to_the_previous_account_with_everything_working() {
        let _guard = lock_active_dir();
        let cfg = ScratchConfig::new("rollback");
        let (a, b) = (
            account(ACCOUNT_A, "a@example.com"),
            account(ACCOUNT_B, "b@example.com"),
        );
        let server = sync_server();
        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let mut engine = MatchEngine::new();
        let mut active = a.clone();
        let mut store = session_store::SessionStore::new(accounts::session_path_for(
            cfg.path(),
            &a.id,
        ));
        bw_path::set_active_data_dir(Some(accounts::data_dir_for(cfg.path(), &a.id)));
        settled_on(&cache, &mut engine, &[("a-item", "notepad.exe")]);

        let mut seen: Vec<(Option<std::path::PathBuf>, std::path::PathBuf)> = Vec::new();
        let outcome = switch_to_account(
            cfg.path(),
            &a,
            &b,
            &mut active,
            &mut store,
            |_cfg, account, store| {
                // AT THE MOMENT the sequence runs -- which is the moment
                // `run_login_flow_for` spawns `bw` and the moment the token it
                // produces is saved.
                seen.push((bw_path::active_data_dir(), store.path().to_path_buf()));
                if account.id == b.id {
                    torn_down(&cache, &mut engine);
                    ResettleReport::NotStarted
                } else {
                    settled_on(&cache, &mut engine, &[("a-item", "notepad.exe")]);
                    ResettleReport::Settled
                }
            },
        );

        assert!(
            matches!(outcome, SwitchOutcome::RolledBack { .. }),
            "expected a rollback, got {outcome:?}"
        );
        assert_eq!(
            bw_path::active_data_dir(),
            Some(accounts::data_dir_for(cfg.path(), &a.id)),
            "the CLI was left on B's directory beside A's cache"
        );
        assert_eq!(
            store.path(),
            accounts::session_path_for(cfg.path(), &a.id),
            "the token store was left on B's file, so A's next token would be written there"
        );
        assert_eq!(active.id, a.id, "the app thinks it switched");
        assert!(cache.is_populated(), "A's vault is gone after a failed switch");
        assert!(
            engine.lookup(&foreground("notepad.exe")).is_some(),
            "A's autofill is dead after a failed switch"
        );
        assert!(
            accounts::session_path_for(cfg.path(), &a.id).exists(),
            "A's token was discarded, so the rollback cost a second password prompt for an \
             account the user never asked to leave"
        );
        assert!(
            accounts::session_path_for(cfg.path(), &b.id).exists(),
            "the switch that failed deleted the token of the account it failed to reach"
        );

        // THE pin: the directory and the token store really were swapped
        // BEFORE the sequence ran, and swapped back before the rollback ran.
        assert_eq!(
            seen,
            vec![
                (
                    Some(accounts::data_dir_for(cfg.path(), &b.id)),
                    accounts::session_path_for(cfg.path(), &b.id)
                ),
                (
                    Some(accounts::data_dir_for(cfg.path(), &a.id)),
                    accounts::session_path_for(cfg.path(), &a.id)
                ),
            ],
            "the sequence did not run against the account it was switching to"
        );
    }

    /// A declined master password is the user changing their mind, not a
    /// failure, and it must leave them on the account they were already using.
    ///
    /// It is also the outcome Task 7's `run_login_flow_for` exists to make
    /// possible: `run_login_flow` exits the process instead of answering, so a
    /// switch built on it would kill a running app because the user pressed
    /// Escape. Nothing in this test could observe that -- the process would be
    /// gone -- which is why the ban below is a source guard.
    #[test]
    fn a_declined_master_password_leaves_the_app_on_the_account_it_started_on() {
        let _guard = lock_active_dir();
        let cfg = ScratchConfig::new("declined");
        let (a, b) = (
            account(ACCOUNT_A, "a@example.com"),
            account(ACCOUNT_B, "b@example.com"),
        );
        let server = sync_server();
        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let mut engine = MatchEngine::new();
        let mut active = a.clone();
        let mut store = session_store::SessionStore::new(accounts::session_path_for(
            cfg.path(),
            &a.id,
        ));
        bw_path::set_active_data_dir(Some(accounts::data_dir_for(cfg.path(), &a.id)));
        settled_on(&cache, &mut engine, &[("a-item", "notepad.exe")]);

        let outcome = switch_to_account(
            cfg.path(),
            &a,
            &b,
            &mut active,
            &mut store,
            |_cfg, account, _store| {
                if account.id == b.id {
                    torn_down(&cache, &mut engine);
                    ResettleReport::Declined
                } else {
                    settled_on(&cache, &mut engine, &[("a-item", "notepad.exe")]);
                    ResettleReport::Settled
                }
            },
        );

        assert_eq!(
            outcome,
            SwitchOutcome::Declined,
            "a cancelled prompt was reported to the user as a failure"
        );
        assert_eq!(active.id, a.id);
        assert_eq!(
            bw_path::active_data_dir(),
            Some(accounts::data_dir_for(cfg.path(), &a.id))
        );
        assert!(engine.lookup(&foreground("notepad.exe")).is_some());
        assert!(cache.is_populated());
        assert!(accounts::session_path_for(cfg.path(), &a.id).exists());
    }

    /// The rollback can fail too -- A's own `bw serve` may not come back up --
    /// and the answer is the state this app already ships and already tells
    /// the user how to recover from, not a claim that the switch landed.
    #[test]
    fn a_switch_whose_rollback_also_fails_stands_down_rather_than_claiming_it_landed() {
        let _guard = lock_active_dir();
        let cfg = ScratchConfig::new("stooddown");
        let (a, b) = (
            account(ACCOUNT_A, "a@example.com"),
            account(ACCOUNT_B, "b@example.com"),
        );
        let server = sync_server();
        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let mut engine = MatchEngine::new();
        let mut active = a.clone();
        let mut store = session_store::SessionStore::new(accounts::session_path_for(
            cfg.path(),
            &a.id,
        ));
        bw_path::set_active_data_dir(Some(accounts::data_dir_for(cfg.path(), &a.id)));
        settled_on(&cache, &mut engine, &[("a-item", "notepad.exe")]);

        let outcome = switch_to_account(
            cfg.path(),
            &a,
            &b,
            &mut active,
            &mut store,
            |_cfg, _account, _store| {
                torn_down(&cache, &mut engine);
                ResettleReport::NotStarted
            },
        );

        match &outcome {
            SwitchOutcome::StoodDown { reason } => {
                assert!(
                    reason.contains("b@example.com") && reason.contains("a@example.com"),
                    "the message names neither the account that failed nor the one that could \
                     not be restored: {reason}"
                );
            }
            other => panic!("expected a stand-down, got {other:?}"),
        }
        assert_eq!(
            active.id, a.id,
            "the app adopted an account whose backend never came up"
        );
        assert_eq!(
            bw_path::active_data_dir(),
            Some(accounts::data_dir_for(cfg.path(), &a.id)),
            "a stood-down app is left pointing at the account it could not reach, so the tray's \
             Sync -- the recovery it tells the user about -- would recover the wrong one"
        );
        assert!(
            accounts::session_path_for(cfg.path(), &a.id).exists(),
            "the app stood down AND threw away the token it needs to come back"
        );
    }

    /// `switch_to_account`'s own body, for the two source guards below.
    ///
    /// Cut at `fn job_ref(`, the function defined immediately after it, rather
    /// than at a closing brace: a `"\n}"` needle passes on LF and fails on
    /// CRLF, and this repository has both.
    fn the_switch_body() -> &'static str {
        let production = production_half_of_this_file();
        let after = production
            .split_once(concat!("fn switch_to", "_account("))
            .expect("`switch_to_account` must still exist")
            .1;
        let body = after
            .split_once(concat!("fn job_", "ref("))
            .expect("`job_ref` must still be the function defined after the switch")
            .0;
        assert!(
            body.len() < after.len(),
            "control: the split isolated a region rather than keeping the rest of the file"
        );
        assert!(
            body.contains(concat!("SwitchOutcome::Stood", "Down")),
            "control: the region really is the switch's own body"
        );
        body
    }

    /// The spec's explicit warning. `start_backend` -- startup's wrapper
    /// around `try_start_backend` -- calls `fatal_startup_error`, and killing
    /// a running app because the OTHER account's backend would not start is
    /// not acceptable. The `start_backend(` needle also matches
    /// `try_start_backend(`, deliberately and correctly: a switch reaches
    /// either one only through `resettle_session`, which is what all the
    /// tests above drive.
    ///
    /// `run_login_flow(` is banned for the same reason and does NOT match
    /// `run_login_flow_for(`: the wrapper exits when the user declines, and
    /// the cancellable form is the one a switch has to call.
    #[test]
    fn no_switch_path_can_reach_the_fatal_startup_error() {
        let body = the_switch_body();
        let production = production_half_of_this_file();
        for banned in [
            concat!("fatal_startup", "_error("),
            concat!("start_", "backend("),
            concat!("run_login", "_flow("),
        ] {
            assert!(
                production.contains(banned),
                "control: `{banned}` is really spelled that way in this file, so the ban below \
                 is not vacuous"
            );
            assert!(
                !body.contains(banned),
                "`{banned}` is reachable from a switch: a failed switch would kill a running app"
            );
        }
    }

    /// The spec's dominant risk, made mechanical: "if an implementation finds
    /// itself writing a second teardown-and-repopulate path, that is the
    /// signal it has gone wrong."
    ///
    /// `switch_to_account` is that implementation's most likely author, and
    /// the tempting version of it is easy to write and impossible to see in a
    /// green suite: clear the cache here, start a backend there, rebuild the
    /// engine at the end. It would pass every behavioural test above, because
    /// those tests only ever assert on the state the sequence leaves. This
    /// guard is what says the state came from THE sequence.
    #[test]
    fn a_switch_reimplements_none_of_the_sequence_it_is_supposed_to_reuse() {
        let body = the_switch_body();
        let production = production_half_of_this_file();
        for banned in [
            concat!("cache.", "clear("),
            concat!("stop_bw_", "serve("),
            concat!("engine.", "rebuild("),
            concat!("stand_down_after", "_unlock("),
            concat!("settle_vault_after", "_unlock("),
            concat!("repopulate_and_refresh_after", "_unlock("),
        ] {
            assert!(
                production.contains(banned),
                "control: `{banned}` is really spelled that way in this file, so the ban below \
                 is not vacuous"
            );
            assert!(
                !body.contains(banned),
                "the switch does `{banned}` itself: that is a second teardown-and-repopulate \
                 path, and it does not have the tests the first one has"
            );
        }
    }

    // ---------------------------------------------------------------------
    // Task 12 -- adding an account.
    // ---------------------------------------------------------------------

    /// The names directly under a directory, sorted. Missing directory reads as
    /// empty, so "nothing was left behind" and "the root was never made" are
    /// the same answer -- which is why every test below pairs the count with
    /// the surviving account's own name rather than only with a length.
    fn dir_entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn accounts_state(
        availability: deskwarden::bw_path::MultiAccountAvailability,
        list: Vec<Account>,
        active: &Account,
    ) -> accounts::AccountsState {
        accounts::AccountsState::new(availability, list, active.id.clone())
        .expect("a non-empty account list")
    }

    fn signed_in_as(email: &str) -> login_ui::BwStatusDetails {
        login_ui::BwStatusDetails {
            status: login_ui::BwStatus::Locked,
            user_email: Some(email.to_string()),
            server_url: Some("https://vault.example.com".to_string()),
        }
    }

    // ---------------------------------------------------------------------
    // Filling in the email an account was minted without.
    // ---------------------------------------------------------------------

    /// **The startup prefetch's staleness decision, evaluated.**
    ///
    /// `bw status` is spawned before the startup window's outcome is
    /// dispatched, and that window can switch, add or remove an account. So its
    /// answer can land describing somebody else — and it is not only a toolbar
    /// label, it is WRITTEN to `settings.json` by
    /// `learn_active_account_details`.
    ///
    /// The predecessor of this test asserted only that the condition's source
    /// text appeared in `main.rs`; flipping that condition's `&&` to `||` — so
    /// that account A's answer is adopted onto, and persisted against, account
    /// B — left the whole suite green. Every row below is a case that mutation
    /// gets wrong.
    #[test]
    fn the_prefetch_is_adopted_only_by_the_account_it_was_started_for() {
        let a = account(ACCOUNT_A, "ana@example.com").id;
        let b = account(ACCOUNT_B, "ben@example.com").id;
        let a_again = account(ACCOUNT_A, "someone-else@example.com").id;

        for (started_for, active_now, adopt, why) in [
            (
                Some(&a),
                Some(&a),
                true,
                "the app is on the very account the prefetch was started for, so the one \
                 `bw status` spawn the startup paid for is thrown away for nothing",
            ),
            (
                Some(&a),
                Some(&a_again),
                true,
                "control: the decision compares ids, not the `Account` values around them -- \
                 the same account carrying a different email must still be the same account",
            ),
            (
                Some(&a),
                Some(&b),
                false,
                "the startup window switched account, and account A's address is adopted \
                 onto -- and persisted against -- account B",
            ),
            (
                None,
                Some(&a),
                false,
                "there was no account when the prefetch was spawned, so `bw status` reported \
                 on whatever profile the CLI defaults to; nothing says that is this account",
            ),
            (
                Some(&a),
                None,
                false,
                "there is no active account to learn the answer onto",
            ),
            (None, None, false, "neither side names an account at all"),
        ] {
            assert_eq!(
                prefetch_still_describes_the_active_account(started_for, active_now),
                adopt,
                "started for {started_for:?}, now on {active_now:?}: {why}"
            );
        }
    }

    /// **The drain itself, evaluated: a prefetch started for account A is
    /// neither learned nor persisted once the startup window has moved this app
    /// to account B.**
    ///
    /// This is the assertion no amount of reading `main`'s source could make,
    /// and the reason `adopt_startup_prefetch` exists as a function at all.
    /// Three one-line edits at the old INLINE drain — negating the guard,
    /// handing it the active account's id as BOTH arguments (so it compares
    /// that account with itself), and `let _ = guard(..); if true {` — each
    /// wrote account A's email and server URL into account B's entry in
    /// `settings.json`, and each left all 1330 tests green, because the only
    /// test over the drain read its source text.
    ///
    /// Nothing downstream could have caught them either:
    /// `learn_active_account_details` compares `active_account` against
    /// `accounts_state`, two copies `adopt` keeps in step and which both name
    /// B, never against the id the prefetch was started for.
    #[test]
    fn a_prefetch_for_the_account_left_behind_is_neither_learned_nor_persisted() {
        let scratch = ScratchConfig::with_accounts("prefetch-drain", &[ACCOUNT_A, ACCOUNT_B]);
        let settings_path = scratch.path().join("settings.json");

        let signed_in = account(ACCOUNT_A, "ana@example.com");
        let blank_a = account(ACCOUNT_A, "");
        // Account B is the one the startup window settled this app onto, and it
        // is the blank first-install account -- so if account A's answer is
        // adopted here, B's entry gains a confident, persisted, WRONG address.
        let b = account(ACCOUNT_B, "");
        let mut state = Some(accounts_state(
            deskwarden::bw_path::MultiAccountAvailability::Available,
            vec![signed_in.clone(), b.clone()],
            &b,
        ));
        let mut active = Some(b.clone());

        let cached = adopt_startup_prefetch(
            &settings_path,
            &mut state,
            &mut active,
            Some(&signed_in.id),
            signed_in_as("ana@example.com"),
        );

        assert!(
            cached.is_none(),
            "account A's `bw status` answer was handed back to be cached, so the vault window \
             would go on to say account B's vault belongs to ana@example.com: {cached:?}"
        );
        let state_ref = state.as_ref().unwrap();
        assert_eq!(
            state_ref.active().email,
            "",
            "account A's address was written into account B's entry -- the copy that gets \
             persisted -- which is the \"random hash\" bug with a confident wrong answer \
             instead of a blank one"
        );
        assert_eq!(
            state_ref.active().server_url,
            None,
            "account B learned account A's SERVER, so this app would go on to talk to the \
             wrong host for it"
        );
        assert_eq!(
            active.as_ref().unwrap().email,
            "",
            "`active_account` -- what `login_context` and the master-password prompt name the \
             account by -- learned the address of the account this app is NOT on"
        );
        assert!(
            !settings_path.exists(),
            "account A's address was persisted under account B's id, so it survives the launch \
             and no later `bw status` ever corrects it: {:?}",
            std::fs::read_to_string(&settings_path)
        );

        // The positive control, on the same scratch directory and the same
        // answer: with the prefetch's OWN account active, every one of the
        // above happens. Without it each assertion passes just as well against
        // a function that adopts nothing at all -- including the file check,
        // since only a real write can create `settings.json` here.
        let mut state = Some(accounts_state(
            deskwarden::bw_path::MultiAccountAvailability::Available,
            vec![blank_a.clone(), b.clone()],
            &blank_a,
        ));
        let mut active = Some(blank_a.clone());
        let cached = adopt_startup_prefetch(
            &settings_path,
            &mut state,
            &mut active,
            Some(&blank_a.id),
            signed_in_as("ana@example.com"),
        );
        assert_eq!(
            cached.as_ref().and_then(|d| d.user_email.as_deref()),
            Some("ana@example.com"),
            "control: the matching case must hand the answer back to be cached, or the \
             `is_none` above says nothing"
        );
        assert_eq!(
            state.as_ref().unwrap().active().email,
            "ana@example.com",
            "control: the matching case must teach `AccountsState`"
        );
        assert_eq!(
            active.as_ref().unwrap().email,
            "ana@example.com",
            "control: the matching case must teach `active_account`"
        );
        assert!(
            settings_path.exists(),
            "control: nothing else in this harness can write settings.json, so the file \
             assertion above says nothing"
        );
        let reloaded = settings::Settings::load(&settings_path);
        assert_eq!(
            reloaded
                .accounts
                .iter()
                .find(|a| a.id == b.id)
                .map(|a| a.email.as_str()),
            Some(""),
            "control: account B's entry on disk gained an address from a run in which only \
             account A was ever active: {:?}",
            reloaded.accounts
        );
    }

    /// The two other ways a prefetch can fail to describe the active account,
    /// driven through the drain rather than through the pure guard: no account
    /// at all when it was spawned (so `bw status` reported on whatever profile
    /// the CLI defaults to), and no active account to learn it onto.
    #[test]
    fn a_prefetch_that_names_nobody_is_dropped_by_the_drain() {
        let scratch = ScratchConfig::with_accounts("prefetch-nobody", &[ACCOUNT_A]);
        let settings_path = scratch.path().join("settings.json");
        let blank = account(ACCOUNT_A, "");

        let mut state = Some(accounts_state(
            deskwarden::bw_path::MultiAccountAvailability::Available,
            vec![blank.clone()],
            &blank,
        ));
        let mut active = Some(blank.clone());

        // Spawned before this app had an account of its own.
        assert!(
            adopt_startup_prefetch(
                &settings_path,
                &mut state,
                &mut active,
                None,
                signed_in_as("ana@example.com"),
            )
            .is_none(),
            "an answer about the CLI's default profile was adopted as this account's own"
        );
        assert_eq!(
            state.as_ref().unwrap().active().email,
            "",
            "the CLI's default profile taught this account an address"
        );
        assert!(
            !settings_path.exists(),
            "the CLI's default profile was persisted as this account's address: {:?}",
            std::fs::read_to_string(&settings_path)
        );

        // Nothing to learn it onto: `StartupAccounts::NoAccountList`.
        let mut no_state = None;
        let mut no_active = None;
        assert!(
            adopt_startup_prefetch(
                &settings_path,
                &mut no_state,
                &mut no_active,
                Some(&blank.id),
                signed_in_as("ana@example.com"),
            )
            .is_none(),
            "an app with no account list cached an answer it has nowhere to attribute"
        );
        assert!(
            !settings_path.exists(),
            "an app with no account list wrote settings.json anyway: {:?}",
            std::fs::read_to_string(&settings_path)
        );

        // The positive control, same scratch directory, same answer.
        assert!(
            adopt_startup_prefetch(
                &settings_path,
                &mut state,
                &mut active,
                Some(&blank.id),
                signed_in_as("ana@example.com"),
            )
            .is_some(),
            "control: the matching case must be adopted, or the `is_none`s above say nothing"
        );
        assert!(
            settings_path.exists(),
            "control: nothing else in this harness can write settings.json, so the file \
             assertions above say nothing"
        );
    }

    /// **Two copies that disagree teach each other nothing, and nothing is
    /// written.**
    ///
    /// `AccountsState` is what `state.all()` persists; `active_account` is what
    /// `login_context` and `switch_to_account` name the account by. They are
    /// kept in step by `adopt`, so a disagreement here is a bug elsewhere — and
    /// the guard exists precisely so that bug does not become a confident,
    /// PERSISTED, wrong address.
    ///
    /// The guard used to be checked one line too late: `AccountsState` — the
    /// copy that gets written to `settings.json` — was updated first, so the
    /// answer landed in the file under the other account's id while the `else`
    /// arm logged that the address "was left alone". Deleting the guard
    /// entirely left the whole suite green.
    #[test]
    fn details_are_learned_onto_neither_copy_when_the_two_copies_disagree() {
        let scratch = ScratchConfig::with_accounts("learn-disagree", &[ACCOUNT_A, ACCOUNT_B]);
        let settings_path = scratch.path().join("settings.json");

        let a = account(ACCOUNT_A, "ana@example.com");
        let b = account(ACCOUNT_B, "ben@example.com");
        // The disagreement: `AccountsState` has settled onto B, `active_account`
        // is still A, and the answer that arrives describes A.
        let mut state = Some(accounts_state(
            deskwarden::bw_path::MultiAccountAvailability::Available,
            vec![a.clone(), b.clone()],
            &b,
        ));
        let mut active = Some(a.clone());

        learn_active_account_details(
            &settings_path,
            &mut state,
            &mut active,
            &signed_in_as("ana-NEW@example.com"),
        );

        let state_ref = state.as_ref().unwrap();
        assert_eq!(
            state_ref.active().email,
            "ben@example.com",
            "account A's address was written into account B's entry -- the copy that gets \
             persisted -- which is the address bug with a confident wrong answer instead of a \
             blank one"
        );
        assert_eq!(
            state_ref.active().server_url,
            None,
            "account B learned account A's SERVER, so this app would go on to talk to the \
             wrong host for it"
        );
        assert_eq!(
            active.as_ref().unwrap().email,
            "ana@example.com",
            "`active_account` learned from an answer the two copies could not agree described \
             it"
        );
        assert!(
            !settings_path.exists(),
            "the disagreement was written to settings.json, so it survives the launch: {:?}",
            std::fs::read_to_string(&settings_path)
        );

        // The positive control, on the same scratch directory and the same
        // details: with the two copies agreed, all of the above DOES happen.
        // Without this every assertion above passes against a function that
        // learns nothing from anything.
        let mut state = Some(accounts_state(
            deskwarden::bw_path::MultiAccountAvailability::Available,
            vec![a.clone(), b.clone()],
            &a,
        ));
        let mut active = Some(a.clone());
        learn_active_account_details(
            &settings_path,
            &mut state,
            &mut active,
            &signed_in_as("ana-NEW@example.com"),
        );
        assert_eq!(
            state.as_ref().unwrap().active().email,
            "ana-NEW@example.com",
            "control: the agreeing case must learn, or the assertions above say nothing"
        );
        assert_eq!(
            active.as_ref().unwrap().email,
            "ana-NEW@example.com",
            "control: `active_account` must learn in the agreeing case"
        );
        assert!(
            settings_path.exists(),
            "control: nothing in this harness can write settings.json, so the file assertion \
             above says nothing"
        );
    }

    /// **The "random hash" bug, end to end.** `resolve_startup` mints a
    /// first-install account with an empty email, `account_label` falls back to
    /// the 32-character directory name, and every menu that names the account
    /// names that. This is where the address arrives and where it is written
    /// down so the next launch does not have to ask again.
    #[test]
    fn a_first_run_accounts_address_is_learned_and_written_to_settings() {
        let scratch = ScratchConfig::with_accounts("learn", &[ACCOUNT_A, ACCOUNT_B]);
        let settings_path = scratch.path().join("settings.json");

        let blank = account(ACCOUNT_A, "");
        let other = account(ACCOUNT_B, "other@example.com");
        let mut state = Some(accounts_state(
            deskwarden::bw_path::MultiAccountAvailability::Available,
            vec![blank.clone(), other.clone()],
            &blank,
        ));
        let mut active = Some(blank.clone());
        assert_eq!(
            account_label(state.as_ref().unwrap().active()),
            ACCOUNT_A,
            "control: before it learns anything the account really is named by its directory"
        );

        learn_active_account_details(
            &settings_path,
            &mut state,
            &mut active,
            &signed_in_as("ana@example.com"),
        );

        assert_eq!(
            account_label(state.as_ref().unwrap().active()),
            "ana@example.com",
            "the menus still name this account by its directory"
        );
        assert_eq!(
            active.as_ref().unwrap().email,
            "ana@example.com",
            "`active_account` -- which `login_context` and `switch_to_account` name the \
             account by -- did not learn it, so a master-password prompt still says the hash"
        );

        // And it SURVIVES the launch, which is the half a purely in-memory fix
        // would not have: `settings.json` is what `resolve_startup` reads next
        // time.
        let reloaded = settings::Settings::load(&settings_path);
        assert_eq!(
            reloaded.accounts.first().map(|a| a.email.as_str()),
            Some("ana@example.com"),
            "the address was learned but never written down, so the next launch shows the \
             hash again; the file holds: {:?}",
            reloaded.accounts
        );
        assert_eq!(
            reloaded.accounts.get(1).map(|a| a.email.as_str()),
            Some("other@example.com"),
            "the other account's entry was rewritten by the active account's status"
        );
        assert_eq!(
            reloaded.active_account.as_ref(),
            Some(&blank.id),
            "the active account was moved by something that only learns an address"
        );
    }

    /// `bw status` answers `null` for both fields when the CLI is logged out or
    /// could not be spawned at all — `check_bw_status_details_in` returns
    /// exactly that on a failed spawn. Writing that would put the hash back and
    /// then persist it.
    #[test]
    fn a_silent_bw_status_writes_nothing_at_all() {
        let scratch = ScratchConfig::with_accounts("learn-silent", &[ACCOUNT_A, ACCOUNT_B]);
        let settings_path = scratch.path().join("settings.json");

        let known = account(ACCOUNT_A, "ana@example.com");
        let mut state = Some(accounts_state(
            deskwarden::bw_path::MultiAccountAvailability::Available,
            vec![known.clone(), account(ACCOUNT_B, "other@example.com")],
            &known,
        ));
        let mut active = Some(known.clone());

        learn_active_account_details(
            &settings_path,
            &mut state,
            &mut active,
            &login_ui::BwStatusDetails {
                status: login_ui::BwStatus::Unauthenticated,
                user_email: None,
                server_url: None,
            },
        );
        assert_eq!(active.as_ref().unwrap().email, "ana@example.com");
        assert!(
            !settings_path.exists(),
            "an answer that said nothing still rewrote settings.json, so an idle app writes \
             the file on every window it opens"
        );

        // The positive control on the same scratch directory: a real answer
        // that changes something DOES write.
        learn_active_account_details(
            &settings_path,
            &mut state,
            &mut active,
            &signed_in_as("ana@example.com"),
        );
        assert!(
            settings_path.exists(),
            "control: nothing in this harness can write settings.json, so the assertion above \
             says nothing"
        );
    }

    /// Learning an address is not a reason to rewrite the file. An app that
    /// persists on every open writes `settings.json` on a schedule set by how
    /// often the user looks at their vault.
    #[test]
    fn learning_nothing_new_persists_nothing() {
        let scratch = ScratchConfig::with_accounts("learn-again", &[ACCOUNT_A, ACCOUNT_B]);
        let settings_path = scratch.path().join("settings.json");

        let known = account(ACCOUNT_A, "ana@example.com");
        let mut state = Some(accounts_state(
            deskwarden::bw_path::MultiAccountAvailability::Available,
            vec![known.clone(), account(ACCOUNT_B, "other@example.com")],
            &known,
        ));
        let mut active = Some(known.clone());

        // The first call has a server URL to learn, so it writes.
        learn_active_account_details(
            &settings_path,
            &mut state,
            &mut active,
            &signed_in_as("ana@example.com"),
        );
        assert!(settings_path.exists(), "control: the first call wrote the file");
        let written = std::fs::metadata(&settings_path).unwrap().len();
        let _ = std::fs::remove_file(&settings_path);
        assert!(written > 0);

        // The second says exactly the same thing, so there is nothing to write.
        learn_active_account_details(
            &settings_path,
            &mut state,
            &mut active,
            &signed_in_as("ana@example.com"),
        );
        assert!(
            !settings_path.exists(),
            "the same answer twice wrote settings.json twice"
        );
    }

    /// `StartupAccounts::NoAccountList`: this app has no `Account` of its own,
    /// so there is nothing to record an address against — and `Settings::save`
    /// refuses to write over the unreadable file anyway.
    #[test]
    fn an_app_with_no_account_list_records_nothing() {
        let scratch = ScratchConfig::with_accounts("learn-none", &[ACCOUNT_A]);
        let settings_path = scratch.path().join("settings.json");
        let mut state = None;
        let mut active = None;
        learn_active_account_details(
            &settings_path,
            &mut state,
            &mut active,
            &signed_in_as("ana@example.com"),
        );
        assert!(state.is_none() && active.is_none());
        assert!(!settings_path.exists());
    }

    /// The whole of a cancelled sign-in: no directory, no entry, no change of
    /// active account, and nothing written to `settings.json`.
    ///
    /// Every assertion here is a negative, and negatives pass trivially against
    /// a build that never creates anything -- so the sign-in closure asserts,
    /// from inside, that the directory really was there while it ran.
    #[test]
    fn a_declined_sign_in_removes_the_directory_and_persists_no_account() {
        let _guard = lock_active_dir();
        let cfg = ScratchConfig::with_accounts("add-declined", &[ACCOUNT_A]);
        let a = account(ACCOUNT_A, "a@example.com");
        let settings_path = cfg.path().join("settings.json");
        let mut state = accounts_state(
            deskwarden::bw_path::MultiAccountAvailability::Available,
            vec![a.clone()],
            &a,
        );
        let mut active = a.clone();
        let mut store =
            session_store::SessionStore::new(accounts::session_path_for(cfg.path(), &a.id));
        bw_path::set_active_data_dir(Some(accounts::data_dir_for(cfg.path(), &a.id)));
        assert_eq!(
            dir_entries(&accounts::accounts_root(cfg.path())),
            vec![ACCOUNT_A.to_string()],
            "precondition: exactly one account directory, so the count below is about cleanup"
        );

        let mut asked = 0usize;
        let outcome = add_account(
            cfg.path(),
            &settings_path,
            &mut state,
            &mut active,
            &mut store,
            |prepared| {
                asked += 1;
                // The positive control the negatives below need: the directory
                // was there while the sign-in ran, so "gone afterwards" means
                // it was removed rather than never created.
                assert!(
                    accounts::data_dir_for(cfg.path(), &prepared.id).is_dir(),
                    "the sign-in ran with no directory to sign in to, so the very first \
                     `store.save` would fail"
                );
                None
            },
            |_| panic!("`bw status` was asked about an account nobody signed in to"),
            |_, _, _| panic!("the app was resettled for an account nobody signed in to"),
        );

        assert_eq!(asked, 1, "the sign-in never ran at all");
        assert_eq!(outcome, SwitchOutcome::Declined);
        assert_eq!(
            state.all().len(),
            1,
            "a half-created account was left in the list"
        );
        assert_eq!(state.active().id, a.id);
        assert_eq!(active.id, a.id);
        assert_eq!(
            dir_entries(&accounts::accounts_root(cfg.path())),
            vec![ACCOUNT_A.to_string()],
            "an empty profile directory was left behind, which presents as an account that is \
             permanently signed out"
        );
        assert!(
            !settings_path.exists(),
            "settings.json was written for an account that was never added"
        );
        assert_eq!(
            bw_path::active_data_dir(),
            Some(accounts::data_dir_for(cfg.path(), &a.id)),
            "the CLI was left pointing at the discarded account's directory"
        );
        assert!(
            accounts::session_path_for(cfg.path(), &a.id).exists(),
            "the existing account's session token went with the cancelled sign-in"
        );
    }

    /// WIRING, and the one that decides whether "Add account" ADDS an account
    /// or LOGS THE EXISTING ONE OUT. `bw login` signs in to whatever profile
    /// `BITWARDENCLI_APPDATA_DIR` names and replaces what was there -- which,
    /// aimed at the wrong directory, is the account the user is already on.
    ///
    /// **Two halves, and review 13 moved the boundary between them.** The
    /// sign-in must reach the NEW account's directory, and the process-global
    /// must not move to get it there: `bw_serve` spawns `bw` off that global
    /// from background threads, so a window in which it names another
    /// account's profile is a window in which a sync can land in the wrong
    /// vault -- the rule `remove_account`'s doc states and this function used
    /// to break, for as long as a user takes to type a master password.
    ///
    /// So what is observed is the account handed to `sign_in`, which is what
    /// `login_ui::profile_dir_for` turns into the directory the CLI is told
    /// about, plus `bw_path::active_data_dir()` read from inside the sign-in
    /// and asserted UNCHANGED. `login_ui`'s own
    /// `the_login_window_names_the_profile_directory_it_acts_on` covers the
    /// other end of that argument.
    #[test]
    fn the_sign_in_runs_with_the_cli_pointed_at_the_new_accounts_directory() {
        let _guard = lock_active_dir();
        let cfg = ScratchConfig::with_accounts("add-pointed", &[ACCOUNT_A]);
        let a = account(ACCOUNT_A, "a@example.com");
        let settings_path = cfg.path().join("settings.json");
        let mut state = accounts_state(
            deskwarden::bw_path::MultiAccountAvailability::Available,
            vec![a.clone()],
            &a,
        );
        let mut active = a.clone();
        let mut store =
            session_store::SessionStore::new(accounts::session_path_for(cfg.path(), &a.id));
        bw_path::set_active_data_dir(Some(accounts::data_dir_for(cfg.path(), &a.id)));

        // What the sign-in is TOLD to act on: the account it is handed, which
        // `login_ui::profile_dir_for` turns into a directory for
        // `bw_command_in`.
        let seen = std::cell::RefCell::new(None);
        // And what the rest of the process can see while it runs.
        let global_during = std::cell::RefCell::new(None);
        let asked_about = std::cell::RefCell::new(None);
        let outcome = add_account(
            cfg.path(),
            &settings_path,
            &mut state,
            &mut active,
            &mut store,
            |prepared| {
                *seen.borrow_mut() =
                    Some(Some(accounts::data_dir_for(cfg.path(), &prepared.id)));
                *global_during.borrow_mut() = Some(bw_path::active_data_dir());
                Some("session-token".to_string())
            },
            |dir| {
                *asked_about.borrow_mut() = Some(dir.map(Path::to_path_buf));
                signed_in_as("new@example.com")
            },
            |_, _, _| ResettleReport::Settled,
        );

        assert_eq!(outcome, SwitchOutcome::Switched);
        let seen = seen.into_inner().expect("the sign-in never ran");
        assert_eq!(
            seen,
            Some(accounts::data_dir_for(cfg.path(), &state.active().id)),
            "the sign-in ran in {seen:?}, not in the new account's own directory"
        );
        assert!(
            seen.is_some(),
            "the sign-in ran in the CLI's DEFAULT profile -- this would sign the existing \
             account out and replace it"
        );
        assert_ne!(
            seen,
            Some(accounts::data_dir_for(cfg.path(), &a.id)),
            "the sign-in ran in the EXISTING account's profile: `bw login` there does not add \
             an account, it replaces the one already in it"
        );

        // **And the process-global never named it.** This is the half review
        // 13 added: a temporary `set_active_data_dir` across a blocking,
        // user-paced window is a window in which `bw_serve`'s background
        // threads spawn `bw` against the new account's empty profile. The two
        // assertions above pass either way -- that is exactly why this one is
        // separate.
        assert_eq!(
            global_during.into_inner().expect("the sign-in never ran"),
            Some(accounts::data_dir_for(cfg.path(), &a.id)),
            "the active data directory moved off the account this process is on while the \
             sign-in window was up; a background sync in that window lands in the wrong vault"
        );
        assert_eq!(
            bw_path::active_data_dir(),
            Some(accounts::data_dir_for(cfg.path(), &state.active().id)),
            "control: the global does move -- once, when the switch lands -- so the assertion \
             above is about the window and not about a global that never changes"
        );

        // And the email came from asking the CLI about that same directory,
        // rather than about whatever profile this process happens to be on.
        assert_eq!(
            asked_about.into_inner().expect("the status was never read"),
            Some(accounts::data_dir_for(cfg.path(), &state.active().id)),
            "`bw status` was asked about the wrong profile, so the account would be labelled \
             with somebody else's address"
        );
    }

    /// The successful add, end to end: the account is on disk, it is active,
    /// it carries the address the sign-in produced, and it survives a restart.
    #[test]
    fn a_successful_add_persists_the_account_and_makes_it_active() {
        let _guard = lock_active_dir();
        let cfg = ScratchConfig::with_accounts("add-ok", &[ACCOUNT_A]);
        let a = account(ACCOUNT_A, "a@example.com");
        let settings_path = cfg.path().join("settings.json");
        let mut state = accounts_state(
            deskwarden::bw_path::MultiAccountAvailability::Available,
            vec![a.clone()],
            &a,
        );
        let mut active = a.clone();
        let mut store =
            session_store::SessionStore::new(accounts::session_path_for(cfg.path(), &a.id));
        bw_path::set_active_data_dir(Some(accounts::data_dir_for(cfg.path(), &a.id)));

        let outcome = add_account(
            cfg.path(),
            &settings_path,
            &mut state,
            &mut active,
            &mut store,
            |_| Some("session-token".to_string()),
            |_| signed_in_as("new@example.com"),
            |_, _, _| ResettleReport::Settled,
        );

        assert_eq!(outcome, SwitchOutcome::Switched);
        assert_eq!(state.all().len(), 2);
        let added = state.active().clone();
        assert_ne!(added.id, a.id, "the add settled back onto the old account");
        assert_eq!(active.id, added.id, "the app still reports itself as A");
        assert!(accounts::data_dir_for(cfg.path(), &added.id).is_dir());
        assert_eq!(
            added.email, "new@example.com",
            "the new account is a blank row in the switcher"
        );
        assert_eq!(added.server_url.as_deref(), Some("https://vault.example.com"));
        assert_eq!(
            bw_path::active_data_dir(),
            Some(accounts::data_dir_for(cfg.path(), &added.id)),
            "the CLI is not pointed at the account the app says it is on"
        );
        assert_eq!(
            store.path(),
            accounts::session_path_for(cfg.path(), &added.id),
            "the next token would be written over the account the user just left"
        );
        // The token from the sign-in is where the switch's authentication
        // looks, so the user is not asked for the master password twice.
        assert_eq!(
            session_store::SessionStore::new(accounts::session_path_for(cfg.path(), &added.id))
                .load()
                .as_deref(),
            Some("session-token"),
            "the token the user just typed their master password for was thrown away"
        );
        // The old account is still offered, so the add did not replace it.
        assert_eq!(
            state.switchable().iter().map(|x| x.id.clone()).collect::<Vec<_>>(),
            vec![a.id.clone()]
        );

        let loaded = settings::Settings::load(&settings_path);
        assert_eq!(loaded.accounts.len(), 2, "persisted, not just held in memory");
        assert_eq!(loaded.active_account.as_ref(), Some(&added.id));
        assert_eq!(
            loaded.accounts.last().map(|x| x.email.as_str()),
            Some("new@example.com"),
            "the address was held in memory but never written, so the next launch shows a \
             blank row"
        );
    }

    /// The `relativeDataDir` trap reaches here, and adding an account under it
    /// would write a profile the app cannot reliably reach again -- the CLI
    /// ignores `BITWARDENCLI_APPDATA_DIR` there, so both accounts are served
    /// the one portable profile.
    #[test]
    fn add_is_refused_while_accounts_state_says_it_cannot_be_done() {
        let _guard = lock_active_dir();
        let cfg = ScratchConfig::with_accounts("add-blocked", &[ACCOUNT_A]);
        let a = account(ACCOUNT_A, "a@example.com");
        let settings_path = cfg.path().join("settings.json");
        let mut blocked = accounts_state(
            deskwarden::bw_path::MultiAccountAvailability::BlockedByUnknownCliPath,
            vec![a.clone()],
            &a,
        );
        let mut active = a.clone();
        let mut store =
            session_store::SessionStore::new(accounts::session_path_for(cfg.path(), &a.id));
        bw_path::set_active_data_dir(Some(accounts::data_dir_for(cfg.path(), &a.id)));

        assert!(!blocked.can_add());
        let outcome = add_account(
            cfg.path(),
            &settings_path,
            &mut blocked,
            &mut active,
            &mut store,
            |_| panic!("a sign-in was opened for an account that may not be added"),
            |_| panic!("`bw status` was run for an account that may not be added"),
            |_, _, _| panic!("the app was resettled for an account that may not be added"),
        );
        assert_eq!(outcome, SwitchOutcome::Declined);
        assert_eq!(blocked.all().len(), 1);
        assert_eq!(
            dir_entries(&accounts::accounts_root(cfg.path())),
            vec![ACCOUNT_A.to_string()],
            "a profile directory was created for an account that may not be added"
        );
        assert!(!settings_path.exists());

        // The positive control on the same call, same fixture: an unblocked
        // state really does get through, so the refusal above is the gate
        // rather than an `add_account` that refuses everything.
        let mut open = accounts_state(
            deskwarden::bw_path::MultiAccountAvailability::Available,
            vec![a.clone()],
            &a,
        );
        assert_eq!(
            add_account(
                cfg.path(),
                &settings_path,
                &mut open,
                &mut active,
                &mut store,
                |_| Some("session-token".to_string()),
                |_| signed_in_as("new@example.com"),
                |_, _, _| ResettleReport::Settled,
            ),
            SwitchOutcome::Switched
        );
        assert_eq!(open.all().len(), 2);
    }

    /// An add whose switch fails must end exactly where it started -- and the
    /// rollback has to land on the ORIGINAL directory, not on the one that is
    /// about to be deleted. `switch_to_account` reads the active data directory
    /// as the place to roll back to, so an add that left it pointing at the new
    /// account would restore the app into a directory it then removes: a
    /// running app pointed at nothing, and no end-state list assertion sees it.
    #[test]
    fn an_add_whose_switch_fails_leaves_the_app_and_the_disk_exactly_as_they_were() {
        let _guard = lock_active_dir();
        let cfg = ScratchConfig::with_accounts("add-failed", &[ACCOUNT_A]);
        let a = account(ACCOUNT_A, "a@example.com");
        let settings_path = cfg.path().join("settings.json");
        // What the existing account has that must survive: its own quick-unlock
        // blob, beside its token.
        std::fs::write(accounts::hello_blob_path_for(cfg.path(), &a.id), b"sealed").unwrap();
        let mut state = accounts_state(
            deskwarden::bw_path::MultiAccountAvailability::Available,
            vec![a.clone()],
            &a,
        );
        let mut active = a.clone();
        let mut store =
            session_store::SessionStore::new(accounts::session_path_for(cfg.path(), &a.id));
        bw_path::set_active_data_dir(Some(accounts::data_dir_for(cfg.path(), &a.id)));

        let minted = std::cell::RefCell::new(None);
        let outcome = add_account(
            cfg.path(),
            &settings_path,
            &mut state,
            &mut active,
            &mut store,
            |prepared| {
                *minted.borrow_mut() = Some(prepared.id.clone());
                // The user ticked "Use Windows Hello" on the way through, so
                // there is now a blob sealing a master password for an account
                // that is about to stop existing.
                std::fs::write(
                    accounts::hello_blob_path_for(cfg.path(), &prepared.id),
                    b"sealed",
                )
                .unwrap();
                Some("session-token".to_string())
            },
            |_| signed_in_as("new@example.com"),
            |_, to, _| {
                if to.id == a.id {
                    ResettleReport::Settled
                } else {
                    ResettleReport::NotStarted
                }
            },
        );

        let minted = minted.into_inner().expect("the sign-in never ran");
        assert!(
            matches!(outcome, SwitchOutcome::RolledBack { .. }),
            "expected a rollback, got {outcome:?}"
        );
        assert_eq!(state.all().len(), 1, "a failed add was left in the list");
        assert_eq!(state.active().id, a.id);
        assert_eq!(active.id, a.id);
        assert_eq!(
            bw_path::active_data_dir(),
            Some(accounts::data_dir_for(cfg.path(), &a.id)),
            "the app was restored onto the directory the failed add then deleted"
        );
        assert_eq!(
            store.path(),
            accounts::session_path_for(cfg.path(), &a.id),
            "the token store was left on the discarded account's file"
        );
        assert_eq!(
            dir_entries(&accounts::accounts_root(cfg.path())),
            vec![ACCOUNT_A.to_string()],
            "the failed add's profile directory is still there"
        );
        assert!(
            !accounts::hello_blob_path_for(cfg.path(), &minted).exists(),
            "a Windows Hello blob sealing a master password outlived the account it belongs to"
        );
        // The file EXISTS now, and that is the point of the ordering: the
        // account was claimed before the switch was attempted, so what has to
        // be checked is that the claim was withdrawn rather than that it was
        // never made. Reading the file back rather than trusting `state`,
        // because `state` is memory and this is what the next launch sees.
        let written = std::fs::read_to_string(&settings_path)
            .expect("the add claimed the account before switching, so the file must be there");
        assert!(
            !written.contains(minted.as_str()),
            "the failed add is still named in settings.json, so the next launch offers an              account whose directory was deleted: {written}"
        );
        assert!(
            written.contains(ACCOUNT_A),
            "control: the rollback wrote a file that no longer names the existing account either"
        );
        // Positive controls: the existing account kept everything it had, so
        // the four negatives above are about the new account rather than about
        // a rollback that wiped the accounts root.
        assert!(
            accounts::session_path_for(cfg.path(), &a.id).exists(),
            "the failed add cost the user their existing session"
        );
        assert!(
            accounts::hello_blob_path_for(cfg.path(), &a.id).exists(),
            "the failed add took the existing account's quick unlock with it"
        );
    }

    /// The invariant the deleted `migration` module's scan-and-adopt path
    /// existed to recover from, closed by ordering instead.
    ///
    /// `bw login` writes `accounts/<id>/data.json`. If the account list is
    /// written only after the switch, a failed write leaves that directory
    /// signed in and named by nothing — no launch can ever reach it, and the
    /// only way back was a scan of the accounts root against the stored ids.
    /// Claiming the account *before* the switch means a failed write has
    /// nothing to undo: the directory goes, and the cost is the sign-in that
    /// just happened.
    ///
    /// The failure is real rather than injected: `Settings::save` refuses to
    /// write over a file it could not parse, so an unreadable `settings.json`
    /// makes every `persist_accounts` in this function fail — which is also
    /// the one way this can happen in production.
    ///
    /// Fails if the persist moves back after the switch, and fails if the
    /// discard is dropped.
    #[test]
    fn a_failed_persist_strands_no_signed_in_profile() {
        let _guard = lock_active_dir();
        let cfg = ScratchConfig::with_accounts("add-persist-fails", &[ACCOUNT_A]);
        let a = account(ACCOUNT_A, "a@example.com");
        let settings_path = cfg.path().join("settings.json");
        // Not valid settings, so `Settings::save` refuses every write.
        std::fs::write(&settings_path, b"{ this is not settings").unwrap();
        let mut state = accounts_state(
            deskwarden::bw_path::MultiAccountAvailability::Available,
            vec![a.clone()],
            &a,
        );
        let mut active = a.clone();
        let mut store =
            session_store::SessionStore::new(accounts::session_path_for(cfg.path(), &a.id));
        bw_path::set_active_data_dir(Some(accounts::data_dir_for(cfg.path(), &a.id)));

        let minted = std::cell::RefCell::new(None);
        let outcome = add_account(
            cfg.path(),
            &settings_path,
            &mut state,
            &mut active,
            &mut store,
            |prepared| {
                *minted.borrow_mut() = Some(prepared.id.clone());
                // What `bw login` leaves: a real, signed-in CLI profile in the
                // new account's directory. Without this the assertions below
                // would be about an empty directory and would say nothing
                // about stranding a vault.
                std::fs::write(
                    accounts::data_dir_for(cfg.path(), &prepared.id).join("data.json"),
                    b"{\"profile\":\"signed in\"}",
                )
                .unwrap();
                Some("session-token".to_string())
            },
            |_| signed_in_as("new@example.com"),
            |_, _, _| ResettleReport::Settled,
        );

        let minted = minted
            .into_inner()
            .expect("control: the sign-in never ran, so nothing was signed in to strand");
        assert!(
            matches!(outcome, SwitchOutcome::RolledBack { .. }),
            "a write that failed was reported as a completed add: {outcome:?}"
        );
        assert_eq!(
            dir_entries(&accounts::accounts_root(cfg.path())),
            vec![ACCOUNT_A.to_string()],
            "a signed-in profile was left in a directory that nothing on disk names -- no \
             launch can reach it and there is no longer a scan that would find it"
        );
        // The app is where it started, and the account the user was already on
        // kept everything: the positive controls that stop the assertion above
        // being satisfied by a rollback that wiped the accounts root.
        assert_eq!(state.all().len(), 1);
        assert_eq!(state.active().id, a.id);
        assert_eq!(active.id, a.id);
        assert!(accounts::session_path_for(cfg.path(), &a.id).exists());
        assert!(
            !accounts::data_dir_for(cfg.path(), &minted).exists(),
            "control: the minted directory is the one that is gone"
        );
    }

    /// `add_account`'s own body, for the source guard below.
    ///
    /// Both ends are named and both are controlled: it starts at the function
    /// and stops at the first line of the next item's doc comment, rather than
    /// at a closing brace -- a `"\n}"` needle passes on LF and fails on CRLF,
    /// and this file is CRLF throughout.
    fn the_add_body() -> &'static str {
        let production = production_half_of_this_file();
        let after = production
            .split_once(concat!("fn add_", "account("))
            .expect("`add_account` must still exist")
            .1;
        let body = after
            .split_once(concat!(
                "/// Switches the app from one account",
                " to another, or leaves it exactly where it"
            ))
            .expect("`switch_to_account`'s doc comment must still follow `add_account`")
            .0;
        assert!(
            body.len() < after.len(),
            "control: the split isolated a region rather than keeping the rest of the file"
        );
        assert!(
            body.contains(concat!("discard_prepared", "_account(")),
            "control: the region really is the add's own body"
        );
        assert!(
            !body.contains(concat!("SwitchOutcome::Stood", "Down")),
            "control: the region really stops before `switch_to_account`'s body"
        );
        body
    }

    /// The sign-in window and the `bw status` read are injected, and the tests
    /// above observe them to decide whether an add ADDS or REPLACES. A body
    /// that also called the real ones directly would make those observations a
    /// lie -- two sign-in windows in production, one of them in the wrong
    /// profile -- with every test still green.
    ///
    /// `fatal_startup_error` and `start_backend` are banned for the same reason
    /// they are banned from the switch: a second account that will not come up
    /// is not a reason to take a running app down.
    #[test]
    fn adding_an_account_opens_no_window_and_asks_the_cli_nothing_by_itself() {
        let body = the_add_body();
        let production = production_half_of_this_file();
        // `run_login_flow_for(` is not spelled with a paren anywhere in this
        // file, so its control has to be the module that declares it.
        let login_ui_source = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src")
                .join("login_ui.rs"),
        )
        .expect("cannot read login_ui.rs");

        for (banned, control) in [
            (concat!("run_login_flow", "_for("), login_ui_source.as_str()),
            (concat!("run_login", "_flow("), production),
            // No trailing paren on this one: `check_bw_status_details_in` is
            // spelled without one where it is handed on as a function item,
            // and handing it on is as much a direct call as making one. The
            // needle covers the active-profile form
            // (`check_bw_status_details`) too, which is worse still -- it would
            // label the new account with whatever profile this process is on.
            (concat!("check_bw_status_", "details"), production),
            (concat!("fatal_startup", "_error("), production),
            (concat!("start_", "backend("), production),
        ] {
            assert!(
                control.contains(banned),
                "control: `{banned}` is really spelled that way, so the ban below is not vacuous"
            );
            assert!(
                !body.contains(banned),
                "`{banned}` is reachable from `add_account`: the sign-in and the status read \
                 are injected precisely so a test can see WHICH PROFILE they run against, and \
                 a second, direct call is invisible to every one of them"
            );
        }
    }

    // ---------------------------------------------------------------------
    // Task 13 -- removing an account.
    // ---------------------------------------------------------------------

    /// Everything an account keeps on disk beside its CLI profile. Planted for
    /// both accounts in every removal test, so "the doomed account's secrets
    /// went" is always paired with "the other account's did not".
    fn plant_secrets(cfg: &Path, id: &deskwarden::accounts::AccountId) {
        std::fs::create_dir_all(accounts::data_dir_for(cfg, id).join("bw")).unwrap();
        std::fs::write(accounts::session_path_for(cfg, id), b"a-wrapped-token").unwrap();
        std::fs::write(accounts::hello_blob_path_for(cfg, id), b"sealed").unwrap();
    }

    /// The two accounts, the state they are in, and the app pointed at `a`.
    fn an_app_on_a_with_b_beside_it(
        cfg: &ScratchConfig,
        availability: deskwarden::bw_path::MultiAccountAvailability,
    ) -> (
        Account,
        Account,
        accounts::AccountsState,
        Account,
        session_store::SessionStore,
    ) {
        let a = account(ACCOUNT_A, "a@example.com");
        let b = account(ACCOUNT_B, "b@example.com");
        for id in [&a.id, &b.id] {
            plant_secrets(cfg.path(), id);
        }
        let state = accounts_state(availability, vec![a.clone(), b.clone()], &a);
        let store =
            session_store::SessionStore::new(accounts::session_path_for(cfg.path(), &a.id));
        bw_path::set_active_data_dir(Some(accounts::data_dir_for(cfg.path(), &a.id)));
        let active = a.clone();
        (a, b, state, active, store)
    }

    /// WIRING, and the whole of it. `bw logout` with the wrong profile
    /// directory -- or with none, which is the ACTIVE one -- logs the wrong
    /// account out, and the account being removed stays signed in on the
    /// server forever.
    ///
    /// The closure also observes the order from the inside, which is the only
    /// place it is visible: at logout time the profile must still be there
    /// (logging out after deleting it is a `bw logout` against nothing) and
    /// `settings.json` must not have been written yet (a list written first
    /// disagrees with the disk the moment anything below fails).
    #[test]
    fn removing_an_account_logs_out_in_that_accounts_own_directory() {
        let _guard = lock_active_dir();
        let cfg = ScratchConfig::new("remove-logout");
        let (a, b, mut state, mut active, mut store) = an_app_on_a_with_b_beside_it(
            &cfg,
            deskwarden::bw_path::MultiAccountAvailability::Available,
        );
        let settings_path = cfg.path().join("settings.json");

        let seen = std::cell::RefCell::new(Vec::new());
        remove_account(
            cfg.path(),
            &settings_path,
            &mut state,
            &b.id,
            &mut active,
            &mut store,
            |_, _, _| panic!("the app was resettled to remove an account it was not on"),
            |dir| {
                seen.borrow_mut().push(dir.map(Path::to_path_buf));
                assert!(
                    accounts::data_dir_for(cfg.path(), &b.id).is_dir(),
                    "the profile was deleted before it was logged out, so `bw logout` ran \
                     against a directory that was not there"
                );
                assert!(
                    !settings_path.exists(),
                    "the account list was written before the profile was deleted: a removal \
                     that fails from here leaves settings.json disagreeing with the disk"
                );
                Ok(())
            },
        )
        .expect("the removal must succeed");

        let seen = seen.into_inner();
        assert_eq!(seen.len(), 1, "`bw logout` ran {} times", seen.len());
        assert_eq!(
            seen[0],
            Some(accounts::data_dir_for(cfg.path(), &b.id)),
            "`bw logout` ran in {:?} rather than in the removed account's own directory",
            seen[0]
        );
        assert!(
            seen[0].is_some(),
            "`bw logout` ran with no profile directory at all, which is the ACTIVE account's: \
             removing one account would sign the user out of the other"
        );
        assert_ne!(
            seen[0],
            Some(accounts::data_dir_for(cfg.path(), &a.id)),
            "`bw logout` ran in the account the app is ON"
        );
    }

    /// The account's whole profile goes -- its `session.bin` and its
    /// `hello.bin` with it, because a sealed master password for an account the
    /// CLI no longer knows is a liability -- and nothing else does.
    #[test]
    fn removing_an_account_deletes_its_secrets_and_only_its_own() {
        let _guard = lock_active_dir();
        let cfg = ScratchConfig::new("remove-secrets");
        let (a, b, mut state, mut active, mut store) = an_app_on_a_with_b_beside_it(
            &cfg,
            deskwarden::bw_path::MultiAccountAvailability::Available,
        );
        let settings_path = cfg.path().join("settings.json");
        assert!(
            accounts::hello_blob_path_for(cfg.path(), &b.id).exists(),
            "control: the doomed account really had a sealed master password to lose"
        );

        remove_account(
            cfg.path(),
            &settings_path,
            &mut state,
            &b.id,
            &mut active,
            &mut store,
            |_, _, _| panic!("the app was resettled to remove an account it was not on"),
            |_| Ok(()),
        )
        .expect("the removal must succeed");

        assert!(!accounts::session_path_for(cfg.path(), &b.id).exists());
        assert!(
            !accounts::hello_blob_path_for(cfg.path(), &b.id).exists(),
            "a Windows Hello blob sealing a master password outlived the account it belongs to"
        );
        assert!(!accounts::data_dir_for(cfg.path(), &b.id).exists());
        // The positive controls, without which every assertion above passes
        // against a build that deleted the whole accounts root.
        assert!(
            accounts::session_path_for(cfg.path(), &a.id).exists(),
            "the WRONG account's token went"
        );
        assert!(
            accounts::hello_blob_path_for(cfg.path(), &a.id).exists(),
            "the WRONG account's quick unlock went"
        );
        assert_eq!(
            dir_entries(&accounts::accounts_root(cfg.path())),
            vec![ACCOUNT_A.to_string()],
            "exactly the other account's directory is left"
        );

        // The app did not move, and the shorter list is on disk.
        assert_eq!(state.all().len(), 1);
        assert_eq!(state.active().id, a.id);
        assert_eq!(active.id, a.id);
        assert!(
            state.switchable().is_empty(),
            "the switcher still offers a door onto a directory that has been deleted"
        );
        let stored = settings::Settings::load(&settings_path);
        assert_eq!(
            stored.accounts.iter().map(|x| x.id.clone()).collect::<Vec<_>>(),
            vec![a.id.clone()],
            "the removed account is still in settings.json, so it is back on the next launch"
        );
        assert_eq!(stored.active_account, Some(a.id));
    }

    /// Removing the account the app is ON. It has to land somewhere coherent
    /// first, and `bw serve` must not be serving the profile that is about to
    /// be deleted -- so the switch runs BEFORE anything is removed, and a
    /// switch that does not land is a removal that does not happen.
    ///
    /// And the last account cannot be removed at all: there would be no profile
    /// to point the CLI at, no `session.bin` to load and no account for the
    /// login window to enrol Windows Hello against.
    #[test]
    fn removing_the_active_account_switches_away_first_and_never_removes_the_last_one() {
        let _guard = lock_active_dir();
        let cfg = ScratchConfig::new("remove-active");
        let (a, b, mut state, mut active, mut store) = an_app_on_a_with_b_beside_it(
            &cfg,
            deskwarden::bw_path::MultiAccountAvailability::Available,
        );
        let settings_path = cfg.path().join("settings.json");

        let settled_onto = std::cell::RefCell::new(Vec::new());
        remove_account(
            cfg.path(),
            &settings_path,
            &mut state,
            &a.id,
            &mut active,
            &mut store,
            |_, to, store| {
                settled_onto.borrow_mut().push(to.id.clone());
                assert!(
                    accounts::data_dir_for(cfg.path(), &a.id).is_dir(),
                    "the app was settled onto the survivor only AFTER the account it was on \
                     had been deleted"
                );
                assert_eq!(
                    store.path(),
                    accounts::session_path_for(cfg.path(), &to.id),
                    "the switch authenticated into the wrong account's token file"
                );
                ResettleReport::Settled
            },
            |_| Ok(()),
        )
        .expect("the removal must succeed");

        assert_eq!(
            settled_onto.into_inner(),
            vec![b.id.clone()],
            "the app did not settle onto the survivor exactly once"
        );
        assert_eq!(
            state.active().id, b.id,
            "the app was left pointing at a removed account"
        );
        assert_eq!(active.id, b.id);
        assert_eq!(
            bw_path::active_data_dir(),
            Some(accounts::data_dir_for(cfg.path(), &b.id)),
            "the CLI was left pointed at the directory the removal deleted"
        );
        assert!(!accounts::data_dir_for(cfg.path(), &a.id).exists());
        assert_eq!(
            dir_entries(&accounts::accounts_root(cfg.path())),
            vec![ACCOUNT_B.to_string()],
            "positive control: the survivor's profile is still there"
        );
        assert_eq!(
            settings::Settings::load(&settings_path).active_account,
            Some(b.id.clone()),
            "settings.json still names the removed account as the active one"
        );

        // The last account, on the same state and the same call.
        let e = remove_account(
            cfg.path(),
            &settings_path,
            &mut state,
            &b.id,
            &mut active,
            &mut store,
            |_, _, _| panic!("the app was resettled to remove the only account it has"),
            |_| panic!("the only account was logged out"),
        )
        .expect_err("the last account was removed");
        assert!(
            e.contains("only account"),
            "the refusal must say why, got: {e}"
        );
        assert_eq!(state.all().len(), 1);
        assert_eq!(state.active().id, b.id);
        assert!(
            accounts::session_path_for(cfg.path(), &b.id).exists(),
            "the refused removal took the last account's token anyway"
        );
        assert!(
            accounts::hello_blob_path_for(cfg.path(), &b.id).exists(),
            "the refused removal took the last account's quick unlock anyway"
        );
        assert_eq!(
            dir_entries(&accounts::accounts_root(cfg.path())),
            vec![ACCOUNT_B.to_string()]
        );
    }

    /// A switch that does not land leaves the account exactly where it was.
    /// `bw serve` is still on the doomed profile at that point -- deleting it
    /// anyway would take the vault the running app is serving.
    #[test]
    fn a_removal_whose_switch_does_not_land_deletes_nothing() {
        let _guard = lock_active_dir();
        let cfg = ScratchConfig::new("remove-nosettle");
        let (a, _b, mut state, mut active, mut store) = an_app_on_a_with_b_beside_it(
            &cfg,
            deskwarden::bw_path::MultiAccountAvailability::Available,
        );
        let settings_path = cfg.path().join("settings.json");

        let e = remove_account(
            cfg.path(),
            &settings_path,
            &mut state,
            &a.id,
            &mut active,
            &mut store,
            |_, _, _| ResettleReport::NotStarted,
            |_| panic!("a profile was logged out for a removal that could not go ahead"),
        )
        .expect_err("an account was removed without the app settling anywhere");
        assert!(
            e.contains("nothing has been deleted"),
            "the failure must say the account is still there, got: {e}"
        );

        assert_eq!(state.all().len(), 2);
        assert_eq!(state.active().id, a.id);
        assert_eq!(active.id, a.id);
        assert!(accounts::session_path_for(cfg.path(), &a.id).exists());
        assert!(accounts::hello_blob_path_for(cfg.path(), &a.id).exists());
        assert_eq!(
            dir_entries(&accounts::accounts_root(cfg.path())),
            vec![ACCOUNT_A.to_string(), ACCOUNT_B.to_string()],
            "a removal that could not go ahead deleted a profile anyway"
        );
        assert!(
            !settings_path.exists(),
            "a removal that did not happen was written to disk"
        );
    }

    /// `remove_dir_all` on a mis-built path -- an empty id, a `..` that slipped
    /// past `parse`, one `parent()` too many -- takes `settings.json`, the log,
    /// and the account list naming the survivors with it. And it would take
    /// the OTHER accounts' profiles too.
    #[test]
    fn a_removal_never_deletes_above_the_accounts_directory() {
        let _guard = lock_active_dir();
        let cfg = ScratchConfig::new("remove-scope");
        let (a, b, mut state, mut active, mut store) = an_app_on_a_with_b_beside_it(
            &cfg,
            deskwarden::bw_path::MultiAccountAvailability::Available,
        );
        let settings_path = cfg.path().join("settings.json");
        let log = cfg.path().join("deskwarden.log");
        std::fs::write(&log, b"a line").unwrap();

        remove_account(
            cfg.path(),
            &settings_path,
            &mut state,
            &b.id,
            &mut active,
            &mut store,
            |_, _, _| panic!("the app was resettled to remove an account it was not on"),
            |_| Ok(()),
        )
        .expect("the removal must succeed");

        assert!(
            !accounts::data_dir_for(cfg.path(), &b.id).exists(),
            "positive control: the removal really did delete the account it was given"
        );
        assert!(log.is_file(), "the log went with the account");
        assert!(cfg.path().is_dir(), "the config directory was deleted");
        assert!(
            accounts::accounts_root(cfg.path()).is_dir(),
            "the accounts root was deleted"
        );
        assert!(
            accounts::data_dir_for(cfg.path(), &a.id).is_dir(),
            "the OTHER account's profile went with it"
        );
        assert!(accounts::data_dir_for(cfg.path(), &b.id)
            .starts_with(accounts::accounts_root(cfg.path())));
    }

    /// The gate, and it is `AccountsState`'s. Where multiple accounts are
    /// unavailable every account shares ONE profile, so `bw logout` in the
    /// doomed account's directory would log out the account the app is on and
    /// the deletion would take a directory the CLI never used.
    #[test]
    fn a_removal_is_refused_while_accounts_state_says_the_account_cannot_be_reached() {
        let _guard = lock_active_dir();
        let cfg = ScratchConfig::new("remove-blocked");
        let (_a, b, mut blocked, mut active, mut store) = an_app_on_a_with_b_beside_it(
            &cfg,
            deskwarden::bw_path::MultiAccountAvailability::BlockedByUnknownCliPath,
        );
        let settings_path = cfg.path().join("settings.json");
        assert!(blocked.switchable().is_empty(), "control: nothing is reachable");

        let e = remove_account(
            cfg.path(),
            &settings_path,
            &mut blocked,
            &b.id,
            &mut active,
            &mut store,
            |_, _, _| panic!("the app was resettled for an account it may not remove"),
            |_| panic!("a profile was logged out for an account that may not be removed"),
        )
        .expect_err("an account was removed while the app could not reach it");
        assert!(
            blocked
                .blocked_reason()
                .is_some_and(|why| e.contains(why)),
            "the refusal must carry the reason the user can act on, got: {e}"
        );
        assert_eq!(blocked.all().len(), 2);
        assert_eq!(
            dir_entries(&accounts::accounts_root(cfg.path())),
            vec![ACCOUNT_A.to_string(), ACCOUNT_B.to_string()],
            "a profile was deleted for an account that may not be removed"
        );
        assert!(!settings_path.exists());

        // The positive control, on the same fixture and the same call: an
        // unblocked state really does get through, so the refusal above is the
        // gate rather than a `remove_account` that refuses everything.
        let mut open = accounts_state(
            deskwarden::bw_path::MultiAccountAvailability::Available,
            blocked.all().to_vec(),
            &account(ACCOUNT_A, "a@example.com"),
        );
        remove_account(
            cfg.path(),
            &settings_path,
            &mut open,
            &b.id,
            &mut active,
            &mut store,
            |_, _, _| panic!("the app was resettled to remove an account it was not on"),
            |_| Ok(()),
        )
        .expect("the unblocked removal must succeed");
        assert_eq!(open.all().len(), 1);
        assert!(!accounts::data_dir_for(cfg.path(), &b.id).exists());
    }

    /// `remove_account`'s own body, for the source guard below.
    ///
    /// Both ends are named and both are controlled: it starts at the function
    /// and stops at the first line of the next item's doc comment, rather than
    /// at a closing brace -- a `"\n}"` needle passes on LF and fails on CRLF,
    /// and this file is CRLF throughout.
    fn the_removal_body() -> &'static str {
        let production = production_half_of_this_file();
        let after = production
            .split_once(concat!("fn remove_", "account("))
            .expect("`remove_account` must still exist")
            .1;
        let body = after
            .split_once(concat!(
                "/// Adds a Bitwarden account: mints one,",
                " signs in to it, and then settles onto"
            ))
            .expect("`add_account`'s doc comment must still follow `remove_account`")
            .0;
        assert!(
            body.len() < after.len(),
            "control: the split isolated a region rather than keeping the rest of the file"
        );
        assert!(
            body.contains(concat!("delete_account", "_dir(")),
            "control: the region really is the removal's own body"
        );
        assert!(
            !body.contains(concat!("discard_prepared", "_account(")),
            "control: the region really stops before `add_account`'s body"
        );
        body
    }

    /// What a removal may not do itself, and the one thing it must.
    ///
    /// Every needle here is invisible to the behavioural tests above. The
    /// `bw logout` is injected so those tests can see WHICH PROFILE it runs
    /// against; a direct `bw_logout()` beside it would log the active account
    /// out with the suite still green. A raw `remove_dir_all` would work in
    /// every test here and skip the one check that stands between a mis-built
    /// path and the user's other vaults. Deleting the secrets by name would
    /// leave the CLI profile itself behind. Rebuilding `AccountsState` or
    /// re-asking the machine would be a second reading of the two facts that
    /// type exists to be the single answer to.
    ///
    /// `hello::enroll_for(` is banned rather than incidental: ONE Windows Hello
    /// credential seals every account, separated by the KDF suffix, so a
    /// removal that re-created it would lock every OTHER account's quick unlock
    /// out. It does not match the `hello::unenroll_for(` the body does call.
    #[test]
    fn a_removal_deletes_through_the_one_guarded_path_and_logs_out_through_the_injected_one() {
        let body = the_removal_body();
        let production = production_half_of_this_file();
        let source_of = |name: &str| {
            std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(name),
            )
            .unwrap_or_else(|e| panic!("cannot read {name}: {e}"))
        };
        let login_ui_source = source_of("login_ui.rs");
        let accounts_source = source_of("accounts.rs");
        let bw_path_source = source_of("bw_path.rs");

        for (banned, control) in [
            (concat!("bw_", "logout("), login_ui_source.as_str()),
            (concat!("remove_dir", "_all("), accounts_source.as_str()),
            (concat!("remove_", "file("), production),
            (concat!("AccountsState", "::new("), production),
            (
                concat!("multi_account_", "availability("),
                bw_path_source.as_str(),
            ),
            (concat!("hello::", "enroll_for("), login_ui_source.as_str()),
            (concat!("fatal_startup", "_error("), production),
        ] {
            assert!(
                control.contains(banned),
                "control: `{banned}` is really spelled that way, so the ban below is not vacuous"
            );
            assert!(
                !body.contains(banned),
                "`{banned}` is reachable from `remove_account`, and no test above can see it"
            );
        }

        // And the one thing that has no behavioural witness: the sealed master
        // password is dropped BEFORE the directory, so a profile that cannot be
        // deleted -- a `bw` still holding `data.json` open -- does not leave one
        // behind for an account this app has forgotten.
        let required = concat!("unenroll", "_for(");
        assert!(
            source_of("hello.rs").contains(required),
            "control: `{required}` is really spelled that way"
        );
        assert!(
            body.contains(required),
            "`{required}` is gone from `remove_account`: the whole directory usually takes the \
             blob with it, and on the one occasion it does not, a master password stays sealed \
             on disk for an account nothing names"
        );
    }

    /// A `bw serve` that answers one item carrying an app match, plus the
    /// folders every populate also fetches.
    fn sync_server() -> mockito::ServerGuard {
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{"success":true,"data":{{"data":[{}]}}}}"#,
                vault_item_with_match("1", "notepad.exe")
            ))
            .create();
        server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[]}}"#)
            .create();
        server
    }

    /// The ordinary case, so the guard below cannot pass by simply
    /// discarding everything: nothing cleared the vault underneath this
    /// sync, so its own entries arm the engine.
    #[test]
    fn a_completed_sync_arms_the_engine_from_the_entries_it_fetched() {
        let server = sync_server();
        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let outcome = sync_outcome_from(&cache, cache.epoch(), None);

        let mut engine = MatchEngine::new();
        // No `{other:?}` here, and `SettledSync` deliberately does not derive
        // `Debug`: its applicable variant carries vault items, and this is a
        // type whose whole job is to be matched on rather than printed.
        match settle_sync_outcome(outcome, &cache) {
            SettledSync::Applicable { items } => engine.rebuild(&match_entries(&items)),
            SettledSync::NothingToApply => panic!("expected a refreshed sync, got NothingToApply"),
            SettledSync::Failed(e) => panic!("expected a refreshed sync, got Failed({e})"),
        }
        assert!(
            engine.lookup(&foreground("notepad.exe")).is_some(),
            "a sync nothing interfered with must still refresh the match engine"
        );
    }

    /// Review 17's third finding. `SyncOutcome::Refreshed` used to be a unit
    /// variant, and `apply_backend_op` answered it by rebuilding the engine
    /// from a RE-READ of `cache.items()` on the main thread. That read
    /// happens after the worker sent the outcome, and a lock recovery can
    /// land in between -- reachable because `open_vault_window` abandons an
    /// in-flight backend operation at `BACKEND_OP_TIMEOUT` and carries on
    /// into a recovery that clears the cache. If that recovery then went
    /// `Ready` with a failing `list_folders` (engine armed from the probe's
    /// items, cache empty by design), the late sync rebuilt the engine from
    /// nothing and disarmed autofill for the whole session -- review 15's
    /// finding through a different door.
    ///
    /// Review 18 removed the entries again and left the epoch to close this,
    /// which is why the test is worth reading twice: the recovery's own
    /// `cache.clear()` starts a new epoch, so the outcome is discarded before
    /// anything is read from the empty cache. The engine the recovery armed
    /// is left exactly as it is -- the same assertion as before, now resting
    /// on the epoch rather than on frozen entries.
    #[test]
    fn a_late_sync_result_cannot_disarm_an_engine_a_lock_recovery_just_armed() {
        let server = sync_server();
        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let outcome = sync_outcome_from(&cache, cache.epoch(), None);

        // The lock recovery: clears the cache, then arms the engine from its
        // own readiness probe while `populate_with`'s folders request fails.
        cache.clear();
        let mut engine = MatchEngine::new();
        engine.rebuild(&match_entries(&probe_items(&[("2", "code.exe")])));

        assert!(
            matches!(
                settle_sync_outcome(outcome, &cache),
                SettledSync::NothingToApply
            ),
            "a sync whose snapshot was cleared before its result reached the main thread must \
             not be applied to anything"
        );
        assert!(
            engine.lookup(&foreground("code.exe")).is_some(),
            "the engine the recovery armed must survive the late arrival"
        );
        assert!(
            engine.lookup(&foreground("notepad.exe")).is_none(),
            "the pre-clear vault's matches must not be armed by the late arrival"
        );
    }

    /// The same guard in the shape with the worst consequence, and the one
    /// review 18's redesign had to be checked against hardest: the recovery
    /// SUCCEEDS and repopulates for a DIFFERENT account, so at settle time
    /// the cache is populated and non-empty. Since the engine is now built
    /// from the cache at apply time rather than from the sync's own frozen
    /// entries, getting this wrong would not merely lose a refresh -- it
    /// would arm account B's engine on a click that belongs to account A, and
    /// (the mirror of it) an unguarded apply would write A's matches over B.
    /// The epoch is what closes it: `clear` starts a new one, and nothing
    /// afterwards can make the old one current again.
    #[test]
    fn a_late_sync_from_a_previous_account_is_discarded_even_though_the_cache_refilled() {
        let server = sync_server();
        let cache = VaultCache::new(VaultBridge::new(server.url()));
        // Account A's sync: fetched, populated, outcome queued.
        let outcome = sync_outcome_from(&cache, cache.epoch(), None);

        // Lock, re-authentication into account B, and a recovery that works:
        // the cache is cleared and repopulated from B's own readiness probe.
        cache.clear();
        let epoch_b = cache.epoch();
        let refilled = cache.populate_with(probe_items(&[("2", "code.exe")]), epoch_b);
        assert_eq!(refilled.unwrap(), PopulateOutcome::Populated);
        let mut engine = MatchEngine::new();
        engine.rebuild(&match_entries(&probe_items(&[("2", "code.exe")])));

        assert!(
            matches!(
                settle_sync_outcome(outcome, &cache),
                SettledSync::NothingToApply
            ),
            "a sync from the previous account must be discarded even when the cache has since \
             been refilled for the new one"
        );
        assert!(
            engine.lookup(&foreground("code.exe")).is_some(),
            "the new account's matches must stay armed"
        );
        assert!(
            engine.lookup(&foreground("notepad.exe")).is_none(),
            "the previous account's matches must not be armed after an account switch"
        );
    }

    /// Review 18's third finding, and the composed case that matters: a sync
    /// in flight, a WRITE landing while it is in flight, the sync's outcome
    /// applying afterwards. The write must survive.
    /// `bw serve` answers a PUT with the item as it now holds it, carrying a
    /// bumped `revisionDate` -- and the app must adopt that copy, because the
    /// value it SENT holds a token the write has already superseded (see
    /// `vault_bridge`'s `REVISION_DATE_KEY`). A mock that answers an empty
    /// 200 models no backend at all, which is how that defect stayed
    /// invisible. The lib's own `vault_bridge::echoing_item_put` is
    /// `#[cfg(test)]`, so it is not reachable from this binary's tests; this
    /// is the same shape, locally.
    fn echoing_item_put(server: &mut mockito::Server, path: &str) -> mockito::Mock {
        server
            .mock("PUT", path)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body_from_request(|req| {
                let sent = req.body().expect("a PUT this app makes always carries a body");
                let mut item: serde_json::Value =
                    serde_json::from_slice(sent).expect("this app's write bodies are JSON");
                if let Some(map) = item.as_object_mut() {
                    map.insert(
                        "revisionDate".to_string(),
                        serde_json::json!("2026-08-03T02:33:03.427Z"),
                    );
                }
                serde_json::json!({ "success": true, "data": item }).to_string().into_bytes()
            })
    }

    #[test]
    fn an_app_match_saved_while_a_sync_was_in_flight_survives_that_sync() {
        const ONE_ITEM_NO_MATCH: &str =
            r#"{"success":true,"data":{"data":[{"id":"1","name":"1","type":1,"fields":[]}]}}"#;
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(ONE_ITEM_NO_MATCH)
            .create();
        server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[]}}"#)
            .create();
        echoing_item_put(&mut server, "/object/item/1").create();

        let cache = VaultCache::new(VaultBridge::new(server.url()));
        // The tray Sync worker: its `list_items` returns the vault as it was
        // BEFORE the save below, and its outcome sits in the channel while
        // the picker windows block the main thread.
        let outcome = sync_outcome_from(&cache, cache.epoch(), None);

        // "Add app...": the user saves a match through the cache, and
        // the save path's rebuild from the cache arms the engine from it.
        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        cache
            .set_app_match(
                &item,
                &deskwarden::app_match::AppMatch::for_process("notepad.exe", deskwarden::app_match::TriggerMode::Auto),
            )
            .unwrap();
        let mut engine = MatchEngine::new();
        engine.rebuild(&match_entries(&cache.items()));
        assert!(
            engine.lookup(&foreground("notepad.exe")).is_some(),
            "the save itself must arm the engine"
        );

        // Only now does the main thread drain the sync outcome. Nothing
        // cleared the cache, so it is applied -- and applying it must not
        // reinstate the pre-save vault the worker happened to have fetched.
        match settle_sync_outcome(outcome, &cache) {
            SettledSync::Applicable { items } => engine.rebuild(&match_entries(&items)),
            SettledSync::NothingToApply => panic!(
                "a write is not a supersession: the vault session is the same one, so this sync \
                 is still applicable"
            ),
            SettledSync::Failed(e) => panic!("expected a refreshed sync, got Failed({e})"),
        }
        assert!(
            engine.lookup(&foreground("notepad.exe")).is_some(),
            "the app match the user saved while the sync was in flight was silently dropped by \
             the sync's own, older item list"
        );
    }

    /// REVIEW 21'S CRITICAL, composed, and the ORDERING THE SUITE ABOVE DOES
    /// NOT COVER. `an_app_match_saved_while_a_sync_was_in_flight_survives_that
    /// _sync` runs `sync_outcome_from` to completion *before* the save, so the
    /// save always lands on top of a populate that has already finished -- the
    /// one ordering that worked. Here the save lands INSIDE the sync's fetch
    /// window, which is `spawn_sync`'s `!currently_running` branch exactly:
    /// mark captured before anything, the readiness probe's `list_items`
    /// handed to `sync_outcome_from` as `probe_items`, and `populate_with`
    /// writing that fetch back afterwards. That fetch predates the save, and
    /// before the fix it was assigned to the snapshot wholesale.
    ///
    /// It asserts the survival TWICE, because the two consequences are
    /// different in kind: in what reaches the match engine (autofill dead
    /// until the next sync -- session-scoped) and in the CACHE (a later
    /// vault-window edit PUTs the stale item back, and the item's `fields`
    /// array is always present in that body, so `bw serve`'s
    /// merge-on-omitted-keys behaviour cannot save it -- permanent).
    #[test]
    fn an_app_match_saved_while_a_syncs_fetch_was_in_flight_survives_that_sync() {
        const ONE_ITEM_NO_MATCH: &str =
            r#"{"success":true,"data":{"data":[{"id":"1","name":"1","type":1,"fields":[]}]}}"#;
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(ONE_ITEM_NO_MATCH)
            .create();
        server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[]}}"#)
            .create();
        echoing_item_put(&mut server, "/object/item/1").create();

        let cache = VaultCache::new(VaultBridge::new(server.url()));
        // Startup, so the cache is populated exactly as it is when the tray
        // Sync item can be clicked at all.
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);

        // The tray Sync worker, up to the point its fetch has happened and
        // nothing has been written back yet.
        let sync_epoch = cache.epoch();
        let probe = cache.bridge().list_items().unwrap();

        // "Add app...": two picker windows, then a save through the cache --
        // all of it inside the window above, because the handler does not
        // block on `backend_task_in_progress`.
        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        cache
            .set_app_match(
                &item,
                &deskwarden::app_match::AppMatch::for_process("notepad.exe", deskwarden::app_match::TriggerMode::Auto),
            )
            .unwrap();
        let mut engine = MatchEngine::new();
        engine.rebuild(&match_entries(&cache.items()));
        assert!(
            engine.lookup(&foreground("notepad.exe")).is_some(),
            "the save itself must arm the engine"
        );

        // Only now does the worker write its (older) fetch back and report.
        let outcome = sync_outcome_from(&cache, sync_epoch, Some(probe));
        match settle_sync_outcome(outcome, &cache) {
            SettledSync::Applicable { items } => engine.rebuild(&match_entries(&items)),
            SettledSync::NothingToApply => panic!(
                "a write is not a supersession: the vault session is the same one, so this sync \
                 is still applicable"
            ),
            SettledSync::Failed(e) => panic!("expected a refreshed sync, got Failed({e})"),
        }

        assert!(
            engine.lookup(&foreground("notepad.exe")).is_some(),
            "the app match the user saved while the sync was FETCHING was reverted by the \
             sync's populate, so the engine was rebuilt without it"
        );
        assert!(
            cache
                .items()
                .iter()
                .find(|i| i.id == "1")
                .unwrap()
                .fields
                .iter()
                .any(|f| f.name.as_deref() == Some(deskwarden::app_match::APP_MATCH_FIELD_NAME)),
            "the CACHE lost the app match, which is the worse half: the next edit of this item \
             PUTs the stale copy back and the loss stops being session-scoped"
        );
    }

    /// Review 28's Important 1. `settle_sync_outcome` asked
    /// `items_unless_superseded`, which collapses `Superseded` and
    /// `Unpopulated` into one `None`, and then logged "after the vault was
    /// cleared (era X -> Y)" -- a reason it had not distinguished. It read
    /// TRUE only by a chain of facts held elsewhere (`Refreshed{era}` is
    /// emitted solely after a successful populate, and only `clear` can
    /// unpopulate), so the moment anything else can unpopulate the snapshot
    /// the post-mortem starts naming an event that did not happen.
    #[test]
    fn a_discarded_syncs_log_line_names_a_clear_only_when_a_clear_is_what_happened() {
        // Real eras from a real cache: `VaultEra`'s inner counter is private
        // precisely so nothing outside `vault_cache` can mint one, and a test
        // that had to would be testing a different type than production uses.
        let cache = VaultCache::new(VaultBridge::new("http://127.0.0.1:1".to_string()));
        let before = cache.epoch().era();
        cache.clear();
        let after = cache.epoch().era();

        let superseded = sync_discard_reason(VaultUnavailable::Superseded, before, after);
        assert!(
            superseded.contains("cleared") && superseded.contains("era 0 -> 1"),
            "a superseded era IS a clear, and the era transition is the evidence: {superseded}"
        );

        let unpopulated = sync_discard_reason(VaultUnavailable::Unpopulated, after, after);
        assert!(
            !unpopulated.contains("cleared"),
            "the cache being unpopulated in the sync's OWN era is not a clear, and claiming one \
             sends the reader looking for a lock/re-auth that never happened: {unpopulated}"
        );
        assert!(
            !unpopulated.contains("->"),
            "there is no era transition to report when the era never moved: {unpopulated}"
        );

        // REVIEW 30'S MINOR 3. The `Unpopulated` line used to say "in this
        // sync's own era (X) -- nothing started a new one" while DISCARDING
        // `current_era` entirely, so it asserted in the present tense a fact it
        // had not observed: a `clear` landing between the checked read and the
        // caller's `cache.epoch().era()` makes "nothing started a new one"
        // false, and the doc excuses exactly that staleness for the OTHER arm
        // while the arm making the stronger claim went unmentioned. The
        // decision is still made entirely by the single checked read -- this is
        // not a second read seam -- so the fix is to stop claiming what was not
        // observed, using the parameter the function already takes.
        let raced = sync_discard_reason(VaultUnavailable::Unpopulated, before, after);
        assert!(
            raced.contains("(0)") && raced.contains("era 1"),
            "when the era HAS moved on by the time the line is written, the line must report \
             both rather than assert nothing started a new one: {raced}"
        );
        assert!(
            !raced.contains("nothing"),
            "and it must not keep the claim that nothing started a new era, which is the one \
             thing this case disproves: {raced}"
        );
    }

    /// Review 30's Important 1. A `ServerOnly` whose id is absent from a
    /// POPULATED, same-era snapshot must arm NOTHING for that item.
    ///
    /// This test used to assert the opposite -- that the match was pushed onto
    /// the entries anyway -- on the ground that "the match is what autofill
    /// needs, not the item's existence". Every consumer resolves the item BY
    /// ID (`app::handle_match` looks it up in the cache; `fill_from_vault`
    /// falls through to `bridge().get_item`), so an entry for an id nothing
    /// can resolve arms a match that can only fail: Prompt mode shows an
    /// anonymous "fill something?" overlay whose Fill button 404s into a
    /// `log::error!`, and Auto mode does the same round-trip silently on every
    /// foregrounding until the next sync or unlock.
    ///
    /// And it fires precisely when the evidence says the item is GONE: the
    /// target came from `pick_vault_item` -> `load_items_for_picker` -> the
    /// snapshot in this same era, so it WAS there at pick time; for a
    /// populated same-era snapshot to lack it now, a populate's fetch dropped
    /// it. A missing match is silent, correct and self-heals; a firing match
    /// that never fills teaches the user to distrust autofill.
    #[test]
    fn a_server_only_save_arms_no_match_for_an_item_the_snapshot_has_lost() {
        let target: deskwarden::vault_bridge::VaultItem =
            serde_json::from_str(r#"{"id":"7","name":"Seven","type":1,"fields":[]}"#)
                .expect("the fixture must deserialize");
        let saved = deskwarden::picker_ui::SavedAppMatch {
            app_match: deskwarden::app_match::AppMatch::for_process("notepad.exe", deskwarden::app_match::TriggerMode::Auto),
            write: deskwarden::vault_cache::AppMatchWrite::ServerOnly,
        };
        // A populated, same-era snapshot that holds another item but not
        // item 7 -- so "armed nothing at all" cannot pass this by accident.
        let snapshot = Ok(deskwarden::vault_cache::VaultSnapshot {
            items: probe_items(&[("1", "calc.exe")]),
            folders: vec![],
        });

        let items = add_app_rebuild_source(snapshot, &saved, &target)
            .expect("a populated same-era snapshot is still the right thing to arm from");
        assert!(
            items.iter().all(|i| i.id != "7"),
            "an item the snapshot has lost must not be synthesised back into the engine's source"
        );
        let entries = match_entries(&items);
        assert_eq!(
            entries.len(),
            1,
            "only the snapshot's own entries: {entries:?}"
        );
        assert_eq!(
            entries[0].0, "1",
            "the surviving entry is the snapshot's, not the phantom"
        );
    }

    /// Review 30's Minor 5. The era capture, the two picker windows and the
    /// rebuild used to be four unbound statements in `main`'s event loop, so
    /// moving the capture below `pick_vault_item` -- an innocent-looking tidy,
    /// since that call captures its own era internally -- silently narrowed
    /// the guard to the second window and NO test noticed. `AddAppFlow` is
    /// what binds them; this pins the half that can be tested without an event
    /// loop, that the era it captured still outranks the rebuild.
    #[test]
    fn the_add_app_flows_captured_era_outranks_the_rebuild_after_a_clear() {
        let target: deskwarden::vault_bridge::VaultItem =
            serde_json::from_str(r#"{"id":"1","name":"One","type":1,"fields":[]}"#)
                .expect("the fixture must deserialize");
        let saved = deskwarden::picker_ui::SavedAppMatch {
            app_match: deskwarden::app_match::AppMatch::for_process("notepad.exe", deskwarden::app_match::TriggerMode::Auto),
            write: deskwarden::vault_cache::AppMatchWrite::WroteThrough,
        };
        // No server needed: a `clear` supersedes without any request, which is
        // the fact under test.
        //
        // REVIEW 31'S IMPORTANT 2. This used to read `AddAppFlow { era:
        // cache.epoch().era() }` -- a literal that proved the type did NOT make
        // the wrong ordering unrepresentable, since the same literal compiles
        // anywhere in this file. The fields are now private to `mod add_app`
        // and this goes through `begin_without_the_picker`, which shares
        // `begin`'s own capture.
        let cache = std::sync::Arc::new(VaultCache::new(VaultBridge::new(
            "http://127.0.0.1:1".to_string(),
        )));
        let flow = AddAppFlow::begin_without_the_picker(cache.clone());
        cache.clear();

        assert!(
            flow.rebuild_source(&saved, &target).is_none(),
            "a vault session the user did not just edit must arm nothing"
        );
    }

    /// The ordinary case, so the test above cannot pass by unconditionally
    /// injecting the match: a write-through save arms from the snapshot as it
    /// stands, which already holds it plus anything else written since.
    #[test]
    fn a_wrote_through_save_arms_the_engine_from_the_snapshot_as_it_stands() {
        let target: deskwarden::vault_bridge::VaultItem =
            serde_json::from_str(r#"{"id":"7","name":"Seven","type":1,"fields":[]}"#)
                .expect("the fixture must deserialize");
        let saved = deskwarden::picker_ui::SavedAppMatch {
            app_match: deskwarden::app_match::AppMatch::for_process("notepad.exe", deskwarden::app_match::TriggerMode::Auto),
            write: deskwarden::vault_cache::AppMatchWrite::WroteThrough,
        };
        let snapshot = Ok(deskwarden::vault_cache::VaultSnapshot {
            items: probe_items(&[("1", "calc.exe")]),
            folders: vec![],
        });

        let items = add_app_rebuild_source(snapshot, &saved, &target)
            .expect("a populated same-era snapshot must arm the engine");
        let entries = match_entries(&items);
        assert_eq!(entries.len(), 1, "the snapshot's own entry, and only it");
        assert_eq!(entries[0].0, "1");
    }

    /// A `ServerOnly` whose id the snapshot DOES hold by the time `main` gets
    /// here (a populate landed in between and its fetch carried the item)
    /// must be replaced, not appended: two entries for one vault item would
    /// make the engine's behaviour depend on iteration order.
    #[test]
    fn a_server_only_save_replaces_rather_than_duplicates_an_id_the_snapshot_holds() {
        let target: deskwarden::vault_bridge::VaultItem =
            serde_json::from_str(r#"{"id":"1","name":"One","type":1,"fields":[]}"#)
                .expect("the fixture must deserialize");
        let saved = deskwarden::picker_ui::SavedAppMatch {
            app_match: deskwarden::app_match::AppMatch::for_process("notepad.exe", deskwarden::app_match::TriggerMode::Auto),
            write: deskwarden::vault_cache::AppMatchWrite::ServerOnly,
        };
        let snapshot = Ok(deskwarden::vault_cache::VaultSnapshot {
            items: probe_items(&[("1", "calc.exe")]),
            folders: vec![],
        });

        let items = add_app_rebuild_source(snapshot, &saved, &target)
            .expect("a populated same-era snapshot must arm the engine");
        assert_eq!(items.len(), 1, "one vault item, one entry in the list");
        let entries = match_entries(&items);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].1.process, "notepad.exe",
            "the just-saved match is newer truth than whatever the snapshot's copy carried"
        );
    }

    /// The guard the era exists for, and it outranks the injection above: a
    /// `clear` inside the two picker windows means the snapshot belongs to a
    /// vault session the user did not just edit, and arming ANYTHING from it
    /// -- including this flow's own match -- would disarm autofill for the
    /// session that actually exists.
    #[test]
    fn a_superseded_era_arms_nothing_even_for_a_server_only_save() {
        let target: deskwarden::vault_bridge::VaultItem =
            serde_json::from_str(r#"{"id":"7","name":"Seven","type":1,"fields":[]}"#)
                .expect("the fixture must deserialize");
        let saved = deskwarden::picker_ui::SavedAppMatch {
            app_match: deskwarden::app_match::AppMatch::for_process("notepad.exe", deskwarden::app_match::TriggerMode::Auto),
            write: deskwarden::vault_cache::AppMatchWrite::ServerOnly,
        };

        for refusal in [VaultUnavailable::Superseded, VaultUnavailable::Unpopulated] {
            assert!(
                add_app_rebuild_source(Err(refusal), &saved, &target).is_none(),
                "{refusal:?} must leave the engine exactly as it is"
            );
        }
    }

    // `is_pid_running` used to live here: a `tasklist` spawn whose stdout was
    // substring-searched for the pid, used by the four tests that prove a
    // `Child` was stopped rather than merely dropped. It is gone, replaced by
    // `ProcessWatch` (see its doc). Two reasons, and the first is the one that
    // was actually costing runs:
    //
    //   1. It answered by starting a whole extra process and enumerating the
    //      system's process table -- measured at ~6.0s per call on the
    //      developer's machine, unloaded and repeatably, and past 60s under the
    //      suite's own parallelism. In a 40-run sample of the bin suite, one
    //      run took 89s with SIX tests over the 60-second mark, and every one
    //      of the six was a test that spawns processes. Those tests were not
    //      racing on anything they assert; they were racing the clock.
    //   2. A pid is not an identity. `stop_bw_serve` kills and then `wait()`s,
    //      and `wait()` lets the number go, so "is this pid running?" is a
    //      question about a number the OS is free to reissue. Windows was
    //      measured NOT to reissue promptly (0 of 1500 on the next spawn), so
    //      this was not the trigger -- but a held handle removes it as a future
    //      one for free, because a pid cannot be recycled while a handle to the
    //      process object is open.
    //
    // Not replaced with a sleep-then-look: a sleep long enough to be reliable
    // is slow every other time, and one short enough to be fast is the flake
    // again.

    // ---------------------------------------------------------------------
    // Task 11 -- startup: resolve, point, resume.
    //
    // `fn main()` never returns and opens real windows, so none of it is
    // reachable from a test. Everything it DECIDES lives in
    // `accounts::resolve_startup`, which is pure and is driven through every
    // branch in `accounts.rs`. What is left here is the wiring -- the part
    // that has twice in this feature's history been the whole bug -- and the
    // only handle on it is its own source.
    //
    // Every needle below is `concat!`-split so none matches its own
    // declaration, single-line so none turns on LF versus CRLF (this file is
    // entirely CRLF), and paired with a positive control so a scan that read
    // nothing cannot pass.
    // ---------------------------------------------------------------------

    /// The startup block, from the availability probe to the point where the
    /// account-aware part is done and `fill_stats` resumes.
    ///
    /// Cut at a named comment and a named statement rather than at braces for
    /// the CRLF reason above, and both ends are controlled: the region must be
    /// shorter than what it was cut from, and must contain the resolution call
    /// it exists to talk about.
    fn the_startup_account_block() -> &'static str {
        let production = production_half_of_this_file();
        let after = production
            .split_once(concat!("// Which account is ", "this launch?"))
            .expect("the startup account block must still be there and still say so")
            .1;
        let block = after
            .split_once(concat!("let fill_stats_path", " ="))
            .expect("`fill_stats_path` must still be the statement after the account block")
            .0;
        assert!(
            block.len() < after.len(),
            "control: the split isolated a region rather than keeping the rest of the file"
        );
        assert!(
            block.contains(concat!("accounts::resolve", "_startup(")),
            "control: the region really is the startup account block"
        );
        block
    }

    /// The plan's own ordering test. The account has to be resolved before
    /// its directory can be created, the directory has to exist before a
    /// `SessionStore` is built inside it, and the CLI and the store both have
    /// to be pointed at that account before `store.load()` and before
    /// `check_bw_status_with_session` spawns `bw` -- or a token is validated
    /// against the wrong profile and silently re-authenticated, which the user
    /// reads as "the update lost my login".
    ///
    /// Pure ordering, and invisible to any value-based test: every one of
    /// these mutations leaves an app that starts, signs in, and works.
    #[test]
    fn startup_points_the_cli_at_the_account_before_it_loads_a_session() {
        // The production half, not the whole file: a needle that also appears
        // in this module would otherwise let a deleted call site be satisfied
        // by its own test.
        let source = production_half_of_this_file();
        let at = |what: &str, needle: &str| {
            source
                .find(needle)
                .unwrap_or_else(|| panic!("startup no longer {what} (`{needle}` is not there)"))
        };
        let resolve_at = at("resolves an account", concat!("accounts::resolve", "_startup("));
        let ensure_at = at(
            "creates the account's directory",
            concat!("accounts::ensure_account", "_dir("),
        );
        let set_dir = at("points the CLI", concat!("set_active_data", "_dir("));
        let build_store = at("builds a token store", concat!("SessionStore", "::new("));
        let load = at("reads a cached token", concat!("store", ".load()"));

        assert!(
            resolve_at < ensure_at,
            "the account's directory is created before there is an account to create it for"
        );
        assert!(
            ensure_at < build_store,
            "the token store is built for a directory that may not exist yet, and `SessionStore` \
             does not create its own parent: the first save fails and the master password is \
             asked for on every launch, forever"
        );
        assert!(
            set_dir < build_store,
            "the CLI is pointed at the account AFTER the store is built"
        );
        assert!(
            build_store < load,
            "a cached session token is read before there is a store pointed at the account"
        );

        // Positive control: five distinct positions were found, so the
        // assertions above are between five real call sites rather than
        // between repeated hits on one.
        let mut all = vec![resolve_at, ensure_at, set_dir, build_store, load];
        let n = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), n, "two of the five needles found the same position");
    }

    /// The wire `fn main()` makes untestable: `resolve_startup` computes
    /// `first_run` and startup has to turn it into the id every login window
    /// is gated on.
    ///
    /// The mutation is a one-line revert -- `let first_run_account = None` --
    /// and it is invisible in every end state: the app starts, signs in and
    /// works, `login_context`'s own decision stays correct, `login_ui`'s notice
    /// tests stay green, and the one login window every new user meets asks
    /// for a master password on a machine they were already signed in on with
    /// nothing to say why. Found by exactly that mutation, which the value
    /// test below did not catch.
    #[test]
    fn the_minted_accounts_id_is_what_the_first_run_notice_is_gated_on() {
        let block = the_startup_account_block();
        let at = |what: &str, needle: &str| {
            block
                .find(needle)
                .unwrap_or_else(|| panic!("startup no longer {what} (`{needle}` is not there)"))
        };
        let derived = at(
            "derives the first-run account from what the resolution answered",
            concat!("first_run.", "then("),
        );
        let passed = at(
            "hands that id to the login context",
            concat!("first_run_account", ".as_ref()"),
        );
        assert!(
            derived < passed,
            "the login context is built before the minted account is known, so it cannot be \
             carrying it"
        );
        // Positive controls on the same region and the same reader: it really
        // is the region that builds login contexts, and the two needles found
        // two different places rather than one twice.
        assert!(
            block.contains(concat!("login", "_context(")),
            "control: the sliced region does not build a login context at all"
        );
        assert_ne!(derived, passed, "control: one needle found both positions");
    }

    /// The first-run notice is offered to **exactly one** account: the one
    /// this launch minted.
    ///
    /// A value test on the one constructor, because the two mutations that
    /// matter are both invisible in an end state. `first_run: true` for every
    /// window puts a "sign in once to set this one up" line in front of a user
    /// switching between two accounts they set up months ago. `first_run:
    /// false` always deletes the notice while leaving the constant, the
    /// painting code and every one of `login_ui`'s own tests green -- the user
    /// meets an unexplained master-password prompt and the suite says nothing.
    #[test]
    fn the_first_run_notice_is_offered_to_exactly_one_account() {
        let cfg = std::path::Path::new(r"C:\cfg");
        let minted = Account {
            id: accounts::AccountId::parse(&"a".repeat(32)).unwrap(),
            email: String::new(),
            server_url: None,
        };
        let other = Account {
            id: accounts::AccountId::parse(&"b".repeat(32)).unwrap(),
            email: "someone@example.com".to_string(),
            server_url: None,
        };

        assert!(
            login_context(cfg, Some(&minted), Some(&minted.id)).first_run,
            "the account this launch minted is not told why it is being asked to sign in"
        );
        assert!(
            !login_context(cfg, Some(&other), Some(&minted.id)).first_run,
            "a switch to an account that already existed shows the first-run notice"
        );
        assert!(
            !login_context(cfg, Some(&minted), None).first_run,
            "a launch that minted nothing still shows the notice, so it shows on every launch \
             forever"
        );
        assert!(
            !login_context(cfg, None, Some(&minted.id)).first_run,
            "an account-less window claims to be a first run for an account it does not have"
        );
        // Positive control on the same calls: the account really is being
        // carried, so the assertions above are not all reading a context that
        // was never built.
        assert!(login_context(cfg, Some(&minted), None).account.is_some());
    }

    /// The app's worst survivable startup does not happen in silence.
    ///
    /// `StartupAccounts::NoAccountList` is no switcher, no accounts submenu
    /// and a signed-out CLI, and it repeats identically every launch until
    /// `settings.json` is fixed or removed. The only way in is a settings file
    /// that exists and cannot be parsed — which the user can act on, but only
    /// if they are told which file and that it is the reason. Naming it in
    /// `deskwarden.log` alone leaves a user who never opens it with a
    /// permanently account-less app and no idea why.
    ///
    /// Fails without the fix: delete the `message_box` from that arm.
    #[test]
    fn the_account_less_startup_says_so_on_screen_and_says_why() {
        let source = production_half_of_this_file();
        let arm = source
            .split_once(concat!("StartupAccounts::", "NoAccountList { reason } =>"))
            .expect("startup must still have an account-less arm")
            .1;
        // Bounded by the arm's own end — the tuple it evaluates to — and not
        // by a byte count. A fixed window is the shape that already overran
        // into a neighbouring arm in this file and passed on its text.
        let arm = arm
            .split_once("(None, None, None)")
            .expect("the account-less arm must still evaluate to no account and no state")
            .0;
        // From the `message_box(` onwards, NOT the whole arm. The arm already
        // holds a `log::warn!("{reason}; ...")`, so an assertion over the arm
        // is satisfied by the log line this test exists to say is not enough.
        let notice = arm
            .split_once(concat!("message", "_box("))
            .unwrap_or_else(|| {
                panic!(
                    "the account-less startup shows the user nothing: the app has no switcher, \
                     no accounts submenu and a signed-out CLI, and the only trace is a log line"
                )
            })
            .1;
        assert!(
            notice.contains("{reason}"),
            "a notice is shown but the reason is not in it — which is the half that names the \
             file the user has to fix. It reads: {notice:?}"
        );
        // Short needles, never one spanning the wrap: the literal is
        // line-wrapped with `\` continuations in the source this reads, so a
        // needle that crossed a wrap could never match however right the
        // message is.
        for phrase in [
            "could not read its account list",
            "Nothing has been deleted",
            "start Deskwarden again",
        ] {
            assert!(
                notice.contains(phrase),
                "the notice no longer tells the user what happened or what to do about it \
                 ({phrase:?} is not in it): {notice:?}"
            );
        }
        // Positive controls: the region really is that arm, and it is an arm
        // rather than the rest of the file.
        assert!(
            arm.contains(concat!("log::", "warn!")),
            "control: the sliced region is not the account-less arm"
        );
        assert!(
            arm.len() < source.len() / 4,
            "control: the slice is {} of {} bytes, so it is not one arm and these assertions \
             could be about anything",
            arm.len(),
            source.len()
        );
    }

    /// The debt Task 7 left and named: it made `run_login_flow_for` take an
    /// account and `run_login_flow` pass `None`, because startup had none to
    /// give. It has one now.
    ///
    /// The mutation this exists for is a one-word revert -- `login.account`
    /// back to `None` -- and it is invisible in every other way: the app
    /// starts, signs in, and works. The only difference is that the quick
    /// unlock panel is never offered in the one login window every user meets,
    /// and there is no account-less fallback that could offer it, because the
    /// only derivation available without an account id is the empty KDF suffix
    /// (`accounts::hello_kdf_suffix_for`) -- one account sealed under which is
    /// one account's master password every other account can open.
    #[test]
    fn every_login_window_this_process_opens_is_scoped_to_an_account() {
        let production = production_half_of_this_file();
        let call = concat!("run_login", "_flow(");
        assert!(
            production.contains(call),
            "control: `{call}` is really spelled that way in this file"
        );
        assert_eq!(
            production.matches(call).count(),
            1,
            "there is more than one fatal login call site; each is a chance to pass no account"
        );
        let args = production
            .split_once(call)
            .expect("the fatal login wrapper must still be called")
            .1;
        let args = &args[..args.len().min(120)];
        assert!(
            args.contains(concat!("login.", "account")),
            "the startup login window is opened without an account, so it offers no Windows \
             Hello quick unlock at all -- got `{args}`"
        );
        assert!(
            args.contains(concat!("login.", "first_run")),
            "the first-run notice never reaches the window that shows it, so the one login \
             window every new user meets asks for a master password and says nothing about why \
             -- got `{args}`"
        );

        // And nothing rebuilds the context beside the one construction, which
        // is what makes the assertions above cover every window rather than
        // one call site.
        assert_eq!(
            production.matches(concat!("LoginContext", " {")).count(),
            1,
            "a second `LoginContext` is a second answer to which account a login window seals a \
             master password for"
        );
    }

    /// Task 10 shipped `AccountsState` with no production caller, and nothing
    /// asserted that a real caller would ASK it rather than decide for itself.
    ///
    /// The mutation: startup reading `availability.explanation()` a second
    /// time for its own log line instead of `blocked_reason()`. It gives the
    /// same answer today and drifts silently the first time `AccountsState`
    /// learns to block for a reason the availability alone does not carry.
    #[test]
    fn startup_asks_accounts_state_what_it_needs_rather_than_deciding_for_itself() {
        let block = the_startup_account_block();
        for needle in [
            concat!("AccountsState::blocked", "_reason"),
            concat!("accounts::AccountsState", "::new("),
        ] {
            assert!(
                block.contains(needle),
                "startup does not go through `{needle}`: whether this process may switch is \
                 `AccountsState`'s answer and nothing else's"
            );
        }
        // Positive control on the same region and the same reader: a needle
        // spelled the same way that is deliberately NOT there.
        assert!(
            !block.contains(concat!("AccountsState::switch", "able(")),
            "control: this region is not simply matching every needle it is shown"
        );
    }

    /// An account list that cannot be read must leave a RUNNING app.
    /// `start_backend` is startup's wrapper that calls `fatal_startup_error`,
    /// and neither belongs anywhere in this block: an unparseable
    /// `settings.json` is not a reason to refuse to run at all, and the whole
    /// `NoAccountList` arm exists so the user gets an app that can tell them
    /// which file to fix.
    ///
    /// The mutation: the account-less arm handled with a `fatal_startup_error`
    /// rather than a `log::warn!`, a modal and the fallback. Every value-based
    /// test stays green, because the app that would die never reaches one.
    #[test]
    fn an_unreadable_account_list_does_not_take_the_app_down() {
        let block = the_startup_account_block();
        let production = production_half_of_this_file();
        for banned in [
            concat!("fatal_startup", "_error("),
            concat!("std::process", "::exit("),
        ] {
            assert!(
                production.contains(banned),
                "control: `{banned}` is really spelled that way in this file, so the ban below \
                 is not vacuous"
            );
            assert!(
                !block.contains(banned),
                "`{banned}` is reachable from the startup account block: a settings file \
                 that could not be parsed would stop the app from starting at all, leaving the \
                 user with nothing that could tell them which file to fix"
            );
        }
        // The fallback it must take instead.
        assert!(
            block.contains(concat!("StartupAccounts::", "NoAccountList")),
            "there is no fallback arm at all, so an unreadable account list has nowhere to go"
        );
    }

    /// Whether this machine can hold more than one account is a question about
    /// the machine, and it has to be asked OF the machine.
    ///
    /// The mutation: a hard-coded `MultiAccountAvailability::Available`. A
    /// `bitwarden-cli` directory beside `bw.exe` makes the CLI ignore
    /// `BITWARDENCLI_APPDATA_DIR` entirely, so every account this app offers
    /// would silently be served the one portable profile -- two identities,
    /// one vault, and no error anywhere.
    #[test]
    fn startup_asks_the_machine_whether_more_than_one_account_is_possible() {
        let block = the_startup_account_block();
        let required = concat!("bw_path::multi_account_", "availability()");
        assert!(
            block.contains(required),
            "startup does not ask `{required}`: whether the CLI reads the directory this app \
             points it at is a fact about the machine, and it is answered from the machine or \
             not at all"
        );
        // `CARGO_MANIFEST_DIR`, not `file!()`: the latter is relative to the
        // package root and would depend on the test binary's working
        // directory. The banned needle is controlled against the module that
        // really declares it, so a misspelled ban cannot pass by matching
        // nothing anywhere.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let hard_coded = concat!("MultiAccountAvailability::", "Available");
        let elsewhere = std::fs::read_to_string(src.join("bw_path.rs"))
            .unwrap_or_else(|e| panic!("cannot read bw_path.rs: {e}"));
        assert!(
            elsewhere.contains(hard_coded),
            "the needle `{hard_coded}` is not spelled that way in bw_path.rs, so the ban below \
             proves nothing"
        );
        assert!(
            !block.contains(hard_coded),
            "startup writes `{hard_coded}` itself instead of asking, so the account switcher is \
             offered on the machine where every account shares one profile"
        );
        // Positive control on the same reader and the same region: a needle of
        // exactly that shape that IS present, so the ban is not passing
        // against a region that was cut down to nothing.
        assert!(
            block.contains(concat!("StartupAccounts::", "Ready")),
            "control: the region really does name the variants it resolves to"
        );
    }

    /// **A tray click made while one of this app's blocking windows was up.**
    ///
    /// The report: "if user clicks 3 times on tray icon (while waiting for a
    /// window) - if you close it, it will launch 2 more times once closed".
    /// `open_vault_window` owns the main thread for as long as the window is
    /// up, and the tray's channels keep filling behind it because eframe is
    /// pumping the same thread's messages -- so the clicks were queued, then
    /// replayed one window at a time.
    mod requests_queued_behind_a_window {
        use super::*;
        use tray_icon::menu::{MenuEvent, MenuId};

        fn click(id: &str) -> MenuEvent {
            MenuEvent { id: MenuId::new(id) }
        }

        fn labels(events: &[MenuEvent]) -> Vec<&str> {
            events.iter().map(|e| e.id.0.as_str()).collect()
        }

        const OPEN_VAULT: &str = "open-vault";
        const PREFERENCES: &str = "preferences";
        const QUIT: &str = "quit";

        /// The report itself. Three clicks, one window: the two that landed
        /// while it was already open asked for a window that was already on
        /// screen and already raised, so they have been answered.
        #[test]
        fn the_extra_clicks_made_while_the_window_was_up_do_not_reopen_it() {
            let survivors = requests_outliving_a_window(
                vec![click(OPEN_VAULT), click(OPEN_VAULT)],
                &MenuId::new(OPEN_VAULT),
            );
            assert!(
                survivors.is_empty(),
                "two \"open the vault\" clicks queued behind the vault window survived the \
                 sweep, so closing it opens it {} more time(s) -- the report. They read: {:?}",
                survivors.len(),
                labels(&survivors)
            );
        }

        /// **The positive control for the test above**, and the reason this is
        /// a rule rather than a blanket drain of the channel. An
        /// implementation that threw the whole queue away passes that test and
        /// fails this one: a Quit clicked at the tray while the vault window
        /// sat there loading is a request nothing has served, and swallowing
        /// it trades one report for a worse one.
        #[test]
        fn a_request_the_window_did_not_answer_is_still_honoured() {
            let survivors = requests_outliving_a_window(
                vec![click(OPEN_VAULT), click(QUIT), click(OPEN_VAULT)],
                &MenuId::new(OPEN_VAULT),
            );
            assert_eq!(
                labels(&survivors),
                vec![QUIT],
                "the sweep is \"drop the requests this window already answered\", not \"drop \
                 everything that is queued\" -- a Quit clicked while the window was up must \
                 still quit"
            );
        }

        /// A window answers ONE request, named by the caller. Preferences
        /// closing does not entitle it to swallow a queued "Open Vault".
        #[test]
        fn a_sweep_only_drops_requests_for_the_window_that_was_actually_up() {
            let survivors = requests_outliving_a_window(
                vec![click(PREFERENCES), click(OPEN_VAULT), click(PREFERENCES)],
                &MenuId::new(PREFERENCES),
            );
            assert_eq!(
                labels(&survivors),
                vec![OPEN_VAULT],
                "only the id the caller named as satisfied may be dropped"
            );
        }

        /// The survivors are replayed as if the window had never been in the
        /// way, so they keep the order the user made them in.
        #[test]
        fn the_survivors_keep_the_order_the_user_clicked_them_in() {
            let survivors = requests_outliving_a_window(
                vec![click(PREFERENCES), click(OPEN_VAULT), click(QUIT)],
                &MenuId::new(OPEN_VAULT),
            );
            assert_eq!(
                labels(&survivors),
                vec![PREFERENCES, QUIT],
                "order is the user's; a sweep that reordered them would run Quit before the \
                 Preferences window they asked for first"
            );
        }

        /// Nothing queued is the overwhelmingly common case -- a window closed
        /// with the user's hands nowhere near the tray -- and it must not
        /// invent work.
        #[test]
        fn an_empty_queue_survives_as_an_empty_queue() {
            assert!(requests_outliving_a_window(Vec::new(), &MenuId::new(OPEN_VAULT)).is_empty());
        }

        /// **Both doors into the vault window sweep on the way out.** The
        /// window has two -- the tray menu's "Open Vault" item and a left
        /// click on the icon -- and the report came through the second one, so
        /// sweeping only the first would have left it standing.
        ///
        /// Per call site, not a count, for the reason the sibling
        /// `every_way_into_the_vault_window_rebuilds_the_submenu_on_the_way_out`
        /// gives: a third door that forgets the sweep must fail this with
        /// nothing to "update".
        #[test]
        fn every_way_into_the_vault_window_sweeps_the_requests_queued_behind_it() {
            let production = production_half_of_this_file();
            // `concat!`-split and single-line, this file's usual two reasons.
            // The call needle does not match `fn open_vault_window<A: ...>(`,
            // whose `(` comes after the generics.
            let call = concat!("open_vault", "_window(");
            let sweep = concat!("drop_vault_requests_queued_behind", "_the_window(");
            // The end of each call's statement -- a real boundary in the code
            // (both sites forget the last dispatched window on the way out),
            // not a byte count, which is the shape that already overran an arm
            // in this file once.
            let end_of_statement = concat!("last_dispatched_hwnd", " = None;");

            let mut sites = 0usize;
            let mut from = 0usize;
            while let Some(offset) = production[from..].find(call) {
                let at = from + offset;
                from = at + call.len();
                // The DECLARATION now matches this needle too:
                // dropping the vault window's unused `Injector`
                // parameter took `open_vault_window`'s `<A, B>`
                // generics with it, so its `(` follows the name
                // directly, exactly as a call's does. Skipped
                // rather than respelled: a needle that matched only
                // calls would have to encode the argument list, and
                // would go quietly vacuous the next time one changed.
                if production[..at].ends_with("fn ") {
                    continue;
                }
                sites += 1;
                let rest = &production[from..];
                let end = rest.find(end_of_statement).unwrap_or_else(|| {
                    panic!(
                        "the vault-window call at byte {at} is not followed by \
                         {end_of_statement:?}, so this test cannot tell where its statement \
                         ends -- give it one, or reshape this boundary"
                    )
                });
                let statement = &rest[..end];
                assert!(
                    statement.contains(sweep),
                    "the vault-window call at byte {at} does not sweep the tray requests that \
                     queued behind it. Every click the user made at the tray while the window \
                     was up is still in the channel, and the loop replays them -- so closing \
                     the window opens it again, once per click. The statement reads: \
                     {statement:?}"
                );
            }

            // Controls for that loop: it walked real call sites, and the
            // needles are spelled the way the production code spells them.
            // `>=`, not `== 2`, for the reason the sibling test records: an
            // equality here makes a legitimate third door fail a test whose
            // only green is editing a number.
            assert!(
                sites >= 2,
                "control: the two known ways of opening the vault window -- the tray menu's \
                 \"Open Vault\" item and a left click on the tray icon -- are what this walked; \
                 finding {sites} means the needle stopped matching"
            );
            assert!(
                production.contains(concat!(
                    "fn drop_vault_requests_queued_behind",
                    "_the_window("
                )),
                "control: the sweep helper is still declared here"
            );
        }


        /// **The vault window is not the only one that blocks the loop**, and
        /// the tray can open three of them: "Open Vault", "Preferences", and
        /// "Add app...", which is the longest of the three because it opens
        /// two windows back to back. All three queued repeat clicks and
        /// replayed them, so all three sweep.
        ///
        /// Read off the argument each sweep is actually given, not off a
        /// count: a fourth blocking tray window that forgets to sweep fails
        /// this by name, and a sweep given the wrong window's id fails it too.
        #[test]
        fn every_blocking_window_the_tray_opens_sweeps_the_requests_queued_behind_it() {
            /// The window, and the tray id a sweep for it must be handed.
            const WINDOWS: [(&str, &str); 3] = [
                ("the vault window", concat!("&tray.open_vault", "_id")),
                ("the preferences window", concat!("&tray.preferences", "_id")),
                ("the \"Add app...\" picker", concat!("&tray.add_app", "_id")),
            ];
            let production = production_half_of_this_file();
            // Both spellings of a sweep. The declarations match these needles
            // too and contribute nothing, because a parameter list holds no
            // `&tray.` -- so they need no special case.
            let sweeps = [
                concat!("drain_requests_queued_behind", "_a_window("),
                concat!("drop_vault_requests_queued_behind", "_the_window("),
            ];

            let mut swept: Vec<&str> = Vec::new();
            for needle in sweeps {
                let mut from = 0usize;
                while let Some(offset) = production[from..].find(needle) {
                    let at = from + offset;
                    from = at + needle.len();
                    let rest = &production[from..];
                    // The end of the call's own argument list -- a real
                    // boundary in the code rather than a byte count, which is
                    // the shape that already overran an arm in this file once.
                    let end = rest.find(");").unwrap_or_else(|| {
                        panic!(
                            "the sweep at byte {at} has no closing `);`, so this test cannot \
                             read which window it was given"
                        )
                    });
                    for (window, id) in WINDOWS {
                        if rest[..end].contains(id) {
                            swept.push(window);
                        }
                    }
                }
            }

            for (window, id) in WINDOWS {
                assert!(
                    swept.contains(&window),
                    "nothing sweeps the tray requests queued behind {window}. It blocks \
                     `main`'s loop for as long as it is up, so every repeat click on its own \
                     tray item is queued and replayed -- opening it again, once per click. \
                     Hand {id} to a sweep after it returns. Swept: {swept:?}"
                );
            }
        }

        /// **The vault window has two doors, and the sweep has two halves.**
        /// The sibling tests above walk the sweep's CALL SITES and read which
        /// `&tray.*_id` each was handed -- both of them only ever see the
        /// menu-event half. Neither looks inside
        /// `drop_vault_requests_queued_behind_the_window`, so deleting the one
        /// line in its body that drains the tray-icon channel left the entire
        /// suite green: the menu half kept being swept, every assertion kept
        /// passing, and the bug the report was actually made through came
        /// straight back. Three left clicks on the icon while the vault loaded
        /// became three windows.
        ///
        /// Held by reading the source, for the reason `foreground`'s raise
        /// guard is: `tray::discard_queued_icon_events` reads the global
        /// `tray-icon` channel, whose sender is `pub(crate)` to that crate, so
        /// nothing here can put an event into it and no test can call this
        /// function and observe anything at all.
        ///
        /// **Not touched, deliberately:** all three call sites drain after
        /// `rebuild_after_vault_window` rather than the instant the window
        /// returns. Nothing can arrive in that gap -- the rebuild is
        /// synchronous `muda` menu calls with no message pump, and both
        /// channels are fed from the window-message pump on this same thread --
        /// so the ordering is a reading nit rather than a race.
        #[test]
        fn the_vault_sweep_drains_the_tray_icon_channel_and_not_only_the_menu_one() {
            let production = production_half_of_this_file();
            // `concat!`-split and single-line, this file's usual two reasons.
            // No needle here carries a `\n`: one that did would pass on LF and
            // fail on CRLF, which has burned this repo.
            let decl = concat!("fn drop_vault_requests_queued_behind", "_the_window(");
            let icon_sweep = concat!("tray::discard_queued", "_icon_events(");
            let menu_sweep = concat!("drain_requests_queued_behind", "_a_window(");

            assert_eq!(
                production.matches(decl).count(),
                1,
                "control: the sweep helper is declared exactly once here, so the body sliced \
                 below is unambiguous"
            );
            // `<= 1`, not `== 1`. An equality here is what a deleted drain
            // trips FIRST, and it would fail with a message about needles
            // rather than about the bug -- the assertion at the bottom of this
            // test is the one that should report a missing drain, and it
            // cannot if this fires ahead of it. A SECOND spelling still fails,
            // which is what this control is actually for: the slice below
            // would then be reasoning about a call other than the one this
            // test is named for.
            assert!(
                production.matches(icon_sweep).count() <= 1,
                "control: the tray-icon drain is spelled more than once in the production half, \
                 so `{icon_sweep}` no longer names one call and the slice below is about the \
                 wrong one"
            );

            // The body, by brace matching from the declaration's own `{`. A
            // byte count is the shape that already overran an arm in this file
            // once, and "up to the next `fn`" depends on the order these
            // helpers happen to sit in. Braces are neither: this body holds no
            // string or comment carrying one, and the two controls below refuse
            // a slice that got the wrong region anyway.
            let at = production.find(decl).expect("checked above");
            let open = at + production[at..].find('{').expect("the helper has a body");
            let mut depth = 0usize;
            let mut close = None;
            for (offset, ch) in production[open..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            close = Some(open + offset);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let body = &production[open..close.expect("the helper's body is closed")];

            // Controls on the slice itself, in both directions: it reached the
            // whole body (the menu half of the sweep is in there), and it did
            // not run past the end into whatever follows (no second `fn`).
            assert!(
                body.contains(menu_sweep),
                "control: the sliced body is not this helper's -- it does not even call the \
                 menu-event sweep. It reads: {body:?}"
            );
            assert!(
                !body.contains("fn "),
                "control: the slice overran this helper's body into the next function, so the \
                 assertion below could be satisfied by code that is not the sweep. It reads: \
                 {body:?}"
            );

            assert!(
                body.contains(icon_sweep),
                "`drop_vault_requests_queued_behind_the_window` no longer drains the tray-icon \
                 channel, only the menu-event one. The vault window has TWO doors and the tray \
                 icon is the one the report came through: every left click on the icon while \
                 the window was up is still queued, and `main`'s loop replays them the moment \
                 it closes -- three clicks while the vault loads, three windows. The call sites \
                 are no help here; they hand this function a `MenuId` and cannot see inside it. \
                 The body reads: {body:?}"
            );
        }

        /// The queue is not a write-only sink: `main`'s loop has to actually
        /// pop from it, and pop from it BEFORE the live channel, or a survivor
        /// sits behind everything that has arrived since and is handled out of
        /// order (or, if the user never touches the tray again, never).
        #[test]
        fn the_loop_prefers_a_survivor_to_a_freshly_arrived_event() {
            let production = production_half_of_this_file();
            let needle = concat!(
                "pending_menu_events.pop_front()",
                ".or_else(tray::next_menu_event)"
            );
            assert!(
                production.contains(needle),
                "`main`'s loop no longer reads the survivors of a sweep before the live tray \
                 channel. Whatever they are read through must put them first: a Quit the user \
                 clicked while the vault window was up would otherwise wait behind every event \
                 that has arrived since"
            );
        }
    }

    /// **Autofill's one production dispatch, driven end to end.**
    ///
    /// Two holes lived here, and both were behaviour that could be deleted
    /// outright with the whole suite green:
    ///
    /// 1. Replacing `handle_match(cache, injector, fill_stats, item_id, m,
    ///    event)` with `None::<(String, isize)>` -- autofill switched off for
    ///    *every* trigger mode, which is the entirety of what this app is for
    ///    -- compiled and left 1217 lib + 113 bin tests passing. `app.rs` pins
    ///    what `handle_match` does (`prompt_request`, `window_label`), but
    ///    every one of those pins lives *below* this call, and a call that is
    ///    never made reaches none of them.
    /// 2. Replacing the log line's `window_label(&event.exe_name,
    ///    &event.title)` with `&event.exe_name` was equally invisible. Its
    ///    sibling inside `handle_match` is the one that produced the reported
    ///    "ApplicationFrameHost.exe wants your password" overlay; this one
    ///    writes that same meaningless name into the log file a user is asked
    ///    for when autofill misbehaves, so the record of which app was matched
    ///    names the frame host instead of the app.
    ///
    /// **Why these are real tests and not a source guard.** Nothing in
    /// `process_foreground_event` needs a window: `TriggerMode::Hotkey` only
    /// arms a pair, and `TriggerMode::Auto` goes to `fill_from_vault`, which
    /// takes an `Injector`. The only thing in the way was that this function
    /// named `Injector<RealUiAutomation, RealSendInput>` concretely, so no
    /// test could hand it a fake; making it generic is the whole of the
    /// production change. `TriggerMode::Prompt` is deliberately *not* driven
    /// here -- it opens a real overlay window -- and it does not need to be:
    /// its decision is already a pure, directly tested `app::prompt_request`,
    /// and what was missing was only that something reach `handle_match` at
    /// all.
    mod the_dispatch_that_actually_fills {
        use super::*;
        use deskwarden::app_match::{AppMatch, TriggerMode};
        use std::sync::{Arc, Mutex};

        const HOST: &str = "ApplicationFrameHost.exe";

        /// A pid that is definitely not ours. `dispatch::is_own_process`
        /// returns early on our own, and every test below would then pass by
        /// dispatching nothing at all.
        fn other_pid() -> u32 {
            std::process::id().wrapping_add(1)
        }

        fn window(exe_name: &str, title: &str, hwnd: isize) -> window_watch::ForegroundEvent {
            window_watch::ForegroundEvent {
                hwnd,
                pid: other_pid(),
                exe_name: exe_name.to_string(),
                title: title.to_string(),
            }
        }

        /// Every `(hwnd, username, password)` the injector was asked to type.
        #[derive(Clone, Default)]
        struct Filled(Arc<Mutex<Vec<(isize, String, String)>>>);

        impl Filled {
            fn seen(&self) -> Vec<(isize, String, String)> {
                self.0.lock().unwrap().clone()
            }
        }

        /// Records instead of driving UI Automation, and answers `Ok(true)` so
        /// the fill is complete and the fallback is never consulted.
        struct RecordingUi(Filled);
        impl UiAutomationFiller for RecordingUi {
            fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<bool, String> {
                self.0 .0.lock().unwrap().push((hwnd, user.to_string(), pass.to_string()));
                Ok(true)
            }
        }

        /// The fallback must never run here: the real one types with
        /// `SendInput`, into whatever window holds focus on the machine
        /// running the suite.
        struct NeverTypes;
        impl SendInputFiller for NeverTypes {
            fn fill(&self, _: isize, _: &str, _: &str) -> Result<(), String> {
                panic!("the SendInput fallback must not be reached: UI Automation answered Ok(true)")
            }

            /// This used to be inherited from a defaulted trait method, which
            /// meant the panic above guarded only half the door: changing that
            /// default to `Ok(())` sent these tests down the sequence path,
            /// where they succeeded quietly and recorded a fill. Overriding it
            /// here states the same thing `fill` does -- **no test in this
            /// module reaches `SendInput`** -- for both paths.
            fn fill_sequence(
                &self,
                _: isize,
                _: deskwarden::injector::sequence::Plan,
                _: deskwarden::injector::SequenceGuard,
            ) -> Result<(), String> {
                panic!("the SendInput fallback must not be reached: no test here types a sequence")
            }
        }

        fn recording_injector(filled: &Filled) -> Injector<RecordingUi, NeverTypes> {
            Injector { ui: RecordingUi(filled.clone()), fallback: NeverTypes }
        }

        /// A `FillStats` on its own throwaway file -- never the real one under
        /// `%APPDATA%`, which a successful fill would write to.
        fn scratch_fill_stats() -> (fill_stats::FillStats, std::path::PathBuf) {
            let path = std::env::temp_dir().join(format!(
                "deskwarden-test-fill-stats-{}-{:?}.json",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_file(&path);
            (fill_stats::FillStats::new(path.clone()), path)
        }

        fn engine_with(entries: &[(&str, AppMatch)]) -> MatchEngine {
            let owned: Vec<(String, AppMatch)> =
                entries.iter().map(|(id, m)| ((*id).to_string(), m.clone())).collect();
            let mut engine = MatchEngine::new();
            engine.rebuild(&owned);
            engine
        }

        /// A match as the picker saves it for a Microsoft Store app: the
        /// attributed process, the title its host frame carried, and the
        /// `hosted` flag that makes the title matchable.
        fn hosted_match(process: &str, title: &str, trigger: TriggerMode) -> AppMatch {
            AppMatch {
                process: process.to_string(),
                title: title.to_string(),
                hosted: true,
                path: String::new(),
                args: String::new(),
                sequence: String::new(),
                trigger,
            }
        }

        /// Captures this thread's `log` output for the duration of `f`.
        ///
        /// The log line under test returns nothing and changes nothing, so
        /// there is no other observable to assert on. The logger is global and
        /// installed once; the buffer is thread-local, so tests running in
        /// parallel cannot see each other's lines. If the install ever loses a
        /// race to some other logger the buffer simply stays empty -- which is
        /// why every test using this also asserts on a string it knows the
        /// line contains, so "captured nothing" fails rather than passes.
        fn captured_logs(f: impl FnOnce()) -> Vec<String> {
            use std::cell::RefCell;

            thread_local! {
                static LINES: RefCell<Option<Vec<String>>> = RefCell::new(None);
            }

            struct Capture;
            impl log::Log for Capture {
                fn enabled(&self, _: &log::Metadata) -> bool {
                    true
                }
                fn log(&self, record: &log::Record) {
                    LINES.with(|l| {
                        if let Some(lines) = l.borrow_mut().as_mut() {
                            lines.push(record.args().to_string());
                        }
                    });
                }
                fn flush(&self) {}
            }

            static INSTALL: std::sync::Once = std::sync::Once::new();
            INSTALL.call_once(|| {
                let _ = log::set_boxed_logger(Box::new(Capture));
                log::set_max_level(log::LevelFilter::Trace);
            });

            LINES.with(|l| *l.borrow_mut() = Some(Vec::new()));
            f();
            LINES.with(|l| l.borrow_mut().take().unwrap_or_default())
        }

        /// The line `process_foreground_event` writes when the engine matched.
        fn matched_line(lines: &[String]) -> &str {
            lines
                .iter()
                .find(|l| l.starts_with("matched "))
                .map(|l| l.as_str())
                .unwrap_or_else(|| panic!("no \"matched ...\" line was logged; got {lines:?}"))
        }

        /// The recorder every dispatching test below hands to
        /// `process_foreground_event`, and the reason there is no lock here
        /// any more.
        ///
        /// `Injector::fill` takes the process-global `SEQUENCE_IN_FLIGHT`
        /// flag, so two of these tests that overlapped would hand one of them
        /// `ALREADY_TYPING` -- and this binary links the library **without**
        /// `cfg(test)`, so when the refusal went to a `RealNotifier` whose
        /// silence was a `#[cfg(test)]` gate, that notice was a real
        /// task-modal message box on the desktop of whoever ran `cargo test`.
        /// A `FILL_LOCK` mutex used to narrow that race, and a source-position
        /// guard used to check that all five tests took it.
        ///
        /// Neither is needed now, because the hazard is not narrowed but
        /// **absent**: the notifier is a parameter, this is the only one these
        /// tests have, and it opens nothing. A sixth dispatching test cannot
        /// forget to take a lock, because there is no lock to take -- it
        /// cannot compile without passing a notifier, and the only notifier
        /// that opens a window is the library's production one, which
        /// `the_production_dispatch_uses_the_real_notifier` holds to the three
        /// lines in `run` that no test executes -- and forbids outright here.
        fn recorder() -> deskwarden::injector::sequence::RecordingNotifier {
            deskwarden::injector::sequence::RecordingNotifier::default()
        }

        /// **What is left unreachable, held by source position.**
        ///
        /// Handing the notifier in is what makes a test-opened dialog
        /// unrepresentable; it also means production has to hand in the real
        /// one, on lines inside `run` that no test can execute. Swap those for
        /// a recorder and every test in this file stays green while the
        /// shipped app silently stops telling anyone why a fill did nothing --
        /// the exact failure mode `app::prompt_wiring_tests` guards for the
        /// overlay. So the three production hand-overs are counted here.
        #[test]
        fn the_production_dispatch_uses_the_real_notifier() {
            let source = include_str!("main.rs");
            // Split so this test's own text is not one of the three.
            let real = concat!("&deskwarden::injector::sequence::", "REAL_", "NOTIFIER,");
            // Bare, for the test half: not merely "not passed as an argument"
            // but *not nameable at all* below the boundary, so
            // a bare `.refused("x")` on the production notifier, written
            // straight into a test body, is caught too.
            let named = concat!("REAL_", "NOTIFIER");
            // Only the half of the file above the tests: the definition of
            // `process_foreground_event` separates production from fixtures.
            let boundary = source
                .find(concat!("fn process_foreground", "_event<A:"))
                .expect("`process_foreground_event`'s definition is gone from main.rs");
            assert_eq!(
                source[..boundary].matches(real).count(),
                3,
                "expected the real notifier to be named exactly three times in `run` -- the \
                 two `process_foreground_event` calls and the hotkey `fill_from_vault`. Fewer \
                 means a production fill can refuse without telling the user; more means a \
                 fourth production path appeared that nobody has thought about"
            );
            assert_eq!(
                source[boundary..].matches(named).count(),
                0,
                "a test passes the real notifier. That opens a task-modal `MessageBoxW` on \
                 the desktop of whoever ran `cargo test`, which is the entire hazard this \
                 parameter exists to remove: use `recorder()`"
            );
        }

        /// **Finding 1, for `TriggerMode::Auto`.** Deleting the `handle_match`
        /// call leaves this with an empty `Filled`.
        #[test]
        fn an_auto_trigger_match_is_filled_from_the_vault_into_the_window_that_matched() {
            let mut server = mockito::Server::new();
            // The cache is empty, so this goes down `fill_from_vault`'s
            // documented bridge fallback rather than needing a populate.
            let _item = server
                .mock("GET", "/object/item/1")
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(
                    r#"{"success":true,"data":{"id":"1","name":"Ledgerline","fields":[],
                        "login":{"username":"denis@example.com","password":"hunter2","totp":null}}}"#,
                )
                .create();

            let cache = VaultCache::new(VaultBridge::new(server.url()));
            let engine = engine_with(&[(
                "1",
                AppMatch::for_process("Ledgerline.exe", TriggerMode::Auto),
            )]);
            let filled = Filled::default();
            let (stats, stats_path) = scratch_fill_stats();
            let mut pending_hotkey_fill = None;
            let mut last_dispatched_hwnd = None;

            process_foreground_event(
                &window("Ledgerline.exe", "Ledgerline -- Invoices", 0x4321),
                &cache,
                &recording_injector(&filled),
                &stats,
                &engine,
                &mut pending_hotkey_fill,
                &mut last_dispatched_hwnd,
                &recorder(),
            );

            assert_eq!(
                filled.seen(),
                vec![(0x4321, "denis@example.com".to_string(), "hunter2".to_string())],
                "an Auto-trigger match must be filled, with that item's credentials, into the \
                 window that matched. An empty list means `handle_match` was never called at \
                 all -- autofill switched off, which is this app's whole purpose"
            );
            assert_eq!(stats.count("1"), 1, "a completed fill is counted for the detail pane");
            assert_eq!(
                pending_hotkey_fill, None,
                "Auto fills now; there is nothing left for the fill hotkey to do"
            );
            assert_eq!(last_dispatched_hwnd, Some(0x4321));

            let _ = std::fs::remove_file(stats_path);
        }

        /// **Finding 1, for `TriggerMode::Hotkey`** -- the arm whose entire
        /// output is the value the mutation replaced with `None`.
        #[test]
        fn a_hotkey_trigger_match_arms_the_pending_fill_instead_of_filling_now() {
            // Nothing here may reach the network: the Hotkey arm returns
            // before any vault read, and a bridge pointed at a closed port
            // proves it.
            let cache = VaultCache::new(VaultBridge::new("http://127.0.0.1:1".to_string()));
            let engine = engine_with(&[(
                "7",
                AppMatch::for_process("Ledgerline.exe", TriggerMode::Hotkey),
            )]);
            let filled = Filled::default();
            let (stats, _path) = scratch_fill_stats();
            let mut pending_hotkey_fill = None;
            let mut last_dispatched_hwnd = None;

            process_foreground_event(
                &window("Ledgerline.exe", "Ledgerline", 0x99),
                &cache,
                &recording_injector(&filled),
                &stats,
                &engine,
                &mut pending_hotkey_fill,
                &mut last_dispatched_hwnd,
                &recorder(),
            );

            assert_eq!(
                pending_hotkey_fill,
                Some(("7".to_string(), 0x99)),
                "a Hotkey match must arm (item, hwnd) for the loop's separate fill-hotkey check. \
                 `None` means `handle_match`'s answer is being dropped, so Ctrl+Alt+B would \
                 never fill anything again"
            );
            assert!(
                filled.seen().is_empty(),
                "Hotkey must not fill until the user actually presses the hotkey"
            );
            assert_eq!(last_dispatched_hwnd, Some(0x99));
        }

        /// The positive control for both tests above: they would also pass
        /// against a `process_foreground_event` that filled and armed
        /// unconditionally, without asking the engine anything.
        #[test]
        fn a_window_that_matches_nothing_is_neither_filled_nor_armed() {
            let cache = VaultCache::new(VaultBridge::new("http://127.0.0.1:1".to_string()));
            let engine = engine_with(&[(
                "1",
                AppMatch::for_process("Ledgerline.exe", TriggerMode::Auto),
            )]);
            let filled = Filled::default();
            let (stats, _path) = scratch_fill_stats();
            let mut pending_hotkey_fill = None;
            let mut last_dispatched_hwnd = None;

            process_foreground_event(
                &window("Solitaire.exe", "Solitaire", 0x11),
                &cache,
                &recording_injector(&filled),
                &stats,
                &engine,
                &mut pending_hotkey_fill,
                &mut last_dispatched_hwnd,
                &recorder(),
            );

            assert!(filled.seen().is_empty(), "an unmatched window must never be filled");
            assert_eq!(pending_hotkey_fill, None, "an unmatched window arms nothing");
        }

        /// **Finding 2.** A Store app matched through the title table arrives
        /// wearing the frame host's `exe_name`, and the log entry that records
        /// which app was matched must not say that name -- it is the exact
        /// string the user reported being shown, and this is the file they are
        /// asked for when autofill goes wrong.
        #[test]
        fn the_matched_log_line_names_a_title_matched_store_app_by_its_title() {
            let cache = VaultCache::new(VaultBridge::new("http://127.0.0.1:1".to_string()));
            let engine = engine_with(&[(
                "42",
                hosted_match("Speedtest.exe", "Speedtest", TriggerMode::Hotkey),
            )]);
            let filled = Filled::default();
            let (stats, _path) = scratch_fill_stats();
            let mut pending_hotkey_fill = None;
            let mut last_dispatched_hwnd = None;

            let lines = captured_logs(|| {
                process_foreground_event(
                    &window(HOST, "Speedtest", 0x55),
                    &cache,
                    &recording_injector(&filled),
                    &stats,
                    &engine,
                    &mut pending_hotkey_fill,
                    &mut last_dispatched_hwnd,
                    &recorder(),
                );
            });

            let line = matched_line(&lines);
            // Proves the capture worked at all, so the two assertions below
            // cannot pass on an empty buffer.
            assert!(line.contains("vault item 42"), "captured the wrong line: {line}");
            assert!(
                line.contains("Speedtest"),
                "a title-matched Store app must be logged under its own title, not the host \
                 frame's name: {line}"
            );
            assert!(
                !line.contains(HOST),
                "this is the exact string the user reported being shown -- \
                 `window_label` has been dropped from the log line: {line}"
            );
        }

        /// The positive control for the test above: a log line that always
        /// used the title would pass it and fail this one.
        #[test]
        fn the_matched_log_line_names_an_ordinary_app_by_its_executable() {
            let cache = VaultCache::new(VaultBridge::new("http://127.0.0.1:1".to_string()));
            let engine = engine_with(&[(
                "8",
                AppMatch::for_process("Ledgerline.exe", TriggerMode::Hotkey),
            )]);
            let filled = Filled::default();
            let (stats, _path) = scratch_fill_stats();
            let mut pending_hotkey_fill = None;
            let mut last_dispatched_hwnd = None;

            let lines = captured_logs(|| {
                process_foreground_event(
                    &window("Ledgerline.exe", "Ledgerline -- Invoices", 0x66),
                    &cache,
                    &recording_injector(&filled),
                    &stats,
                    &engine,
                    &mut pending_hotkey_fill,
                    &mut last_dispatched_hwnd,
                    &recorder(),
                );
            });

            let line = matched_line(&lines);
            assert!(line.contains("vault item 8"), "captured the wrong line: {line}");
            assert!(
                line.contains("Ledgerline.exe"),
                "an ordinary window is named by its executable, as every log line in this app \
                 always has been: {line}"
            );
        }
    }
}

/// **Startup's two shapes, and the one that must not gain a window.**
///
/// None of this can be observed by running anything: `main` is the process
/// entry point and every window it opens blocks on a real winit event loop. So
/// it is held at the source, and the slices below are bounded by real syntax in
/// the code rather than by byte counts.
#[cfg(test)]
mod startup_shape_tests {
    /// The `} else {` at `main`'s own indentation -- the one that ends the
    /// cached-session arm, and not one of the many nested inside it. The
    /// leading newline is what makes the indentation load-bearing, and it
    /// matches under LF and CRLF alike.
    const OUTER_ELSE: &str = concat!("\n    } else", " {");

    /// Everything before the big test module. Named separately from
    /// `tests::production_half_of_this_file` because this module cannot reach
    /// into that one, and split with `concat!` for the same reason it is there:
    /// a plain literal would match its own declaration.
    fn production() -> &'static str {
        let source = include_str!("main.rs");
        let production = source
            .split_once(concat!("mod te", "sts {"))
            .expect("main.rs must still have its test module")
            .0;
        assert!(
            production.len() < source.len(),
            "control: the split really cut the test module off the end"
        );
        production
    }

    /// The arm taken when the session on disk is still good, bounded by the
    /// `if let` that opens it and the `} else {` that ends it.
    fn cached_session_arm() -> &'static str {
        let production = production();
        let head = concat!("if let Some(token) = cached_", "session {");
        let at = production.find(head).unwrap_or_else(|| {
            panic!(
                "no {head:?} in main.rs -- startup no longer tells a launch with a good \
                 cached session apart from one that has to sign in, which is the whole of \
                 what decides whether a window opens at all"
            )
        });
        let rest = &production[at + head.len()..];
        let end = rest
            .find(OUTER_ELSE)
            .expect("the cached-session arm is never closed by an `} else {`");
        &rest[..end]
    }

    /// The arm taken when the launch has to sign in.
    fn signing_in_arm() -> &'static str {
        let production = production();
        let head = concat!("if let Some(token) = cached_", "session {");
        let at = production.find(head).expect("no cached-session gate in main.rs");
        let rest = &production[at..];
        let start = rest
            .find(OUTER_ELSE)
            .expect("no `} else {` closing the cached-session arm");
        &rest[start..]
    }

    /// **A launch with a good cached session must open no window.** The app
    /// starting silently into the tray is what it has always done and is not
    /// what the user reported; the single window is for the launch that signs
    /// in. This is precisely the kind of property that quietly stops holding
    /// while someone is chasing "one window from the beginning".
    #[test]
    fn a_launch_with_a_good_cached_session_opens_no_startup_window() {
        let arm = cached_session_arm();
        let single_window = concat!("app_window::", "run(");
        assert!(
            !arm.contains(single_window),
            "a launch that never asks for a master password now opens the sign-in window \
             anyway -- a window popping up on every boot, for an app whose whole shape is \
             that it lives in the tray: {arm:?}"
        );
        // Paired positive control on the same needle: the single window really
        // is reached from the OTHER arm, so the negative above is about which
        // arm it is in and not about a needle that never matches anything.
        assert!(
            signing_in_arm().contains(single_window),
            "the single startup window is never opened at all, so signing in is back to \
             three windows"
        );
        // And the silent launch really is still the old path, so the slice
        // above is the launch it claims to be.
        assert!(
            arm.contains(concat!("wait_for_vault_ready_with_", "spinner(")),
            "the cached-session launch no longer waits for the backend at all: {arm:?}"
        );
    }

    /// **The vault stage is reached, and its answer is acted on.**
    ///
    /// `app_window::advance` can be a perfect transition table and the vault
    /// still never enter production -- this codebase has shipped complete,
    /// correct and unreachable code three functions at a time. Two halves are
    /// pinned here: `main` asks for a vault frame at all, and what that vault
    /// session reported is dispatched rather than dropped.
    #[test]
    fn the_startup_window_builds_a_vault_frame_and_dispatches_what_it_reports() {
        let arm = signing_in_arm();
        assert!(
            arm.contains(concat!("vault_window::build_", "frame(")),
            "the single window's `build_vault` never builds a vault frame, so the window \
             shows the spinner and then closes -- the user signs in and is dropped to the \
             tray without the vault they just unlocked, which is the half of the report \
             that was never about window count: {arm:?}"
        );
        assert!(
            arm.contains(concat!("startup_vault = outcome.", "vault;")),
            "what the vault session reported is thrown away at the call site: {arm:?}"
        );

        let production = production();
        let handed_in = concat!("startup_vault.", "take()");
        assert_eq!(
            production.matches(handed_in).count(),
            1,
            "expected exactly one hand-in of the startup window's outcome. None means a \
             Lock, a Preferences click or an account switch made in that window is silently \
             ignored -- the vault stays unlocked after the user locked it. More than one \
             means it is dispatched twice."
        );
        // It is handed to the ONE dispatcher, not acted on beside it. Bounded
        // by the statement's own end rather than by a byte window, which is the
        // shape that has already overrun an arm in this file.
        // Bounded from the hand-in BACKWARDS to the nearest call, rather than
        // from a call forwards: there are three `open_vault_window(` sites and
        // only one of them is this one, so a slice anchored on a call picks
        // whichever happens to be last in the file.
        let hand = production.find(handed_in).expect("checked just above");
        let call = concat!("open_vault", "_window(");
        let call_at = production[..hand]
            .rfind(call)
            .expect("main.rs no longer calls the vault-window dispatcher at all");
        let between = &production[call_at..hand];
        assert!(
            !between.contains(");"),
            "the startup window's outcome is not an argument to `open_vault_window` -- there \
             is a closed call between the two -- so it is being acted on somewhere else. For \
             a lock that means a SECOND call of `resettle_session`, the one sequence this \
             app has hardened and must reach from one place: {between:?}"
        );
        // Positive control on that boundary: a closed call really is detectable
        // by this needle, so the negative above is not vacuous.
        assert!(
            production[..call_at].contains(");"),
            "control: a closed call is undetectable by this needle, so the check above \
             passes against anything"
        );
    }

    /// The work between the stages must not run on the frame thread, and it
    /// must not try to talk to the user from the thread it does run on.
    #[test]
    fn the_startup_work_handed_to_the_window_is_the_off_thread_one() {
        let arm = signing_in_arm();
        assert!(
            arm.contains(concat!("StartupWork::", "produce(")),
            "the single window is handed something other than `StartupWork::produce`; that \
             is the function whose whole contract is that it never opens a window and never \
             exits the process, because it runs on a worker thread: {arm:?}"
        );

        let production = production();
        let body = production
            .split_once(concat!("fn produce", "("))
            .expect("`StartupWork::produce` is gone")
            .1;
        let body = &body[..body
            .find(concat!("fn start_", "backend("))
            .expect("`start_backend` is expected to follow `produce`, and bounds this slice")];
        assert!(
            !body.contains(concat!(" start_", "backend(token")),
            "`StartupWork::produce` calls the FATAL backend start, on a worker thread: the \
             message box it raises goes up behind the window that owns the main thread, \
             where the user cannot reach it: {body:?}"
        );
        // Positive control on that negative: `produce` does start a backend, so
        // the assertion above is about WHICH one and not about a needle that
        // never matches.
        assert!(
            body.contains(concat!("try_start_", "backend(")),
            "control: `produce` does not start a backend at all: {body:?}"
        );
    }
}

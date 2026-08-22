use crate::accounts::{Account, AccountId};
// No `bw_command` import. Every `bw` this window spawns names its profile
// directory (`bw_command_in`), because this window is also the one
// `main`'s `add_account` opens for an account that is NOT the active one --
// and the active-profile form there would sign the existing account out and
// replace it. See `profile_dir_for`.
use crate::hello::{self, HelloState};
use crate::theme;
use eframe::egui::{self, Color32, CornerRadius, Margin, Pos2, RichText, Sense, Stroke, Vec2};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use zeroize::Zeroize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BwStatus {
    Unauthenticated,
    Locked,
    Unlocked,
}

/// Parses `bw status` JSON output into a [`BwStatus`].
///
/// Split out from the process spawn so the (only interesting) part is
/// testable without a Bitwarden CLI on PATH.
pub fn parse_bw_status(stdout: &str) -> BwStatus {
    if stdout.contains("\"status\":\"unlocked\"") {
        BwStatus::Unlocked
    } else if stdout.contains("\"status\":\"locked\"") {
        BwStatus::Locked
    } else {
        BwStatus::Unauthenticated
    }
}

/// Everything the login window wants from `bw status` beyond the bare
/// status: whose vault this is and which server it talks to. Design 3h shows
/// both -- the account email beside the password label and the server in the
/// window footer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BwStatusDetails {
    pub status: BwStatus,
    pub user_email: Option<String>,
    pub server_url: Option<String>,
}

/// Parses the full `bw status` JSON. Falls back field-by-field rather than
/// all-or-nothing: a malformed or partial payload still yields whatever
/// status [`parse_bw_status`]'s substring check can salvage, with the
/// optional fields absent.
pub fn parse_bw_status_details(stdout: &str) -> BwStatusDetails {
    let parsed: serde_json::Value = serde_json::from_str(stdout).unwrap_or_default();
    let string_field = |key: &str| {
        parsed
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    BwStatusDetails {
        status: parse_bw_status(stdout),
        user_email: string_field("userEmail"),
        server_url: string_field("serverUrl"),
    }
}

/// The display form of a server URL for the login footer: scheme and path
/// stripped, so `https://vault.example.eu/api` reads as `vault.example.eu`.
/// `None` -- the CLI's default -- is Bitwarden's own cloud.
pub fn server_host(server_url: Option<&str>) -> String {
    match server_url {
        Some(url) if !url.trim().is_empty() => {
            let stripped = url
                .trim()
                .trim_start_matches("https://")
                .trim_start_matches("http://");
            stripped
                .split(['/', '?', '#'])
                .next()
                .unwrap_or(stripped)
                .to_string()
        }
        _ => "bitwarden.com".to_string(),
    }
}

pub fn check_bw_status() -> BwStatus {
    check_bw_status_with_session(None)
}

/// Runs `bw status` and returns its raw stdout, optionally under a
/// `BW_SESSION`. `None` when the CLI could not be run at all (no verified
/// `bw.exe`, or the spawn failed) -- both already logged here.
fn bw_status_stdout(session_token: Option<&str>) -> Option<String> {
    bw_status_stdout_in(crate::bw_path::active_data_dir().as_deref(), session_token)
}

/// [`bw_status_stdout`] against a **named** profile directory rather than the
/// active account's.
///
/// `Some(dir)` is the only interesting case: asking the CLI "whose vault is
/// in *this* directory?" is how `add_account` learns the address a sign-in
/// just landed under, and it has to be a different directory from the one this
/// process is currently pointed at. `None` keeps
/// `bw_command_in`'s meaning -- no override, so the child inherits whatever
/// `BITWARDENCLI_APPDATA_DIR` the environment already had.
fn bw_status_stdout_in(data_dir: Option<&Path>, session_token: Option<&str>) -> Option<String> {
    // `login_ui.rs` is one of the files `job_object`'s tree walk excuses: these
    // are short-lived `bw` children waited on inline, and none of them outlives
    // the call. Taking the command out of `BareCommand` is the visible act of
    // leaving the kill-on-close job's reach.
    let mut cmd = match crate::bw_path::bw_command_in(data_dir) {
        Ok(cmd) => cmd.into_jobless_command(),
        Err(e) => {
            log::error!("cannot run `bw status`: {e}");
            return None;
        }
    };
    cmd.arg("status");
    if let Some(token) = session_token {
        cmd.env("BW_SESSION", token);
    }

    match cmd.output() {
        Ok(output) => Some(String::from_utf8_lossy(&output.stdout).into_owned()),
        Err(e) => {
            log::error!(
                "failed to run `bw status` from the verified Bitwarden CLI path \
                 (see bw_path::resolve_bw_exe for where that path comes from): {e}"
            );
            None
        }
    }
}

/// Runs `bw status`, optionally with a `BW_SESSION` so the CLI can report
/// `unlocked` for a *specific* session token rather than only for whatever is
/// in the ambient environment.
///
/// A cached session token is worthless if it has since been invalidated (a
/// manual `bw lock`, a password change, a reboot), so this is how startup
/// checks a cached token before trusting it. Failure to run the CLI at all --
/// whether because no verified `bw.exe` was recorded at startup or because
/// spawning it failed -- is logged and reported as `Unauthenticated` rather
/// than panicking the whole app.
pub fn check_bw_status_with_session(session_token: Option<&str>) -> BwStatus {
    bw_status_stdout(session_token)
        .map(|stdout| parse_bw_status(&stdout))
        .unwrap_or(BwStatus::Unauthenticated)
}

/// [`check_bw_status_with_session`] plus the account email and server URL.
///
/// Exists so that startup gets the status **and** the account identity out of
/// a single `bw status` spawn. The identity is what
/// [`crate::vault_disk_cache::account_fingerprint`] hashes into the encrypted
/// disk cache's header; a second spawn to learn it would cost the launch
/// another one to three seconds, on the one path whose entire purpose is to
/// be fast.
pub fn check_bw_status_details_with_session(session_token: Option<&str>) -> BwStatusDetails {
    bw_status_stdout(session_token)
        .map(|stdout| parse_bw_status_details(&stdout))
        .unwrap_or(BwStatusDetails {
            status: BwStatus::Unauthenticated,
            user_email: None,
            server_url: None,
        })
}

/// [`check_bw_status`] plus the account email and server URL, for the login
/// window's 3h chrome.
pub fn check_bw_status_details() -> BwStatusDetails {
    check_bw_status_details_in(crate::bw_path::active_data_dir().as_deref())
}

/// [`check_bw_status_details`] asked of a **named** profile directory.
///
/// Every caller that has an account in hand uses this form rather than
/// [`check_bw_status_details`]: `add_account` asks it about the directory the
/// new sign-in landed in, which is not the one this process is pointed at. A
/// version that ignored its argument would answer for whatever profile the
/// process happens to be on, and the new account would inherit the previous
/// account's address.
pub fn check_bw_status_details_in(data_dir: Option<&Path>) -> BwStatusDetails {
    bw_status_stdout_in(data_dir, None)
        .map(|stdout| parse_bw_status_details(&stdout))
        .unwrap_or_else(unknown_status_details)
}

/// What `bw status` is taken to have said when it did not say anything: no
/// account, no address, no server.
///
/// The same value whether the CLI could not be spawned at all, answered
/// something unparseable, or simply did not answer inside
/// [`STATUS_DEADLINE`]. Shared rather than written out three times because
/// [`status_details_within`] has to be able to return *exactly* what a failed
/// spawn returns -- a bounded wait that invented a different "unknown" would
/// be a second unauthenticated-looking state for the rest of the app to
/// disagree about.
pub fn unknown_status_details() -> BwStatusDetails {
    BwStatusDetails {
        status: BwStatus::Unauthenticated,
        user_email: None,
        server_url: None,
    }
}

/// How long anything may wait on a `bw status` before deciding it is not
/// coming.
///
/// **Deliberately much shorter than `bw_serve::BACKEND_OP_TIMEOUT` (90s),
/// which is the number this crate uses for the other untimed `bw` spawns**,
/// and the difference is not impatience -- it is what the two failures cost.
/// `BACKEND_OP_TIMEOUT` bounds *starting the backend*: give up early there
/// and there is no vault. This bounds `bw status`, whose only consumer is the
/// account name, address and server the vault window's toolbar shows
/// (`StartupWork::produce` -> `vault_window::AccountDetails::Ready`). Give up
/// early here and the toolbar is missing a name; the vault still opens, the
/// session is still good, and nothing the user did is thrown away.
///
/// That asymmetry runs the other way too, and is the reason this number is
/// small rather than merely bounded: this phase's budget is charged to
/// `app_window::WORKING_DEADLINE`, the watchdog that CAN throw a healthy
/// sign-in away. Every second credited to a cosmetic phase is a second the
/// window spends refusing to give up on a `bw status` nobody is waiting to
/// read.
///
/// Thirty seconds. `bw status` is a single CLI spawn, measured at 2.39s on
/// the user's machine (see `main`'s `account_details_source`), so this is
/// over ten times a real one and still an order of magnitude away from the
/// backend-start budget. A literal rather than a borrowed constant: it is a
/// claim about how long ONE `bw status` takes, and nothing in `bw_serve`
/// should be able to move it.
pub const STATUS_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// Waits up to `budget` for a `bw status` that `spawn` is expected to run on
/// a thread, and reports [`unknown_status_details`] if it does not arrive.
///
/// **The decision, separated from the spawning.** `check_bw_status_details`
/// is a bare `Command::output()` with no timeout of its own, and every caller
/// that has a deadline was previously bounding only *itself*: the window's
/// watchdog would fire while the `bw` child was still running, and the user
/// had by then watched a frozen spinner through a budget that claimed to
/// cover this call and did not. This is what actually covers it.
///
/// It bounds the WAIT, not the child -- the `bw` process is left to finish
/// and its answer is dropped. That is the same trade `main`'s
/// `resettle_session_with` already makes against `BACKEND_OP_TIMEOUT`
/// ("giving up and proceeding anyway is strictly safer"), and it is the only
/// one available: `std::process::Child` has no timed wait, and killing a
/// `bw status` mid-flight to save a toolbar label would be a worse bargain
/// than dropping the label.
///
/// `spawn` is a parameter for the reason `account_details_source`'s is: it is
/// how the timeout decision gets tested without a `bw` CLI, a real profile
/// directory, or thirty seconds of waiting.
pub fn status_details_within(
    budget: std::time::Duration,
    spawn: impl FnOnce(std::sync::mpsc::Sender<BwStatusDetails>),
) -> BwStatusDetails {
    let (tx, rx) = std::sync::mpsc::channel();
    spawn(tx);
    match rx.recv_timeout(budget) {
        Ok(details) => details,
        Err(e) => {
            log::warn!(
                "`bw status` did not answer within {budget:?} ({e}); opening without the \
                 account name and server. The `bw` process is left to finish on its own -- \
                 this bounds the wait, not the child."
            );
            unknown_status_details()
        }
    }
}

/// [`check_bw_status_details`] that cannot hold its caller for longer than
/// [`STATUS_DEADLINE`].
///
/// The form `StartupWork::produce` calls, and the reason
/// `app_window::WORKING_DEADLINE` can now name a real bound for its third
/// phase instead of borrowing the backend-start budget as a guess.
pub fn check_bw_status_details_bounded() -> BwStatusDetails {
    status_details_within(STATUS_DEADLINE, |tx| {
        std::thread::spawn(move || {
            let _ = tx.send(check_bw_status_details());
        });
    })
}

/// Runs `bw logout`, for 3h's "Log out" footer action. Already being logged
/// out counts as success -- the goal state is "no account", however we got
/// there.
///
/// **Acts on whatever profile this process is currently pointed at**, which is
/// what "Log out" means and is the *wrong* thing for a removal: the account
/// being removed is not necessarily the active one. That caller wants
/// [`bw_logout_in`], and `main.rs`'s `the_removal_logs_the_doomed_account_out`
/// bans this form from reaching it.
pub fn bw_logout() -> Result<(), String> {
    bw_logout_in(crate::bw_path::active_data_dir().as_deref())
}

/// [`bw_logout`] against a **named** profile directory rather than the active
/// account's.
///
/// The same shape, and for the same reason, as
/// [`check_bw_status_details_in`]: `account_removal` has to log out the
/// account it is about to delete, which is not the account this process is on.
/// The alternative -- temporarily pointing `bw_path::set_active_data_dir` at
/// the doomed directory and calling [`bw_logout`] -- is a process-global that
/// background threads spawn `bw` against, so the window in which it names
/// another account's profile is a window in which a sync can land in the wrong
/// vault.
///
/// `None` keeps `bw_command_in`'s meaning: no override, so the child inherits
/// whatever `BITWARDENCLI_APPDATA_DIR` the environment already had.
pub fn bw_logout_in(data_dir: Option<&Path>) -> Result<(), String> {
    let output = logout_command_in(data_dir)?
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.to_lowercase().contains("not logged in") {
        Ok(())
    } else if stderr.is_empty() {
        Err("`bw logout` failed".to_string())
    } else {
        Err(stderr)
    }
}

/// The `bw logout` invocation, built but not run.
///
/// Separated from [`bw_logout_in`] purely so a test can read back the
/// environment and the arguments it carries: `.output()` needs a real `bw.exe`
/// and would sign the user out of a real vault, and there is no other way to
/// see *which directory* a logout was aimed at -- a `bw_logout_in` that
/// dropped its argument would leave the same "already logged out" success
/// behind as one that used it, having signed the wrong account out.
fn logout_command_in(data_dir: Option<&Path>) -> Result<std::process::Command, String> {
    let mut cmd = crate::bw_path::bw_command_in(data_dir)?.into_jobless_command();
    cmd.arg("logout");
    Ok(cmd)
}

/// Points the Bitwarden CLI at a self-hosted server, in a **named** profile
/// directory.
///
/// The directory is a parameter for the same reason
/// [`check_bw_status_details_in`]'s and [`bw_logout_in`]'s are, and it matters
/// more here than the name suggests: `bw config server` is not process state,
/// it is a **write into that profile's `data.json`**. Aimed at the wrong
/// directory it re-points the account the user is already signed into at
/// somebody else's server. Adding a self-hosted account is exactly the case
/// where the two directories differ.
///
/// Returns `Err` rather than panicking: a typo in a self-hosted URL is
/// ordinary user error and belongs inline in the login window (the same way
/// `run_bw_with_password` failures already are), not as a process-killing
/// panic with a Rust backtrace.
pub fn configure_server_in(url: &str, data_dir: Option<&Path>) -> Result<(), String> {
    let output = crate::bw_path::bw_command_in(data_dir)?
        .into_jobless_command()
        .args(["config", "server", url])
        .output()
        .map_err(|e| {
            format!(
                "failed to run `bw config server` from the verified Bitwarden CLI path \
                 (see bw_path::resolve_bw_exe for where that path comes from): {e}"
            )
        })?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("`bw config server {url}` failed")
        } else {
            stderr
        })
    }
}

/// Runs `bw` with the given args plus a password supplied via an
/// environment variable (`--passwordenv`), never as a bare CLI argument --
/// a bare-argument password would be visible to other processes/users
/// via the OS process list.
///
/// The binary this spawns is the one startup resolved *and* verified as
/// Bitwarden-signed (`bw_path::bw_command_in`), never a freshly-resolved one:
/// this is the single call site that hands over the master password, so it
/// must not be able to pick up a `bw.exe` that appeared after that check.
///
/// `data_dir` names the profile the `bw login`/`bw unlock` acts on. It is a
/// parameter rather than `bw_path::active_data_dir()` because of what the
/// caller would otherwise have to do to sign in to a *new* account: point the
/// process-global at that account's directory across a blocking, user-paced
/// window and put it back afterwards. Background threads spawn `bw` against
/// that global (`bw_serve::start_bw_serve`, `bw_serve::sync_now`), so a
/// window in which it names another account's profile is a window in which a
/// sync can land in the wrong vault -- the rule `main`'s `remove_account`
/// already states and `bw_logout_in` already obeys.
pub(crate) fn run_bw_with_password(
    args: &[&str],
    password: &str,
    data_dir: Option<&Path>,
) -> Result<String, String> {
    let mut cmd = crate::bw_path::bw_command_in(data_dir)?.into_jobless_command();
    cmd.args(args);
    cmd.args(["--passwordenv", "DESKWARDEN_BW_PASSWORD"]);
    cmd.env("DESKWARDEN_BW_PASSWORD", password);
    let output = cmd.output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Turns the Bitwarden CLI's raw stderr from a failed `bw login`/`bw unlock`
/// into one line a person can act on.
///
/// A mistyped master password makes the CLI print four framework-level log
/// lines, led by `ERROR bitwarden_crypto::keys::master_key: error=The
/// decryption operation failed` — alarming, and useless to someone who
/// simply typed the wrong thing. The raw text still reaches the log at the
/// call site; this is only what the window shows.
///
/// Unrecognised failures fall back to the CLI's own wording with the
/// `ERROR <module>: error=` scaffolding stripped, rather than a blanket
/// "something went wrong": an unfamiliar but real message is far easier to
/// act on — or report — than none at all.
/// The inline error for a submit that can't succeed because a required
/// field is blank, or `None` when there's something to send.
///
/// Checked before the CLI is spawned rather than after. `bw` receives the
/// master password through an environment variable (`--passwordenv`, so it
/// never appears in the process list), and on Windows setting a variable to
/// an empty string is indistinguishable from not setting it at all — so an
/// empty field produced a spawned CLI answering "Provided passwordenv
/// DESKWARDEN_BW_PASSWORD is not set", which describes our own plumbing
/// rather than telling the user they left a box blank.
///
/// The email is only required when signing in; unlocking an existing
/// account already knows whose vault it is.
pub fn missing_credential_message(
    status: BwStatus,
    email: &str,
    password: &str,
) -> Option<&'static str> {
    if status == BwStatus::Unauthenticated && email.trim().is_empty() {
        Some("Enter your email address first.")
    } else if password.is_empty() {
        Some("Enter your master password first.")
    } else {
        None
    }
}

pub fn friendly_auth_error(stderr: &str) -> String {
    let haystack = stderr.to_ascii_lowercase();
    let mentions = |needles: &[&str]| needles.iter().any(|n| haystack.contains(n));

    // Unlock against a bad password fails inside the crypto layer (the vault
    // key simply won't decrypt), which is why this reads as a cryptography
    // fault rather than an authentication one.
    if mentions(&[
        "decryption operation failed",
        "invalid master password",
        "cryptography error",
    ]) {
        return "That master password didn't work. Check it and try again.".to_string();
    }
    // Sign-in, by contrast, is rejected by the server, and either field
    // could be the wrong one.
    if mentions(&[
        "username or password is incorrect",
        "invalid username or password",
    ]) {
        return "That email or master password didn't work. Check them and try again."
            .to_string();
    }
    if mentions(&["two-step", "two step", "two-factor", "twofactor"]) {
        return "This account uses two-step login, which Deskwarden can't prompt for. \
                Run `bw login` in a terminal once to complete it, then come back."
            .to_string();
    }
    if mentions(&["too many", "rate limit", "traffic from your network"]) {
        return "Too many failed attempts. Wait a few minutes, then try again.".to_string();
    }
    if mentions(&[
        "econnrefused",
        "enotfound",
        "etimedout",
        "getaddrinfo",
        "failed to fetch",
    ]) {
        return "Couldn't reach the server. Check your connection — and the server URL, \
                if this is a self-hosted account."
            .to_string();
    }

    strip_cli_log_scaffolding(stderr)
}

/// The most human line the CLI printed, with its log framing removed. Used
/// only for failures [`friendly_auth_error`] has no specific wording for.
fn strip_cli_log_scaffolding(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();

    // The CLI prints its own plain summary alongside the `ERROR <module>:`
    // lines; that summary is the most readable thing available, so prefer it.
    if let Some(summary) = lines.iter().find(|line| !line.starts_with("ERROR ")) {
        return (*summary).to_string();
    }
    // Otherwise unwrap the first framework line: everything after `error=`
    // is the real message.
    if let Some(first) = lines.first() {
        return match first.split_once("error=") {
            Some((_, message)) => message.to_string(),
            None => (*first).to_string(),
        };
    }
    "Could not unlock the vault. The log has the details.".to_string()
}

/// Which server the sign-in talks to — the native client's bottom-of-page
/// "Logging in on" dropdown. Selection matters *before* `bw login`, because
/// the CLI's server is global config (`bw config server`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServerChoice {
    #[default]
    UsCloud,
    EuCloud,
    SelfHosted,
}

impl ServerChoice {
    pub fn label(self) -> &'static str {
        match self {
            Self::UsCloud => "bitwarden.com",
            Self::EuCloud => "bitwarden.eu",
            Self::SelfHosted => "Self-hosted",
        }
    }

    /// The URL `bw config server` gets for the cloud choices; `None` for
    /// self-hosted (the form's URL field supplies it).
    pub fn config_url(self) -> Option<&'static str> {
        match self {
            Self::UsCloud => Some("https://vault.bitwarden.com"),
            Self::EuCloud => Some("https://vault.bitwarden.eu"),
            Self::SelfHosted => None,
        }
    }

    /// Initial dropdown state from the CLI's currently-configured server, so
    /// re-login on a self-hosted setup doesn't silently flip to the cloud.
    pub fn from_configured(server_url: Option<&str>) -> (Self, String) {
        match server_url {
            None => (Self::UsCloud, String::new()),
            Some(url) if url.contains("bitwarden.eu") => (Self::EuCloud, String::new()),
            Some(url) if url.contains("bitwarden.com") => (Self::UsCloud, String::new()),
            Some(url) => (Self::SelfHosted, url.to_string()),
        }
    }
}

/// Space the window keeps below the flowing content: the pinned footer row
/// plus the body's bottom margin.
pub const FOOTER_RESERVE: f32 = 62.0;

// --- The login card on the empty app --------------------------------------
//
// The user asked for the sign-in window to "pre-load the big screen with the
// position and size etc as it will be after log-in but empty kinda of and
// then login on top blocking it". This window is therefore no longer a 470x588
// card that is the whole window: it opens at the VAULT window's saved
// geometry, paints that window's empty chrome, and floats the unchanged card
// in the middle of it.
//
// It is still its own `run_native`. Merging the two eframe apps is not an
// option and was not attempted: eframe cannot nest event loops, and
// `vault_window::run` needs a live backend and a populated cache that by
// definition do not exist before sign-in.

/// What the placeholder titlebar says. **The vault window's own wordmark**,
/// taken from its own constant: the bar the user is looking at while they sign
/// in is the bar that is still there afterwards, and a different word in it
/// would be the same jump this change removes, just smaller.
///
/// The OS-level window title stays "Log in to Deskwarden" -- that is what the
/// taskbar and `round_window_corners` see, and it is what tells the two
/// windows apart to everything outside this process.
const WINDOW_TITLE: &str = "Log in to Deskwarden";
const VAULT_WINDOW_TITLE: &str = crate::vault_window::WINDOW_TITLE;

/// The login card's width -- unchanged. The card's composition is the design's
/// 3h and was not asked to change; only where it sits did.
const LOGIN_CARD_WIDTH: f32 = 470.0;

/// The card's height before the first frame has measured its content.
///
/// The window used to BE the card, at 470x588 with a 40px `ChromeMetrics::LOGIN`
/// bar above the body; 588 - 40 is what that composition measured. Any wrong
/// value here would settle within one frame (see `login_card_height`), but
/// the frame it is wrong on is the first one the user sees.
const LOGIN_CARD_INITIAL_HEIGHT: f32 = 548.0;

/// The card's own padding: 3h's body margins, kept exactly.
const CARD_MARGIN_X: f32 = 26.0;
const CARD_MARGIN_TOP: f32 = 24.0;
/// Deeper than the top: the footer row's dropdown otherwise sits too close to
/// the card's edge. Note this is ALREADY counted inside [`FOOTER_RESERVE`],
/// which is why `login_card_height` does not add it twice.
const CARD_MARGIN_BOTTOM: f32 = 30.0;

/// What is painted over the empty panes so the card reads as blocking them.
///
/// Over the body only, NOT over the titlebar. The bar carries the live
/// ✕ and — controls, and dimming a control the user is still expected to be
/// able to find and hit says the opposite of what a scrim is for. The panes
/// underneath are the part that is inert.
const SCRIM: Color32 = Color32::from_black_alpha(56);

/// The vault window's layout, as empty regions: the titlebar, and the three
/// panes at the widths `vault_window` really uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VaultSkeleton {
    pub bar: egui::Rect,
    /// Everything under the bar: one flat region, and what [`SCRIM`] covers.
    ///
    /// This used to be divided into the vault window's three panes, painted
    /// in their real fills with the hairlines between them. It read as a
    /// mock of the app rather than as the app waiting, and the user asked
    /// for the divisions gone. The bar stays -- it is the same bar the vault
    /// window keeps, so it is continuity rather than decoration.
    pub body: egui::Rect,
}

/// Divides `full` the way `vault_window::run` divides its window.
///
/// The widths come from `vault_window`'s own constants and the bar height from
/// [`ChromeMetrics::VAULT`], because the whole point of this shape is that it
/// is the app the user is about to be looking at. Numbers copied over here
/// would be numbers that drift, and a placeholder whose panes are the wrong
/// widths is worse than no placeholder: the window would visibly re-flow at
/// the moment of sign-in, which is the jump this exists to remove.
pub fn vault_skeleton(full: egui::Rect) -> VaultSkeleton {
    let bar = egui::Rect::from_min_max(
        full.min,
        Pos2::new(full.max.x, full.min.y + ChromeMetrics::VAULT.bar_height),
    );
    let body = egui::Rect::from_min_max(Pos2::new(full.min.x, bar.max.y), full.max);
    VaultSkeleton { bar, body }
}

/// Paints the body behind the card: one flat [`theme::CANVAS`] region under
/// the scrim, and nothing else.
///
/// It was the vault window's three panes in their own fills with the
/// hairlines between them. That drew a picture *of* the app, and a picture of
/// an app with nothing in it is a mock -- the divisions promise a sidebar and
/// a list that are not there and cannot be interacted with. One region
/// promises nothing and simply gets out of the card's way.
///
/// The bar above it is still painted (by the caller): that one is not
/// decoration, it is the same titlebar the vault window keeps afterwards, so
/// it is the part that makes this window and the next one feel continuous.
pub fn paint_vault_skeleton(painter: &egui::Painter, skeleton: &VaultSkeleton) {
    painter.rect_filled(skeleton.body, CornerRadius::ZERO, theme::CANVAS);
    painter.rect_filled(skeleton.body, CornerRadius::ZERO, SCRIM);
}

/// Where the card sits: centred in the empty app's BODY, not in the whole
/// window -- the titlebar is chrome, and a card centred against it sits
/// visibly high in the region it is blocking.
pub fn login_card_rect(skeleton: &VaultSkeleton, card_height: f32) -> egui::Rect {
    egui::Rect::from_center_size(
        skeleton.body.center(),
        Vec2::new(LOGIN_CARD_WIDTH, card_height),
    )
}

/// The card's content area: the card less 3h's own margins.
pub fn login_card_content_rect(card: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        Pos2::new(card.min.x + CARD_MARGIN_X, card.min.y + CARD_MARGIN_TOP),
        Pos2::new(card.max.x - CARD_MARGIN_X, card.max.y - CARD_MARGIN_BOTTOM),
    )
}

/// The height the card wants, given where `draw_login_window`'s flowing
/// content ended and where its content area began.
///
/// The window used to send itself a `ViewportCommand::InnerSize` from exactly
/// this measurement (`flow_bottom + FOOTER_RESERVE`); it now sizes the card
/// instead and leaves the window alone. [`CARD_MARGIN_BOTTOM`] is not added:
/// [`FOOTER_RESERVE`] is the footer row PLUS that margin, which is why the
/// two numbers do not both appear here.
pub fn login_card_height(flow_bottom: f32, content_top: f32) -> f32 {
    (CARD_MARGIN_TOP + (flow_bottom - content_top) + FOOTER_RESERVE).ceil()
}

/// Gap between a field's label and the field itself.
const LABEL_GAP: f32 = 7.0;
/// Gap between one label+field group and the next (and before the action
/// button). Deliberately much larger than [`LABEL_GAP`] -- roughly 3x -- so
/// each label reads as bound to the field under it rather than floating
/// between two of them.
const GROUP_GAP: f32 = 22.0;

/// Track width of the in-flight bar beside Continue.
///
/// **This was a 20px rotating disc**, and design turn 7 retires the disc from
/// every surface a user meets before the vault — the sign-in card included.
/// The owner's instruction was "it should always be one screen with line
/// spinner as per design", and the card is the first of the screens that one
/// window shows.
///
/// Narrower than the waiting bodies' 260/200px tracks because this one sits
/// INSIDE a row beside a button rather than centred in an empty frame; the
/// proportions it is drawn with (32% knob, 3px, 1.4s) are
/// [`theme::progress_bar`]'s and are the same here as there.
/// `ui.horizontal`'s `Align::Center` keeps it centred on the taller button,
/// and the row's height is still the button's, so nothing reflows when it
/// appears — which is what the disc's own note was protecting and is only
/// more true of something 3px tall.
const AUTH_BAR_WIDTH: f32 = 96.0;

/// What the custom titlebar asked for this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromeAction {
    None,
    Minimize,
    Close,
}

/// Whether the chrome's ✕ is live this frame.
///
/// A window that cannot be closed right now is a real state in this app -- the
/// single startup window's WORKING stage holds the only handle to a `bw serve`
/// that is still coming up, and closing would strand that process on the port
/// the recovery then needs to bind. That stage used to have no chrome at all,
/// so the question never arose; now that it wears the same heading as every
/// other window, it has a ✕, and a ✕ that silently refuses is worse than one
/// that shows it is unavailable. This is how the chrome says so, rather than a
/// second copy of the titlebar that would start diverging from this one.
///
/// The same judgement the sign-in card already makes about its credential
/// fields, which grey out while a sign-in is in flight instead of staying live
/// and ignoring input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseControl {
    /// Clickable, and reports [`ChromeAction::Close`]. Every window whose ✕
    /// means something.
    Active,
    /// Drawn ghosted in [`theme::TEXT_GHOST`] -- the same treatment the ▢ gets
    /// on a fixed-size window -- with no hover and no interaction registered
    /// at all, so it cannot report a close for a host to have to refuse.
    Disabled,
}

/// Sizing/color knobs for [`draw_window_chrome_with_extra`]'s bar, mark, and
/// title. The login window (design 3h) and the vault window (design 2b)
/// share every other line of the chrome-painting logic but use different
/// exact values for these -- rather than duplicate the whole function per
/// window, this is the one part that varies, passed in instead of
/// hardcoded.
#[derive(Debug, Clone, Copy)]
pub struct ChromeMetrics {
    pub bar_height: f32,
    /// Left padding from the bar's left edge to the mark.
    pub left_padding: f32,
    pub mark_size: Vec2,
    pub title_font_size: f32,
    /// [`theme::SEMIBOLD`] or [`theme::BOLD`].
    pub title_family: &'static str,
    pub title_color: Color32,
    /// Width of each of the three ✕/▢/— control zones.
    pub control_width: f32,
}

impl ChromeMetrics {
    /// Design 3h (login/unlock window): 40px bar, 14px left padding, 15×17
    /// mark, 12px SemiBold title in `TEXT_SECONDARY`, 40px control zones.
    pub const LOGIN: Self = Self {
        bar_height: 40.0,
        left_padding: 14.0,
        mark_size: Vec2::new(15.0, 17.0),
        title_font_size: 12.0,
        title_family: theme::SEMIBOLD,
        title_color: theme::TEXT_SECONDARY,
        control_width: 40.0,
    };

    /// Design 2b (vault window): 46px bar, 16px left padding, 22×22 mark,
    /// 14px ExtraBold title (the design's `font-weight: 800`, same wordmark
    /// treatment as the login window's larger one). The design's title
    /// `<div>` carries no explicit
    /// `color`, unlike the login window's (`#444141`/`TEXT_SECONDARY`) --
    /// it inherits the page's default body text color (`#201e1d`/`INK`,
    /// set on `html, body` in `Deskwarden.dc.html`), so that's what this
    /// uses. 42px control zones (design: `width: 42px` per — /▢/✕ cell,
    /// versus the login window's 40px).
    pub const VAULT: Self = Self {
        bar_height: 46.0,
        left_padding: 16.0,
        mark_size: Vec2::new(22.0, 22.0),
        title_font_size: 14.0,
        title_family: theme::EXTRABOLD,
        title_color: theme::INK,
        control_width: 42.0,
    };
}

/// Draws 3h's window chrome — the design's titlebar is a custom white bar
/// (15×17 mark, 12px title, ghost window controls), which the native
/// Windows frame cannot be themed into, so the window runs frameless and
/// this paints the chrome: full-window background, a 40px draggable
/// titlebar, a hairline under it, and — ▢ ✕ controls (▢ inert: the window
/// is fixed-size). Reserves the titlebar's space in `ui`'s layout; window
/// rounding comes from DWM (see [`round_window_corners`]).
///
/// Thin wrapper around [`draw_window_chrome_with_extra`] with no extra
/// content, [`ChromeMetrics::LOGIN`]'s sizing, `maximizable: false` -- this
/// window is fixed-size, so ▢ stays the inert, ghosted affordance it has
/// always been -- and a live ✕. See that function for the shared
/// implementation.
pub fn draw_window_chrome(ui: &mut egui::Ui, title: &str) -> ChromeAction {
    draw_window_chrome_with_extra(
        ui,
        title,
        ChromeMetrics::LOGIN,
        false,
        CloseControl::Active,
        |_ui| {},
    )
}

/// Same as [`draw_window_chrome`], but calls `extra_content` to draw
/// additional widgets in the bar between the title and the window controls
/// (used by the vault window's toolbar buttons: Lock, the account avatar,
/// and Sync), paints the bar/mark/title per `metrics` (see
/// [`ChromeMetrics`]), and -- when `maximizable` is true -- makes the ▢
/// control a real, clickable maximize/restore toggle instead of the
/// permanently-ghosted affordance both windows used to show unconditionally.
/// `close` says whether the ✕ is live or drawn disabled (see
/// [`CloseControl`]).
/// `extra_content` is laid out right-to-left, packed against the left edge
/// of the window-control zones, so it reads as continuing naturally into
/// ✕/▢/—. The draggable region is shrunk to end where `extra_content`'s
/// actually-rendered content begins (not just the reserved area for it), so
/// clicking those widgets doesn't also start dragging the window; when
/// `extra_content` draws nothing (the plain [`draw_window_chrome`] case
/// above) the drag zone is unchanged from before this function existed.
pub fn draw_window_chrome_with_extra(
    ui: &mut egui::Ui,
    title: &str,
    metrics: ChromeMetrics,
    maximizable: bool,
    close: CloseControl,
    extra_content: impl FnOnce(&mut egui::Ui),
) -> ChromeAction {
    let mut action = ChromeAction::None;
    let full = ui.max_rect();
    let bar = egui::Rect::from_min_max(
        full.min,
        egui::Pos2::new(full.max.x, full.min.y + metrics.bar_height),
    );

    // Backgrounds first: window body, titlebar, hairline.
    ui.painter()
        .rect_filled(full, CornerRadius::ZERO, theme::WINDOW_BG);
    ui.painter()
        .rect_filled(bar, CornerRadius::ZERO, theme::CARD);
    ui.painter().rect_filled(
        egui::Rect::from_min_max(egui::Pos2::new(bar.min.x, bar.max.y - 1.0), bar.max),
        CornerRadius::ZERO,
        theme::HAIRLINE,
    );
    ui.painter().rect_stroke(
        full,
        CornerRadius::ZERO,
        Stroke::new(1.0, theme::BORDER),
        egui::StrokeKind::Inside,
    );

    // Left: the mark and the title, sized/colored per `metrics`.
    let mark_rect = egui::Rect::from_min_size(
        egui::Pos2::new(
            bar.min.x + metrics.left_padding,
            bar.center().y - metrics.mark_size.y / 2.0,
        ),
        metrics.mark_size,
    );
    theme::paint_mark(ui.painter(), mark_rect);
    ui.painter().text(
        egui::Pos2::new(mark_rect.right() + 10.0, bar.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::new(
            metrics.title_font_size,
            egui::FontFamily::Name(metrics.title_family.into()),
        ),
        metrics.title_color,
    );

    // Right: the three control zones (40px login, 42px vault). Glyphs are
    // drawn, not typed, so they can't fall through to a fallback font's
    // rendition.
    let control = |i: usize| {
        egui::Rect::from_min_max(
            egui::Pos2::new(bar.max.x - metrics.control_width * (i + 1) as f32, bar.min.y + 1.0),
            egui::Pos2::new(bar.max.x - metrics.control_width * i as f32, bar.max.y - 1.0),
        )
    };
    let controls_left = control(2).min.x;
    let glyph_stroke = Stroke::new(1.2, theme::TEXT_FAINT);

    // Between the title and the control zones: `extra_content`'s reserved
    // area, right-to-left so it packs against the controls rather than
    // floating in the middle of the bar. The left bound (200px in from the
    // titlebar's left edge) is just generously past where the mark+title
    // ends -- it only caps how far left content is *allowed* to grow, it
    // isn't where empty content would visually start.
    let extra_max_rect = egui::Rect::from_min_max(
        egui::Pos2::new(bar.min.x + 200.0, bar.min.y),
        egui::Pos2::new(controls_left - 16.0, bar.max.y),
    );
    let extra_response = ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(extra_max_rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
        extra_content,
    );
    let extra_used = extra_response.response.rect;
    // Empty `extra_content` (the plain `draw_window_chrome` case) leaves
    // `extra_used` a zero-width rect with nothing actually drawn in it --
    // fall back to the controls' own left edge, exactly matching this
    // function's drag-zone width before `extra_content` existed.
    let drag_zone_end_x = if extra_used.width() > 0.0 {
        extra_used.left().min(controls_left)
    } else {
        controls_left
    };

    // Close (✕). When `CloseControl::Disabled`, NO interaction is registered
    // for it at all and the glyph is drawn in `TEXT_GHOST` -- exactly the
    // ghosted ▢ treatment below, for exactly the same reason: the control is
    // there because the heading is, and it cannot do anything right now. Not
    // registering the interaction is the load-bearing half; a hover-less but
    // still-clickable ✕ would hand the host a `Close` it has to refuse, which
    // is the silent refusal this avoids.
    let close_rect = control(0);
    let close_stroke = match close {
        CloseControl::Active => {
            let close = ui.interact(close_rect, ui.id().with("chrome-close"), Sense::click());
            if close.hovered() {
                ui.painter()
                    .rect_filled(close_rect, CornerRadius::ZERO, theme::CANVAS);
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if close.clicked() {
                action = ChromeAction::Close;
            }
            glyph_stroke
        }
        CloseControl::Disabled => Stroke::new(1.2, theme::TEXT_GHOST),
    };
    let c = close_rect.center();
    ui.painter().line_segment(
        [c + egui::vec2(-4.5, -4.5), c + egui::vec2(4.5, 4.5)],
        close_stroke,
    );
    ui.painter().line_segment(
        [c + egui::vec2(-4.5, 4.5), c + egui::vec2(4.5, -4.5)],
        close_stroke,
    );

    // Maximize (▢). When `maximizable` (the vault window): the same active
    // glyph/hover treatment as ✕/— below, and a click toggles the OS-level
    // maximized state. The current state is queried fresh from
    // `ViewportInfo::maximized` on every click rather than tracked in a
    // local flag here -- this function has no persistent state of its own,
    // and querying avoids this control ever drifting out of sync with a
    // maximize/restore that happened some other way (e.g. a taskbar
    // action). When not `maximizable` (the login window, which is
    // fixed-size): unchanged from before -- drawn ghosted, with no hover or
    // click handler at all.
    let max_rect = control(1);
    if maximizable {
        let maximize = ui.interact(max_rect, ui.id().with("chrome-max"), Sense::click());
        if maximize.hovered() {
            ui.painter()
                .rect_filled(max_rect, CornerRadius::ZERO, theme::CANVAS);
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        ui.painter().rect_stroke(
            egui::Rect::from_center_size(max_rect.center(), egui::Vec2::splat(9.0)),
            CornerRadius::ZERO,
            glyph_stroke,
            egui::StrokeKind::Middle,
        );
        if maximize.clicked() {
            let currently_maximized =
                ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(!currently_maximized));
        }
    } else {
        ui.painter().rect_stroke(
            egui::Rect::from_center_size(max_rect.center(), egui::Vec2::splat(9.0)),
            CornerRadius::ZERO,
            Stroke::new(1.2, theme::TEXT_GHOST),
            egui::StrokeKind::Middle,
        );
    }

    // Minimize (—).
    let min_rect = control(2);
    let minimize = ui.interact(min_rect, ui.id().with("chrome-min"), Sense::click());
    if minimize.hovered() {
        ui.painter()
            .rect_filled(min_rect, CornerRadius::ZERO, theme::CANVAS);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let m = min_rect.center();
    ui.painter().line_segment(
        [m + egui::vec2(-4.5, 0.0), m + egui::vec2(4.5, 0.0)],
        glyph_stroke,
    );
    if minimize.clicked() {
        action = ChromeAction::Minimize;
    }

    // Everything left of the controls (and left of any `extra_content` that
    // was actually drawn) drags the window.
    let drag_zone =
        egui::Rect::from_min_max(bar.min, egui::Pos2::new(drag_zone_end_x, bar.max.y));
    let drag = ui.interact(
        drag_zone,
        ui.id().with("chrome-drag"),
        Sense::click_and_drag(),
    );
    if drag.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }

    ui.advance_cursor_after_rect(bar);
    action
}

/// Asks DWM to round this window's corners (and give it the standard
/// drop-shadow) even though it is frameless. Windows 11 only; on Windows 10
/// the call fails harmlessly and the window stays square. Resolved by title
/// because eframe never exposes the HWND.
///
/// **Scoped to this process**, through
/// [`crate::foreground::own_window_titled`]. It used to ask
/// `FindWindowW(None, title)`, which searches the WHOLE DESKTOP and returns
/// whichever top-level window `EnumWindows` reaches first with that exact
/// title -- and "Deskwarden" is a common noun: a File Explorer window open on
/// this repo's own `deskwarden\` folder carries exactly that title, and so
/// does a second copy of this app. This function then wrote a DWM attribute
/// into a window belonging to a process it does not own, non-deterministically
/// and permanently for that window's lifetime. Measured happening: the frame
/// harness reaches the `!styled` block six times per suite run, and it took
/// this call with it.
///
/// Process-scoping is the fix rather than a test seam because it is what
/// production wanted all along -- this app only ever means its own window --
/// so the production function is corrected rather than hidden from tests. In
/// a test process, which owns no windows, the lookup is `None` and this is a
/// no-op that touches nothing.
pub fn round_window_corners(window_title: &str) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };

    unsafe {
        let hwnd: HWND = match crate::foreground::own_window_titled(window_title) {
            Some(handle) => HWND(handle as *mut core::ffi::c_void),
            None => return,
        };
        let preference = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &preference as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&preference) as u32,
        );
    }
}

/// Thickness of the invisible band along each window *edge* that starts a
/// resize drag, in egui points. 6 is what Windows itself uses for a
/// non-themed frame's sizing border; wider starts eating clicks meant for the
/// sidebar's own left edge.
const RESIZE_BAND: f32 = 6.0;
/// Side of the square zone at each *corner*, which resizes in both axes at
/// once. Deliberately larger than [`RESIZE_BAND`]: a corner is the hardest
/// target to hit and the only one that cannot be reached any other way.
const RESIZE_CORNER: f32 = 14.0;

/// One resize hit-zone: where it is, which way it resizes, and the cursor it
/// shows while the pointer is over it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResizeZone {
    pub rect: egui::Rect,
    pub direction: egui::viewport::ResizeDirection,
    pub cursor: egui::CursorIcon,
    /// Stable, per-zone suffix for the interaction id. Not the direction's
    /// `Debug` string: ids are load-bearing across frames and must not move
    /// if that formatting ever changes.
    pub id: &'static str,
}

/// The eight zones -- four corners, four edges -- for a window occupying
/// `window`.
///
/// Pure and separate from any drawing so the geometry can be asserted
/// directly; the drawing half ([`draw_resize_handles`]) needs a real window
/// and cannot be.
///
/// The zones DO NOT OVERLAP, and that is the whole design. egui resolves a
/// hit to one widget, so overlapping corner and edge zones would mean the
/// corner works only where it happens to have been registered later -- a rule
/// that lives in call order and cannot be read off the geometry. Here the
/// corners take their `RESIZE_CORNER` squares first and each edge spans only
/// what is left between them, so which zone a point belongs to is a property
/// of the rects alone.
///
/// An edge whose span between the corners would be empty (a window narrower
/// or shorter than two corners) is omitted rather than emitted inverted: an
/// inverted `Rect` silently contains nothing, so it would be a zone that
/// exists in the list and can never be hit.
pub fn resize_zones(window: egui::Rect, band: f32, corner: f32) -> Vec<ResizeZone> {
    use egui::viewport::ResizeDirection as Dir;
    use egui::CursorIcon as Cursor;

    let (left, top, right, bottom) = (window.min.x, window.min.y, window.max.x, window.max.y);
    let mut zones = vec![
        ResizeZone {
            rect: egui::Rect::from_min_max(Pos2::new(left, top), Pos2::new(left + corner, top + corner)),
            direction: Dir::NorthWest,
            cursor: Cursor::ResizeNorthWest,
            id: "nw",
        },
        ResizeZone {
            rect: egui::Rect::from_min_max(Pos2::new(right - corner, top), Pos2::new(right, top + corner)),
            direction: Dir::NorthEast,
            cursor: Cursor::ResizeNorthEast,
            id: "ne",
        },
        ResizeZone {
            rect: egui::Rect::from_min_max(Pos2::new(left, bottom - corner), Pos2::new(left + corner, bottom)),
            direction: Dir::SouthWest,
            cursor: Cursor::ResizeSouthWest,
            id: "sw",
        },
        ResizeZone {
            rect: egui::Rect::from_min_max(Pos2::new(right - corner, bottom - corner), Pos2::new(right, bottom)),
            direction: Dir::SouthEast,
            cursor: Cursor::ResizeSouthEast,
            id: "se",
        },
    ];

    if right - corner > left + corner {
        zones.push(ResizeZone {
            rect: egui::Rect::from_min_max(Pos2::new(left + corner, top), Pos2::new(right - corner, top + band)),
            direction: Dir::North,
            cursor: Cursor::ResizeNorth,
            id: "n",
        });
        zones.push(ResizeZone {
            rect: egui::Rect::from_min_max(Pos2::new(left + corner, bottom - band), Pos2::new(right - corner, bottom)),
            direction: Dir::South,
            cursor: Cursor::ResizeSouth,
            id: "s",
        });
    }
    if bottom - corner > top + corner {
        zones.push(ResizeZone {
            rect: egui::Rect::from_min_max(Pos2::new(left, top + corner), Pos2::new(left + band, bottom - corner)),
            direction: Dir::West,
            cursor: Cursor::ResizeWest,
            id: "w",
        });
        zones.push(ResizeZone {
            rect: egui::Rect::from_min_max(Pos2::new(right - band, top + corner), Pos2::new(right, bottom - corner)),
            direction: Dir::East,
            cursor: Cursor::ResizeEast,
            id: "e",
        });
    }
    zones
}

/// Makes the window's edges and corners draggable to resize it.
///
/// **Only the vault window calls this.** The login and preferences windows
/// are fixed-size by design (their `ViewportBuilder`s say
/// `with_resizable(false)`, and their ▢ control is drawn permanently ghosted
/// with no click handler at all -- see `draw_window_chrome_with_extra`'s
/// `maximizable` parameter). That is why this is a separate function rather
/// than another branch inside the shared chrome: a window becomes resizable
/// by *calling* this, so a window that does not call it cannot become
/// resizable by accident.
///
/// Why it exists at all: this window runs `with_decorations(false)` so the
/// chrome can be drawn to match the design, and with no OS frame there are no
/// OS sizing borders -- `with_resizable(true)` alone is inert on Windows.
/// `ViewportCommand::BeginResize` hands the drag to the OS's own resize loop
/// (`winit::Window::drag_resize_window`), exactly as the titlebar's
/// `StartDrag` hands over a move, so the OS keeps enforcing
/// `with_min_inner_size`, snapping, and the live-resize feel.
///
/// The zones live in their own `Order::Foreground` [`egui::Area`] rather than
/// in the window's panel layer. Two reasons, and it is worth separating them
/// because only one is proven:
///
///  * **Layout.** These rects are absolute window-edge coordinates. Allocated
///    into the panel layer they would take part in its layout and push the
///    real content around; an `Area` has no place in any flow.
///  * **Hit-testing.** Belt and braces. MEASURED, not assumed: egui's hit
///    test already prefers the *smaller* candidate, so the thin zones win
///    against a `CentralPanel` that claims the whole window even when they
///    are registered in that same layer -- `draw_resize_handles_tests` was
///    run against a root-layer variant and stayed green, so those tests do
///    NOT distinguish the layer choice. The foreground layer is here so the
///    behaviour does not rest on that preference, which is an egui
///    implementation detail, not a documented guarantee.
///
/// Being a layer rather than a call-order trick also means this can be called
/// at the top of the frame, before the early returns for the loading and
/// unavailable states -- a window that is still loading is still resizable.
pub fn draw_resize_handles(ctx: &egui::Context) {
    // `viewport_rect`, not `content_rect`: the resize borders belong on the
    // window's real edges, not inside any platform safe-area inset.
    let window = ctx.input(|i| i.viewport_rect());
    egui::Area::new(egui::Id::new("window-resize-handles"))
        .order(egui::Order::Foreground)
        .fixed_pos(window.min)
        // `Area::new` defaults to `movable: true`, which registers a drag
        // sense over the area's WHOLE rect -- here, the entire window -- and
        // would both swallow every drag meant for the zones below and start
        // sliding this layer around. `constrain: false` for the same class of
        // reason: this area is deliberately flush with the window's edges,
        // and constraining it inside the screen nudges it off them.
        .movable(false)
        .constrain(false)
        .show(ctx, |ui| {
            for zone in resize_zones(window, RESIZE_BAND, RESIZE_CORNER) {
                let response = ui.interact(
                    zone.rect,
                    egui::Id::new("window-resize").with(zone.id),
                    Sense::drag(),
                );
                // `dragged()` as well as `hovered()`: once the OS resize loop
                // has the pointer it can leave the band, and the cursor
                // flicking back to an arrow mid-drag reads as the drag having
                // been dropped.
                if response.hovered() || response.dragged() {
                    ctx.set_cursor_icon(zone.cursor);
                }
                if response.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(zone.direction));
                }
            }
        });
}

/// Every monitor's work area (the monitor minus the taskbar and any other
/// appbars), primary first, in egui points -- the space
/// `settings::clamp_window_geometry` and `ViewportBuilder::with_position`
/// both work in.
///
/// Empty if the enumeration fails, which `clamp_window_geometry` treats as
/// "restore the size but let the OS place the window".
///
/// HONEST LIMIT, because this is the one approximation in the restore path:
/// Win32 reports monitor rects in *physical pixels*, and the conversion below
/// divides by a single scale factor read from the desktop DC, which under
/// winit's per-monitor-DPI-v2 awareness is the *primary* monitor's. On a
/// uniform-DPI desktop (including every 100% one) that is exact. On a mixed-
/// DPI desktop a secondary monitor's work area is off by the ratio between
/// the two scales, so a window restored onto it can be clamped a little too
/// eagerly or not quite enough. Doing better needs `GetDpiForMonitor`, which
/// lives behind a `Win32_UI_HiDpi` Cargo feature this crate does not enable.
/// The failure mode is a slightly wrong clamp, never an unreachable window on
/// the primary monitor.
pub fn monitor_work_areas() -> Vec<crate::settings::WorkArea> {
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetDeviceCaps, GetDC, GetMonitorInfoW, ReleaseDC, HDC, HMONITOR,
        LOGPIXELSX, MONITORINFO,
    };
    use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

    // Collected as (is_primary, work area in physical pixels) so the primary
    // can be sorted to the front -- `EnumDisplayMonitors` promises no order,
    // and "the primary is the fallback" is a rule `clamp_window_geometry`
    // spells as "the first element".
    let mut found: Vec<(bool, RECT)> = Vec::new();

    unsafe extern "system" fn callback(
        monitor: HMONITOR,
        _hdc: HDC,
        _clip: *mut RECT,
        lparam: LPARAM,
    ) -> windows::Win32::Foundation::BOOL {
        let found = unsafe { &mut *(lparam.0 as *mut Vec<(bool, RECT)>) };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            found.push((info.dwFlags & MONITORINFOF_PRIMARY != 0, info.rcWork));
        }
        true.into()
    }

    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(callback),
            LPARAM(&mut found as *mut Vec<(bool, RECT)> as isize),
        );
    }

    let scale = unsafe {
        let hdc = GetDC(None);
        if hdc.is_invalid() {
            1.0
        } else {
            let dpi = GetDeviceCaps(hdc, LOGPIXELSX);
            ReleaseDC(None, hdc);
            if dpi > 0 {
                dpi as f32 / 96.0
            } else {
                1.0
            }
        }
    };

    found.sort_by_key(|(is_primary, _)| !is_primary);
    found
        .into_iter()
        .map(|(_, work)| {
            let to_points = |v: i32| (v as f32 / scale).round() as i32;
            crate::settings::WorkArea {
                x: to_points(work.left),
                y: to_points(work.top),
                width: to_points(work.right - work.left),
                height: to_points(work.bottom - work.top),
            }
        })
        .collect()
}

/// The login window's form state, owned by the caller across frames.
///
/// It holds a live master password for as long as the window is open, so it
/// wipes it on drop -- see the [`Drop`] impl below.
#[derive(Default)]
pub struct LoginForm {
    pub server_choice: ServerChoice,
    pub server_url: String,
    pub email: String,
    pub password: String,
    pub reveal_password: bool,
    /// The opt-in for Windows Hello quick unlock (see `hello::enroll`).
    pub enable_hello: bool,
    pub error: Option<String>,
}

/// **The backstop: no `LoginForm` is released to the allocator holding a
/// plaintext master password.**
///
/// [`apply_auth_result`] wipes on every answer the CLI gives, which is the
/// path that matters -- it is the only one that ends the plaintext's life while
/// the window is still open, and on the `close_on_success: false` host the
/// window stays open for the whole vault session. This covers the exits that
/// never reach an answer at all: a window the user closed with the field
/// filled in, and an unwind out of the event loop. It also covers a success on
/// the host that does close, a second time, which costs nothing.
///
/// On drop rather than at some chosen point in `run_login_flow_for`, because
/// the form is move-captured by the frame closure and that function can no
/// longer reach it after the event loop returns.
///
/// **Every `String` the form owns, not only `password`.** `password` is the
/// only field that holds the master password today, and for a while this body
/// wiped only that one. What made that the wrong shape is not a leak that
/// exists -- it is that nothing anywhere would notice one: a form whose
/// `server_url` or `email` or `error` had picked up a copy of the plaintext
/// (an error message built from the wrong variable, a "remember this" that
/// assigned the wrong field) would be released to the allocator in the clear
/// with the whole suite green, because the backstop only looked at one field.
/// Wiping all four costs three `zeroize` calls on strings that are almost
/// always short or empty, and makes the guarantee this docstring states true of
/// the struct rather than of one of its fields.
///
/// Tested by `password_lifetime_tests`, which watches the global allocator for
/// the plaintext going past on the way out -- emptying this body makes those
/// tests fail.
impl Drop for LoginForm {
    fn drop(&mut self) {
        self.password.zeroize();
        self.server_url.zeroize();
        self.email.zeroize();
        if let Some(error) = self.error.as_mut() {
            error.zeroize();
        }
    }
}

/// What an in-flight sign-in's answer does to the form, and what the window
/// should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthOutcome {
    /// The CLI produced a session token. The window is finished.
    Succeeded(String),
    /// The CLI refused. The window stays open showing the message.
    Failed,
}

/// Applies the answer from an in-flight `bw login`/`bw unlock` to the form.
///
/// **The password is wiped on EVERY answer, and the two arms differ only in
/// what they say about it.** The user's instruction ("clear if unsuccessful
/// only") was about what the *field* shows: a failed attempt empties the box
/// the user must retype into, a successful one does not blank a box that is
/// about to be replaced by a vault. It was never about how long the plaintext
/// stays in memory, and reading it that way is what left the master password
/// live for a whole session.
///
/// What is NOT allowed is wiping at the moment of *submission*, which is where
/// this call used to sit -- one line after `spawn_auth`, so the field emptied
/// while the CLI was still running and the window spent the whole attempt
/// showing a blank master-password box that the user had just filled in. That
/// is what `theme::disabled_password_field`'s fixed mask covers on screen; this
/// is the other half, and the two have to agree, or the mask is painting over a
/// buffer that was cleared for no reason anybody can see. Wiping on the
/// *answer* keeps the mask honest for the length of the attempt and still ends
/// the plaintext's life within a frame of the attempt finishing.
///
/// **Why the success arm matters, given [`LoginForm`]'s [`Drop`].** Drop is a
/// backstop, and it only fires when the form dies. One of this window's two
/// hosts never lets it die on a success: `app_window` passes
/// `close_on_success: false` and move-captures the frame closure -- form and
/// all -- into a window that goes on to become the spinner and then the vault.
/// Without the wipe here, the plaintext master password would sit in that
/// closure for the entire vault session.
///
/// Extracted from the frame closure so it can be tested at all: that closure
/// runs only inside a live eframe event loop, and none of this is observable
/// from outside one -- the field is masked, and on the `close_on_success: true`
/// host the window is closing anyway.
pub fn apply_auth_result(result: Result<String, String>, form: &mut LoginForm) -> AuthOutcome {
    match result {
        Ok(session_token) => {
            form.error = None;
            // **After the token is out of `result` and before anything else.**
            // The session token is what this attempt was for; the master
            // password has no further reader on any path -- the worker got its
            // own copy at submit time (and wipes it), the Hello enrolment was
            // done inside that worker, and no later frame reads this field
            // except to paint a box that a successful sign-in replaces.
            form.password.zeroize();
            AuthOutcome::Succeeded(session_token)
        }
        Err(e) => {
            // Raw CLI output to the log (it's the only diagnostic channel
            // this console-less binary has), one actionable line to the
            // window.
            log::warn!("bw login/unlock failed: {e}");
            form.error = Some(friendly_auth_error(&e));
            // The retype the user has to do anyway, and the one moment where
            // emptying the buffer costs them nothing they still wanted.
            form.password.zeroize();
            AuthOutcome::Failed
        }
    }
}

/// What the user asked the login window to do this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginAction {
    /// Continue was clicked (or Enter pressed): log in / unlock.
    Submit,
    /// The Hello panel was clicked (or Ctrl+H): unlock via Windows Hello.
    HelloUnlock,
    /// The footer's "Log out" was clicked: drop the account.
    LogOut,
}

/// Draws the login/unlock window body (design 3h): brand lockup, title
/// block, credential card with the account email and in-field reveal toggle,
/// and the bottom account/server footer. Pure view -- the caller owns the
/// state and performs the `bw` side effects for whatever action comes back.
///
/// Public (rather than folded into `run_login_flow`'s closure) so the
/// `ui_preview` example renders the exact window the app ships, same as
/// `overlay_ui::draw_overlay_card`.
///
/// Deviations from the 3h mock, backend-gated: "Forgot it?" (the CLI has no
/// password-hint API) and "Switch account" ("Log out" is the same
/// primitive). The Hello panel appears once quick unlock is enrolled
/// (`hello.enrolled`); the mock's static "Bitwarden · EU" footer is a live
/// server dropdown while signing in.
/// What the window says the first time it asks for a master password against
/// a freshly minted account.
///
/// **Why it exists at all.** Deskwarden keeps each account in its own CLI
/// profile under `accounts/<id>/`, and it does not import whatever profile the
/// Bitwarden CLI already had. So the first launch under this layout opens a
/// login window on a machine the user was already signed in on. With nothing
/// on screen to explain it that reads as a bug -- or as a phishing prompt --
/// rather than as one-off setup.
///
/// **Both halves are load-bearing.** The second sentence is not a nicety: the
/// Windows Hello key is derived per account
/// ([`accounts::hello_kdf_suffix_for`](crate::accounts::hello_kdf_suffix_for)),
/// so an enrolment made by an earlier version cannot be opened here. A quick
/// unlock that silently stops existing is indistinguishable from Windows Hello
/// never having been set up.
///
/// Worded as setup rather than as an error, because nothing has gone wrong --
/// and nothing of the user's has been touched: the CLI's own profile
/// directory is left exactly where it is.
pub const FIRST_RUN_NOTICE: &str = "Deskwarden now keeps each account in its own profile -- sign \
     in once to set this one up. Windows Hello quick unlock has to be set up again too.";

/// Paints [`FIRST_RUN_NOTICE`]. One definition, two call sites (with the Hello
/// panel, and standing alone when there is no panel), so the two cannot drift
/// into saying different things.
fn draw_first_run_notice(ui: &mut egui::Ui) {
    ui.label(
        RichText::new(FIRST_RUN_NOTICE)
            .size(11.0)
            .color(theme::TEXT_MUTED),
    );
    ui.add_space(8.0);
}

pub fn draw_login_window(
    ui: &mut egui::Ui,
    status: BwStatus,
    account_email: Option<&str>,
    server_host: &str,
    hello: HelloState,
    form: &mut LoginForm,
    flow_bottom: &mut f32,
    // True from the moment credentials are handed to the `bw` CLI until it
    // answers. Disables every control that could start a second sign-in and
    // shows the spinner beside Continue.
    auth_in_progress: bool,
    // This account was minted by THIS launch and has never been signed in
    // to. Draws [`FIRST_RUN_NOTICE`]. Not something the window infers: the
    // fact travels from `accounts::resolve_startup`, and without it the user
    // meets a master-password prompt on a machine they were already signed
    // in on with nothing to say why.
    first_run: bool,
) -> Option<LoginAction> {
    let mut action = None;

    draw_brand_lockup(ui);

    ui.add_space(14.0);

    // 3b/3h language: matches are counted but never named until the vault
    // opens -- unlocking is what this window is for.
    let (title, subtitle) = if status == BwStatus::Unauthenticated {
        (
            "Sign in to your vault",
            "Works with bitwarden.com and self-hosted servers.",
        )
    } else {
        (
            "Unlock your vault",
            "Matches stay hidden until the vault opens.",
        )
    };
    ui.label(theme::bold(title, 19.0).color(theme::INK));
    ui.label(RichText::new(subtitle).size(13.0).color(theme::TEXT_FAINT));

    ui.add_space(14.0);

    egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0, theme::HAIRLINE))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // Spacing is explicit per gap, not one uniform value: a label
            // must sit closer to *its* field (LABEL_GAP) than one field
            // group does to the next (GROUP_GAP), or the rows read as an
            // undifferentiated stack.
            ui.spacing_mut().item_spacing.y = 0.0;

            // **While an attempt is in flight the whole credential zone is
            // greyed and inert**, at the user's direction: the credentials
            // are with the CLI, and a field that still takes typing during
            // that is a field whose contents cannot affect the answer coming
            // back. `theme::disabled_text_field` and
            // `theme::disabled_password_field` paint the box and a galley
            // with no `TextEdit` in it at all -- see their docs for why a
            // read-only `TextEdit` is not the same thing.
            //
            // Continue is disabled by `add_enabled_ui` further down and Enter
            // and the Hello panel are gated below that. This is the third
            // door, and the one the user actually named.
            let label = if auth_in_progress {
                theme::disabled_field_label
            } else {
                theme::field_label
            };
            if status == BwStatus::Unauthenticated {
                if form.server_choice == ServerChoice::SelfHosted {
                    label(ui, "Server URL");
                    ui.add_space(LABEL_GAP);
                    if auth_in_progress {
                        theme::disabled_text_field(ui, &form.server_url);
                    } else {
                        theme::text_field(ui, &mut form.server_url, false);
                    }
                    ui.add_space(GROUP_GAP);
                }
                label(ui, "Email");
                ui.add_space(LABEL_GAP);
                if auth_in_progress {
                    theme::disabled_text_field(ui, &form.email);
                } else {
                    theme::text_field(ui, &mut form.email, false);
                }
                ui.add_space(GROUP_GAP);
            }

            // 3h's label row: field name left, account email right, so the
            // user can see whose vault they're about to open.
            ui.horizontal(|ui| {
                label(ui, "Master password");
                if let Some(email) = account_email {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(email).size(11.0).color(theme::TEXT_GHOST));
                    });
                }
            });
            ui.add_space(LABEL_GAP);
            if auth_in_progress {
                // **Takes no value**, so the bullets cannot track the buffer
                // -- see `theme::MASKED_BULLETS`. The user's words were "keep
                // showing masked password (even if there is nothing
                // already)": a mask that shortened as the field emptied would
                // announce the emptying, which is the one thing it is here to
                // avoid.
                theme::disabled_password_field(ui);
            } else {
                theme::password_field(ui, &mut form.password, &mut form.reveal_password);
            }
            ui.add_space(GROUP_GAP);

            // While the credentials are with the server, Continue is
            // disabled and a spinner sits beside it, sized to the button so
            // the two read as one control. Disabling matters beyond looks:
            // a second submit would spawn another `bw login` against the
            // same account while the first is still running.
            ui.horizontal(|ui| {
                let continue_button = ui
                    .add_enabled_ui(!auth_in_progress, |ui| {
                        theme::primary_button(ui, "Continue", Some("↵"))
                    })
                    .inner;
                if continue_button.clicked() {
                    action = Some(LoginAction::Submit);
                }
                if auth_in_progress {
                    ui.add_space(10.0);
                    theme::progress_bar(ui, AUTH_BAR_WIDTH);
                }
            });
        });

    if let Some(err) = &form.error {
        ui.add_space(6.0);
        ui.label(RichText::new(err).size(12.0).color(theme::ERROR));
    }

    // 3h's alternative path: the "or" divider and the Windows Hello panel.
    // Enrolled: click (or Ctrl+H) unlocks via Hello. Not yet enrolled: the
    // same panel is the opt-in — its toggle arms enrollment, which then
    // happens on the next successful password unlock, so the password
    // being sealed is one that provably works.
    // 3h's alternative path: the Hello panel is a *button* — click it (or
    // Ctrl+H) to unlock with Windows Hello. Before enrollment it is the
    // same button with a first-use twist: with the master password typed,
    // clicking unlocks with it AND seals it for Hello, so every later visit
    // is one click.
    let needs_setup = !hello.enrolled;
    let show_panel = hello.available && (needs_setup || status != BwStatus::Unauthenticated);
    // Deliberately NOT nested inside `show_panel`. If Hello has since been
    // turned off on this machine there is no panel to hang the line from,
    // and the line is then the only thing that says why the app is asking
    // for a master password at all. `needs_setup` because an account that
    // has already enrolled on this layout has nothing left to be told --
    // which is also what stops the notice reappearing after the first
    // sign-in within one launch.
    let show_first_run_notice = first_run && needs_setup;
    if show_panel {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let half = (ui.available_width() - 30.0) / 2.0;
            hairline_segment(ui, half);
            ui.label(RichText::new("or").size(11.0).color(theme::TEXT_GHOST));
            hairline_segment(ui, half);
        });
        ui.add_space(10.0);

        if show_first_run_notice {
            draw_first_run_notice(ui);
        }

        let subtitle = if needs_setup {
            "First use: enter your master password, then click here"
        } else {
            "Face, fingerprint, or PIN"
        };
        // Gated like Continue and Enter: this is the third way to start a
        // sign-in, and it must not fire a second one over an in-flight
        // first.
        let clicked = !auth_in_progress
            && (hello_panel(ui, subtitle)
                || ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::H)));
        if clicked {
            if !needs_setup {
                action = Some(LoginAction::HelloUnlock);
            } else if form.password.is_empty() {
                form.error = Some(
                    "Type your master password first — Windows Hello is set up right \
                     after it unlocks the vault."
                        .to_string(),
                );
            } else {
                form.enable_hello = true;
                action = Some(LoginAction::Submit);
            }
        }
    } else if show_first_run_notice {
        ui.add_space(10.0);
        draw_first_run_notice(ui);
    }

    // Enter submits from anywhere in the form, same as clicking Continue --
    // 3h's Continue carries the ↵ affordance. Gated on `auth_in_progress`
    // for the same reason the button itself is: Enter is the *easier* way
    // to fire a second login while the first is still in flight.
    if !auth_in_progress && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        action = Some(LoginAction::Submit);
    }

    // Where the flowing content ends, in window coordinates. The caller
    // resizes the window from this: the content stack varies per state
    // (self-hosted URL field, Hello panel, wrapped error text), and a fixed
    // height either wastes space or overflows.
    *flow_bottom = ui.next_widget_position().y;

    // Footer pinned to the window bottom (3h): account action left; on the
    // right, the server — a live dropdown while signing in (the native
    // client's "Logging in on"), static text once an account is attached.
    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        ui.horizontal(|ui| {
            if status != BwStatus::Unauthenticated {
                let log_out = ui.add(
                    egui::Button::new(RichText::new("Log out").size(12.0).color(theme::TEXT_FAINT))
                        .fill(egui::Color32::TRANSPARENT)
                        .stroke(Stroke::NONE),
                );
                if log_out.clicked() {
                    action = Some(LoginAction::LogOut);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if status == BwStatus::Unauthenticated {
                    egui::ComboBox::from_id_salt("server-choice")
                        .selected_text(
                            RichText::new(form.server_choice.label())
                                .size(12.0)
                                .color(theme::TEXT_MUTED),
                        )
                        .show_ui(ui, |ui| {
                            for choice in [
                                ServerChoice::UsCloud,
                                ServerChoice::EuCloud,
                                ServerChoice::SelfHosted,
                            ] {
                                ui.selectable_value(
                                    &mut form.server_choice,
                                    choice,
                                    choice.label(),
                                );
                            }
                        });
                    ui.label(
                        RichText::new("Logging in on:")
                            .size(12.0)
                            .color(theme::TEXT_GHOST),
                    );
                } else {
                    ui.label(
                        RichText::new(server_host)
                            .size(12.0)
                            .color(theme::TEXT_GHOST),
                    );
                }
            });
        });
    });

    action
}

// --- Brand lockup (design 3h) -----------------------------------------------
//
// `display: flex; align-items: center; gap: 13px` of a 38×44 mark box and a
// two-line text column (`gap: 2px`): the 25px/800 wordmark over the 10px/700
// tracked tag.

/// The mark's box in the lockup. The design sizes it 38×44, *not* square —
/// which is what the mark's own 24:28 artboard scales to at 44 tall. Passing
/// `theme::mark` a single 44 (as this used to) allocates 44×44 and leaves
/// ~6px of dead space around a mark drawn at the same visual size, pushing
/// the wordmark right and widening the whole lockup.
const LOCKUP_MARK_SIZE: Vec2 = Vec2::new(38.0, 44.0);
/// Design: `gap: 13px` between the mark box and the text column.
const LOCKUP_GAP_X: f32 = 13.0;
/// Design: `gap: 2px` between the wordmark and the tag under it.
const LOCKUP_GAP_Y: f32 = 2.0;
/// The wordmark's line box. The design sets `font-size: 25px` with
/// `line-height: 1`, so its box is exactly the font size — whereas the
/// galley egui lays out is the font's natural ascent + descent, which
/// measures 27px for Archivo ExtraBold at this size. Using the galley's own
/// height dropped the tag 2px below where the design puts it and made the
/// whole lockup read taller.
const LOCKUP_WORDMARK_LINE_BOX: f32 = 25.0;

/// Draws the brand lockup by painting into one explicitly-allocated band
/// rather than nesting a `horizontal` around a `vertical` around two
/// labels.
///
/// Layout containers were giving away control of exactly the things the
/// design pins down here: an `egui` label's box is the font's full ascent +
/// descent, whereas the design sets `line-height: 1` on the wordmark, so the
/// text column came out taller than 25 + 2 + tag and centred differently
/// against the mark. Painting positions both runs directly, so the gaps and
/// the vertical centring are the design's numbers rather than a byproduct of
/// font metrics.
fn draw_brand_lockup(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), LOCKUP_MARK_SIZE.y),
        Sense::hover(),
    );

    // Shift the whole lockup left by the shield's own artboard padding, so
    // what lines up with the window's content edge is the shield's *visible*
    // edge rather than its invisible 24×28 artboard. Every heading and card
    // below starts at that edge, and against them a mathematically-flush
    // artboard reads as an indented logo (the padding measures 3.3px here).
    // Shifting the whole lockup keeps the design's mark-to-text gap intact.
    let nominal = egui::Rect::from_min_size(rect.min, LOCKUP_MARK_SIZE);
    let artboard_inset = theme::mark_ink_rect(nominal).left() - nominal.left();
    let mark_rect = nominal.translate(Vec2::new(-artboard_inset, 0.0));
    theme::paint_mark(ui.painter(), mark_rect);

    // The design tracks the wordmark tight (-0.03em at 25px = -0.75pt) and
    // sets it in weight 800 (`theme::EXTRABOLD`); the tag is tracked wide
    // (0.15em at 10px = 1.5pt) in weight 700 — Bold, not SemiBold, which is
    // what this drew before.
    let wordmark = ui.painter().layout_job(theme::letterspaced(
        "Deskwarden",
        25.0,
        theme::EXTRABOLD,
        -0.75,
        theme::INK,
    ));
    let tag = ui.painter().layout_job(theme::letterspaced(
        "FILLS NATIVE WINDOWS",
        10.0,
        theme::BOLD,
        1.5,
        theme::TEXT_FAINT,
    ));

    let column_x = mark_rect.right() + LOCKUP_GAP_X;
    let column_height = LOCKUP_WORDMARK_LINE_BOX + LOCKUP_GAP_Y + tag.size().y;
    let column_top = rect.center().y - column_height / 2.0;

    // Positioned by ink, not by galley origin: the two runs are different
    // sizes, so their glyphs' left side bearings differ (1.0px vs 0.0px
    // here) and painting both at `column_x` leaves the tag visibly hanging
    // a pixel further left than the wordmark above it.
    let wordmark_height = wordmark.size().y;
    let wordmark_x = column_x - theme::ink_offset_x(&wordmark);
    let tag_x = column_x - theme::ink_offset_x(&tag);

    ui.painter().galley(
        Pos2::new(
            wordmark_x,
            // Centre the glyphs in the design's 25px line box; since the
            // galley is taller than that box, this lifts it slightly, which
            // is exactly what `line-height: 1` does in the design.
            column_top + (LOCKUP_WORDMARK_LINE_BOX - wordmark_height) / 2.0,
        ),
        wordmark,
        theme::INK,
    );
    ui.painter().galley(
        Pos2::new(tag_x, column_top + LOCKUP_WORDMARK_LINE_BOX + LOCKUP_GAP_Y),
        tag,
        theme::TEXT_FAINT,
    );
}

/// A fixed-width horizontal hairline, for the "or" divider.
fn hairline_segment(ui: &mut egui::Ui, width: f32) {
    let (rect, _) =
        ui.allocate_exact_size(egui::Vec2::new(width.max(0.0), 1.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, theme::HAIRLINE);
}

/// The 3h Windows Hello panel: a blue-washed button row with a padlock
/// tile, "Use Windows Hello", the given subtitle, and the CTRL+H chip.
/// Returns true when clicked.
fn hello_panel(ui: &mut egui::Ui, subtitle: &str) -> bool {
    let panel = egui::Frame::new()
        .fill(theme::BLUE_WASH)
        .stroke(Stroke::new(1.0, theme::FOCUS_RING))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(14, 13))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                // The white padlock tile.
                let (tile, _) =
                    ui.allocate_exact_size(egui::Vec2::splat(30.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(tile, CornerRadius::same(8), theme::CARD);
                ui.painter().rect_stroke(
                    tile,
                    CornerRadius::same(8),
                    Stroke::new(1.0, theme::BLUE_EDGE),
                    egui::StrokeKind::Middle,
                );
                paint_padlock(ui.painter(), tile.shrink(7.5), theme::BLUE);

                ui.add_space(3.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(theme::semibold("Use Windows Hello", 13.0).color(theme::BLUE_DEEP));
                    ui.label(RichText::new(subtitle).size(11.0).color(theme::TEXT_MUTED));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme::kbd_chip_on_card(ui, "CTRL+H");
                });
            });
        });
    panel
        .response
        .interact(egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

/// A minimal padlock glyph: shackle (upper circle stroke, lower half masked)
/// over a filled, rounded body. Drawn rather than typed because the bundled
/// fonts have no padlock glyph at this size.
fn paint_padlock(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let stroke_width = 1.6;
    let shackle_center = egui::Pos2::new(rect.center().x, rect.top() + rect.height() * 0.32);
    let shackle_radius = rect.width() * 0.30;
    painter.circle_stroke(
        shackle_center,
        shackle_radius,
        Stroke::new(stroke_width, color),
    );

    let body = egui::Rect::from_min_max(
        egui::Pos2::new(rect.left(), rect.top() + rect.height() * 0.42),
        rect.max,
    );
    painter.rect_filled(body, CornerRadius::same(2), color);
}

/// Runs a `bw login`/`bw unlock` on a one-shot background thread and reports
/// the result over `tx`.
///
/// This used to run inline in the update closure, which froze the login
/// window for the whole of a CLI spawn plus a network round trip — several
/// seconds with no repaint, so not even a spinner could animate. `password`
/// is moved in and zeroized here once the CLI and any Hello enrollment are
/// done with it, so the worker does not leave a live copy behind either.
///
/// Enrollment runs here, on success only, *before* that wipe: the password
/// sealed for Windows Hello has to be one that provably opened the vault. A
/// failed enrollment is logged rather than surfaced — the unlock itself
/// succeeded and the window is about to close.
/// The Hello state for the account this window belongs to, or "no quick
/// unlock at all" when there is no account -- there is no account-less blob
/// to fall back to. Named rather than inlined so the two re-probes the window
/// does after a Hello failure or a log-out cannot disagree with the first.
/// Which profile directory this window's `bw` calls act on.
///
/// **Derived from the account the window belongs to, never from
/// `bw_path::active_data_dir()`.** Those two agree for every login window
/// except the one that matters: `main`'s `add_account` opens this window for
/// an account that has just been minted and is not the active one, and its
/// `bw login` *replaces* whatever profile it lands in -- which would otherwise
/// be the account the user is already signed into.
///
/// The alternative is what `add_account` used to do: set the process-global
/// to the new directory, run the window, set it back. `bw_serve.rs` reads that
/// global from background threads, so the window in which it names another
/// account's profile is a window in which a sync can land in the wrong vault.
/// `main`'s `remove_account` states that rule in its own doc; taking the
/// directory here is what lets the function beside it obey the same one.
///
/// `None` is the one startup condition with no account at all
/// (`accounts::StartupAccounts::NoAccountList`), and it keeps `bw_command_in`'s
/// meaning: no override, so the child resolves the profile the CLI would by
/// itself. That is also what the global held in that state, so nothing about
/// it changed.
///
/// **Public because the daemon's unlock prompt needs exactly this answer.**
/// `crate::unlock_prompt::ask` runs `bw unlock` against a profile directory,
/// and its own doc names this function's rule as the one it must obey -- "the
/// account's, never `bw_path::active_data_dir()` read in here". A second
/// derivation over there would be a second chance to make precisely the
/// mistake the paragraphs above describe.
pub fn profile_dir_for(account: Option<(&Path, &Account)>) -> Option<PathBuf> {
    account.map(|(config_dir, account)| crate::accounts::data_dir_for(config_dir, &account.id))
}

fn probe_hello(scope: &Option<(PathBuf, AccountId)>) -> HelloState {
    match scope {
        Some((config_dir, id)) => hello::state_for(config_dir, id),
        None => HelloState::unavailable(),
    }
}

fn spawn_auth(
    tx: std::sync::mpsc::Sender<Result<String, String>>,
    args: Vec<String>,
    mut password: String,
    // `Some` when the user asked for quick unlock this submit: the account
    // whose blob the password gets sealed into, owned because the worker
    // outlives this frame. Enrollment is per-account (`hello::enroll_for`) --
    // there is no such thing as enrolling "the app".
    enroll_hello: Option<(PathBuf, AccountId)>,
    // The profile directory the `bw login`/`bw unlock` acts on, owned because
    // the worker outlives this frame. `None` only when this window belongs to
    // no account at all (`StartupAccounts::NoAccountList`), where there is no
    // override to set and the CLI resolves its own.
    data_dir: Option<PathBuf>,
) {
    std::thread::spawn(move || {
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let result = run_bw_with_password(&arg_refs, &password, data_dir.as_deref());

        if let (true, Some((config_dir, id))) = (result.is_ok(), &enroll_hello) {
            match hello::enroll_for(config_dir, id, &password) {
                Ok(()) => log::info!("Windows Hello quick unlock enrolled"),
                Err(e) => log::warn!("could not enroll Windows Hello quick unlock: {e}"),
            }
        }
        password.zeroize();

        let _ = tx.send(result);
    });
}

/// Opens a blocking egui window that shows a server-choice + email field
/// when the CLI reports `Unauthenticated`, or just a password field when
/// `Locked`/`Unlocked`; runs `bw login`/`bw unlock` accordingly and returns
/// the resulting session token -- or `None` if the user closed the window
/// without producing one.
///
/// **Returning `None` is the entire point of this function existing
/// separately from [`run_login_flow`].** Closing this window is fatal only
/// for the callers that have nothing to go back to (startup, and the
/// lock/re-auth recovery). An account switch does have somewhere to go back
/// to -- the account the user is already signed into, with its vault intact
/// -- and declining the prompt there is an ordinary gesture that must leave
/// the running app exactly as it was. Anything that is not startup or
/// re-auth calls this and handles `None`.
///
/// `account` is the account this window's Windows Hello enrollment belongs
/// to, with the config directory it lives under. `None` is the one startup
/// condition in which this app has no account at all --
/// `accounts::StartupAccounts::NoAccountList`, a machine whose `settings.json`
/// could not be read -- and the window then offers no quick unlock, because there
/// is no account whose blob it could read or write. It never falls back to a
/// shared, account-less enrollment: the only derivation available without an
/// account is the empty KDF suffix, which would make one account's sealed
/// master password openable by every other one.
///
/// `first_run` draws [`FIRST_RUN_NOTICE`]; see [`draw_login_window`].
///
/// This BUILDS the window's per-frame closure; it does not open a window. Two
/// hosts run it: [`run_login_flow_for`], which gives it an event loop of its
/// own, and `app_window`, which draws it as the first state of the single
/// window that then becomes the spinner and the vault. eframe cannot nest
/// event loops, so the second host cannot call a function that owns one --
/// which is the whole reason this split exists. See `pre_styled` and
/// `close_on_success` at their use sites for what each host asks for.
pub fn build_login_frame(
    account: Option<(&Path, &Account)>,
    first_run: bool,
    pre_styled: bool,
    close_on_success: bool,
) -> (eframe::NativeOptions, LoginFrameFn, LoginFrameHandles) {
    // **Every `bw` this window runs is aimed at THIS account's directory**,
    // derived from the account it was already given rather than read off the
    // process-global. See `profile_dir_for`, which says what that buys and
    // what the alternative cost.
    let profile_dir = profile_dir_for(account);
    let details = check_bw_status_details_in(profile_dir.as_deref());

    // Owned, because the update closure is `'static` and both the Hello
    // re-probes and the enrollment handed to `spawn_auth` need it after this
    // function's borrows are gone.
    let hello_scope: Option<(PathBuf, AccountId)> = account
        .map(|(config_dir, account)| (config_dir.to_path_buf(), account.id.clone()));

    // The update closure is FnMut + 'static and must move-capture its
    // state, so a plain local `Option<String>` can't be read back by this
    // function after `run_simple_native` returns. Instead, the result
    // lives in an `Rc<RefCell<_>>`: a clone is moved into the closure, and
    // the original is read here once the (blocking) call returns. This is
    // safe because eframe runs the closure on the same thread that's
    // blocked inside `run_simple_native` -- there's no cross-thread
    // sharing happening. (Same pattern as picker_ui::run_picker /
    // overlay_ui::show_prompt_overlay.)
    let token: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let token_for_closure = token.clone();

    // Mutable because 3h's "Log out" flips the window into the sign-in state
    // without closing it.
    let mut status = details.status;
    let mut account_email = details.user_email;
    let host = server_host(details.server_url.as_deref());
    let mut form = LoginForm::default();
    // The dropdown starts on whatever server the CLI is already pointed at,
    // so re-login on a self-hosted setup doesn't silently flip to the cloud.
    let (choice, url) = ServerChoice::from_configured(details.server_url.as_deref());
    form.server_choice = choice;
    form.server_url = url;
    // Probed once: Hello support doesn't change mid-dialog, and enrollment
    // changes only through the actions handled below.
    let mut hello_state = probe_hello(&hello_scope);

    // Remeasured every frame and applied when it changes: the content
    // stack differs per state (self-hosted URL field, Hello panel, wrapped
    // error text), so any fixed height either clips or leaves a gap. It sizes
    // the CARD now, not the window -- the window is the vault window's size
    // and does not move.
    let mut card_height = LOGIN_CARD_INITIAL_HEIGHT;

    // Outcome of an in-flight `bw login`/`bw unlock` (see `spawn_auth`),
    // polled non-blockingly each frame. `auth_in_progress` gates every
    // control that could start a second one and drives the spinner beside
    // Continue.
    let (auth_tx, auth_rx) = std::sync::mpsc::channel::<Result<String, String>>();
    let mut auth_in_progress = false;

    // **The vault window's placement, read the way the vault window reads
    // it.** Same `Settings::vault_window`, same `monitor_work_areas`, same
    // `clamp_window_geometry` -- by calling `vault_window::initial_placement`
    // rather than by reimplementing it, so the two cannot answer differently
    // and leave the window jumping at the exact moment this change exists to
    // stop it jumping. `None` on disk falls back to that window's own
    // `WINDOW_SIZE`, which is the same fallback it takes.
    //
    // Read here, on the main thread, before the window exists; every failure
    // inside `Settings::load` already falls back to defaults, so this cannot
    // be a reason the login window does not open.
    let placement = crate::vault_window::initial_placement(
        crate::settings::default_path()
            .as_deref()
            .and_then(|path| crate::settings::Settings::load(path).vault_window),
        &monitor_work_areas(),
    );
    let mut viewport = egui::ViewportBuilder::default()
            .with_inner_size([placement.width as f32, placement.height as f32])
            // **Still fixed-size, deliberately, even though the window is now
            // the vault window's shape.**
            //
            // Three reasons, and they compound. `with_decorations(false)`
            // makes this flag inert on its own (see `draw_resize_handles`):
            // the vault window is resizable because it PAINTS eight grab
            // zones, and this window paints none. Painting them would then
            // need somewhere to put the result -- and the only geometry on
            // disk is the vault window's, which this window is borrowing and
            // must not write back, or an aborted sign-in would silently
            // re-home the vault. And nothing in the card grows: it is a fixed
            // composition floating on a placeholder, so a resize would stretch
            // the empty panes behind it and nothing else.
            //
            // A window the user can drag the edges of and whose size is then
            // thrown away is worse than one that plainly cannot be resized,
            // so ▢ stays the ghosted affordance it has always been here.
            .with_resizable(false)
            .with_maximize_button(false)
            // The titlebar is the design's own (white, mark, ghost
            // controls), drawn by draw_window_chrome; the native frame
            // can't be themed into it.
            .with_decorations(false)
            // The taskbar icon (there is no native titlebar to show one);
            // eframe windows don't inherit the exe's icon resource.
            .with_icon(theme::window_icon());
    if let Some((x, y)) = placement.position {
        viewport = viewport.with_position([x as f32, y as f32]);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };

    // `pre_styled` when someone else already owns this window's first frame
    // -- see this function's doc. `false` is `run_login_flow_for`'s own value
    // and the behaviour this window has always had.
    let mut styled = pre_styled;

    let login_frame_fn = move |ui: &mut egui::Ui, _frame: &mut eframe::Frame| {
        if !styled {
            // egui applies a new font set at the *start* of the next frame,
            // not the one that calls set_fonts -- drawing Archivo-styled
            // text in this same frame would look up a family that doesn't
            // exist yet and panic. Skip drawing this frame; the real UI
            // starts on the next one, once the fonts are actually live.
            theme::paint_window_background(ui);
            theme::apply(ui.ctx());
            round_window_corners(WINDOW_TITLE);
            // Same hook, same reason as every other window this app opens:
            // this is the first frame on which the OS window exists, so it
            // is the first moment it can be asked to come forward. Without
            // it the login window can land behind whatever the user was
            // doing while `bw serve` started -- and the Hello prompt this
            // window raises is itself parented to nothing, so a login
            // window that is already behind takes the prompt down with it.
            let _ = crate::foreground::raise_window(WINDOW_TITLE);
            styled = true;
            ui.ctx().request_repaint();
            return;
        }

        // **The vault window's own titlebar, not this window's.** Same
        // metrics (`ChromeMetrics::VAULT`) and the same wordmark, because the
        // point is that the bar the user is looking at now is the bar that
        // will still be there afterwards. `maximizable: false` for the reason
        // the viewport is not resizable, above.
        let full = ui.max_rect();
        match draw_window_chrome_with_extra(
            ui,
            VAULT_WINDOW_TITLE,
            ChromeMetrics::VAULT,
            false,
            CloseControl::Active,
            |_ui| {},
        ) {
            ChromeAction::Close => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
            ChromeAction::Minimize => ui
                .ctx()
                .send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            ChromeAction::None => {}
        }

        // The empty app behind the card, at the real pane widths, dimmed.
        let skeleton = vault_skeleton(full);
        paint_vault_skeleton(ui.painter(), &skeleton);

        // The card: the unchanged 3h composition, floated in the middle of
        // the body on its own surface so it reads as sitting ON the blocked
        // app rather than as part of it.
        let card = login_card_rect(&skeleton, card_height);
        ui.painter().rect(
            card,
            CornerRadius::same(12),
            theme::WINDOW_BG,
            Stroke::new(1.0, theme::BORDER),
            egui::StrokeKind::Inside,
        );
        let content = login_card_content_rect(card);

        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(content)
                .layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| {
                ui.set_min_width(ui.available_width());

                // Non-blocking: the worker reports here and the window keeps
                // painting (and animating its spinner) until it does.
                if let Ok(result) = auth_rx.try_recv() {
                    auth_in_progress = false;
                    // Every decision this answer implies -- the message, and
                    // the wipe of the master-password buffer -- lives in
                    // `apply_auth_result`, where it can be tested. All that
                    // is left here is the two things that need the window.
                    //
                    // **By the time this `if let` binds, `form.password` is
                    // already empty**, on the success arm as well as the
                    // failure one. That is load-bearing rather than tidy: this
                    // window's other host passes `close_on_success: false` and
                    // keeps this whole closure -- `form` included -- alive
                    // through the spinner and the vault, so a wipe deferred to
                    // `LoginForm`'s `Drop` would be a wipe deferred to the end
                    // of the session.
                    if let AuthOutcome::Succeeded(session_token) =
                        apply_auth_result(result, &mut form)
                    {
                        *token_for_closure.borrow_mut() = Some(session_token);
                        // The token is recorded either way; what differs is
                        // whether producing one ENDS THE WINDOW.
                        //
                        // `run_login_flow_for` owns its window and has
                        // nothing else to show in it, so it closes -- which
                        // is how it returns at all. `app_window` owns a
                        // window that is about to become the spinner and then
                        // the vault, in that same frame's own event loop; it
                        // passes `false`, reads the token out of the same
                        // cell, and moves the window to its next state. A
                        // `Close` here would be the exact three-window
                        // flicker this whole change exists to remove.
                        if close_on_success {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                }

                let mut flow_bottom = 0.0f32;
                let action = draw_login_window(
                    ui,
                    status,
                    account_email.as_deref(),
                    &host,
                    hello_state,
                    &mut form,
                    &mut flow_bottom,
                    auth_in_progress,
                    first_run,
                );

                // Content height + the footer row and the bottom margin the
                // footer is pinned above. This used to resize the WINDOW from
                // exactly this measurement; it now sizes the card and leaves
                // the window at the vault window's geometry. Rounded up to
                // whole pixels (inside `login_card_height`) so a sub-pixel
                // wobble can't ping-pong the card's size frame after frame.
                let wanted = login_card_height(flow_bottom, content.min.y);
                if (wanted - card_height).abs() > 0.5 {
                    card_height = wanted;
                    // The card is painted before its content is measured, so
                    // the new height first appears on the next frame -- which
                    // will not happen on its own in a window nobody is
                    // touching.
                    ui.ctx().request_repaint();
                }

                match action {
                    Some(LoginAction::Submit) => {
                        // Blank fields are caught here, before anything is
                        // spawned -- see `missing_credential_message`. This
                        // is deliberately ahead of the server config below,
                        // which also shells out to `bw`: there is no point
                        // configuring a server for a submit that cannot go
                        // anywhere.
                        let missing = missing_credential_message(
                            status,
                            &form.email,
                            &form.password,
                        );

                        // The server must be configured before `bw login` --
                        // it's global CLI config. A bad or missing
                        // self-hosted URL is inline UI error, not a panic:
                        // bail out of this submit and let the user correct
                        // it.
                        let server_configured = if missing.is_some() {
                            false
                        } else if status == BwStatus::Unauthenticated {
                            let target = match form.server_choice {
                                ServerChoice::SelfHosted => {
                                    let url = form.server_url.trim().to_string();
                                    if url.is_empty() {
                                        form.error =
                                            Some("Enter your server's URL first.".to_string());
                                        None
                                    } else {
                                        Some(url)
                                    }
                                }
                                choice => choice.config_url().map(str::to_string),
                            };
                            match target {
                                // Into THIS account's `data.json`, not
                                // whatever profile the process is pointed at:
                                // `bw config server` is a write, and aimed at
                                // the wrong directory it re-points the account
                                // the user is already signed into.
                                Some(url) => match configure_server_in(&url, profile_dir.as_deref())
                                {
                                    Ok(()) => true,
                                    Err(e) => {
                                        log::warn!("bw config server failed: {e}");
                                        form.error = Some(e);
                                        false
                                    }
                                },
                                None => false,
                            }
                        } else {
                            true
                        };

                        if let Some(message) = missing {
                            form.error = Some(message.to_string());
                        } else if server_configured {
                            let args = match status {
                                BwStatus::Unauthenticated => {
                                    vec!["login".to_string(), form.email.clone(), "--raw".to_string()]
                                }
                                BwStatus::Locked | BwStatus::Unlocked => {
                                    vec!["unlock".to_string(), "--raw".to_string()]
                                }
                            };
                            // `None` unless the user asked for quick unlock
                            // AND there is an account to enrol it for: the
                            // sealed password belongs to one account's blob
                            // or to nothing.
                            let enroll_hello = (form.enable_hello
                                && hello_state.available
                                && !hello_state.enrolled)
                                .then(|| hello_scope.clone())
                                .flatten();

                            // Handed to the worker, which wipes its own copy
                            // when it is done with it (see `spawn_auth`).
                            //
                            // **The form's copy is NOT wiped here**, and it
                            // used to be. Emptying it at the moment of
                            // submission is what put a blank master-password
                            // box on screen for the length of the attempt --
                            // the state the user described as needing to keep
                            // showing a mask. The buffer is wiped instead when
                            // the ANSWER arrives, by `apply_auth_result`, on
                            // both arms; the mask therefore covers a real
                            // buffer for exactly as long as the attempt runs,
                            // and not one frame longer.
                            spawn_auth(
                                auth_tx.clone(),
                                args,
                                form.password.clone(),
                                enroll_hello,
                                profile_dir.clone(),
                            );
                            form.error = None;
                            auth_in_progress = true;
                        }
                    }
                    Some(LoginAction::HelloUnlock) => {
                        // The biometric prompt itself stays on this thread:
                        // it puts system UI on screen and returns as soon as
                        // the user responds. Only the `bw unlock` that
                        // follows -- a CLI spawn plus a network round trip --
                        // moves to the worker, which is the part that was
                        // freezing the window.
                        let opened = match &hello_scope {
                            Some((config_dir, id)) => hello::unlock_password_for(config_dir, id),
                            // Unreachable through the UI -- with no account
                            // the panel and its Ctrl+H shortcut are never
                            // drawn -- but stated rather than unwrapped: the
                            // alternative would be an account-less blob.
                            None => Err("There is no account to unlock with Windows Hello."
                                .to_string()),
                        };
                        match opened {
                            Ok(password) => {
                                spawn_auth(
                                    auth_tx.clone(),
                                    vec!["unlock".to_string(), "--raw".to_string()],
                                    // Clone the inner `String` out of the
                                    // `Zeroizing` wrapper: that wrapper still
                                    // wipes its own copy when it drops at the
                                    // end of this arm, and `spawn_auth` wipes
                                    // the worker's.
                                    (*password).clone(),
                                    // Already enrolled -- that is where this
                                    // password came from.
                                    None,
                                    profile_dir.clone(),
                                );
                                form.error = None;
                                auth_in_progress = true;
                            }
                            Err(e) => {
                                log::warn!("Windows Hello quick unlock failed: {e}");
                                form.error = Some(e);
                                // A failed open deletes the blob (see
                                // `hello::unlock_password_for`); re-probe so
                                // the panel disappears rather than erroring
                                // forever.
                                hello_state = probe_hello(&hello_scope);
                            }
                        }
                    }
                    // `bw_logout_in(this window's directory)`, never the
                    // active-profile `bw_logout()`. The two lines below
                    // unenroll Hello for THIS window's account, so the global
                    // form logged one account out and dropped another
                    // account's sealed master password. Unreachable today only
                    // by accident -- three conditions have to line up -- and
                    // it is the same defect `profile_dir_for` exists to close.
                    Some(LoginAction::LogOut) => match bw_logout_in(profile_dir.as_deref()) {
                        Ok(()) => {
                            log::info!("logged out at the user's request; showing sign-in");
                            // A sealed master password for an account the
                            // CLI no longer knows is a liability: drop the
                            // enrollment with the account. THIS account's
                            // only -- logging out here says nothing about
                            // any other account's enrollment.
                            // And the encrypted copy of that account's vault,
                            // in the same breath and for the stronger version
                            // of the same reason. It survives a lock -- that
                            // is the whole point of it -- but a log out is not
                            // a lock: it means the account is gone from this
                            // machine, and a decrypted dump of its vault is a
                            // larger liability than a sealed password.
                            if let Some((config_dir, id)) = &hello_scope {
                                hello::unenroll_for(config_dir, id);
                                crate::vault_disk_cache::forget_for(
                                    &crate::accounts::data_dir_for(config_dir, id),
                                );
                            }
                            hello_state = probe_hello(&hello_scope);
                            status = BwStatus::Unauthenticated;
                            account_email = None;
                            form.error = None;
                        }
                        Err(e) => {
                            log::warn!("bw logout failed: {e}");
                            form.error = Some(e);
                        }
                    },
                    None => {}
                }
            },
        );
    };

    (options, Box::new(login_frame_fn), LoginFrameHandles { token })
}

/// The login UI's per-frame closure, boxed so it can be stored in a struct and
/// handed to either host.
pub type LoginFrameFn = Box<dyn FnMut(&mut egui::Ui, &mut eframe::Frame)>;

/// The cell [`build_login_frame`]'s closure reports a produced session token
/// through.
pub struct LoginFrameHandles {
    token: Rc<RefCell<Option<String>>>,
}

impl LoginFrameHandles {
    /// The session token the sign-in produced, TAKEN -- a second call answers
    /// `None`.
    ///
    /// Taking rather than cloning is what makes this safe to poll every frame,
    /// which is exactly what `app_window` does: the frame after the one that
    /// produced the token is already the spinner, and a cell that kept
    /// answering `Some` would drive that transition again on every frame for
    /// the rest of the session.
    pub fn take_token(&self) -> Option<String> {
        self.token.borrow_mut().take()
    }
}

/// Opens the sign-in window in its OWN event loop and blocks until it closes.
///
/// This is the host for every caller that has nothing to put in the window
/// afterwards: the lock/re-auth recovery, an account switch, adding an
/// account. Startup's host is `app_window`, which calls [`build_login_frame`]
/// directly and keeps the same window for the spinner and the vault.
pub fn run_login_flow_for(
    account: Option<(&Path, &Account)>,
    first_run: bool,
) -> Option<String> {
    let (options, mut frame_fn, handles) = build_login_frame(
        account,
        first_run,
        // This host owns its window, so its first frame is the one that
        // installs the fonts, rounds the corners and raises it...
        false,
        // ...and a produced token is the end of the window, because there is
        // no next state for it to enter.
        true,
    );

    let _ = eframe::run_ui_native(WINDOW_TITLE, options, move |ui, frame| frame_fn(ui, frame));

    // `None` means the user closed the window with the X button rather than
    // completing the flow. What that *costs* is the caller's to decide, and
    // is decided in exactly one place: [`run_login_flow`].
    handles.take_token()
}

/// The login flow for the two callers that genuinely cannot continue without
/// a session: startup, and the lock/re-auth recovery. A closed window ends
/// the process.
///
/// Everything else -- an account switch, adding an account -- must call
/// [`run_login_flow_for`] and handle `None`. That distinction is the whole
/// reason these are two functions and not one: declining a switch is an
/// ordinary gesture, and if the process-level exit lived in the body, closing
/// the master-password prompt during a switch would kill an app that was
/// running perfectly well with a vault already open.
///
/// `account` and `first_run` are passed straight through to
/// [`run_login_flow_for`]; this function decides what a closed window costs
/// and nothing else. It takes them rather than hard-coding `None` because a
/// hard-coded `None` is an app whose *startup* login window silently has no
/// quick unlock -- which is the one login window every user meets.
pub fn run_login_flow(account: Option<(&Path, &Account)>, first_run: bool) -> String {
    match run_login_flow_for(account, first_run) {
        Some(session_token) => session_token,
        None => {
            // Exit cleanly with a logged reason rather than a raw panic
            // backtrace: every downstream operation needs a session.
            log::error!("login window was closed without producing a session token; exiting");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod round_window_corners_stays_inside_this_process {
    use super::{round_window_corners, VAULT_WINDOW_TITLE, WINDOW_TITLE};

    /// **This process's window lookup answers `None` in a test process, for
    /// every title this crate rounds the corners of.**
    ///
    /// The reported defect: `round_window_corners` asked
    /// `FindWindowW(None, title)`, which walks the WHOLE DESKTOP and returns
    /// the first top-level window with that exact title -- and
    /// `crate::vault_window::WINDOW_TITLE` is "Deskwarden", which a File
    /// Explorer window open on this repo's own `deskwarden\` folder carries.
    /// It then wrote `DWMWA_WINDOW_CORNER_PREFERENCE` into a window belonging
    /// to another process, non-deterministically, permanently for that
    /// window's lifetime. A reviewer measured the frame harness reaching it
    /// six times per suite run **while the user's real Deskwarden was
    /// running**.
    ///
    /// `foreground::own_window_titled` filters by `GetCurrentProcessId`, and
    /// the test process opens no windows, so this is `None` no matter what is
    /// on the desktop -- which is exactly the property: a title collision
    /// with anything outside this process can no longer be reached.
    ///
    /// This is a live call, not a source scan: it fails against a build that
    /// went back to a desktop-wide `FindWindowW` **while an app or an
    /// Explorer window with one of these titles is open**, and it costs
    /// nothing when none is.
    #[test]
    fn no_window_of_this_process_answers_to_any_title_this_crate_rounds() {
        for title in [WINDOW_TITLE, VAULT_WINDOW_TITLE, "Deskwarden Preferences"] {
            assert_eq!(
                crate::foreground::own_window_titled(title),
                None,
                "a test process owns no windows, so nothing should answer to {title:?}; \
                 whatever did belongs to another process and `round_window_corners` would \
                 have written a DWM attribute into it"
            );
            // And the caller itself is therefore inert. Calling it is the
            // point: a `None` lookup that the caller ignored would be no fix
            // at all.
            round_window_corners(title);
        }
    }

    /// **The whole crate resolves its own window one way.**
    ///
    /// A source guard beside the live one above, because the live one is
    /// silent on a desktop that happens to have no colliding window open --
    /// which is most desktops, and none of the CI ones. `FindWindowW` is the
    /// only desktop-wide window lookup Win32 offers by name, so its absence
    /// from every call position in this crate is the whole of "nobody asks
    /// that question any more". It may still be NAMED, in the two paragraphs
    /// that explain why it is not called.
    ///
    /// The scan is over `use` LINES rather than over the whole text, because
    /// the whole text is where those two paragraphs are: a guard that
    /// forbade the name outright would forbid writing down why. `FindWindowW`
    /// is a Win32 import and cannot be called without one, so a file that
    /// does not import it does not call it.
    #[test]
    fn nothing_in_this_crate_imports_the_desktop_wide_window_lookup() {
        // Split so this line does not match itself.
        let name = concat!("FindWindow", "W");
        let mut scanned = 0usize;
        for (module, source) in [
            ("login_ui.rs", include_str!("login_ui.rs")),
            ("foreground.rs", include_str!("foreground.rs")),
            ("prefs_ui.rs", include_str!("prefs_ui.rs")),
            ("vault_window/mod.rs", include_str!("vault_window/mod.rs")),
        ] {
            for line in source.lines() {
                let trimmed = line.trim_start();
                if !trimmed.starts_with("use ") {
                    continue;
                }
                scanned += 1;
                assert!(
                    !trimmed.contains(name),
                    "{module} imports {name}, which is not scoped to this process: it returns \
                     the first window ANYWHERE on the desktop with the title asked for, and \
                     this app's titles are common enough that a folder window answers to one. \
                     Use `foreground::own_window_titled`. The line was: {trimmed}"
                );
            }
        }
        assert!(
            scanned > 40,
            "control: only {scanned} `use` lines were inspected across four modules, so this \
             guard is looking at almost nothing"
        );
    }
}

#[cfg(test)]
mod close_control_tests {
    //! The chrome's ✕, in both of its states.
    //!
    //! [`CloseControl::Disabled`] exists because one host -- the single startup
    //! window's working stage -- must not be closed while it holds a `bw serve`
    //! that is still coming up, and, now that stage wears the same heading as
    //! every other window, it has a ✕ to account for. Both halves of "disabled"
    //! are asserted here, because either alone is a bug that ships: a ghosted ✕
    //! that still closes the window strands the backend, and a live-looking ✕
    //! that quietly does nothing is the thing this was written to avoid.

    use super::*;
    use eframe::egui::{Color32, Pos2};

    const WIDTH: f32 = 840.0;
    const HEIGHT: f32 = 600.0;

    fn raw_input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(WIDTH, HEIGHT),
            )),
            events,
            ..Default::default()
        }
    }

    /// A context with `theme::apply`'s fonts live -- the chrome draws its title
    /// in one of them, and a frame that looks up a family which does not exist
    /// yet panics.
    fn styled_context() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw_input(Vec::new()), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(raw_input(Vec::new()), |_ui| {});
        ctx
    }

    /// A full primary press-and-release at `pos`: egui reports
    /// `Response::clicked` on the RELEASE, so a press alone is not a click.
    fn click(pos: Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    /// Every straight line segment painted, as (start, end, colour). The ✕ is
    /// two diagonals and the — is one horizontal; nothing else in the chrome is
    /// a line segment, so the two are told apart by their own geometry rather
    /// than by paint order.
    fn segments(output: &egui::FullOutput) -> Vec<(Pos2, Pos2, Color32)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(Pos2, Pos2, Color32)>) {
            match shape {
                egui::Shape::LineSegment { points, stroke } => {
                    out.push((points[0], points[1], stroke.color))
                }
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn diagonal_colors(output: &egui::FullOutput) -> Vec<Color32> {
        segments(output)
            .into_iter()
            .filter(|(a, b, _)| (a.y - b.y).abs() > 1.0)
            .map(|(_, _, color)| color)
            .collect()
    }

    fn horizontal_colors(output: &egui::FullOutput) -> Vec<Color32> {
        segments(output)
            .into_iter()
            .filter(|(a, b, _)| (a.y - b.y).abs() <= 1.0)
            .map(|(_, _, color)| color)
            .collect()
    }

    /// One frame of the vault window's chrome, with `close` and `events`.
    fn chrome(ctx: &egui::Context, close: CloseControl, events: Vec<egui::Event>) -> ChromeAction {
        let mut action = ChromeAction::None;
        let _ = ctx.run_ui(raw_input(events), |ui| {
            action = draw_window_chrome_with_extra(
                ui,
                "Deskwarden",
                ChromeMetrics::VAULT,
                true,
                close,
                |_ui| {},
            );
        });
        action
    }

    fn painted(close: CloseControl) -> egui::FullOutput {
        let ctx = styled_context();
        ctx.run_ui(raw_input(Vec::new()), |ui| {
            let _ = draw_window_chrome_with_extra(
                ui,
                "Deskwarden",
                ChromeMetrics::VAULT,
                true,
                close,
                |_ui| {},
            );
        })
    }

    /// **A disabled ✕ looks disabled** -- `TEXT_GHOST`, the same ink the ▢
    /// wears on a window that cannot be maximized, and the same ink the sign-in
    /// card's field labels take while a sign-in is in flight.
    #[test]
    fn a_disabled_close_is_ghosted_and_a_live_one_is_not() {
        let ghosted = diagonal_colors(&painted(CloseControl::Disabled));
        assert!(
            !ghosted.is_empty(),
            "control: no ✕ glyph was painted at all, so its colour proves nothing"
        );
        assert!(
            ghosted.iter().all(|color| *color == theme::TEXT_GHOST),
            "the disabled ✕ is painted in {ghosted:?} rather than `theme::TEXT_GHOST` -- it \
             looks exactly like a working close button and does nothing"
        );

        // The pairing: the ACTIVE ✕ is not ghosted, so the assertion above is
        // about `CloseControl` rather than about a ✕ that is always grey.
        let live = diagonal_colors(&painted(CloseControl::Active));
        assert!(
            live.iter().all(|color| *color == theme::TEXT_FAINT),
            "control: the live ✕ is painted in {live:?}, not the chrome's usual \
             `theme::TEXT_FAINT`"
        );

        // And the disabling is aimed at the ✕ alone: minimising a window whose
        // backend is still starting strands nothing, so — stays live in both.
        for close in [CloseControl::Active, CloseControl::Disabled] {
            let flat = horizontal_colors(&painted(close));
            assert!(
                flat.iter().any(|color| *color == theme::TEXT_FAINT),
                "{close:?} ghosted the — control as well: {flat:?}"
            );
        }
    }

    /// **A disabled ✕ acts disabled.** No interaction is registered for it, so
    /// a click cannot produce a `Close` for a host to have to refuse.
    #[test]
    fn only_a_live_close_reports_a_close_when_it_is_clicked() {
        // The rightmost control zone's centre, from the metrics the frame is
        // drawn with -- where the ✕ is, not what it is expected to do.
        let at = Pos2::new(
            WIDTH - ChromeMetrics::VAULT.control_width / 2.0,
            ChromeMetrics::VAULT.bar_height / 2.0,
        );

        let ctx = styled_context();
        let _ = chrome(&ctx, CloseControl::Active, Vec::new());
        assert_eq!(
            chrome(&ctx, CloseControl::Active, click(at)),
            ChromeAction::Close,
            "control: clicking the ✕ of a normal window does not close it either, so the \
             assertion below is about the click never landing"
        );

        let ctx = styled_context();
        let _ = chrome(&ctx, CloseControl::Disabled, Vec::new());
        assert_eq!(
            chrome(&ctx, CloseControl::Disabled, click(at)),
            ChromeAction::None,
            "the disabled ✕ still reports a close when clicked, so the one stage that must not \
             close is relying on its host to refuse -- and the startup window's refusal leaves \
             the user clicking a control that visibly does nothing"
        );
    }
}

#[cfg(test)]
mod resize_zone_tests {
    //! The eight-direction resize hit-zone geometry.
    //!
    //! The behavioural half -- that a hover over a zone sets its cursor, that
    //! a drag emits `ViewportCommand::BeginResize`, and that a panel drawn
    //! over the same pixels does not steal either -- is in
    //! `draw_resize_handles_tests` below, which drives real frames.
    //!
    //! WHAT NEITHER MODULE CAN ASSERT, said plainly rather than dressed up as
    //! coverage: that Windows then actually resizes the window. Everything
    //! past `send_viewport_cmd` is `egui_winit` calling
    //! `Window::drag_resize_window`, which needs a real winit window inside a
    //! real event loop and a real mouse button held down; a headless
    //! `egui::Context` records the command and there is nobody on the other
    //! end of it. So these tests prove the command is issued for the right
    //! direction from the right pixels, and nothing about what the OS does
    //! with it. That last step, and the `with_min_inner_size` floor the OS
    //! enforces during the drag, are UNVERIFIED HERE.
    use super::{resize_zones, RESIZE_BAND, RESIZE_CORNER};
    use eframe::egui;

    const BAND: f32 = 6.0;
    const CORNER: f32 = 14.0;

    fn window() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1240.0, 740.0))
    }

    fn zones() -> Vec<super::ResizeZone> {
        resize_zones(window(), BAND, CORNER)
    }

    #[test]
    fn all_eight_directions_are_present_exactly_once() {
        // The user asked for all eight. A missing one is invisible: the edge
        // simply does nothing, which is indistinguishable from the bug this
        // whole feature fixes.
        let zones = zones();
        let mut directions: Vec<_> = zones.iter().map(|z| format!("{:?}", z.direction)).collect();
        directions.sort();
        assert_eq!(
            directions,
            vec![
                "East", "North", "NorthEast", "NorthWest", "South", "SouthEast", "SouthWest",
                "West"
            ]
        );
        let mut ids: Vec<_> = zones.iter().map(|z| z.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 8, "each zone needs its own stable interaction id");
    }

    #[test]
    fn no_two_zones_overlap() {
        // The property the corner-first layout exists to give: which zone a
        // point belongs to is readable off the rects, not off the order they
        // happen to be registered in.
        let zones = zones();
        for (i, a) in zones.iter().enumerate() {
            for b in &zones[i + 1..] {
                let overlap = a.rect.intersect(b.rect);
                assert!(
                    !overlap.is_positive(),
                    "{} and {} overlap at {overlap:?} -- whichever is registered later would \
                     silently win, and the corners are the ones that would lose",
                    a.id,
                    b.id
                );
            }
        }
    }

    #[test]
    fn every_zone_lies_on_the_window_edge_it_names() {
        let window = window();
        for zone in zones() {
            assert!(
                window.contains_rect(zone.rect),
                "{} at {:?} pokes outside the window",
                zone.id,
                zone.rect
            );
            let touches_edge = zone.rect.min.x == window.min.x
                || zone.rect.min.y == window.min.y
                || zone.rect.max.x == window.max.x
                || zone.rect.max.y == window.max.y;
            assert!(touches_edge, "{} at {:?} is not on any edge", zone.id, zone.rect);
        }
    }

    #[test]
    fn each_corner_is_hit_before_the_edges_that_meet_there() {
        // A corner point must belong to the corner zone and to nothing else.
        // Without the non-overlapping split it belongs to two edges as well,
        // and diagonal resizing quietly becomes single-axis resizing.
        let window = window();
        for (corner, id) in [
            (window.min, "nw"),
            (egui::pos2(window.max.x - 1.0, window.min.y), "ne"),
            (egui::pos2(window.min.x, window.max.y - 1.0), "sw"),
            (egui::pos2(window.max.x - 1.0, window.max.y - 1.0), "se"),
        ] {
            let hits: Vec<_> = zones()
                .into_iter()
                .filter(|z| z.rect.contains(corner))
                .map(|z| z.id)
                .collect();
            assert_eq!(hits, vec![id], "{corner:?} should belong only to {id}");
        }
    }

    #[test]
    fn a_point_in_the_middle_of_the_window_belongs_to_no_zone() {
        // Otherwise the entire window body starts resizing instead of
        // clicking, which would be far worse than not resizing at all.
        let centre = window().center();
        assert!(zones().into_iter().all(|z| !z.rect.contains(centre)));
    }

    #[test]
    fn a_window_too_small_for_two_corners_emits_corners_only() {
        // The floor makes this unreachable in the vault window, but an
        // inverted `Rect` contains nothing, so emitting one would create a
        // zone that exists in the list and can never be hit -- a silent hole
        // rather than an obvious absence.
        let sliver = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(20.0, 20.0));
        let zones = resize_zones(sliver, BAND, CORNER);
        assert_eq!(zones.len(), 4);
        for zone in &zones {
            assert!(zone.rect.is_positive(), "{} is inverted or empty", zone.id);
        }
    }

    #[test]
    fn the_corner_target_is_larger_than_the_edge_band() {
        // Not decoration: a corner is a two-axis target reachable no other
        // way, and at `RESIZE_BAND` it is a 6x6 square in the very pixel the
        // window ends.
        assert!(RESIZE_CORNER > RESIZE_BAND);
    }
}

#[cfg(test)]
mod draw_resize_handles_tests {
    //! Real frames through `draw_resize_handles`, reading back the cursor it
    //! asked for and the viewport commands it issued.
    //!
    //! These exist because the geometry tests above prove only that the rects
    //! are in the right places. Two things sit between correct rects and a
    //! window that resizes, and both are invisible: whether the zones win the
    //! hit test against the panels laid out over the very same pixels, and
    //! whether a drag on one issues the command naming ITS direction.
    //!
    //! WHAT THESE DO NOT DISTINGUISH, recorded because it was checked and the
    //! obvious reading of them is wrong: they stay green when
    //! `draw_resize_handles` is rewritten to register its zones directly in
    //! the root panel layer, with no `Area` at all. egui's hit test prefers
    //! the smaller candidate, so the thin zones beat a whole-window
    //! `CentralPanel` either way. The `Order::Foreground` layer is defence in
    //! depth against that preference changing -- these tests do not pin it.
    use super::draw_resize_handles;
    use eframe::egui;

    const WINDOW: egui::Vec2 = egui::Vec2::new(1240.0, 740.0);

    fn base_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, WINDOW)),
            ..Default::default()
        }
    }

    /// Draws the handles and then a `CentralPanel` that fills the entire
    /// window with an interactive widget -- i.e. the vault window's own
    /// layout, which covers every edge the handles sit on. Anything the
    /// handles win here, they win in spite of that.
    fn frame(ctx: &egui::Context, input: egui::RawInput) -> egui::FullOutput {
        ctx.run_ui(input, |ui| {
            draw_resize_handles(ui.ctx());
            egui::CentralPanel::default().show(ui, |ui| {
                let rect = ui.max_rect();
                ui.interact(rect, egui::Id::new("a-panel-covering-everything"), egui::Sense::click_and_drag());
            });
        })
    }

    fn hover_at(pos: egui::Pos2) -> egui::CursorIcon {
        let ctx = egui::Context::default();
        let input = |pos: egui::Pos2| egui::RawInput {
            events: vec![egui::Event::PointerMoved(pos)],
            ..base_input()
        };
        // Three settling frames, measured not guessed: egui resolves an
        // interaction against the widget rects registered in the PREVIOUS
        // pass, and an `Area`'s own rect is itself only known one pass after
        // its contents were first laid out -- so the hover does not resolve
        // until the third.
        for _ in 0..3 {
            let _ = frame(&ctx, input(pos));
        }
        frame(&ctx, input(pos)).platform_output.cursor_icon
    }

    #[test]
    fn each_edge_and_corner_shows_its_own_resize_cursor_through_the_panels() {
        // Pixel-by-pixel: 2 points in from an edge is inside the 6pt band; 4
        // points in from a corner is inside the 14pt corner square. Every one
        // of these points is also covered by the `CentralPanel` above.
        for (pos, expected, what) in [
            (egui::pos2(2.0, 370.0), egui::CursorIcon::ResizeWest, "left edge"),
            (egui::pos2(1238.0, 370.0), egui::CursorIcon::ResizeEast, "right edge"),
            (egui::pos2(620.0, 2.0), egui::CursorIcon::ResizeNorth, "top edge"),
            (egui::pos2(620.0, 738.0), egui::CursorIcon::ResizeSouth, "bottom edge"),
            (egui::pos2(4.0, 4.0), egui::CursorIcon::ResizeNorthWest, "top-left corner"),
            (egui::pos2(1236.0, 4.0), egui::CursorIcon::ResizeNorthEast, "top-right corner"),
            (egui::pos2(4.0, 736.0), egui::CursorIcon::ResizeSouthWest, "bottom-left corner"),
            (egui::pos2(1236.0, 736.0), egui::CursorIcon::ResizeSouthEast, "bottom-right corner"),
        ] {
            assert_eq!(
                hover_at(pos),
                expected,
                "the {what} at {pos:?} did not offer a resize cursor -- either the zone is not \
                 there, or the `CentralPanel` drawn over every one of these pixels won the hit \
                 test instead"
            );
        }
    }

    #[test]
    fn the_middle_of_the_window_offers_no_resize_cursor() {
        // The complement, and the one that would catch a band so wide (or a
        // zone rect so wrong) that the window body starts resizing instead of
        // clicking.
        assert_ne!(hover_at(egui::pos2(620.0, 370.0)), egui::CursorIcon::ResizeWest);
        assert_eq!(hover_at(egui::pos2(620.0, 370.0)), egui::CursorIcon::default());
    }

    #[test]
    fn dragging_an_edge_asks_the_os_to_resize_in_that_edges_direction() {
        // The direction is the part that can silently be wrong: a copy-paste
        // that gives the west zone `ResizeDirection::East` compiles, hovers
        // correctly, and resizes the opposite edge.
        for (start, drift, expected, what) in [
            (egui::pos2(2.0, 370.0), egui::vec2(40.0, 0.0), egui::viewport::ResizeDirection::West, "left edge"),
            (egui::pos2(1238.0, 370.0), egui::vec2(-40.0, 0.0), egui::viewport::ResizeDirection::East, "right edge"),
            (egui::pos2(620.0, 2.0), egui::vec2(0.0, -40.0), egui::viewport::ResizeDirection::North, "top edge"),
            (egui::pos2(620.0, 738.0), egui::vec2(0.0, 40.0), egui::viewport::ResizeDirection::South, "bottom edge"),
            (egui::pos2(4.0, 4.0), egui::vec2(-40.0, -40.0), egui::viewport::ResizeDirection::NorthWest, "top-left corner"),
            (egui::pos2(1236.0, 4.0), egui::vec2(40.0, -40.0), egui::viewport::ResizeDirection::NorthEast, "top-right corner"),
            (egui::pos2(4.0, 736.0), egui::vec2(-40.0, 40.0), egui::viewport::ResizeDirection::SouthWest, "bottom-left corner"),
            (egui::pos2(1236.0, 736.0), egui::vec2(40.0, 40.0), egui::viewport::ResizeDirection::SouthEast, "bottom-right corner"),
        ] {
            assert_eq!(
                begin_resize_commands_for_drag(start, drift),
                vec![expected],
                "dragging the {what} must ask for exactly one resize, in its own direction"
            );
        }
    }

    #[test]
    fn dragging_the_middle_of_the_window_asks_for_no_resize_at_all() {
        assert!(
            begin_resize_commands_for_drag(egui::pos2(620.0, 370.0), egui::vec2(40.0, 40.0))
                .is_empty(),
            "a drag in the window body is a drag in the window body"
        );
    }

    /// Presses at `start`, moves by `drift` with the button held, and returns
    /// every `BeginResize` direction issued across those frames.
    fn begin_resize_commands_for_drag(
        start: egui::Pos2,
        drift: egui::Vec2,
    ) -> Vec<egui::viewport::ResizeDirection> {
        let ctx = egui::Context::default();
        let at = |events: Vec<egui::Event>| egui::RawInput { events, ..base_input() };

        // Three settling frames (see `hover_at`), then the press. Measured,
        // not assumed: `drag_started` fires on the PRESS frame itself, not on
        // the first frame the pointer subsequently moves -- which is the
        // behaviour a resize wants, since the OS takes the drag over from
        // there and egui never sees the motion.
        for _ in 0..3 {
            let _ = frame(&ctx, at(vec![egui::Event::PointerMoved(start)]));
        }
        let mut directions = Vec::new();
        let mut collect = |output: egui::FullOutput| {
            for viewport in output.viewport_output.values() {
                for command in &viewport.commands {
                    if let egui::ViewportCommand::BeginResize(direction) = command {
                        directions.push(*direction);
                    }
                }
            }
        };
        collect(frame(
            &ctx,
            at(vec![egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            }]),
        ));
        // Two further frames with the button still held. They collect too, so
        // the assertion of exactly ONE command also proves the resize is not
        // re-issued on every frame of the drag -- which would fight the OS's
        // own resize loop for the rest of it.
        collect(frame(&ctx, at(vec![egui::Event::PointerMoved(start + drift)])));
        collect(frame(&ctx, at(vec![egui::Event::PointerMoved(start + drift + drift)])));
        directions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **`bw logout` has to run in the doomed account's OWN directory.**
    ///
    /// `remove_account` settles the app onto the SURVIVOR before it logs
    /// anything out, so a logout aimed at "whatever this process is pointed
    /// at" would sign out the account the user is keeping and leave the one
    /// they are deleting signed in on the server. Neither end state is visible
    /// from the outside -- both leave a deleted directory behind and both
    /// report success -- so the only thing that can be checked is what the
    /// command was aimed at, which is why `logout_command_in` is built and
    /// read rather than run.
    ///
    /// Never run, either: `.output()` here would sign the developer out of
    /// their real vault.
    #[test]
    fn a_logout_is_aimed_at_the_directory_it_was_asked_for() {
        use std::ffi::OsStr;
        use std::path::PathBuf;

        // First-wins and idempotent, so this is safe beside `bw_path`'s own
        // tests in the same process.
        crate::bw_path::remember_verified_bw_exe(PathBuf::from(
            r"C:\deskwarden-test\first\bw.exe",
        ));
        let dir = PathBuf::from(r"C:\cfg\accounts\0123456789abcdef0123456789abcdef");

        let cmd = logout_command_in(Some(&dir)).expect("a verified exe was just recorded");

        let appdata: Vec<Option<PathBuf>> = cmd
            .get_envs()
            .filter(|(key, _)| *key == OsStr::new(crate::bw_path::BW_DATA_DIR_ENV))
            .map(|(_, value)| value.map(PathBuf::from))
            .collect();
        assert_eq!(
            appdata,
            vec![Some(dir)],
            "the logout was aimed at some other profile than the one asked for, which for a \
             removal is the account the user is KEEPING"
        );
        assert_eq!(
            cmd.get_args().collect::<Vec<&OsStr>>(),
            vec![OsStr::new("logout")],
            "a `bw logout` that is not `bw logout`"
        );

        // Positive control on the same reader: the directory really is
        // carried per call, not baked in, so the assertion above is about the
        // argument rather than about a variable that is always set.
        let default = logout_command_in(None).expect("a verified exe was just recorded");
        assert!(
            default
                .get_envs()
                .all(|(key, _)| key != OsStr::new(crate::bw_path::BW_DATA_DIR_ENV)),
            "a logout for the CLI's default profile named a directory anyway"
        );
    }

    /// The active-profile wrapper is one line over the directory form, so the
    /// two cannot drift into two different ideas of what a successful logout
    /// is. Compile-time and source-level, because the only way to tell them
    /// apart at runtime is to run `bw`.
    #[test]
    fn the_active_profile_logout_is_a_wrapper_over_the_directory_one() {
        let source = include_str!("login_ui.rs");
        let body = source
            .split_once(concat!("pub fn bw_", "logout() -> Result<(), String> {"))
            .expect("the active-profile logout must still exist")
            .1;
        let body = body
            .split_once('}')
            .expect("its body must still be closed")
            .0;
        assert!(
            body.contains(concat!("bw_logout", "_in(")),
            "`bw_logout` has its own idea of what a logout is: {body:?}"
        );
        assert!(
            body.contains(concat!("active_data", "_dir()")),
            "`bw_logout` no longer acts on the ACTIVE profile, which is the whole of what it \
             means: {body:?}"
        );
        // Positive control: the region really is a body and not the whole file.
        assert!(
            body.len() < source.len() / 4,
            "control: the split isolated a body rather than keeping the rest of the file"
        );
    }

    #[test]
    fn parses_unlocked_status() {
        assert_eq!(
            parse_bw_status(r#"{"status":"unlocked","userEmail":"a@b.c"}"#),
            BwStatus::Unlocked
        );
    }

    #[test]
    fn parses_locked_status() {
        assert_eq!(parse_bw_status(r#"{"status":"locked"}"#), BwStatus::Locked);
    }

    #[test]
    fn treats_unauthenticated_and_unparseable_output_as_unauthenticated() {
        assert_eq!(
            parse_bw_status(r#"{"status":"unauthenticated"}"#),
            BwStatus::Unauthenticated
        );
        assert_eq!(parse_bw_status(""), BwStatus::Unauthenticated);
        assert_eq!(
            parse_bw_status("command not found"),
            BwStatus::Unauthenticated
        );
    }

    #[test]
    fn details_carry_the_account_email_and_server() {
        let details = parse_bw_status_details(
            r#"{"serverUrl":"https://vault.ledgerline.eu","lastSync":"2026-07-29T01:00:00.000Z","userEmail":"a.novak@ledgerline.com","status":"locked"}"#,
        );
        assert_eq!(details.status, BwStatus::Locked);
        assert_eq!(
            details.user_email.as_deref(),
            Some("a.novak@ledgerline.com")
        );
        assert_eq!(
            details.server_url.as_deref(),
            Some("https://vault.ledgerline.eu")
        );
    }

    #[test]
    fn details_survive_garbage_output() {
        let details = parse_bw_status_details("command not found");
        assert_eq!(details.status, BwStatus::Unauthenticated);
        assert_eq!(details.user_email, None);
        assert_eq!(details.server_url, None);
    }

    #[test]
    fn details_treat_empty_strings_as_absent() {
        // The CLI reports `"userEmail":""` in some states; showing an empty
        // label beside "Master password" would look broken.
        let details =
            parse_bw_status_details(r#"{"status":"locked","userEmail":"","serverUrl":""}"#);
        assert_eq!(details.user_email, None);
        assert_eq!(details.server_url, None);
    }

    /// The exact stderr a wrong master password produced, verbatim. Four
    /// lines of crypto-internals is what the login window used to show.
    const WRONG_PASSWORD_STDERR: &str = "\
ERROR bitwarden_crypto::keys::master_key: error=The decryption operation failed
ERROR bitwarden_core::client::internal: error=Cryptography error, The decryption operation failed
ERROR bitwarden_core::key_management::crypto: error=Cryptography error, The decryption operation failed
Cryptography error, The decryption operation failed";

    #[test]
    fn a_wrong_master_password_reads_as_a_wrong_master_password() {
        let shown = friendly_auth_error(WRONG_PASSWORD_STDERR);
        assert_eq!(
            shown,
            "That master password didn't work. Check it and try again."
        );
        // Whatever else changes, none of the CLI's internals may reach the
        // window: those are for the log.
        assert!(!shown.contains("bitwarden_crypto"), "got: {shown}");
        assert!(!shown.contains("decryption"), "got: {shown}");
        assert!(!shown.contains('\n'), "got a multi-line message: {shown}");
    }

    #[test]
    fn a_rejected_sign_in_names_both_fields_it_could_be() {
        let shown =
            friendly_auth_error("Username or password is incorrect. Try again.");
        assert_eq!(
            shown,
            "That email or master password didn't work. Check them and try again."
        );
    }

    #[test]
    fn two_step_login_says_what_to_actually_do_about_it() {
        let shown = friendly_auth_error("Two-step login is required for this account.");
        assert!(shown.contains("bw login"), "got: {shown}");
    }

    #[test]
    fn an_unreachable_server_is_not_reported_as_a_bad_password() {
        let shown = friendly_auth_error("request to https://vault.example.eu failed, \
                                         reason: getaddrinfo ENOTFOUND vault.example.eu");
        assert!(shown.starts_with("Couldn't reach the server"), "got: {shown}");
    }

    #[test]
    fn an_unrecognised_failure_keeps_the_clis_own_wording() {
        // Falling back to something generic would strand whoever hits a
        // failure mode this function has no case for. The CLI's own plain
        // summary line survives; only the `ERROR <module>:` framing goes.
        let shown = friendly_auth_error(
            "ERROR bitwarden_core::something: error=Vault is in an unexpected state\n\
             Vault is in an unexpected state",
        );
        assert_eq!(shown, "Vault is in an unexpected state");
    }

    #[test]
    fn a_framework_only_failure_still_yields_its_message() {
        // No plain summary line this time -- the `error=` payload is all
        // there is, so it gets unwrapped rather than shown with its framing.
        let shown = friendly_auth_error(
            "ERROR bitwarden_core::something: error=Vault is in an unexpected state",
        );
        assert_eq!(shown, "Vault is in an unexpected state");
    }

    #[test]
    fn a_silent_failure_still_says_something() {
        assert!(!friendly_auth_error("").is_empty());
        assert!(!friendly_auth_error("   \n  \n").is_empty());
    }

    #[test]
    fn server_host_strips_scheme_and_path() {
        assert_eq!(
            server_host(Some("https://vault.ledgerline.eu/api")),
            "vault.ledgerline.eu"
        );
        assert_eq!(
            server_host(Some("http://192.168.1.20:8443")),
            "192.168.1.20:8443"
        );
    }

    #[test]
    fn server_host_defaults_to_the_bitwarden_cloud() {
        assert_eq!(server_host(None), "bitwarden.com");
        assert_eq!(server_host(Some("")), "bitwarden.com");
    }

    // -- Blank fields must never reach the CLI ------------------------------
    //
    // An empty password used to be handed to `bw` anyway, and because
    // Windows treats an environment variable set to "" as unset, the CLI
    // answered "Provided passwordenv DESKWARDEN_BW_PASSWORD is not set" --
    // our own plumbing leaking into the window instead of "you left the box
    // blank".

    #[test]
    fn an_empty_password_is_caught_before_the_cli_is_spawned() {
        for status in [
            BwStatus::Unauthenticated,
            BwStatus::Locked,
            BwStatus::Unlocked,
        ] {
            assert_eq!(
                missing_credential_message(status, "a@b.c", ""),
                Some("Enter your master password first."),
                "{status:?} let an empty password through"
            );
        }
    }

    #[test]
    fn a_missing_email_is_caught_only_when_signing_in() {
        // Signing in needs one...
        assert_eq!(
            missing_credential_message(BwStatus::Unauthenticated, "   ", "pw"),
            Some("Enter your email address first.")
        );
        // ...whereas unlocking already knows whose vault this is, so a blank
        // email is irrelevant and must not block the unlock.
        assert_eq!(
            missing_credential_message(BwStatus::Locked, "", "pw"),
            None
        );
    }

    #[test]
    fn a_filled_in_form_reports_nothing_missing() {
        assert_eq!(
            missing_credential_message(BwStatus::Unauthenticated, "a@b.c", "pw"),
            None
        );
    }

    #[test]
    fn a_whitespace_only_password_is_still_submitted() {
        // Spaces are legal in a master password, so unlike the email this is
        // deliberately not trimmed -- rejecting it would lock out anyone
        // whose password legitimately starts or ends with one.
        assert_eq!(missing_credential_message(BwStatus::Locked, "", " "), None);
    }
}

#[cfg(test)]
mod login_entry_point_tests {
    //! Which of the two login entry points may end the process, and which
    //! `hello::` functions this window is allowed to call.
    //!
    //! Source guards, and unavoidably so: `run_login_flow_for` opens a real
    //! eframe window and the `hello::` calls pop OS dialogs, so neither can be
    //! driven from a test. What they pin is not cosmetic -- it is the
    //! difference between a declined account switch that leaves the app
    //! running and one that kills it, and between a log-out that drops this
    //! account's sealed master password and one that drops another account's.

    use super::*;

    const SOURCE: &str = include_str!("login_ui.rs");

    /// **Every `bw` this window runs names the profile directory it acts on,
    /// derived from the account the window belongs to.**
    ///
    /// `main`'s `add_account` opens this window for an account that has just
    /// been minted and is NOT the active one. It used to get the CLI there by
    /// pointing `bw_path::set_active_data_dir` at the new directory across the
    /// whole blocking window and putting it back afterwards -- a temporary
    /// mutation of a process-global that `bw_serve` spawns `bw` off from
    /// background threads, which `remove_account`'s own doc bans in as many
    /// words. Removing it is only safe if the window reaches that directory
    /// some other way, and this is that way.
    ///
    /// Fails without the fix: make `profile_dir_for` answer
    /// `crate::bw_path::active_data_dir()` and the first block fails, because
    /// no active directory is set in a test. Put `bw_logout()` back into the
    /// log-out arm — where it really was, for as long as this guard was a ban
    /// list that did not name it — and the allowlist block fails.
    #[test]
    fn the_login_window_names_the_profile_directory_it_acts_on() {
        let cfg = Path::new(r"C:\cfg");
        let account = Account {
            id: AccountId::parse("0123456789abcdef0123456789abcdef").expect("a valid id"),
            email: "a@example.com".to_string(),
            server_url: None,
        };
        assert_eq!(
            profile_dir_for(Some((cfg, &account))),
            Some(crate::accounts::data_dir_for(cfg, &account.id)),
            "the window's `bw` calls do not name the account they belong to, so a sign-in for \
             a NEW account lands in whatever profile this process happens to be on -- which \
             `bw login` then replaces"
        );
        assert_eq!(
            profile_dir_for(None),
            None,
            "an account-less window (StartupAccounts::NoAccountList) invented a directory rather \
             than leaving the CLI to resolve its own"
        );

        // The spawns themselves, as an ALLOWLIST rather than a ban list.
        //
        // This was three banned spellings -- `bw_command()`,
        // `check_bw_status_details()`, `active_data_dir()` -- and it missed
        // the fourth, `bw_logout()`, which sat in the log-out arm two lines
        // above a `hello::unenroll_for` on THIS window's account. A
        // hand-enumerated ban list is only ever as complete as the last
        // reviewer who read the whole file; this repo has now been bitten by
        // that shape twice. Inverted, the default is refusal: every
        // profile-sensitive call the body makes has to be one this window is
        // permitted to make, so a form nobody has thought of yet fails on the
        // commit that introduces it.
        //
        // Permitted = takes the directory it acts on. `run_bw_with_password`
        // is here because it takes `data_dir` as an argument (asserted below);
        // the other three are the `_in` forms.
        const ALLOWED: &[&str] = &[
            concat!("check_bw_status_details", "_in"),
            concat!("bw_logout", "_in"),
            concat!("configure_server", "_in"),
            concat!("run_bw_with", "_password"),
        ];
        let code = window_body_code();
        let found = profile_sensitive_calls(&code);
        for call in &found {
            assert!(
                ALLOWED.contains(&call.as_str()),
                "the login window calls {call}(), which is not one of the forms that take the \
                 profile directory they act on ({ALLOWED:?}). For the account `add_account` \
                 opens this window for, an active-profile form names the EXISTING account's \
                 vault -- it signs the wrong account out, or reads the wrong one's status. \
                 Either give it a directory-taking form, or add it here on purpose."
            );
        }
        // Positive controls for that negative. The scan found real calls (an
        // empty `found` would pass the loop vacuously), the region really is
        // the window's body, and the predicate really does catch a global
        // form when one is there.
        assert!(
            found.len() >= 3,
            "control: only {} profile-sensitive calls were found in the window body, so the \
             loop above is passing against almost nothing: {found:?}",
            found.len()
        );
        for required in [
            concat!("check_bw_status_details", "_in("),
            concat!("configure_server", "_in("),
            concat!("bw_logout", "_in("),
            concat!("profile_dir", "_for(account)"),
        ] {
            assert!(
                window_body().contains(required),
                "control: {required} is not in the sliced region, so the negatives above are \
                 about the wrong text"
            );
        }
        assert_eq!(
            profile_sensitive_calls(concat!(
                "let a = bw_command", "(); let b = active_data_dir", "();\n",
                "let c = check_bw_status_details", "(); let d = format!(\"x\");"
            )),
            vec![
                concat!("bw_command").to_string(),
                concat!("active_data", "_dir").to_string(),
                concat!("check_bw_status", "_details").to_string(),
            ],
            "control: the predicate does not catch the global forms it exists to catch, so the \
             loop above cannot fail for the reason it claims"
        );
        // And the one spawn that is not in that region: the worker thread.
        assert!(
            SOURCE.contains(concat!("run_bw_with_password(&arg_refs, &password, data_dir", ".")),
            "control: the `bw login`/`bw unlock` worker still takes its directory as an \
             argument rather than reading a global"
        );
    }

    /// Every call in `code` that decides *which profile directory* a `bw`
    /// child reads, named without its `(`, in source order.
    ///
    /// A **pattern**, not a list of names, and that is the whole point: the
    /// list it replaced named three spellings and missed a fourth that had
    /// been there all along. `bw_*` catches `bw_command`, `bw_logout`,
    /// `bw_status_stdout` and anything spelled like them; the three
    /// remaining prefixes cover the entry points in this file that spawn `bw`
    /// under some other name. A form that does match has to earn its place in
    /// `ALLOWED`.
    ///
    /// What it must **not** be read as is "a form that matches none of these
    /// is not a `bw` spawn", which this doc used to say and which is untrue of
    /// this file's own [`logout_command_in`] — a `bw` spawn matching no prefix
    /// here. Nor is it a closed set: `use ... as sign_out;`, a bare
    /// `logout_command()`, a `status_command()`, the helper taken as a
    /// function value and called through the binding, and `bw_logout ()` with
    /// a space all pass straight through. No rule over source text can catch a
    /// rename, so this is a tripwire against reintroducing a *known* spelling,
    /// not a proof of absence; a newly named spawn helper still has to be
    /// reviewed on its own merits.
    fn profile_sensitive_calls(code: &str) -> Vec<String> {
        let bytes = code.as_bytes();
        let mut out = Vec::new();
        let mut i = 0usize;
        while i < bytes.len() {
            if !(bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
                i += 1;
                continue;
            }
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            // A call, not a mention: the identifier is immediately followed
            // by its open paren.
            if bytes.get(i) != Some(&b'(') {
                continue;
            }
            let name = &code[start..i];
            let sensitive = name.starts_with("bw_")
                || name.starts_with("check_bw")
                || name.starts_with("run_bw")
                || name.starts_with("configure_server")
                || name == "active_data_dir";
            if sensitive {
                out.push(name.to_string());
            }
        }
        out
    }

    /// [`window_body`] with `//` line comments removed.
    ///
    /// Prose is not a call. Without this, a comment explaining *why*
    /// `bw_logout()` must not appear would itself trip the guard -- and the
    /// fix for that would be to stop writing the explanation, which is the
    /// wrong direction. `://` is left alone so a URL in a string literal is
    /// not mistaken for a comment.
    fn window_body_code() -> String {
        window_body()
            .lines()
            .map(|line| match line.find("//") {
                Some(at) if !line[..at].ends_with(':') => &line[..at],
                _ => line,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The login WINDOW's body: `build_login_frame`, plus the thin host
    /// `run_login_flow_for` that follows it, up to the fatal wrapper -- the
    /// same slice `only_the_fatal_entry_point_can_end_the_process` uses, named
    /// once so the two cannot disagree.
    ///
    /// It starts at `build_login_frame` and not at `run_login_flow_for`
    /// because that is where the window's frame closure went when the single
    /// window took the login UI as its first state. Left aimed at
    /// `run_login_flow_for`, this would slice the six-line host instead of the
    /// window -- which the length assertion below would catch, and which is
    /// why that assertion is there.
    fn window_body() -> &'static str {
        let body = SOURCE
            .split_once("pub fn build_login_frame(")
            .expect("the login window's frame builder must exist")
            .1
            .split_once("pub fn run_login_flow(")
            .expect("the wrapper must follow it")
            .0;
        assert!(
            body.len() > 5000,
            "the sliced window body is {} bytes, which is not the window: every assertion over \
             it would pass against nothing",
            body.len()
        );
        body
    }

    #[test]
    fn only_the_fatal_entry_point_can_end_the_process() {
        // Needle assembled at compile time so it cannot match its own
        // declaration, and kept to one line so it reads the same under LF and
        // CRLF.
        let needle = concat!("std::process", "::exit(1)");
        assert_eq!(
            SOURCE.matches(needle).count(),
            1,
            "there must be exactly one process exit in this file"
        );

        let wrapper = SOURCE
            .split_once("pub fn run_login_flow(")
            .expect("the fatal wrapper must exist")
            .1;
        assert!(
            wrapper.contains(needle),
            "the exit left the fatal wrapper, so some other caller now ends \
             the process"
        );
        assert!(
            wrapper.len() < SOURCE.len(),
            "positive control: the split actually isolated a region"
        );

        // And the cancellable body -- from its definition up to the
        // wrapper's -- has none of it. Implied by the count above, but stated
        // separately so an exit put back into the body fails with the message
        // that says what it costs.
        let body = SOURCE
            .split_once("pub fn build_login_frame(")
            .expect("the login window's frame builder must exist")
            .1;
        let body = body
            .split_once("pub fn run_login_flow(")
            .expect("the wrapper must follow it")
            .0;
        assert!(
            !body.contains(needle),
            "the cancellable login flow ends the process; a declined account \
             switch would kill a running app"
        );
        // Positive controls for that negative: the isolated region is really
        // the flow's body, and the needle is findable in text containing it.
        assert!(
            body.contains("run_ui_native"),
            "the region searched is not the login flow's body"
        );
        assert_eq!(format!("{needle} {needle}").matches(needle).count(), 2);
    }

    #[test]
    fn the_two_entry_points_differ_in_exactly_what_a_closed_window_costs() {
        // A compile-time pin: if `run_login_flow_for` goes back to returning
        // `String`, or the fatal wrapper starts returning `Option`, this stops
        // compiling -- which is the failure. Task 9's switch is built on that
        // `Option` being there.
        let cancellable: fn(Option<(&Path, &Account)>, bool) -> Option<String> = run_login_flow_for;
        let fatal: fn(Option<(&Path, &Account)>, bool) -> String = run_login_flow;
        assert!(
            !std::ptr::eq(cancellable as *const (), fatal as *const ()),
            "positive control: these must be two distinct functions"
        );
    }

    #[test]
    fn the_login_window_uses_only_the_per_account_hello_entry_points() {
        for stale in [
            concat!("hello", "::unenroll()"),
            concat!("hello", "::state()"),
            concat!("hello", "::unlock_password()"),
            concat!("hello", "::enroll("),
            concat!("hello", "::blob_path()"),
        ] {
            assert!(
                !SOURCE.contains(stale),
                "an account-less hello entry point is still called here: {stale}"
            );
        }
        for required in [
            concat!("hello", "::unenroll_for("),
            concat!("hello", "::state_for("),
            concat!("hello", "::unlock_password_for("),
            concat!("hello", "::enroll_for("),
        ] {
            assert!(
                SOURCE.contains(required),
                "this window no longer calls {required} at all"
            );
        }
        // The `enroll(` needle must not be satisfiable by `enroll_for(`, or
        // the two halves above would contradict each other and one of them
        // would be inert. Same for `state(` against `state_for(`.
        assert!(!concat!("hello", "::enroll_for(").contains(concat!("hello", "::enroll(")));
        assert!(!concat!("hello", "::state_for(").contains(concat!("hello", "::state(")));
    }
}

#[cfg(test)]
mod first_run_notice_tests {
    //! The line that says why a machine the user is already signed in on is
    //! asking for a master password.
    //!
    //! Driven through real frames with a headless `egui::Context`, the way the
    //! rest of this crate's windows are tested: what can go wrong here is not
    //! the wording, it is the window never painting it -- or painting it at
    //! every user on every launch.

    use super::*;

    const WINDOW: Vec2 = egui::vec2(470.0, 588.0);

    /// A context with `theme::apply`'s fonts actually live -- a font set
    /// registered during a frame only becomes usable at the start of the next
    /// one, so the throwaway frames are load-bearing.
    fn styled_context() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw_input(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(raw_input(), |_ui| {});
        ctx
    }

    fn raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, WINDOW)),
            ..Default::default()
        }
    }

    fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            // Everything else is geometry this module does not assert on.
            _ => {}
        }
    }

    /// Every string the login window paints in one frame of a `Locked` vault.
    fn painted(hello: HelloState, first_run: bool) -> Vec<String> {
        let ctx = styled_context();
        let mut form = LoginForm::default();
        let mut flow_bottom = 0.0f32;
        let output = ctx.run_ui(raw_input(), |ui| {
            let _ = draw_login_window(
                ui,
                BwStatus::Locked,
                Some("a.novak@ledgerline.com"),
                "bitwarden.com",
                hello,
                &mut form,
                &mut flow_bottom,
                false,
                first_run,
            );
        });
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut texts);
        }
        texts
    }

    /// Two independent needles, one per half of [`FIRST_RUN_NOTICE`], because
    /// the two halves answer different questions and a notice that dropped
    /// either would still pass a single-needle check. Neither spans the `\`
    /// continuation the constant is wrapped with, and neither is re-derived
    /// from the constant itself -- a test that asserted
    /// `texts.contains(FIRST_RUN_NOTICE)` would pass for every wording,
    /// including an empty one.
    fn says_why_it_is_asking(texts: &[String]) -> bool {
        texts
            .iter()
            .any(|t| t.to_lowercase().contains("its own profile"))
    }

    fn says_hello_must_be_set_up_again(texts: &[String]) -> bool {
        texts
            .iter()
            .any(|t| t.to_lowercase().contains("set up again"))
    }

    /// The Hello panel's own first-use subtitle: the positive control for
    /// every "the notice is absent" assertion below. Without it, a window that
    /// painted nothing at all would pass them.
    fn shows_the_hello_panel(texts: &[String]) -> bool {
        texts.iter().any(|t| t.contains("First use"))
    }

    const ENROLLED: HelloState = HelloState {
        available: true,
        enrolled: true,
    };
    const NOT_ENROLLED: HelloState = HelloState {
        available: true,
        enrolled: false,
    };

    #[test]
    fn a_freshly_minted_account_is_told_why_it_is_being_asked_and_what_it_costs() {
        let texts = painted(NOT_ENROLLED, true);
        assert!(
            says_why_it_is_asking(&texts),
            "the first-run window asks for a master password without saying why; got: {texts:?}"
        );
        assert!(
            says_hello_must_be_set_up_again(&texts),
            "the first-run notice no longer says quick unlock has to be set up again, which is \
             the half the user finds out about by its absence; got: {texts:?}"
        );
        assert!(
            shows_the_hello_panel(&texts),
            "positive control: this state must still draw the Hello panel"
        );
    }

    #[test]
    fn every_later_launch_is_told_nothing() {
        // The gate. `resolve_startup` answers `first_run` only for the account
        // it minted on this launch, so a second launch -- and a switch to any
        // other account -- paints nothing. A notice that appeared every time
        // would be a permanent "something is wrong" on a working app.
        let texts = painted(NOT_ENROLLED, false);
        assert!(
            shows_the_hello_panel(&texts),
            "positive control: the window drew nothing, so the assertion below \
             would pass for the wrong reason; got: {texts:?}"
        );
        assert!(
            !says_why_it_is_asking(&texts) && !says_hello_must_be_set_up_again(&texts),
            "the first-run notice is painted unconditionally; got: {texts:?}"
        );
    }

    #[test]
    fn an_account_that_has_already_enrolled_is_not_told_again() {
        let texts = painted(ENROLLED, true);
        assert!(
            !texts.is_empty(),
            "positive control: the window painted nothing at all"
        );
        assert!(
            !says_hello_must_be_set_up_again(&texts),
            "an enrolled account is still being told to set Hello up again; \
             got: {texts:?}"
        );
    }

    #[test]
    fn the_notice_survives_hello_being_switched_off_on_the_machine() {
        // The case the line exists for. With Hello unavailable there is no
        // panel, so a notice nested inside the panel would vanish with it --
        // and the user's only signal for why they are being asked to sign in
        // again would be the silence this whole notice is about.
        let texts = painted(HelloState::unavailable(), true);
        assert!(
            !shows_the_hello_panel(&texts),
            "positive control: no Hello panel may be drawn in this state; \
             got: {texts:?}"
        );
        assert!(
            says_why_it_is_asking(&texts),
            "the notice is nested inside the Hello panel, so it disappeared \
             with it; got: {texts:?}"
        );
    }
}

#[cfg(test)]
mod empty_app_behind_the_card_tests {
    //! **The login window is now the app it is about to become, with nothing
    //! in it.**
    //!
    //! The user's words: "pre-load the big screen with the position and size
    //! etc as it will be after log-in but empty kinda of and then login on top
    //! blocking it". What can go wrong is geometry -- panes at the wrong
    //! widths, a card centred against the wrong rect, a card that does not fit
    //! the smallest window the vault is allowed to be -- so most of this is
    //! measured. The two halves no harness can reach (the viewport builder and
    //! the frame closure both need a live eframe event loop) are source
    //! guards, in the house shape.

    use super::*;
    use egui::Rect;

    /// A window the size design 2b draws.
    fn full(width: f32, height: f32) -> Rect {
        Rect::from_min_size(Pos2::new(40.0, 30.0), Vec2::new(width, height))
    }

    /// **The bar is the vault window's own, not a number written out again.**
    /// It is the one piece of the next window this one still shows, so a
    /// different height here is a visible jump at the instant of sign-in --
    /// which is the whole reason this window borrows the vault window's
    /// placement in the first place.
    ///
    /// The panes this used to check are gone: the body is one flat region
    /// now, because a drawn sidebar and item list promise controls that are
    /// not there.
    #[test]
    fn the_placeholder_bar_is_the_vault_windows_own_height() {
        let skeleton = vault_skeleton(full(1240.0, 740.0));
        assert_eq!(
            skeleton.bar.height(),
            ChromeMetrics::VAULT.bar_height,
            "the placeholder titlebar is {}px against the vault window's {}px",
            skeleton.bar.height(),
            ChromeMetrics::VAULT.bar_height
        );
    }

    /// The bar and the body tile the window with no seam and no overlap. A
    /// gap here paints the window background through the middle of the app,
    /// which is the one way a single flat region can still look wrong.
    #[test]
    fn the_bar_and_the_body_tile_the_window() {
        let window = full(1240.0, 740.0);
        let skeleton = vault_skeleton(window);

        assert_eq!(skeleton.bar.min, window.min, "the titlebar is not at the window's top");
        assert_eq!(
            skeleton.body.min.y, skeleton.bar.max.y,
            "the body does not begin where the titlebar ends"
        );
        assert_eq!(skeleton.body.min.x, window.min.x, "the body does not reach the left edge");
        assert_eq!(skeleton.body.max, window.max, "the body does not reach the window's edges");
        assert!(
            skeleton.body.height() > 0.0,
            "the body has no height, so every assertion about what is painted in it is vacuous"
        );
    }

    /// **The card is centred in the BODY**, not in the whole window. Centred
    /// against the window it sits half a titlebar high in the region it is
    /// blocking, which is visible and is the kind of thing that gets
    /// "simplified" back.
    #[test]
    fn the_card_is_centred_in_the_body_and_keeps_its_own_width() {
        let window = full(1240.0, 740.0);
        let skeleton = vault_skeleton(window);
        let card = login_card_rect(&skeleton, LOGIN_CARD_INITIAL_HEIGHT);

        assert_eq!(card.center(), skeleton.body.center());
        // The positive control: the body's centre and the window's really are
        // different points here, so the assertion above is a choice between
        // two answers rather than one restated.
        assert_ne!(
            skeleton.body.center(),
            window.center(),
            "the body and the window share a centre, so centring on either is the same test"
        );
        assert_eq!(
            card.width(),
            LOGIN_CARD_WIDTH,
            "the card is no longer the width the 3h composition is drawn at"
        );
    }

    /// **The card fits the smallest window the vault is allowed to open at.**
    /// The login window borrows the vault window's geometry, and
    /// `settings::MIN_VAULT_WINDOW_SIZE` is the floor that geometry is clamped
    /// to -- so this is a real reachable state, not a hypothetical one.
    #[test]
    fn the_card_fits_inside_the_smallest_vault_window() {
        let (min_w, min_h) = crate::settings::MIN_VAULT_WINDOW_SIZE;
        let skeleton = vault_skeleton(full(min_w as f32, min_h as f32));
        let card = login_card_rect(&skeleton, LOGIN_CARD_INITIAL_HEIGHT);
        assert!(
            skeleton.body.contains_rect(card),
            "at the smallest window the vault may open at ({min_w}x{min_h}) the {}x{} card \
             overhangs the {}x{} body it is supposed to float in",
            card.width(),
            card.height(),
            skeleton.body.width(),
            skeleton.body.height()
        );
    }

    /// **The measured height round-trips.** `login_card_height` answers with a
    /// card size; `login_card_content_rect` carves the content area back out
    /// of it. The content area has to come out tall enough for the run that
    /// was measured PLUS the footer row pinned under it -- otherwise the card
    /// clips its own footer, which is the failure the old
    /// `ViewportCommand::InnerSize` version could not have (it sized the
    /// window, and the window had no margins of its own).
    #[test]
    fn a_card_sized_from_a_measured_run_has_room_for_that_run_and_its_footer() {
        for flow in [180.0f32, 462.0, 700.0] {
            let content_top = 137.0f32;
            let height = login_card_height(content_top + flow, content_top);
            let card =
                Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(LOGIN_CARD_WIDTH, height));
            let content = login_card_content_rect(card);
            let needed = flow + FOOTER_RESERVE - CARD_MARGIN_BOTTOM;
            assert!(
                content.height() >= needed - 0.01,
                "a run of {flow}px produced a {height}px card whose content area is only {}px, \
                 which is less than the {needed}px that run plus its pinned footer needs",
                content.height()
            );
            // The other side of it: not wildly oversized either, or the card
            // grows a band of dead space under the footer on every state.
            assert!(
                content.height() <= needed + 1.0,
                "a run of {flow}px produced a content area of {}px against the {needed}px it \
                 needs -- the card is padded with dead space",
                content.height()
            );
        }
    }

    /// The default the first frame paints at, before anything has been
    /// measured, is the height the window's own composition used to be: this
    /// window WAS 470x588 with a 40px `ChromeMetrics::LOGIN` bar over the same
    /// body. Anything else and the first frame the user ever sees is a card of
    /// the wrong size that then resettles.
    #[test]
    fn the_cards_starting_height_is_the_composition_it_replaced() {
        assert_eq!(
            LOGIN_CARD_INITIAL_HEIGHT,
            588.0 - ChromeMetrics::LOGIN.bar_height,
            "the card's starting height is no longer the body of the 470x588 window this \
             composition used to be, so the first frame resettles in front of the user"
        );
    }

    /// What `paint_vault_skeleton` actually puts on screen: the vault window's
    /// own fills, and a scrim over the whole body.
    ///
    /// The fills are read back off the painter rather than argued about --
    /// "the sidebar is CARD and the panes are CANVAS" is the entire difference
    /// between this reading as the app with nothing in it and reading as a
    /// blank rectangle.
    #[test]
    fn the_empty_app_is_painted_in_the_vault_windows_own_fills_under_a_scrim() {
        let ctx = egui::Context::default();
        let window = full(1240.0, 740.0);
        let skeleton = vault_skeleton(window);
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0))),
                ..Default::default()
            },
            |ui| paint_vault_skeleton(ui.painter(), &skeleton),
        );

        let mut rects: Vec<(Rect, Color32)> = Vec::new();
        fn walk(shape: &egui::Shape, out: &mut Vec<(Rect, Color32)>) {
            match shape {
                egui::Shape::Rect(r) => out.push((r.rect, r.fill)),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut rects);
        }

        let painted = |rect: Rect, fill: Color32| {
            rects
                .iter()
                .any(|(r, f)| *f == fill && (r.width() - rect.width()).abs() < 0.01
                    && (r.height() - rect.height()).abs() < 0.01
                    && (r.min.x - rect.min.x).abs() < 0.01
                    && (r.min.y - rect.min.y).abs() < 0.01)
        };

        assert!(
            painted(skeleton.body, theme::CANVAS),
            "the body behind the card is not one flat CANVAS region; painted: {rects:?}"
        );
        // The divisions are gone and must stay gone: a CARD-filled rect in
        // the body is the old sidebar coming back, and it promises a control
        // that is not there.
        assert!(
            !rects.iter().any(|(r, f)| *f == theme::CARD && skeleton.body.contains(r.center())),
            "something is painted as a pane inside the body again; painted: {rects:?}"
        );
        assert!(
            painted(skeleton.body, SCRIM),
            "nothing dims the empty app, so the card does not read as blocking it"
        );
        // ...and the scrim stops at the titlebar, which carries the live
        // window controls the user still has to be able to find. `>=` rather
        // than `intersects`, which counts two rects sharing one edge -- and
        // the bar's bottom edge IS the body's top one.
        for (rect, fill) in &rects {
            if *fill == SCRIM {
                assert!(
                    rect.min.y >= skeleton.bar.max.y - 0.01,
                    "a scrim rect reaches up to {} , above the titlebar's bottom at {} -- it \
                     is dimming the ✕ and — the user is still expected to be able to hit",
                    rect.min.y,
                    skeleton.bar.max.y
                );
            }
        }
    }

    /// The halves no harness can reach: the viewport is built before any frame
    /// exists, and the frame closure runs only inside a live eframe event
    /// loop. Both are pinned at the source, with `concat!`-split single-line
    /// needles -- a needle written as one literal matches its own declaration,
    /// and one carrying a newline passes on LF and fails on CRLF.
    #[test]
    fn the_window_opens_at_the_vault_windows_placement_and_paints_its_empty_panes() {
        let source = include_str!("login_ui.rs");
        let body = source
            .split_once(concat!("pub fn build_login", "_frame("))
            .expect("`build_login_frame` is gone")
            .1
            .split_once(concat!("pub fn run_login", "_flow("))
            .expect("`run_login_flow` no longer follows it, so this slice is unbounded")
            .0;

        for (needle, what) in [
            (
                concat!("vault_window::initial_", "placement("),
                "the login window no longer derives its placement from the vault window's own, \
                 so the two can disagree and the window jumps at sign-in after all",
            ),
            (
                concat!("Settings::load(path)", ".vault_window"),
                "the login window reads no saved vault geometry, so it opens wherever the OS \
                 puts it",
            ),
            (
                concat!("with_inner_size([placement", ".width as f32"),
                "the window is not sized from the placement it just computed",
            ),
            (
                concat!("viewport.with_", "position([x as f32"),
                "the window is not positioned from the placement it just computed, so it opens \
                 at the right size in the wrong place",
            ),
            (
                concat!("paint_vault_", "skeleton(ui.painter()"),
                "nothing paints the empty app behind the card",
            ),
            (
                concat!("login_card_", "rect(&skeleton, card_height)"),
                "the card is not placed against the empty app's own body",
            ),
            // The trailing comma is what makes this the CALL rather than the
            // prose above it: the comment explaining why the window stays
            // fixed-size names the same constant, and a bare needle counts
            // two.
            (
                concat!("ChromeMetrics::VAULT", ","),
                "the placeholder titlebar is not drawn at the vault window's metrics",
            ),
        ] {
            assert_eq!(
                body.matches(needle).count(),
                1,
                "expected exactly one {needle:?} in `build_login_frame`: {what}"
            );
        }

        // ...and the two things it must NOT do any more. The old window resized
        // ITSELF from the measured content; doing that now would drag the
        // borrowed vault geometry around under the user.
        let resizes = concat!("ViewportCommand::", "InnerSize");
        assert!(
            !body.contains(resizes),
            "the login window still resizes itself from its measured content, so it no longer \
             stays at the geometry the vault window will restore to"
        );
        // Positive control for that absence: the needle is one this crate
        // really does spell this way, so its absence here means something.
        assert!(
            source.contains(concat!("ViewportCommand::", "Close")),
            "no ViewportCommand at all is spelled this way any more -- the needle has drifted \
             and the assertion above proves nothing"
        );

        // **And the hazard the paragraph above actually names: writing the
        // borrowed geometry back.** Forbidding `InnerSize` forbids this window
        // RESIZING itself; it says nothing about this window SAVING. The
        // geometry on disk belongs to the vault window, and a login window
        // that persisted anything -- its own position after the user dragged
        // it, or the placement it merely borrowed -- would silently re-home the
        // vault window from a sign-in the user then abandoned. Nothing writes
        // it today; this is what keeps it that way.
        // **`settings.rs`'s PRODUCTION half only.** Read whole -- which it was
        // -- these controls could be satisfied by an occurrence in that file's
        // own test module: production could rename or drop its writer and the
        // needle would go on matching a fixture, leaving the absence
        // assertions above facts about a string that names nothing shipping.
        // The cut is the same one `settings.rs`'s own guards take, and what
        // keeps it where it is lives over there, in
        // `nothing_but_gated_test_modules_lives_below_the_guards_cut`.
        let settings_source = include_str!("settings.rs");
        let settings = settings_source
            .split_once(concat!("#[cfg(", "test)]"))
            .map_or(settings_source, |(production, _)| production);
        assert!(
            settings.len() < settings_source.len(),
            "control: the test gate was not found in settings.rs, so these controls are \
             reading that whole file, fixtures included"
        );
        for (needle, control, what) in [
            (
                concat!("persist_vault_window_", "geometry("),
                concat!("pub fn persist_vault_window_", "geometry("),
                "the login window writes the vault window's saved geometry, so an abandoned \
                 sign-in re-homes the vault window it only borrowed the placement of",
            ),
            // Belt and braces. `Settings::save` is private to the settings
            // module today, so this route does not compile from here at all --
            // which was verified by trying it. The needle is here for the day
            // somebody makes it `pub`, because the whole-struct save carries
            // `vault_window` and is the same hazard by a longer road.
            (
                concat!(".save", "("),
                concat!("on_disk.save", "(path)"),
                "the login window saves a whole `Settings` -- which carries `vault_window` -- so \
                 an abandoned sign-in can rewrite the vault window's geometry the long way round",
            ),
        ] {
            assert!(!body.contains(needle), "{what}");
            // Paired positive control, cross-file: the needle is one the
            // settings module really does spell this way, so counting zero in
            // `build_login_frame` is a fact about this function rather than
            // about a string nothing anywhere matches.
            assert!(
                settings.contains(control),
                "the settings writer is no longer spelled {control:?} -- the assertion above \
                 proves nothing"
            );
        }
    }
}

#[cfg(test)]
mod auth_in_flight_tests {
    //! **The credential zone while a sign-in is with the CLI.**
    //!
    //! The user's instruction was three things at once: keep a mask on the
    //! password field, grey the whole user/password zone out, and clear the
    //! password only when the attempt fails. All three are invisible from
    //! outside a live frame, so these drive real ones -- including real
    //! clicks and real keystrokes, because "the field is disabled" is a claim
    //! about what typing into it does, not about what colour it is.

    use super::*;
    use egui::Rect;

    const WINDOW: Vec2 = egui::vec2(470.0, 700.0);

    fn raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, WINDOW)),
            ..Default::default()
        }
    }

    /// A context with `theme::apply`'s fonts actually live (see
    /// `first_run_notice_tests::styled_context`, which does this for the same
    /// reason).
    fn styled_context() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw_input(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(raw_input(), |_ui| {});
        ctx
    }

    /// One frame of the sign-in window, drawn the way `run_login_flow_for`
    /// draws it: `BwStatus::Unauthenticated`, so the email field is on screen
    /// alongside the password one.
    fn frame(
        ctx: &egui::Context,
        form: &mut LoginForm,
        auth_in_progress: bool,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let mut flow_bottom = 0.0f32;
        ctx.run_ui(
            egui::RawInput { events, ..raw_input() },
            |ui| {
                egui::Frame::new().inner_margin(Margin::same(26)).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    let _ = draw_login_window(
                        ui,
                        BwStatus::Unauthenticated,
                        None,
                        "bitwarden.com",
                        HelloState { available: false, enrolled: false },
                        form,
                        &mut flow_bottom,
                        auth_in_progress,
                        false,
                    );
                });
            },
        )
    }

    /// Every 38px input box the frame painted, in top-to-bottom order, with
    /// the fill and border colour each was painted in.
    ///
    /// Found by [`theme::FIELD_HEIGHT`] -- the one measurement a live field
    /// and a greyed one still share -- so the greyed treatment cannot make
    /// these invisible to a test by changing what it is looking for.
    fn field_boxes(output: &egui::FullOutput) -> Vec<(Rect, Color32, Color32)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(Rect, Color32, Color32)>) {
            match shape {
                egui::Shape::Rect(r) if (r.rect.height() - theme::FIELD_HEIGHT).abs() < 0.01 => {
                    out.push((r.rect, r.fill, r.stroke.color));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out.sort_by(|a, b| a.0.top().total_cmp(&b.0.top()));
        out
    }

    /// The characters actually RENDERED inside `within`, glyph by glyph.
    ///
    /// Not `Galley::text()`, which answers with the SOURCE string and is
    /// therefore blind to truncation -- and, more to the point here, would
    /// answer with the real password rather than with the bullets a masked
    /// `TextEdit` puts on screen.
    fn rendered_glyphs_in(output: &egui::FullOutput, within: Rect) -> String {
        fn walk(shape: &egui::Shape, within: Rect, out: &mut String) {
            match shape {
                egui::Shape::Text(text) => {
                    let rect = shape.visual_bounding_rect();
                    if rect.is_finite() && within.expand(2.0).contains_rect(rect) {
                        for row in &text.galley.rows {
                            for glyph in &row.glyphs {
                                out.push(glyph.chr);
                            }
                        }
                    }
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, within, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = String::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, within, &mut out);
        }
        out
    }

    fn click(at: Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ]
    }

    /// Settles the layout, clicks the middle of the `nth` input box, types
    /// `typed`, and hands back the form and the context it happened in.
    fn click_the_field_and_type(
        auth_in_progress: bool,
        nth: usize,
        typed: &str,
        mut form: LoginForm,
    ) -> (LoginForm, egui::Context) {
        let ctx = styled_context();
        // Two settling frames: egui resolves an interaction against the
        // widget rects registered in the PREVIOUS pass.
        for _ in 0..2 {
            let _ = frame(&ctx, &mut form, auth_in_progress, Vec::new());
        }
        let boxes = field_boxes(&frame(&ctx, &mut form, auth_in_progress, Vec::new()));
        let target = boxes
            .get(nth)
            .unwrap_or_else(|| {
                panic!(
                    "the window painted {} input boxes, so there is no box {nth} to click -- \
                     this harness has stopped finding the credential fields",
                    boxes.len()
                )
            })
            .0;
        let _ = frame(&ctx, &mut form, auth_in_progress, click(target.center()));
        let _ = frame(
            &ctx,
            &mut form,
            auth_in_progress,
            vec![egui::Event::Text(typed.to_string())],
        );
        (form, ctx)
    }

    /// **The positive control for every "it takes no input" assertion below.**
    /// Without it, a window that painted no fields at all -- or a harness that
    /// clicked the wrong pixel -- would satisfy them all.
    #[test]
    fn the_email_field_takes_typing_when_no_attempt_is_in_flight() {
        let (form, ctx) = click_the_field_and_type(false, 0, "n", LoginForm::default());
        assert_eq!(
            form.email, "n",
            "clicking the first input box and typing did not reach the email field at all, so \
             this harness is not driving the window and proves nothing about the disabled case"
        );
        assert!(
            ctx.memory(|m| m.focused()).is_some(),
            "no widget took focus, so the click never landed on a text field"
        );
    }

    /// The same click and the same keystroke, with an attempt in flight.
    #[test]
    fn the_credential_fields_take_no_typing_while_an_attempt_is_in_flight() {
        for (nth, what) in [(0usize, "email"), (1, "password")] {
            let mut seeded = LoginForm::default();
            seeded.email = "a.novak@ledgerline.com".to_string();
            seeded.password = "correct horse".to_string();
            let (form, ctx) = click_the_field_and_type(true, nth, "X", seeded);
            assert_eq!(
                form.email, "a.novak@ledgerline.com",
                "clicking the {what} box during an attempt and typing changed the email"
            );
            assert_eq!(
                form.password, "correct horse",
                "clicking the {what} box during an attempt and typing changed the password"
            );
            assert!(
                ctx.memory(|m| m.focused()).is_none(),
                "a widget took focus when the {what} box was clicked during an attempt -- a \
                 greyed field that still focuses shows a caret and swallows the click"
            );
        }
    }

    /// **The mask must not track the buffer.** "Keep showing masked password
    /// (even if there is nothing already)": bullets that shortened as the
    /// field emptied would announce the emptying, and bullets that counted
    /// the real characters would announce the length.
    #[test]
    fn the_masked_password_reads_the_same_whether_the_buffer_is_empty_or_long() {
        let read = |password: &str| {
            let ctx = styled_context();
            let mut form = LoginForm::default();
            form.password = password.to_string();
            for _ in 0..2 {
                let _ = frame(&ctx, &mut form, true, Vec::new());
            }
            let output = frame(&ctx, &mut form, true, Vec::new());
            let boxes = field_boxes(&output);
            assert_eq!(
                boxes.len(),
                2,
                "expected the email and password boxes; found {}",
                boxes.len()
            );
            rendered_glyphs_in(&output, boxes[1].0)
        };

        let empty = read("");
        let long = read("a-master-password-of-some-considerable-length");

        assert_eq!(
            empty, long,
            "the password field renders {empty:?} for an empty buffer and {long:?} for a long \
             one, so what is on screen leaks what is behind it"
        );
        // The positive control for that: the bullets really are the field's
        // own and not an empty reading of an empty rect, and the count is the
        // one this app declares rather than whatever happened to be painted.
        assert_eq!(
            empty.matches('\u{2022}').count(),
            theme::MASKED_BULLETS,
            "the password field rendered {empty:?}, which is not {} bullets",
            theme::MASKED_BULLETS
        );
    }

    /// **Greyed means the box moved too**, not just that the text stopped
    /// accepting input. Asserted in both directions, so "the fields are
    /// greyed" cannot pass against a window that painted nothing.
    #[test]
    fn the_credential_zone_is_greyed_only_while_an_attempt_is_in_flight() {
        let boxes = |auth_in_progress: bool| {
            let ctx = styled_context();
            let mut form = LoginForm::default();
            for _ in 0..2 {
                let _ = frame(&ctx, &mut form, auth_in_progress, Vec::new());
            }
            field_boxes(&frame(&ctx, &mut form, auth_in_progress, Vec::new()))
                .into_iter()
                .map(|(_, fill, stroke)| (fill, stroke))
                .collect::<Vec<_>>()
        };

        let live = boxes(false);
        let in_flight = boxes(true);
        assert_eq!(
            live.len(),
            2,
            "the resting window painted {} input boxes, not the email and password pair",
            live.len()
        );
        assert_eq!(
            in_flight.len(),
            live.len(),
            "the window in flight painted {} input boxes against the resting window's {} -- \
             the fields did not grey out, they vanished",
            in_flight.len(),
            live.len()
        );
        for (fill, stroke) in &live {
            assert_eq!(
                (*fill, *stroke),
                (theme::CARD, theme::BORDER_STRONG),
                "a resting field is not painted in the live treatment, so the comparison \
                 below says nothing"
            );
        }
        for (fill, stroke) in &in_flight {
            assert_eq!(
                (*fill, *stroke),
                (theme::CANVAS, theme::BORDER),
                "a field in flight is still painted in the live treatment -- it reads as a \
                 field that can be typed into"
            );
        }
    }

    /// The labels above those boxes grey with them. "Gray out the whole
    /// user/pass zone" is not satisfied by a pale box under a black label.
    #[test]
    fn the_field_labels_grey_out_with_their_fields() {
        fn walk(shape: &egui::Shape, found: &mut Vec<(String, Color32)>) {
            match shape {
                egui::Shape::Text(text) => {
                    let color = text
                        .galley
                        .job
                        .sections
                        .first()
                        .map(|s| s.format.color)
                        .unwrap_or(text.fallback_color);
                    found.push((text.galley.text().to_string(), color));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, found);
                    }
                }
                _ => {}
            }
        }
        let label_colors = |auth_in_progress: bool| {
            let ctx = styled_context();
            let mut form = LoginForm::default();
            for _ in 0..2 {
                let _ = frame(&ctx, &mut form, auth_in_progress, Vec::new());
            }
            let output = frame(&ctx, &mut form, auth_in_progress, Vec::new());
            let mut found = Vec::new();
            for clipped in &output.shapes {
                walk(&clipped.shape, &mut found);
            }
            found
        };

        for (auth_in_progress, expected, described) in [
            (false, theme::TEXT_MUTED, "resting"),
            (true, theme::TEXT_GHOST, "in flight"),
        ] {
            let found = label_colors(auth_in_progress);
            for wanted in ["Email", "Master password"] {
                let color = found
                    .iter()
                    .find(|(text, _)| text == wanted)
                    .unwrap_or_else(|| {
                        panic!(
                            "the {described} window painted no {wanted:?} label at all, so its \
                             colour cannot be asserted; painted: {:?}",
                            found.iter().map(|(t, _)| t).collect::<Vec<_>>()
                        )
                    })
                    .1;
                assert_eq!(
                    color, expected,
                    "the {described} window paints its {wanted:?} label in {color:?}"
                );
            }
        }
    }

    /// **Either answer clears the master password; only a failure says why.**
    ///
    /// This used to assert the opposite of its first half -- that a SUCCESS
    /// deliberately left `form.password` full -- on the argument that the
    /// window is closing anyway. It is not, on one of the two hosts:
    /// `app_window` builds this frame with `close_on_success: false` and
    /// move-captures it into a window that becomes the spinner and then the
    /// vault, so "the window closes" happens when the user quits the app. The
    /// user's "clear if unsuccessful only" is about what the FIELD shows
    /// between attempts, which the second half still pins via `form.error`;
    /// it was never a licence to keep the plaintext for a session.
    #[test]
    fn either_answer_clears_the_master_password_and_only_a_failure_leaves_a_message() {
        let mut form = LoginForm::default();
        form.password = "correct horse".to_string();
        let outcome = apply_auth_result(Ok("session-token".to_string()), &mut form);
        assert_eq!(
            outcome,
            AuthOutcome::Succeeded("session-token".to_string()),
            "a successful attempt did not hand its session token back"
        );
        assert!(
            form.password.is_empty(),
            "a SUCCESSFUL attempt left the master password in the form: {:?} -- and the form \
             outlives the sign-in on the single-window host",
            form.password
        );
        assert!(form.error.is_none(), "a successful attempt left an error on screen");

        let mut form = LoginForm::default();
        form.password = "correct horse".to_string();
        let outcome = apply_auth_result(Err("Invalid master password.".to_string()), &mut form);
        assert_eq!(outcome, AuthOutcome::Failed);
        assert!(
            form.password.is_empty(),
            "a FAILED attempt left the master password in the field: {:?}",
            form.password
        );
        assert!(
            form.error.is_some(),
            "a failed attempt said nothing about why -- and the assertion above would then \
             pass against a function that only ever clears"
        );
    }

    /// The half of this that no harness can reach: `build_login_frame`'s
    /// frame closure runs only inside a live eframe event loop, so "the
    /// window routes its answer through `apply_auth_result`" and "submitting
    /// no longer wipes the field on the spot" are source guards.
    ///
    /// The needles are `concat!`-split and single-line, the house shape: a
    /// needle written as one literal matches its own declaration, and one
    /// carrying a newline passes on LF and fails on CRLF.
    #[test]
    fn the_window_routes_its_answer_through_the_tested_decision_and_does_not_wipe_on_submit() {
        let source = include_str!("login_ui.rs");
        let body = source
            .split_once(concat!("pub fn build_login", "_frame("))
            .expect("`build_login_frame` is gone")
            .1
            .split_once(concat!("pub fn run_login", "_flow("))
            .expect("`run_login_flow` no longer follows it, so this slice is unbounded")
            .0;

        let routed = concat!("apply_auth_", "result(result, &mut form)");
        assert!(
            body.contains(routed),
            "the window no longer applies its auth result through the one function that \
             decides what a failure costs, so every assertion about clearing is about dead \
             code"
        );

        let spawn = concat!("spawn_", "auth(");
        let spawn_at = body
            .find(spawn)
            .expect("the window no longer spawns an auth at all");
        // From the submit's `spawn_auth` to the end of that arm. Bounded by
        // the NEXT arm's own marker rather than by a byte count, which is how
        // a fixed window ends up reading someone else's code.
        let after_submit = &body[spawn_at..];
        let arm_end = after_submit
            .find(concat!("Some(LoginAction::Hello", "Unlock) =>"))
            .expect("the Hello arm no longer follows the submit arm, so this slice is unbounded");
        let submit_tail = &after_submit[..arm_end];
        let wipe = concat!("form.password.zero", "ize()");
        assert!(
            !submit_tail.contains(wipe),
            "submitting wipes the master password on the spot again, so the window spends the \
             whole attempt masking a buffer it already emptied: {submit_tail:?}"
        );
        // Positive control for that absence: the needle is one this file
        // really does contain, elsewhere.
        assert!(
            source.contains(wipe),
            "no {wipe:?} anywhere in this file -- the needle has drifted and the assertion \
             above proves nothing"
        );
    }
}

/// **How long the plaintext master password stays in memory.**
///
/// Every other test about the password is a test about what the *field* shows.
/// These are about the buffer behind it, and they are here because the claim
/// this module's production code makes -- "the plaintext is never released to
/// the allocator in the clear" -- was, until these existed, entirely untested:
/// emptying [`LoginForm`]'s `Drop` body left the whole suite green.
///
/// The instrument is a global allocator for this test binary that passes
/// everything through to `System` and, while a thread has armed it, scans each
/// block it is about to free for a probe string. Thread-local, so tests running
/// in parallel cannot see each other's frees; `const`-initialised `Cell<bool>`
/// with no destructor, so touching it from inside `dealloc` cannot re-enter the
/// allocator.
///
/// Every allocator-watch assertion below is paired with a positive control
/// that a bare `String` on the same path IS caught -- otherwise "the form did
/// not leak" would be indistinguishable from a watcher that never sees
/// anything. That promise was false as written for one revision: the success
/// test was the only one of the three without such a control, and that is
/// precisely the test in which two vacuities then went unnoticed. A `!leaked`
/// assertion whose control is missing is not a weaker guard, it is no guard.
///
/// **Two ways a watch assertion can be vacuous, and how each is closed here.**
///
/// * The **instrument** sees nothing -- a watch that was never armed, or a
///   probe short enough that `dealloc`'s size filter skips the block. Closed by
///   the bare-`String` control at the head of each test.
/// * The **subject** is nothing -- the buffer handed to the measured closure is
///   already empty, so its drop frees no memory and no probe could fire whatever
///   the code did. This is what a wipe that *reallocates* produces: it frees the
///   plaintext-bearing allocation inside `apply_auth_result`, before any watch
///   is armed, and leaves behind a zero-capacity `String`. Closed in
///   `a_successful_sign_in_wipes_the_password_while_the_window_stays_open` by
///   arming the watch around the `apply_auth_result` call itself -- which tests
///   the property this module is named for rather than a proxy for it -- and
///   again by asserting the taken buffer still has capacity before it is
///   measured.
///
/// **What is watched, and what is only forgotten.** The two `Drop` tests
/// measure the whole form; the success test measures every `String` field taken
/// out of the form, and forgets the rest, which after that take is three `Copy`
/// fields. An exhaustive (non-binding) destructuring pattern in that test makes
/// a newly added field a compile error there rather than a silent hole.
///
/// **What this cannot prove**, and the README says so too: `Zeroize for String`
/// wipes the allocation the `String` owns *now*. A `TextEdit` that grew the
/// buffer while the user typed left earlier, shorter copies behind, and those
/// were freed by `realloc` long before any of this. Fixing that means a
/// capacity-reserving `Zeroizing<String>` and is not what these tests claim.
#[cfg(test)]
pub(crate) mod password_lifetime_tests {
    use super::*;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::{Cell, RefCell};
    use std::panic::AssertUnwindSafe;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Mutex, MutexGuard};

    /// Long and distinctive: it must not occur by chance in an unrelated
    /// freed block, and it must be longer than a machine word so a partial
    /// overwrite cannot look like a wipe.
    ///
    /// **Assembled from fragments, and that is the whole of a fix.** While
    /// this was one string literal, THE SOURCE TEXT OF THIS FILE CONTAINED
    /// THE PROBE -- and around thirty tests in this crate read `src/` back to
    /// lint it, so every one of them allocated a copy of the probe and freed
    /// it on its own libtest thread, on no schedule at all. The global scan
    /// has no way to tell those bytes from an armed test's own, and credited
    /// them to whoever was armed: a measured false positive, on a security
    /// test, in the suite this repository uses as its mutation oracle.
    ///
    /// Two rounds of downstream filtering were tried against that noise and
    /// each left a channel open. `concat!` closes it at the source instead:
    /// the macro joins these four pieces at compile time, so `PROBE` is the
    /// same `&'static str` constant it always was -- every call site in the
    /// crate is unchanged, and there is no runtime cost -- while no source
    /// file in this tree contains the assembled bytes. A test that reads this
    /// file now reads four short fragments separated by punctuation, which
    /// is not the needle any scan here looks for.
    ///
    /// **So do not re-join these into one literal**, here or anywhere: the
    /// noise comes back, and it comes back as an intermittent failure of
    /// whichever probe test happened to be armed.
    pub(crate) const PROBE: &str =
        concat!("deskwarden-", "drop-probe-", "master-", "password");

    thread_local! {
        /// Whether THIS thread has a watch armed. What [`SEEN`] is gated on.
        static WATCHING: Cell<bool> = const { Cell::new(false) };
        /// Whether a probe-bearing block was freed **on this thread** while
        /// this thread's watch was armed. The verdict
        /// [`plaintext_reached_the_allocator`] returns.
        static SEEN: Cell<bool> = const { Cell::new(false) };
    }

    /// How many threads currently have a watch armed. Non-zero is what makes
    /// the allocator scan on **every** thread rather than only on armed ones.
    ///
    /// **This is the fix for the axis this instrument was blind on.** The
    /// arming was thread-local and so was the check inside `dealloc`, so a
    /// probe-bearing block freed on a thread the test did not itself arm -- a
    /// worker spawned by the body, and several fill paths spawn workers --
    /// went past the allocator with the watch reading clean. An instrument
    /// that reports clean while blind is this codebase's signature failure;
    /// the scan is now global and the *reporting* is what is split in two.
    static ANY_WATCH: AtomicUsize = AtomicUsize::new(0);

    /// Whether a probe-bearing block was freed on **any** thread while a watch
    /// was armed anywhere. Read by
    /// [`plaintext_reached_the_allocator_on_any_thread`], and cross-checked
    /// against [`SEEN`] by [`plaintext_reached_the_allocator`].
    static SEEN_ANYWHERE: AtomicBool = AtomicBool::new(false);

    /// **Only one watch is armed in this process at a time.**
    ///
    /// [`SEEN_ANYWHERE`] is process-global, so two probes armed concurrently
    /// would read each other's leaks. Both entry points take this for the
    /// whole of their armed window, which makes the global flag unambiguous.
    ///
    /// **It is held past the armed window, to the end of the test thread.**
    /// That is [`PROBE_HOLD`], and it is not an optimisation -- serialising
    /// only the armed windows left a hole that was a live flake, reproduced
    /// 20 times out of 20 by running
    /// `vault_cache::tests::a_custom_field_value_does_not_reach_the_allocator_in_the_clear`
    /// beside `a_custom_field_name_is_still_a_plain_string` at
    /// `--test-threads=2`. The name test builds a fixture whose field *name*
    /// is [`PROBE`] and drops a clone of it **outside** any armed window; with
    /// the value test armed on another thread, the global scan attributed that
    /// free to the value test, whose own body was clean, and
    /// [`plaintext_reached_the_allocator`]'s cross-check panicked. Noise, not
    /// blindness -- but noise landing on a security test, and the obvious way
    /// to quieten it is to delete the cross-check, which would trade a false
    /// positive for the false negative that check exists to prevent.
    static PROBE_LOCK: Mutex<()> = Mutex::new(());

    thread_local! {
        /// [`PROBE_LOCK`], taken at this thread's **first** arm and released
        /// only when the thread ends -- which, under libtest, is when the test
        /// ends. This is what serialises the *tests* rather than only their
        /// armed windows.
        ///
        /// **Why the whole test and not the window.** A probe test's fixture
        /// building, its snapshot clones and its final drops all touch probe
        /// plaintext outside the window it arms. Those frees are invisible to
        /// its own verdict but perfectly visible to whatever *other* probe
        /// test is armed at that instant, because the scan is global. Holding
        /// from the first arm covers all of it, because every probe test in
        /// this crate arms its positive control as its first probe act -- the
        /// house rule that each of them asserts its control first is what
        /// makes this cover the whole body, and a new probe test that
        /// allocates probe plaintext *before* its first arm would sit outside
        /// the hold again.
        ///
        /// Reentrant by inspection: a second arm on a thread that already
        /// holds it re-uses the hold instead of deadlocking on a
        /// non-reentrant `Mutex`.
        static PROBE_HOLD: RefCell<Option<ProbeHold>> = const { RefCell::new(None) };
    }

    /// [`PROBE_LOCK`]'s guard. A newtype rather than a bare `MutexGuard` so
    /// that [`PROBE_HOLD`]'s slot has one name to talk about; it carries no
    /// behaviour of its own, and releases the lock when the thread ends.
    ///
    /// It used to also maintain a `PROBE_HOLDERS` count, which existed for one
    /// reader: a gate on [`Watcher::realloc`]'s scan. That gate is gone (see
    /// the note there), and with it a counter whose own doc comment claimed it
    /// meant "threads currently inside a probe test" when the hold it counted
    /// is released only at thread end.
    struct ProbeHold(#[allow(dead_code)] MutexGuard<'static, ()>);

    /// Whether this thread is holding [`PROBE_LOCK`] through [`PROBE_HOLD`].
    /// The observable the serialisation test asserts on; deliberately not a
    /// cross-thread `try_lock`, which would race a third probe test.
    fn this_thread_holds_the_probe_lock() -> bool {
        PROBE_HOLD.with(|held| held.borrow().is_some())
    }

    struct Watcher;

    // SAFETY: every method forwards to `System`, which is a correct
    // `GlobalAlloc`. The only added work is a read of a thread-local `bool`
    // and, when it is set, a read of the block that is about to be freed.
    // Neither allocates, so `dealloc` cannot re-enter.
    //
    // That read is where the honest caveat is. The block is still mapped,
    // still owned by this allocator and not yet handed back -- but `layout`
    // covers the WHOLE allocation, and a `String`/`Vec` whose capacity exceeds
    // its length has never written the tail. So `from_raw_parts` below builds
    // a slice over bytes that are, in the abstract machine, uninitialised, and
    // reading them is UB by the letter of the rules. It is benign under the
    // `System` allocator this forwards to (`HeapAlloc` hands back real,
    // readable pages), and scanning the tail is what the probe wants anyway:
    // a truncate leaves the plaintext exactly there. It is a deliberate
    // trade inside a test-only instrument, not a soundness claim.
    unsafe impl GlobalAlloc for Watcher {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            unsafe { System.alloc_zeroed(layout) }
        }

        /// **The axis this instrument was deaf on.** A `String` grown past
        /// its capacity does not `dealloc` its old buffer -- it `realloc`s,
        /// and the old block goes back to the allocator through *this*
        /// method. While this forwarded blindly, every `!leaked` assertion in
        /// the crate was silent about growth, which is the one mechanism the
        /// `flush` comment in `injector::sequence` says it was worried about.
        ///
        /// **The scan happens before the call, never after.** Once
        /// `System.realloc` returns, the old block may already be freed and
        /// reading it is a use-after-free that the allocator's own debug
        /// assertions abort on -- so the block is read while it is still
        /// unambiguously ours, and the verdict is held until the return value
        /// says whether it moved.
        ///
        /// **Only a block that moved was released.** An in-place grow hands
        /// nothing back to the allocator and must not be flagged, or every
        /// wipe-then-grow would look like a leak at random depending on what
        /// the heap felt like doing.
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            // **UNGATED, AND THAT IS THE POINT.** This scan used to be gated
            // on "some thread is inside a probe test", justified by the claim
            // that no other test ever allocates the probe. That claim was
            // false while `PROBE` was a literal in this file -- the ~30 source
            // linting tests grow a `String` over probe-bearing text by
            // repeated `realloc`, with the gate reading zero -- so the old
            // block went back to the CRT inside `System.realloc`, never
            // reached `dealloc`, and never met the unconditional wipe. It sat
            // in the free list carrying probe bytes exactly as before the wipe
            // existed. `PROBE` is assembled by `concat!` now and that
            // particular producer is gone, but the gate was wrong on its own
            // terms and a gate whose premise has to stay true forever is not
            // a gate worth keeping. Its cost was measured as noise-level.
            let carries = layout.size() >= PROBE.len()
                && {
                    let block = unsafe { std::slice::from_raw_parts(ptr, layout.size()) };
                    block.windows(PROBE.len()).any(|w| w == PROBE.as_bytes())
                };
            if !carries {
                return unsafe { System.realloc(ptr, layout, new_size) };
            }
            // `System.realloc` may free the old block, and once it has, the
            // probe bytes still in it are in the free list and out of reach --
            // there is no moment at which this method could wipe them. So a
            // block that carries the probe is moved by hand instead: copy, then
            // release the old block through `dealloc`, which both scans it and
            // zeroes it. The copy is not free, which is why it is reached only
            // for a block that really does carry the probe.
            let new_layout =
                unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
            let moved = unsafe { System.alloc(new_layout) };
            if moved.is_null() {
                // Contract: a failed `realloc` leaves the old block untouched
                // and owned by the caller. Wiping it here would be a
                // use-after-... no, worse: a live-buffer corruption.
                return moved;
            }
            unsafe { std::ptr::copy_nonoverlapping(ptr, moved, layout.size().min(new_size)) };
            // The hit is recorded by `dealloc`, and only when a watch is armed.
            //
            // **This is now decided, where it used to be a coin toss.** The old
            // code flagged a probe-bearing `realloc` only when the allocator
            // happened to move the block, so the same growth read as a leak or
            // as clean depending on the heap. Taking the copying path whenever
            // the block carries the probe makes that reading deterministic. A
            // wipe-then-grow is unaffected: a wiped block does not carry the
            // probe, so `carries` is false and the fast path is taken.
            unsafe { self.dealloc(ptr, layout) };
            moved
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            if layout.size() >= PROBE.len() && any_watch_is_armed() {
                let block = unsafe { std::slice::from_raw_parts(ptr, layout.size()) };
                if block.windows(PROBE.len()).any(|w| w == PROBE.as_bytes()) {
                    record_a_hit();
                }
            }
            // **THE WIPE, AND IT IS UNCONDITIONAL ON PURPOSE.**
            //
            // The scan above is armed-gated; this is not, and it cannot be.
            // The bug it closes is that a block freed while NOTHING was armed
            // kept its probe bytes, went on the free list, and was handed to
            // some unrelated allocation on some unrelated thread which freed it
            // again inside a later armed window. `layout.size()` covers the
            // whole block and the new owner need not have written all of it, so
            // the old bytes met the scan and were credited to a test whose own
            // body was clean. Measured at 3 failures in 70 full-suite runs.
            //
            // Gating this on "a probe test is running" would leave the same hole
            // one step further out: a block freed before a probe test arms, or
            // after its hold is released at thread end, is still a block with
            // probe bytes in the free list. The only gate that closes it is no
            // gate. The cost is a `memset` per free in a test build, which is
            // the one build this allocator exists in.
            //
            // A genuine leak is untouched: the scan runs first, so the bytes are
            // read and reported before they are erased.
            unsafe { std::ptr::write_bytes(ptr, 0, layout.size()) };
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    #[global_allocator]
    static WATCHER: Watcher = Watcher;

    /// Whether the scan should run at all. **Not this thread's flag.** The
    /// freeing thread is not necessarily the arming one, and while this asked
    /// only `WATCHING` the whole instrument was deaf to every other thread.
    ///
    /// Neither allocates, so it cannot re-enter the allocator.
    fn any_watch_is_armed() -> bool {
        ANY_WATCH.load(Ordering::Relaxed) > 0
    }

    /// Records a probe-bearing block going back to the allocator, on both
    /// channels: the thread-local one the existing assertions read, and the
    /// process-global one that is the only channel a leak on another thread
    /// can appear on.
    fn record_a_hit() {
        if WATCHING.with(Cell::get) {
            SEEN.with(|seen| seen.set(true));
        }
        // **THE SECOND CHANNEL, AND IT IS UNFILTERED ON PURPOSE.**
        //
        // Zeroing freed blocks stops a recycled block carrying *stale* probe
        // bytes into a later window. It does nothing about probe bytes that are
        // genuinely alive and owned by a thread with no connection to the armed
        // test -- and this crate used to manufacture those by the dozen.
        //
        // **The measured culprit, caught by instrumenting the hit rather than
        // guessed at.** This file used to declare [`PROBE`] as a string literal,
        // so THE SOURCE TEXT OF THIS FILE CONTAINED THE PROBE. Around thirty
        // tests in this crate read `src/` back and lint it;
        // `job_object::tests::only_the_files_that_must_leave_the_job_can_open_a_bare_command`
        // was the one caught in the act, freeing a 284,896-byte block whose
        // contents were this very module around the `const` declaration. The
        // global scan had no way to tell those bytes from an armed test's own.
        //
        // That was answered TWICE, and only one of the two answers was a fix.
        // [`PROBE`] is assembled by `concat!`, so no file in this tree contains
        // it and a source reader allocates no probe at all -- pinned by
        // `no_source_file_in_this_crate_contains_the_assembled_probe`, over
        // every file this repository tracks rather than over a hand-kept list
        // of names. On top of that, this store used to be
        // filtered to "threads the window can speak for": the arming thread, and
        // threads whose first visit to this allocator postdated the window.
        //
        // **That filter was a blind spot, and it is deleted.**
        // `std::thread::spawn`'s child touches this allocator during its own
        // runtime start-up, so a worker spawned before the window opened is
        // stamped pre-window even when its user code allocates nothing until
        // afterwards. A genuine, un-wiped leak on such a worker answered
        // `false` -- clean, while blind -- with no panic, which is this
        // codebase's signature failure committed by the instrument built to
        // catch it. And the crate's own house rule that fixtures are built
        // before the measured window arms manufactures exactly that shape:
        // `vault_bridge`'s `mockito::Server::new()` and `breach::spawn_check`
        // both put a live worker behind the window that measures them.
        // `rv_a_leak_on_a_thread_that_predates_the_window_is_still_this_windows_leak`
        // is that leak, made deterministic.
        //
        // What is left holding the noise out is the `concat!` root fix, the
        // unconditional wipe below in `dealloc` (stale free-list bytes), the
        // hand-copy path in `realloc` (the one door the wipe is not behind),
        // and [`PROBE_LOCK`] held to thread end (no second probe test runs
        // beside this one at all). None of those is a filter on the verdict:
        // each removes a producer of bytes that were never ours. A filter on
        // the verdict cannot distinguish "not ours" from "ours, elsewhere",
        // which is why this one silently discarded the second.
        SEEN_ANYWHERE.store(true, Ordering::Relaxed);
    }

    /// Takes [`PROBE_LOCK`] into [`PROBE_HOLD`] for the rest of this thread, if
    /// this thread is not already holding it.
    ///
    /// **Called by [`armed`], and separately callable on purpose.** A test that
    /// allocates or frees probe plaintext BEFORE its first arm is not covered by
    /// the hold, and such frees are not harmlessly invisible: if another probe
    /// test is armed on another thread at that instant, the global scan sees
    /// those frees and reports them as that window's leak. Measured, at one
    /// failure in 45 full-suite runs, landing on the negative control of
    /// `a_leak_on_a_worker_thread_is_seen_by_the_cross_thread_watch_and_never_reported_clean`.
    ///
    /// So the house rule -- every probe test arms its positive control first --
    /// has an escape hatch for the one test that cannot follow it, rather than
    /// an exception.
    fn hold_the_probe_lock() {
        // `PROBE_LOCK` is poisoned by any probe test that panics deliberately
        // -- `an_unwind_does_not_release_the_master_password_in_the_clear`
        // does exactly that -- and a poisoned lock here would turn one
        // deliberate panic into a cascade of unrelated failures. The data is
        // `()`; there is no invariant to have been broken.
        //
        // Taken into `PROBE_HOLD` rather than a local, so it outlives the
        // caller and covers the rest of the test around it; see `PROBE_HOLD`.
        // The `borrow_mut` is dropped before returning, so a nested arm on this
        // thread re-borrows cleanly and finds the hold already there.
        PROBE_HOLD.with(|held| {
            let mut held = held.borrow_mut();
            if held.is_none() {
                let guard = PROBE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                *held = Some(ProbeHold(guard));
            }
        });
    }

    /// Arms the watch for `body` and clears both verdicts, holding
    /// [`PROBE_LOCK`] for the whole armed window. Returns the guard so the
    /// caller disarms after reading whichever verdict it wants.
    fn armed<R>(body: impl FnOnce() -> R) -> R {
        // Before anything else, and for the rest of this thread: no second
        // probe test may run beside this one, window or not.
        hold_the_probe_lock();
        SEEN.with(|seen| seen.set(false));
        SEEN_ANYWHERE.store(false, Ordering::Relaxed);

        /// Disarming in `Drop`, not after the call: [`ANY_WATCH`] is global,
        /// so a `body` that unwinds past a plain decrement would leave the
        /// scan armed for the rest of the process -- every free on every
        /// thread scanned, and the next probe's verdict decided by whatever
        /// else was running. One of the tests here panics on purpose.
        ///
        /// The field is [`WATCHING`] as this arm found it, **restored** rather
        /// than cleared. A nested arm used to set it to `false` on the inner
        /// disarm, which left the ENCLOSING window unable to record on its own
        /// arming thread: [`SEEN`] is gated on `WATCHING`, so the outer window
        /// went deaf for the rest of its body while still reporting a verdict.
        /// Nothing in this tree nests today; this is what stops the first thing
        /// that does from silently reading clean. Pinned by
        /// `rv_an_arm_nested_inside_another_leaves_the_enclosing_window_hearing`.
        struct Disarm(bool);
        impl Drop for Disarm {
            fn drop(&mut self) {
                ANY_WATCH.fetch_sub(1, Ordering::Relaxed);
                WATCHING.with(|watching| watching.set(self.0));
            }
        }

        let previous = WATCHING.with(Cell::get);
        WATCHING.with(|watching| watching.set(true));
        ANY_WATCH.fetch_add(1, Ordering::Relaxed);
        let _disarm = Disarm(previous);
        body()
    }

    /// Runs `body` with the watch armed and answers whether the probe string
    /// went past the allocator in the clear **on this thread**.
    ///
    /// **This is the thread-local verdict, and callers depend on it being
    /// that.** A `!plaintext_reached_the_allocator(..)` assertion is a claim
    /// about the calling thread. Every `Drop`, `zeroize` and take this crate
    /// asserts on happens there, which is why the reading is the useful one --
    /// but a body that spawns a worker and leaks inside it would once have
    /// read clean, silently.
    ///
    /// It no longer can. The scan is global (see [`ANY_WATCH`]) and the global
    /// channel is unfiltered (see [`record_a_hit`]), so a probe-bearing block
    /// freed on ANY thread while this window is open is *observed*; this
    /// function panics rather than returning `false` when the global channel
    /// saw something this thread did not. A body that means to leak elsewhere
    /// must say so by calling
    /// [`plaintext_reached_the_allocator_on_any_thread`] instead.
    ///
    /// # What this does and does not cover
    ///
    /// "Any thread" is now literal, and it did not used to be. The global
    /// channel was filtered to the arming thread plus threads whose first visit
    /// to the allocator postdated the window, on the stated reason that a
    /// pre-existing thread "is not doing the work the window is measuring".
    /// That reason was wrong: `std::thread::spawn`'s child allocates during
    /// runtime start-up, so a worker spawned just before the window was stamped
    /// pre-window even when every byte it touched was inside. A genuine leak on
    /// one of those answered `false`, silently -- see
    /// `rv_a_leak_on_a_thread_that_predates_the_window_is_still_this_windows_leak`,
    /// which is that leak made deterministic, and which the filter fails.
    ///
    /// The remaining boundary is time, not thread identity: a free that happens
    /// after this window closes is not this window's, so `body` must join
    /// whatever it spawns. Bytes that are not ours are kept out at their source
    /// rather than by a verdict filter -- [`PROBE`] is assembled by `concat!` so
    /// no file in this tree contains it, `dealloc` wipes unconditionally and
    /// `realloc` routes probe-bearing blocks through it so no free-list entry
    /// carries the probe, and [`PROBE_LOCK`] is held to thread end so no second
    /// probe test runs beside this one.
    ///
    /// **What still has to be watched** is the crate's house rule that fixtures
    /// are built before the measured window arms: probe plaintext allocated or
    /// freed before this thread's FIRST arm is outside [`PROBE_LOCK`]'s hold and
    /// can be credited to a probe test armed on another thread. The escape
    /// hatch is [`hold_the_probe_lock`], called directly.
    pub(crate) fn plaintext_reached_the_allocator(body: impl FnOnce()) -> bool {
        // Both verdicts are read INSIDE the armed window: `SEEN_ANYWHERE` is
        // process-global and the next probe to arm clears it, so reading it
        // after `armed` returns would be racing the lock this function just
        // released.
        let (here, anywhere) = armed(|| {
            body();
            (SEEN.with(Cell::get), SEEN_ANYWHERE.load(Ordering::Relaxed))
        });
        assert!(
            here || !anywhere,
            "the probe was freed in the clear on a DIFFERENT thread from the one that armed \
             the watch. This function's verdict is about the calling thread only, so it would \
             have answered `false` -- clean, while blind. If the body is meant to leak on a \
             worker, assert with `plaintext_reached_the_allocator_on_any_thread`."
        );
        here
    }

    /// Runs `body` with the watch armed and answers whether the probe string
    /// went past the allocator in the clear **on any thread of this process**.
    ///
    /// The verdict [`plaintext_reached_the_allocator`] cannot give: it reads
    /// the process-global channel, so a worker spawned by `body` is covered.
    /// `body` must join whatever it spawns -- a thread still running when the
    /// watch disarms is outside the window and is not covered by this or by
    /// anything else.
    ///
    /// **"Any thread" is literal**: the arming thread, workers the body
    /// spawned, and threads that were already running when the window opened.
    /// It used to exclude the last of those, which silently hid a genuine leak
    /// on any worker built before the window -- the shape every fixture in this
    /// crate leaves behind. See the section on
    /// [`plaintext_reached_the_allocator`], and
    /// `rv_a_leak_on_a_thread_that_predates_the_window_is_still_this_windows_leak`.
    pub(crate) fn plaintext_reached_the_allocator_on_any_thread(body: impl FnOnce()) -> bool {
        armed(|| {
            body();
            SEEN_ANYWHERE.load(Ordering::Relaxed)
        })
    }

    /// **The axis this instrument was blind on, now demonstrated rather than
    /// asserted.**
    ///
    /// The watch was armed per-thread and checked per-thread, so a probe-
    /// bearing block freed on a worker the body spawned went past the
    /// allocator with the verdict reading `false`: clean, while blind. Several
    /// fill paths in this crate spawn workers, so this was not hypothetical.
    ///
    /// Three readings, and it is the combination that is the claim:
    ///
    /// 1. Its own control, run FIRST (see the note in the body): a worker
    ///    that wipes instead of leaking is not reported -- so reading 2 is a
    ///    leak and not a function that answers `true` to everything.
    /// 2. The cross-thread verdict SEES a leak on a thread it did not arm.
    /// 3. The thread-local verdict does not silently answer `false` to
    ///    reading 2's body: it panics, naming the channel it cannot see on.
    ///    That is what stops the twenty existing call sites -- which all read
    ///    the thread-local verdict -- from ever being quietly blind.
    #[test]
    fn a_leak_on_a_worker_thread_is_seen_by_the_cross_thread_watch_and_never_reported_clean() {
        // 1. THE CONTROL RUNS FIRST, and the order is the whole of a fix.
        //
        //    This assertion is a negative -- a worker that WIPES must not be
        //    reported -- and when it ran second it fired roughly one run in
        //    eight. For a suite used as a mutation oracle that is far too
        //    high, and the cause is not a race between the two readings; it
        //    is the free list.
        //
        //    `Watcher::dealloc` scans the WHOLE block being freed for the
        //    probe bytes. The leak reading below is meant to leave those
        //    bytes in a block that goes back to the allocator un-wiped --
        //    that is the thing it is testing -- and nothing overwrites them
        //    afterwards. With the control second, any unrelated allocation
        //    handed that recycled block and freed inside the control's armed
        //    window carried the old probe bytes past the scan, and the
        //    control read a leak its own body had not produced.
        //    Intermittent exactly as an allocator's reuse is intermittent,
        //    and always in the safe direction, which is how it survived.
        //
        //    Running the negative before anything in this test has leaked
        //    removes the one source of stale probe bytes that is certain to
        //    be there. It cannot remove bytes left by an earlier test in the
        //    process -- `PROBE_LOCK` serialises the probe tests but not the
        //    allocator's memory -- so a residual rate is possible.
        assert!(
            !plaintext_reached_the_allocator_on_any_thread(|| {
                let wiped = zeroize::Zeroizing::new(probe_password());
                std::thread::spawn(move || drop(wiped)).join().expect("the worker ran");
            }),
            "control: the cross-thread watch reports a leak for a worker that wipes, so its \
             `true` below means nothing"
        );

        // 2. Seen.
        assert!(
            plaintext_reached_the_allocator_on_any_thread(|| {
                let leaked = probe_password();
                std::thread::spawn(move || drop(leaked)).join().expect("the worker ran");
            }),
            "a plaintext probe was freed on a worker thread and the cross-thread watch did not \
             see it -- the watch is still per-thread and every fill path that spawns is \
             unobserved"
        );

        // 3. The thread-local verdict refuses to answer `false` to the same
        //    body. Before, it answered `false` and the caller believed it.
        let blind = std::panic::catch_unwind(|| {
            plaintext_reached_the_allocator(|| {
                let leaked = probe_password();
                std::thread::spawn(move || drop(leaked)).join().expect("the worker ran");
            })
        });
        // The payload is a `&'static str`, because the assertion message is a
        // literal with nothing interpolated into it. Both shapes are accepted
        // so that adding a `{}` to that message does not turn this control
        // into a second, confusing failure.
        let payload = blind.expect_err(
            "the thread-local verdict answered instead of panicking, so it is back to \
             reporting clean about a thread it cannot see",
        );
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .expect("the panic carries its explanation");
        assert!(
            message.contains("DIFFERENT thread"),
            "it panicked for some other reason: {message}"
        );

        // 4. And the ordinary same-thread reading still works afterwards --
                //    3 must not have left the watch armed, the lock held, or the global
        //    flag set.
        let bare = probe_password();
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "the instrument did not recover from an unwind out of an armed window"
        );
    }

    /// **The armed windows were serialised; the tests around them were not.**
    ///
    /// That gap was a live flake, not a hypothetical: with
    /// `vault_cache::tests::a_custom_field_value_does_not_reach_the_allocator_in_the_clear`
    /// run beside `a_custom_field_name_is_still_a_plain_string` at
    /// `--test-threads=2`, the value test failed 20 times out of 20 on the
    /// cross-thread cross-check, because the name test frees a [`PROBE`]-
    /// bearing `String` outside any window it arms and the scan is global.
    ///
    /// The fix is that [`PROBE_LOCK`] is now held from a thread's first arm
    /// until the thread ends, so no second probe test runs *at all* -- window
    /// or not -- while one is in flight. This pins that, and it is the only
    /// assertion that can: the exclusion is invisible to a single-threaded
    /// reading of the verdicts, so nothing else in this module would notice
    /// the hold going back to a window-scoped local.
    ///
    /// Its control is the `false` before the first arm, which says the slot is
    /// genuinely empty to start with -- without it, a `this_thread_holds_the_
    /// probe_lock` wired to `true` would satisfy the claim below saying
    /// nothing. It is read on this thread's own slot rather than by racing
    /// another thread for the lock, because a third probe test holding it
    /// would make a `try_lock` control fail for the wrong reason.
    #[test]
    fn the_probe_lock_is_held_past_the_armed_window_so_whole_tests_are_serialised() {
        assert!(
            !this_thread_holds_the_probe_lock(),
            "control: this thread is holding the probe lock before it has armed anything, so \
             the assertion below cannot distinguish a retained hold from a stuck one"
        );

        let bare = probe_password();
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "the probe is deaf, so nothing here means anything"
        );

        assert!(
            this_thread_holds_the_probe_lock(),
            "the hold was released when the armed window closed. Only the windows are \
             serialised again, so this test's own fixtures and drops can be attributed to \
             whatever probe is armed on another thread -- which is the flake this pins"
        );
    }

    /// **The flake this instrument shipped with, with the timing taken out.**
    ///
    /// A reviewer measured 3 failures in 70 consecutive full-suite runs on a
    /// pristine tree -- 4.3% -- spread over this module and
    /// `vault_cache::tests::a_custom_field_value_does_not_reach_the_allocator_in_the_clear`.
    /// Always a false POSITIVE, never a false negative, which is precisely why
    /// it mattered: this suite is used as a mutation oracle, so a spurious
    /// failure corrupts KILLED verdicts, and a reviewer had already mistaken one
    /// for a real kill.
    ///
    /// The mechanism is the free list, not a race between the readings.
    /// [`Watcher::dealloc`] scans the WHOLE block, the arming check is a
    /// process-global counter, and a freed block keeps its bytes. So probe
    /// plaintext freed while nothing was armed sat in the free list until some
    /// unrelated allocation on some unrelated thread was handed that block and
    /// freed it again inside a later armed window -- and the scan, which cannot
    /// tell whose bytes those are, credited them to whoever was armed.
    ///
    /// The previous attempt at this reordered one test's readings so its
    /// negative control ran before it had leaked anything. That removes the
    /// stale bytes *that one test* produces and nothing else: not the ten other
    /// files that arm this probe, and not `vault_cache`, which was never
    /// touched. [`PROBE_LOCK`] serialises the probe tests; it does not serialise
    /// them against the allocator's memory.
    ///
    /// This test is that mechanism made deterministic. Reading 2 frees probe
    /// plaintext with nothing armed; reading 3 then asks for blocks of exactly
    /// that size inside an armed window and frees them **having written nothing
    /// into them**. Against the un-wiped instrument the window reports a leak it
    /// did not produce. It cannot now, because `dealloc` zeroes every block
    /// before handing it back, so no free list entry carries probe bytes at all.
    ///
    /// Reading 1 is the control, and it is the one that stops this being passed
    /// by a probe that has gone blind: a probe genuinely freed inside a window,
    /// after the wipe exists, is still reported.
    #[test]
    fn a_block_recycled_after_an_unarmed_free_cannot_be_read_as_a_leak_in_a_later_window() {
        // The probe plus slack, so the stale bytes have somewhere to sit that a
        // short new occupant will not overwrite -- which is the shape of the
        // real thing, where the recycled block belongs to an unrelated type.
        let size = PROBE.len() * 2;

        // 1. Control: a probe genuinely freed inside a window is still seen.
        let live = probe_password();
        assert!(
            plaintext_reached_the_allocator(move || drop(live)),
            "control: the probe is deaf, so the negative below is satisfied by \
             nothing at all"
        );

        // 2. The stale free, deliberately OUTSIDE any armed window. Nothing
        //    here is a leak this test claims to have found; it is the garbage
        //    every other probe test in the crate also leaves behind.
        let mut stale = String::with_capacity(size);
        stale.push_str(PROBE);
        drop(stale);

        // 3. Blocks of that size, allocated and freed inside a window, written
        //    to by nobody. Which block the allocator hands back is its own
        //    business, so this asks repeatedly rather than once.
        let leaked = plaintext_reached_the_allocator(|| {
            for _ in 0..64 {
                let recycled: Vec<u8> = Vec::with_capacity(size);
                drop(recycled);
            }
        });
        assert!(
            !leaked,
            "a block the armed window never wrote the probe into was reported as a leak, so \
             it carried those bytes in from an earlier free. Every probe verdict in this \
             crate can then be produced by an unrelated test's garbage, and this suite is \
             the mutation oracle"
        );
    }

    /// **The blind spot the participation filter bought its quiet with.**
    ///
    /// The global channel used to record a hit only from the arming thread or a
    /// thread whose FIRST visit to this allocator postdated the window. That
    /// reads as "the arming thread and the workers the body spawned", and it is
    /// not what it measured: `std::thread::spawn`'s child touches the allocator
    /// during its own runtime start-up, so a worker spawned a microsecond before
    /// the window was stamped pre-window forever -- even if its user code
    /// allocated nothing at all until the window was open.
    ///
    /// A genuine, un-wiped leak on such a worker therefore answered `false` from
    /// BOTH entry points, with no panic. Clean, while blind, from the instrument
    /// this crate built to catch exactly that -- and reached by following the
    /// crate's own house rule that fixtures are built before the measured window
    /// arms. `vault_bridge`'s `mockito::Server::new()` and `breach::spawn_check`
    /// are two live workers of that shape.
    ///
    /// This is that leak, made deterministic, in both of its forms: plaintext
    /// allocated before the window and freed inside it, and plaintext allocated
    /// AND freed inside the window on a thread that merely existed beforehand.
    /// The filter answered `false` to both; it is deleted, and this is what
    /// stops it coming back.
    ///
    /// Reading 1 is the control that stops this passing on a probe wired to
    /// `true`: the same pre-existing worker, WIPING, must still read clean.
    #[test]
    fn rv_a_leak_on_a_thread_that_predates_the_window_is_still_this_windows_leak() {
        use std::sync::mpsc::{channel, Receiver, Sender};

        // Before anything: this test frees probe plaintext on workers outside
        // the windows it arms, and it must not do that while another probe test
        // is armed on another thread. See `hold_the_probe_lock`.
        hold_the_probe_lock();

        /// A worker started and handshaked BEFORE any window opens, which does
        /// what `job` says when told to, inside whatever window is open then.
        /// The handshake is the premise rather than a nicety: until `ready`
        /// arrives the worker may not have touched the allocator at all, and a
        /// worker whose first allocation lands inside the window was never in
        /// the blind spot.
        fn outsider(
            job: impl FnOnce() + Send + 'static,
        ) -> (Sender<()>, Receiver<()>, std::thread::JoinHandle<()>) {
            let (ready_tx, ready_rx) = channel::<()>();
            let (go_tx, go_rx) = channel::<()>();
            let (done_tx, done_rx) = channel::<()>();
            let handle = std::thread::spawn(move || {
                // Touches the allocator, on this thread, before the handshake:
                // whatever stamping the instrument does, it is done by now.
                let warm = String::from("warm this thread up");
                ready_tx.send(()).expect("the test is listening");
                drop(warm);
                go_rx.recv().expect("the window opened");
                job();
                done_tx.send(()).expect("the test is listening");
            });
            ready_rx.recv().expect("the outsider reached the allocator");
            (go_tx, done_rx, handle)
        }

        // 1. Control: the same pre-existing worker, wiping. A `true` here would
        //    mean the readings below are a probe that answers `true` to
        //    everything rather than an instrument.
        let wiped = zeroize::Zeroizing::new(probe_password());
        let (go, done, joiner) = outsider(move || drop(wiped));
        assert!(
            !plaintext_reached_the_allocator_on_any_thread(|| {
                go.send(()).expect("the outsider is listening");
                done.recv().expect("the outsider dropped its secret");
            }),
            "control: a pre-existing worker that WIPES is reported as a leak, so the readings \
             below are satisfied by an instrument that says `true` to anything"
        );
        joiner.join().expect("the outsider ran");

        // 2. Plaintext allocated before the window, freed inside it. This is
        //    the reading the deleted filter turned into a silent `false`.
        let held = probe_password();
        let (go, done, joiner) = outsider(move || drop(held));
        assert!(
            plaintext_reached_the_allocator_on_any_thread(|| {
                go.send(()).expect("the outsider is listening");
                done.recv().expect("the outsider freed its probe");
            }),
            "a worker that existed before this window opened freed the plaintext probe INSIDE \
             it, un-wiped, and the window reported clean. Every fixture this crate builds \
             before arming -- `mockito::Server::new()`, `breach::spawn_check` -- leaves a \
             worker of exactly that age behind the window that measures it"
        );
        joiner.join().expect("the outsider ran");

        // 3. And the shape that makes the filter's stated reason wrong rather
        //    than merely narrow: the worker predates the window, but everything
        //    it allocates and frees happens INSIDE the window. By the filter's
        //    own justification -- such a thread "is by construction not doing
        //    the work the window is measuring" -- this thread IS doing that
        //    work, and it was still discarded, because the stamp was set by the
        //    runtime's own start-up allocation and by no work of the body.
        let (go, done, joiner) = outsider(|| drop(probe_password()));
        assert!(
            plaintext_reached_the_allocator_on_any_thread(|| {
                go.send(()).expect("the outsider is listening");
                done.recv().expect("the outsider freed its probe");
            }),
            "a worker allocated AND freed the plaintext probe entirely inside this window and \
             the window reported clean, because the worker happened to be spawned a moment \
             before the window opened. Thread age is not a proxy for whose work this is"
        );
        joiner.join().expect("the outsider ran");

        // 4. The thread-local entry point does not answer `false` to reading 2
        //    either: it panics, naming the channel it cannot see on.
        let held = probe_password();
        let (go, done, joiner) = outsider(move || drop(held));
        let blind = std::panic::catch_unwind(AssertUnwindSafe(|| {
            plaintext_reached_the_allocator(|| {
                go.send(()).expect("the outsider is listening");
                done.recv().expect("the outsider freed its probe");
            })
        }));
        let payload = blind.expect_err(
            "the thread-local verdict answered instead of panicking about a leak it cannot \
             see, so its callers are back to believing a `false` from a blind channel",
        );
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .expect("the panic carries its explanation");
        assert!(
            message.contains("DIFFERENT thread"),
            "it panicked for some other reason: {message}"
        );
        joiner.join().expect("the outsider ran");
    }

    /// **A nested arm must not leave the enclosing window deaf.**
    ///
    /// [`SEEN`] is gated on [`WATCHING`], and `Disarm` used to clear that flag
    /// outright rather than restore it. So an inner arm ending inside an outer
    /// one left `WATCHING` false on the arming thread for the whole remainder of
    /// the outer body: the outer window went on to return a verdict, and that
    /// verdict was `false` no matter what the body did afterwards.
    ///
    /// Nothing in this tree nests today, which is exactly why this needs a test
    /// rather than a comment -- deleting the restore costs nothing observable
    /// otherwise.
    ///
    /// Its control is the leak BEFORE the nested arm, which the outer window
    /// must also hear; without it a `plaintext_reached_the_allocator` wired to
    /// `true` would satisfy the claim.
    #[test]
    fn rv_an_arm_nested_inside_another_leaves_the_enclosing_window_hearing() {
        let before = probe_password();
        let after = probe_password();
        let inner = probe_password();

        let mut heard_before = false;
        let mut heard_after = false;
        // The enclosing window is the cross-thread entry point on purpose: the
        // thread-local one cross-checks its two channels and would panic here
        // for a second, unrelated reason, hiding what is being measured. What
        // this test reads is [`SEEN`] itself, directly, which is the channel
        // `WATCHING` gates and therefore the one a nested disarm silences.
        let _ = plaintext_reached_the_allocator_on_any_thread(|| {
            // The control: the enclosing window hears a leak that happens
            // before anything nests.
            drop(before);
            heard_before = SEEN.with(Cell::get);

            // A nested arm, opened and closed entirely inside the outer body.
            let nested = plaintext_reached_the_allocator(move || drop(inner));
            assert!(nested, "the nested window did not hear its own leak");

            // The nested window's own hit is still sitting in `SEEN` -- it is
            // one thread-local shared by both arms -- so it is cleared here.
            // Without this the reading below is satisfied by the INNER
            // window's leak and says nothing about the enclosing one, which is
            // a vacuity this test was measured to have before it was fixed.
            SEEN.with(|seen| seen.set(false));

            drop(after);
            heard_after = SEEN.with(Cell::get);
        });
        assert!(
            heard_before,
            "control: the enclosing window did not hear a leak that happened before anything \
             nested, so the assertion below is about an instrument that was already deaf"
        );
        assert!(
            heard_after,
            "an arm nested inside this one closed and took the enclosing window's hearing with \
             it: `Disarm` cleared `WATCHING` instead of restoring what it found, and `SEEN` is \
             gated on `WATCHING` -- so every free on the arming thread after the inner window \
             closed was invisible, and the enclosing verdict was `false` by construction"
        );
    }

    /// **The channel `dealloc`'s unconditional wipe never covered.**
    ///
    /// A `String` grown past its capacity does not free its old buffer through
    /// [`Watcher::dealloc`]. It calls `realloc`, and `System.realloc` releases
    /// the old block inside the CRT -- so the wipe that
    /// `a_block_recycled_after_an_unarmed_free_cannot_be_read_as_a_leak_in_a_later_window`
    /// pins is never reached, and the block lands in the free list with its
    /// probe bytes intact, exactly as if the wipe did not exist.
    ///
    /// [`Watcher::realloc`] has a hand-copy path that routes such a block back
    /// through `dealloc` -- but it was gated on a counter that is zero outside a
    /// probe test's own thread, on the premise that no other test ever allocates
    /// the probe. That premise was false: while [`PROBE`] was a string literal
    /// in this file, every one of the ~30 tests that reads `src/` back grew a
    /// `String` over probe-bearing text with the gate reading zero.
    ///
    /// Both halves of that are fixed -- the gate is gone and the probe is
    /// assembled by `concat!` -- and this pins the half that is structural. It
    /// fails deterministically with the gate restored.
    ///
    /// Reading 2 is the control that stops a blind probe passing reading 3.
    #[test]
    fn a_block_released_by_realloc_growth_while_unarmed_cannot_be_read_as_a_leak_later() {
        // The probe plus slack, so the stale bytes have room a short new
        // occupant will not overwrite. Every block seeded at 1 is exactly this
        // size, which is what lets 3 ask for the same size and be handed one.
        let size = PROBE.len() * 2;

        // **THE SEEDING IS UNDER THE LOCK BUT OUTSIDE ANY WINDOW.**
        //
        // Without this line the seeding below frees sixty-four probe-bearing
        // blocks while this thread holds nothing, and if libtest started this
        // test's thread inside ANOTHER probe test's armed window then this
        // thread is one that window can speak for -- so those frees are
        // reported as that test's leak. That is this crate's signature flake,
        // and this test reintroduced it: measured at 1 failure in 45 full-suite
        // runs, landing on the negative control of
        // `a_leak_on_a_worker_thread_is_seen_by_the_cross_thread_watch_and_never_reported_clean`.
        //
        // Taking the hold without arming keeps reading 1's premise intact: no
        // window is open, which is the state the gate this test pins reads as
        // "skip the scan".
        hold_the_probe_lock();
        // **AND THIS IS WHAT PINS THE LINE ABOVE.** The defect that line closes
        // is visible only as a rate -- deleting it costs zero tests and returns
        // a 1-in-45 failure somewhere else entirely, which is not a thing a
        // suite can catch by running. The state it establishes, though, is
        // directly observable, so it is asserted here rather than trusted.
        assert!(
            this_thread_holds_the_probe_lock(),
            "the seeding below is about to free sixty-four probe-bearing blocks with this \
             thread holding nothing. If another probe test is armed on another thread right \
             now, the global scan reports every one of them as ITS leak -- this crate's \
             signature flake, measured at 1 failure in 45 full-suite runs when this test \
             reintroduced it. Call `hold_the_probe_lock()` first"
        );

        // 1. THE SEEDING RUNS FIRST, AND THE ORDER IS THE WHOLE OF THE REPRO.
        //
        //    The gate this pins skips the scan when no window is armed, so the
        //    growth below must happen before the control at 2 opens one -- which
        //    is also exactly the state the ~30 source-linting tests grow in.
        //
        //    All the buffers are held live and grown afterwards, so that the
        //    block each `reserve` releases is not immediately handed back to the
        //    next iteration and overwritten. Sixty-four of them, because which
        //    block the allocator recycles at 3 is its own business.
        let mut seeds: Vec<String> = (0..64)
            .map(|_| {
                let mut seed = String::with_capacity(size);
                seed.push_str(PROBE);
                assert_eq!(seed.capacity(), size, "the growth below must reallocate, not fit");
                seed
            })
            .collect();
        for seed in &mut seeds {
            // Past the capacity, so this reallocates rather than fitting: the
            // old `size` block goes back to the allocator through `realloc`,
            // which is the one door `dealloc`'s unconditional wipe is not behind.
            seed.reserve(size * 4);
            assert!(seed.capacity() > size, "`reserve` did not grow the buffer");
        }
        drop(seeds);

        // 2. Control: a probe genuinely freed inside a window is still seen.
        let live = probe_password();
        assert!(
            plaintext_reached_the_allocator(move || drop(live)),
            "control: the probe is deaf, so the negative below is satisfied by nothing at all"
        );

        // 3. Blocks of that size, allocated and freed inside a window, written
        //    to by nobody.
        let leaked = plaintext_reached_the_allocator(|| {
            for _ in 0..64 {
                let recycled: Vec<u8> = Vec::with_capacity(size);
                drop(recycled);
            }
        });
        assert!(
            !leaked,
            "a block the armed window never wrote the probe into was reported as a leak, so it \
             carried those bytes in from an earlier `realloc`. The wipe in `dealloc` does not \
             reach a block `System.realloc` released, and every probe verdict in this crate can \
             then be produced by an unrelated test's growth"
        );
    }

    /// **No filter on the verdict could have fixed this, which is why the fix
    /// is at the source.**
    ///
    /// The misattribution this instrument was flaking on was a source-linting
    /// test reading this crate's own text back while some probe test was armed.
    /// The filter written for it admitted any thread born at or after the window
    /// opened -- and libtest at `-j 8` starts test threads continuously, so a
    /// source reader that began inside an armed window was admitted anyway. It
    /// had to be: a thread born inside the window is exactly what "a worker the
    /// body spawned" looks like, so no widening or narrowing of that filter
    /// separates the two. (The narrowing it did buy is what made it discard
    /// genuine leaks; it is gone -- see [`record_a_hit`].)
    ///
    /// So it is fixed where it starts. [`PROBE`] is assembled by `concat!`, so
    /// no file in this tree contains it and reading one allocates no probe at
    /// all. This is that, made deterministic, and it is now the whole of what
    /// keeps a source reader quiet: the reader thread is born inside the window
    /// and reads the very file the probe is declared in, with nothing between
    /// its 284,896-byte block and the verdict but the fact that those bytes are
    /// not the probe.
    ///
    /// Reading 1 is the control: a worker born inside the window that really
    /// does leak must still be reported, so reading 2's `false` is the source
    /// text being clean and not the channel being shut.
    #[test]
    fn a_thread_born_inside_the_window_reading_this_crates_source_is_not_that_windows_leak() {
        // 1. Control: a participant's genuine leak is still seen.
        assert!(
            plaintext_reached_the_allocator_on_any_thread(|| {
                let leaked = probe_password();
                std::thread::spawn(move || drop(leaked)).join().expect("the worker ran");
            }),
            "control: a leak on a worker the body itself spawned is not reported, so the global \
             channel is shut and the negative below is satisfied by blindness"
        );

        // 2. The stand-in for every source-linting test in this crate, born
        //    inside the window, so nothing about thread identity is doing the
        //    work here: the 284,896-byte block simply does not carry the probe.
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/login_ui.rs");
        let leaked = plaintext_reached_the_allocator_on_any_thread(move || {
            std::thread::spawn(move || {
                let text = std::fs::read_to_string(&source).expect("this crate's own source");
                // The premise, not decoration: a read that found nothing would
                // satisfy the assertion below saying nothing at all.
                assert!(
                    text.contains("fn probe_password"),
                    "the file read is not this module's source"
                );
                drop(text);
            })
            .join()
            .expect("the reader ran");
        });
        assert!(
            !leaked,
            "a thread that did nothing but read this crate's own source was reported as this \
             window's leak. The source text still contains the assembled probe -- someone \
             re-joined the `concat!` fragments -- and every probe test in the crate is again a \
             coin toss on what libtest schedules beside it"
        );
    }

    /// Whether `dir` is a **repository other than this one**, decided by
    /// shape rather than by name. Used by both paths.
    ///
    /// Three shapes, each a producer of bytes that were never ours:
    ///
    /// * `dir/.git` is a DIRECTORY **containing a `HEAD` file** -- a nested
    ///   full clone. The old test caught only the worktree case: it skipped
    ///   the nested clone's `.git` by name and then walked every working
    ///   file underneath it, which is precisely the another-checkout false
    ///   positive the `git ls-files` bound was introduced to remove, left
    ///   open in fallback mode.
    /// * `dir/.git` is a FILE beginning `gitdir:` -- a sibling worktree, or
    ///   a submodule.
    /// * `dir` is itself a git directory (`HEAD` + `objects/` + `refs/`).
    ///   That is a bare repository, or a `.git` directory reached under
    ///   any name. This is load-bearing on the GIT path too, not only in
    ///   the fallback: measured, `git ls-files --others` collapses a
    ///   nested *clone* to one directory entry, but enumerates a nested
    ///   *bare* repository file by file, hooks and all.
    ///
    /// **What is gone from this list is `CACHEDIR.TAG`, because it is both
    /// unreliable as a marker of build output and forgeable as a switch.**
    ///
    /// Unreliable: cargo writes that tag only when it creates the target
    /// directory *itself*. Measured on cargo 1.96.0, `mkdir X;
    /// CARGO_TARGET_DIR=X cargo build` leaves **no** tag while
    /// `CARGO_TARGET_DIR=Y cargo build` with `Y` absent writes one, and
    /// `CARGO_TARGET_DIR=<pre-existing> cargo test --no-run` produced 18
    /// probe-bearing artifacts with no tag above any of them. A
    /// CI-cache-restored `target/`, a `cp -r`'d build directory and
    /// anything predating cargo's tag support are the same. Copying one of
    /// those 18 into `<export>/buildout/debug/` FAILED this scan in
    /// **both** configurations, reporting this tree's own source as
    /// carrying the probe and naming an `.exe`. The claim that the tag
    /// skipped build output "under any name" was simply false.
    ///
    /// Forgeable, and far worse: with the probe sitting in a **tracked**
    /// `deskwarden/src/vault_window/sidebar.rs`, an empty
    /// `deskwarden/src/vault_window/CACHEDIR.TAG` -- one file, no content
    /// -- turned the fallback scan from FAILED to `ok`, whole subtree
    /// gone, no control firing. A planted `.git` did the same. It failed
    /// to hide `src/` itself only because `login_ui.rs` lives there and
    /// the "the walk reached this file" control caught it: an accident of
    /// layout, not a guard. A directory-level opt-out that any one empty
    /// file can assert is not a filter, it is a switch, and what it
    /// switches off is the instrument.
    ///
    /// So build output is no longer excluded by a directory marker at all;
    /// see [`walk`] for the two paths the fallback still declines to
    /// descend and why they are named rather than sniffed.
    ///
    /// **And `.git` existing is no longer the predicate.** It was, and that
    /// made `mkdir docs/scratch/.git` -- an EMPTY directory, not a
    /// repository -- a one-command opt-out on the GIT path: measured, a
    /// probe-bearing untracked `docs/scratch/notes.md` FAILED this scan,
    /// and with that empty marker beside it the suite went full green, no
    /// control firing, while `git ls-files --others --exclude-standard`
    /// went on listing the file. `touch .git` did the same. So a marker
    /// must now be a REAL repository: a `.git` directory that holds a
    /// `HEAD`, a `.git` file that actually says `gitdir:`, or the bare
    /// layout. An empty `.git` is nothing, the directory is walked, and the
    /// probe in it is reported.
    ///
    /// And a real one is no longer a skip on either path. On the git path
    /// as in the fallback it is RECORDED and the scan fails naming it --
    /// see [`tracked_files`] and [`walk`]. A tracked file is this
    /// repository's by definition and passes through no directory filter
    /// whatsoever; an untracked one under another repository cannot be
    /// attributed, so the scan says so instead of dropping it in silence.
    fn nested_repository_shape(dir: &std::path::Path) -> bool {
        let dot_git = dir.join(".git");
        dot_git.join("HEAD").is_file()
            || std::fs::read(&dot_git).is_ok_and(|b| b.starts_with(b"gitdir:"))
            || (dir.join("HEAD").is_file()
                && dir.join("objects").is_dir()
                && dir.join("refs").is_dir())
    }


    /// The files this repository OWNS, as absolute paths, or `None` if
    /// `git` could not be run or reported failure.
    ///
    /// **Tracked is not the same as owned.** `git ls-files` is index-based
    /// -- a staged-but-uncommitted file IS listed, verified -- but a file
    /// written and not yet `git add`ed is not, and write-then-compile-
    /// before-`add` is the loop this repository is being developed in. So
    /// the list is the union of the tracked files with the untracked ones
    /// that are present and not gitignored.
    ///
    /// **`refused` is an output, and it is why this signature changed.** The
    /// previous round hardened the FALLBACK against a planted repository
    /// marker -- refused, then a hard failure -- and left this path with the
    /// silent-skip semantics that hardening existed to remove. The hole was
    /// exactly here, on untracked entries, which is the class the `--others`
    /// union was added for: measured on the git path with the tree otherwise
    /// pristine, a probe-bearing untracked `docs/scratch/notes.md` FAILED
    /// naming the file, and `mkdir docs/scratch/.git` turned that into a
    /// full green with no control firing. Reproduced identically at
    /// `deskwarden/src/scratch/notes_untracked.rs`, where nothing else in
    /// the suite would have caught it. So an untracked entry under another
    /// repository is no longer dropped: the repository is recorded here and
    /// the scan fails naming it, the same as in the fallback. The other half
    /// of that fix is in [`nested_repository_shape`], which no longer counts
    /// an empty `.git` as a repository at all.
    /// Searches `path` for any of `needles` **without ever holding the whole
    /// file in memory**, returning the label of the first needle found.
    ///
    /// The read loop used to be `std::fs::read(file)`, which is a whole-file
    /// allocation. That was harmless while the listing was a directory walk
    /// that excluded build output, and it stopped being harmless when the
    /// listing became git's: a file FORCE-ADDED under `target/` is tracked,
    /// and tracked files pass through no filter at all, so a multi-gigabyte
    /// artifact is read whole and then window-scanned three times. That is an
    /// undisclosed cost of making the listing git-only, and it is paid here
    /// rather than bought back with a size-based skip -- a skip would be a new
    /// exclusion predicate, which is the exact shape six rounds were spent
    /// deleting from this test.
    ///
    /// The chunk carries `max_needle_len - 1` bytes of the previous chunk
    /// forward, so a needle straddling a chunk boundary is still found; the
    /// only splitting that hides the probe is the one `concat!` performs in
    /// the source, which is the remedy this test recommends.
    ///
    /// I/O errors are RETURNED, not swallowed. See the caller.
    ///
    /// **And so is the number of bytes actually consumed**, which is the whole
    /// reason the return type is a pair. The round before this one closed the
    /// silent-*error* return and then asserted `scanned == files.len()` over
    /// the caller's loop -- but `scanned` counted LOOP TRIPS, not evidence, so
    /// a silent-*success* return from the top of this function
    /// (`if path.ends_with("probe_fixture.bin") { return Ok(None); }`, one
    /// line) left `unreadable` empty, `scanned` at its full value, and BOTH
    /// new equalities holding while a real committed probe went unseen.
    /// Measured on an export: SURVIVED in debug and in release.
    ///
    /// A counter the caller derives from this function's control flow is a
    /// counter this function can satisfy without doing any work. A byte total
    /// is not: the caller checks it against `std::fs::metadata(..).len()`, an
    /// independent syscall this function never makes, and an early return
    /// therefore shows up as MISSING BYTES on a named file. Fabricating it
    /// means writing the true length in here -- a `metadata` call of its own,
    /// not a bare `return` -- which is no longer a silent success.
    ///
    /// On a HIT the count is short by design; the caller asserts the byte
    /// equality only after it has already asserted there was no hit.
    fn scan_for_needles(
        path: &std::path::Path,
        needles: &[(&'static str, &[u8])],
    ) -> std::io::Result<(u64, Option<&'static str>)> {
        use std::io::Read as _;

        const CHUNK: usize = 1 << 20;
        let overlap = needles
            .iter()
            .map(|(_, n)| n.len())
            .max()
            .unwrap_or(0)
            .saturating_sub(1);
        let mut handle = std::fs::File::open(path)?;
        let mut window: Vec<u8> = Vec::with_capacity(overlap + CHUNK);
        let mut chunk = vec![0u8; CHUNK];
        let mut consumed: u64 = 0;
        loop {
            let read = handle.read(&mut chunk)?;
            if read == 0 {
                return Ok((consumed, None));
            }
            consumed += read as u64;
            window.extend_from_slice(&chunk[..read]);
            if let Some((label, _)) = needles
                .iter()
                .find(|(_, n)| !n.is_empty() && window.windows(n.len()).any(|w| w == *n))
            {
                return Ok((consumed, Some(label)));
            }
            let keep = window.len().min(overlap);
            let drop_to = window.len() - keep;
            window.drain(..drop_to);
        }
    }

    fn tracked_files(
        root: &std::path::Path,
        refused: &mut Vec<std::path::PathBuf>,
    ) -> Option<Vec<std::path::PathBuf>> {
        // **Two producers, two `Command`s, not one builder called twice.**
        // `fn list(root, extra)` was a single `Command` builder shared by both
        // questions, so a PATHSPEC appended inside it reached the tracked list
        // AND the untracked list in ONE edit -- the two "independent"
        // enumerations here were one. They are written out separately so that
        // corrupting both costs two edits at two call sites, which is what the
        // doc above claims they cost.
        let tracked = {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(["ls-files", "-z"])
                .output()
                .ok()?;
            out.status.success().then(|| {
                out.stdout
                    .split(|b| *b == 0)
                    .filter(|s| !s.is_empty())
                    .map(<[u8]>::to_vec)
                    .collect::<Vec<Vec<u8>>>()
            })?
        };
        let untracked = {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(["ls-files", "-z", "--others", "--exclude-standard"])
                .output();
            out.ok()
                .filter(|o| o.status.success())
                .map(|o| {
                    o.stdout
                        .split(|b| *b == 0)
                        .filter(|s| !s.is_empty())
                        .map(<[u8]>::to_vec)
                        .collect::<Vec<Vec<u8>>>()
                })?
            // **Not `unwrap_or_default()`.** That read a FAILED `ls-files
            // --others` as "there is nothing untracked here", which is the
            // third instance of the same smell as the `ls-tree` degradation
            // below and the unchecked `check-ignore` in the scan: a producer
            // that could not answer, silently reported as one that answered
            // "none". It was covered -- `git status --porcelain -uall` is the
            // untracked half's second producer and its subset assert would
            // have fired -- but coverage by a sibling is not a reason to
            // accept a silent failure, and the tracked half beside it has
            // always failed loudly on the identical condition.
        };
        // **The INDEX MODE, asked directly -- because the on-disk marker the
        // shape predicate needs can be DELETED, and deleting it costs zero
        // source edits.**
        //
        // The round before this one routed committed gitlinks through
        // `nested_repository_shape` below, and that predicate asks the WORKING
        // TREE: it wants a `.git` directory holding `HEAD`, a `.git` file
        // saying `gitdir:`, or the bare layout. Commit the gitlink, then
        // `rm -rf vendor/nested/.git`, and the shape is gone while the index
        // entry stays. Measured on an export, with a real probe in
        // `vendor/nested/lib.rs` and NO source edit of any kind:
        // `git status --porcelain` empty, `git status --porcelain -uall --
        // vendor` empty, `git ls-files -s vendor/nested` still
        // `160000 commit <sha> vendor/nested` -- and the probe test came back
        // `ok. 1 passed` filtered in debug, with the probe unseen (the
        // previous round's report has the same mutant SURVIVING the full
        // suite at 2253/0 in both profiles; only the filtered debug run was
        // re-measured here, because the filtered run is the verdict and the
        // full suite is the number). The liveness pair at the identical path,
        // tracked as an ordinary file, was KILLED by the main hit assert. `is_dir()` was true, `nested_repository_shape` was now
        // false, so the entry fell through to `if !path.is_file() { continue; }`
        // -- the silent drop the previous round existed to remove. That is
        // STRICTLY CHEAPER than the hole that round closed: same zero edits,
        // same clean status, and now invisible to `refused` as well.
        //
        // So the refusal no longer depends on anything in the working tree.
        // A gitlink is a MODE in the index -- `160000` -- and no amount of
        // deleting, creating or renaming files under the path can change what
        // git recorded there. `git ls-files -s -z` is asked for it directly
        // and every `160000` entry is refused UNCONDITIONALLY. Un-asserting
        // this needs `git rm --cached <path>` or a re-commit, which is an
        // index change and therefore a diff.
        //
        // **Two producers, for the same reason everything else here has two.**
        // `ls-files -s` speaks for the INDEX, so it also catches a gitlink
        // staged and not yet committed; `ls-tree -r -z HEAD` (without
        // `--name-only`, which is what threw the mode away at the existing
        // call site) speaks for the COMMITTED tree. Both are written out here
        // with their own arguments, so a pathspec on one leaves the other
        // answering. Neither is filtered by `is_file`, which is precisely how
        // all three of the old producers dropped this entry independently.
        // Measured: `:!vendor` on the `ls-files -s` invocation alone is
        // KILLED by `ls-tree`; a positive pathspec on the `ls-tree`
        // invocation alone (it will not take an exclude pathspec) is KILLED
        // by `ls-files -s`. One edit is loud either way; it takes two.
        //
        // **This WIDENS the submodule trade, and the widening is deliberate.**
        // The previous round documented that an INITIALISED submodule
        // hard-fails this suite, and kept the trade because this repository
        // has none. Keying on the index mode makes that "ANY submodule",
        // initialised or not: an unchecked-out gitlink has no working-tree
        // shape at all, and refusing it is the point -- a gitlink with an
        // empty directory today is a probe-bearing checkout after one `git
        // submodule update`, and the index entry is the thing that is
        // unattributable either way. Measured: with `vendor/nested` REMOVED
        // from disk entirely and only the `160000` entry left, this still
        // refuses. If this repository ever takes a real submodule, this
        // assert is the thing to revisit, deliberately and in a diff -- not
        // something to soften by re-introducing a working-tree predicate,
        // which is exactly what was just defeated at zero edits.
        let mut gitlinked: Vec<std::path::PathBuf> = Vec::new();
        {
            // `<mode> SP <object> SP <stage> TAB <path>`, and `-z` means the
            // path is verbatim: no quoting, no escaping, nothing to unwrap.
            let staged = std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(["ls-files", "-s", "-z"])
                .output()
                .ok()?;
            if !staged.status.success() {
                return None;
            }
            // `<mode> SP <type> SP <object> TAB <path>`. `-r` does NOT recurse
            // into a commit entry -- it cannot, the objects are not here -- so
            // a gitlink is reported at its own path, at any depth.
            let committed = std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(["ls-tree", "-r", "-z", "HEAD"])
                .output()
                .ok()?;
            // **A nonzero exit is not an answer of "no gitlinks".** This used
            // to degrade to `&[]`, which is the same shape as the
            // `check-ignore` hole below: a producer that fails is read as a
            // producer that found nothing, and the second opinion silently
            // becomes one opinion. `ls-files -s` above covers this case, so
            // this was never a hole on its own -- but a second opinion is not
            // a reason to accept a silent failure, and the two producers are
            // supposed to fail independently, not to cover for each other in
            // silence. `None` is what the sibling `staged` producer already
            // returns and it is loud at BOTH call sites: the probe scan
            // panics on it (`the_probe_scan_requires_git`) and the fixture
            // test `expect`s it. It cannot false-red an empty repository with
            // no `HEAD` that the scan would otherwise accept, because that
            // scan already panics on its own `ls-tree -r HEAD` producer.
            if !committed.status.success() {
                return None;
            }
            let committed_records: &[u8] = &committed.stdout;
            for record in staged
                .stdout
                .split(|b| *b == 0)
                .filter(|s| !s.is_empty())
                .filter(|record| record.starts_with(b"160000 "))
                .chain(
                    committed_records
                        .split(|b| *b == 0)
                        .filter(|s| !s.is_empty())
                        .filter(|record| record.starts_with(b"160000 commit ")),
                )
            {
                let Some(tab) = record.iter().position(|b| *b == b'\t') else {
                    continue;
                };
                // Lossy on purpose: this path is only ever REPORTED, never
                // opened, so a non-UTF-8 gitlink name still produces a loud
                // refusal naming a mangled path rather than a silent skip.
                let path = root.join(String::from_utf8_lossy(&record[tab + 1..]).as_ref());
                if !gitlinked.iter().any(|g| *g == path) {
                    gitlinked.push(path);
                }
            }
        }
        for path in gitlinked {
            if !refused.iter().any(|r| *r == path) {
                refused.push(path);
            }
        }

        let mut undecodable: Vec<String> = Vec::new();
        let mut listed: Vec<std::path::PathBuf> = Vec::new();
        let entries = tracked
            .iter()
            .map(|raw| (raw, true))
            .chain(untracked.iter().map(|raw| (raw, false)));
        for (raw, is_tracked) in entries {
            // A non-UTF-8 tracked name used to go through
            // `from_utf8_lossy`, fail `is_file()` on the mangled path, and
            // be dropped in silence -- a guard that skips without saying
            // so. It is reported instead; see the assertion below.
            let Ok(rel) = std::str::from_utf8(raw) else {
                undecodable.push(String::from_utf8_lossy(raw).into_owned());
                continue;
            };
            let path = root.join(rel);
            // An untracked nested CLONE is listed by `git ls-files
            // --others` as one DIRECTORY entry rather than as its files, so
            // it never reached the ancestor test below and was dropped by
            // the `is_file` guard instead -- silently, with a whole
            // checkout's working tree out of scope. Measured: `git init` in
            // `docs/vendored/` with a probe-bearing `lib.rs` in it SURVIVED
            // on the git path while the fallback refused it. It is the same
            // hole as the planted marker, reached by a real repository
            // rather than a forged one, so it gets the same answer.
            // **Before `is_file`, and for TRACKED entries too -- this is the
            // gitlink hole, and it cost ZERO source edits.** A nested clone
            // COMMITTED as a gitlink (`160000 commit <sha> vendor/nested`) is
            // listed by `git ls-files` as one TRACKED entry, so the old
            // `!is_tracked` gate here sent it straight to the `is_file` guard
            // below, which dropped it in silence -- a whole probe-bearing
            // working tree out of scope with the source completely unmodified
            // and `git status` clean. The three-producer floor bought nothing:
            // `ls-tree -r HEAD` and `ls-files --cached` each apply their own
            // `.filter(|p| p.is_file())`, so all three producers dropped it
            // independently. Measured, and then measured again after this
            // change: refused, loudly.
            //
            // A gitlink IS a repository shape by definition -- the directory
            // on disk holds a `.git` (a FILE, `gitdir: ../.git/modules/...`,
            // for a submodule; a DIRECTORY for a plain nested clone) -- so
            // `nested_repository_shape` already recognises it and the only
            // thing that had to change was the order and the gate. A tracked
            // FILE is never `is_dir()`, so dropping the `!is_tracked` gate
            // changes nothing about ordinary tracked entries; a gitlink whose
            // directory is not checked out at all is not a repository shape
            // and falls through to the guard below, where there is genuinely
            // nothing on disk to read.
            //
            // **This is no longer what catches a gitlink, and it is kept
            // anyway.** It asks the working tree, and the working tree can be
            // edited to answer differently -- `rm -rf vendor/nested/.git`
            // after committing the gitlink took the shape away at zero source
            // edits and put the entry straight back into the silent drop
            // below; see the index-mode refusal above, which is what actually
            // refuses a gitlink now and which no file operation can reach.
            // What this line still does, and the index-mode check cannot, is
            // refuse an UNTRACKED nested clone: `--others` reports one of
            // those as a bare directory with no index entry at all. It is an
            // addition, not a replacement, in both directions.
            if path.is_dir() && nested_repository_shape(&path) {
                if !refused.iter().any(|r| *r == path) {
                    refused.push(path);
                }
                continue;
            }
            // A tracked path can be absent from the working tree (deleted
            // but not yet staged), and an untracked directory that is not a
            // repository -- an empty one, or one git collapsed for another
            // reason -- is not a file to scan and not this test's business
            // to complain about.
            if !path.is_file() {
                continue;
            }
            // Tracked content is this repository's by definition, and git
            // has already decided that -- so no PLANTED MARKER beside or
            // above a tracked FILE can take it out of scope: the ANCESTOR
            // repository-shape test below runs on untracked entries only,
            // and the entry-level test above cannot fire on a tracked file
            // because a tracked file is not a directory. An untracked path
            // has had only `.gitignore` applied to it, which does not know
            // about a nested bare repository, so it gets the same
            // repository-shape test the fallback walk uses -- and, as
            // there, the answer is to REFUSE and say so, not to drop the
            // file in silence.
            //
            // This paragraph used to describe a second filter here, the
            // literal build output one. It is gone. A tracked path now passes
            // through NO directory filter at all, and an untracked one
            // through the repository-shape test only. Build output is out of
            // scope because it is gitignored and therefore untracked, so
            // `--others --exclude-standard` never offers it -- there is no
            // list of directory names in this file left to widen. A file
            // FORCE-ADDED under `target/` is tracked and IS listed and
            // scanned; that is the loud direction, and it is the intended
            // one.
            //
            // The gitlink case that used to be disclosed here and NOT fixed
            // -- a nested clone committed as `160000 commit <sha> path`,
            // dropped by the `is_file` guard with its whole working tree out
            // of scope at zero source edits -- is fixed above: the
            // repository-shape refusal now runs BEFORE `is_file` and on
            // tracked entries too. It is the cheapest hole this test ever
            // had and it left no diff at all.
            if !is_tracked {
                if let Some(nested) = path
                    .ancestors()
                    .skip(1)
                    .take_while(|a| *a != root)
                    .find(|a| nested_repository_shape(a))
                {
                    if !refused.iter().any(|r| r == nested) {
                        refused.push(nested.to_path_buf());
                    }
                    continue;
                }
            }
            listed.push(path);
        }
        assert!(
            undecodable.is_empty(),
            "control: {} listed path(s) are not UTF-8, so they were never opened and the \
             probe could be sitting in one of them unread. This is a silent skip in a \
             guard, reported rather than swallowed. First: {:?}",
            undecodable.len(),
            undecodable.first()
        );
        // An empty listing means this is not a checkout of anything, which
        // is the fallback's case and not a pass.
        (!listed.is_empty()).then_some(listed)
    }

    /// **The pin on [`nested_repository_shape`], which nothing else can
    /// reach.**
    ///
    /// The shapes that predicate refuses only ever occur in someone's working
    /// directory: a nested clone, a sibling worktree, a bare repository. None
    /// of them exists in a clean checkout, so on a green tree every clause can
    /// be deleted with the whole suite still passing -- which is how the
    /// nested-full-clone gap survived the round that introduced the worktree
    /// skip, and how the `CACHEDIR.TAG` opt-out survived the round after it.
    /// Both gaps were found by hand, in an export, and would have been found
    /// by hand again next time.
    ///
    /// So the shapes are BUILT here, in a temporary directory, and three
    /// separate claims are measured over them:
    ///
    /// * the three repository shapes are recognised, and an ordinary
    ///   directory is not -- the control, without which a predicate that
    ///   refuses everything satisfies the other three;
    /// * a `CACHEDIR.TAG` **hides nothing**. This is the C2 mutation in
    ///   miniature: a probe-bearing source file with a tag planted in its own
    ///   directory is still walked to. On the old predicate this file was
    ///   invisible;
    /// * an EMPTY `.git`, of either kind, hides nothing -- the C1 mutation
    ///   in miniature. Cargo build output needs no clause here at all: it is
    ///   gitignored, so git never offers it and no predicate of ours decides
    ///   anything about it;
    /// * a nested repository is REFUSED rather than skipped -- it comes back
    ///   in `refused`, which the scan turns into a failure. A `.git` planted
    ///   inside `src/` therefore reds the suite instead of deleting a
    ///   subtree, in the configuration that ships.
    #[test]
    fn the_nested_repository_shape_predicate_recognises_repositories_and_nothing_else() {
        // This test writes the assembled probe to disk and reads it back, and
        // it never arms. Same reason as the scan above.
        hold_the_probe_lock();

        /// Removes the directory on the way out, however this test leaves.
        struct Scratch(std::path::PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        let root = std::env::temp_dir().join(format!(
            "deskwarden-shape-pin-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _scratch = Scratch(root.clone());
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("the scratch directory is creatable");

        /// `dir/rel`, with the probe in it and every parent created.
        fn plant(root: &std::path::Path, rel: &str) -> std::path::PathBuf {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("the plant has a parent"))
                .expect("the parent is creatable");
            std::fs::write(&path, PROBE.as_bytes()).expect("the plant is writable");
            path
        }

        // A nested full clone: `.git` is a DIRECTORY, and it holds a `HEAD`.
        std::fs::create_dir_all(root.join("nested_clone/.git")).expect("creatable");
        std::fs::write(root.join("nested_clone/.git/HEAD"), b"ref: refs/heads/main")
            .expect("writable");
        plant(&root, "nested_clone/src/lib.rs");
        // A sibling worktree: `.git` is a FILE.
        std::fs::create_dir_all(root.join("nested_worktree")).expect("creatable");
        std::fs::write(root.join("nested_worktree/.git"), b"gitdir: ../elsewhere")
            .expect("writable");
        plant(&root, "nested_worktree/src/lib.rs");
        // A bare repository: no `.git` at all -- it IS the git directory.
        // `git ls-files --others` enumerates one of these file by file.
        std::fs::create_dir_all(root.join("nested_bare/objects")).expect("creatable");
        std::fs::create_dir_all(root.join("nested_bare/refs")).expect("creatable");
        std::fs::write(root.join("nested_bare/HEAD"), b"ref: refs/heads/main").expect("writable");
        plant(&root, "nested_bare/description");

        // A Cargo build output under a name that is not `target`, WITH the
        // tag -- and the tag no longer takes it out of the walk. What keeps
        // the artifact from being reported is that it is not source text; the
        // `.d` file beside it is text and is scanned.
        std::fs::create_dir_all(root.join("buildout")).expect("creatable");
        let tag = root.join("buildout/CACHEDIR.TAG");
        std::fs::write(&tag, b"Signature: 8a477f597d28d172").expect("writable");
        let artifact = root.join("buildout/deskwarden.exe");
        let mut bytes = b"MZ\x90\x00\xff\xfe\xfd".to_vec();
        bytes.extend_from_slice(PROBE.as_bytes());
        std::fs::write(&artifact, &bytes).expect("writable");
        let depfile = root.join("buildout/deskwarden.d");
        std::fs::write(&depfile, b"deskwarden.exe: src/main.rs").expect("writable");

        // The C2 mutation in miniature: a probe-bearing source file with a
        // tag planted in its own directory. Under the old predicate this
        // whole directory left the scan.
        let _ = plant(&root, "src/vault_window/sidebar.rs");
        std::fs::write(root.join("src/vault_window/CACHEDIR.TAG"), b"Signature: 8a477f597d28d172")
            .expect("writable");

        // The C1 mutation in miniature, and the reason `.exists()` is gone:
        // an EMPTY `.git` directory is not a repository. On the old
        // predicate `mkdir docs/scratch/.git` -- one command, no content --
        // deleted this file from the scan on the GIT path with the suite
        // full green. It must be walked to.
        let _ = plant(&root, "docs/scratch/notes.md");
        std::fs::create_dir_all(root.join("docs/scratch/.git")).expect("creatable");
        // And the same forgery in its file form: `touch .git` also passed
        // `.exists()`. A worktree's `.git` says `gitdir:`; this one does not.
        let _ = plant(&root, "docs/touched/notes.md");
        std::fs::write(root.join("docs/touched/.git"), b"").expect("writable");

        // `root`'s OWN git directory, in the bare layout the third clause
        // matches. It is not a NESTED repository, it is this one, and
        // refusing it made the shipping configuration red on every real
        // checkout.
        std::fs::create_dir_all(root.join(".git/objects")).expect("creatable");
        std::fs::create_dir_all(root.join(".git/refs")).expect("creatable");
        std::fs::write(root.join(".git/HEAD"), b"ref: refs/heads/main").expect("writable");
        std::fs::write(root.join(".git/config"), b"[core]").expect("writable");

        // Cargo's build directory under its real name: the fallback's one
        // directory-level exclusion, and the only reason it exists is that
        // an artifact of this crate carries the assembled probe by
        // construction while `.gitignore` -- which the fallback cannot read
        // -- already takes it out on the git path.
        let _ = plant(&root, "target/debug/deskwarden.exe");
        // And the SECOND literal, which is the one a workspace member's
        // artifacts land in and which nothing here reached before: the
        // predicate is a two-term disjunction and the second term was free
        // to delete. Neither of these may appear in `found` below.
        let _ = plant(&root, "deskwarden/target/debug/deskwarden.exe");

        // And one ordinary file, which is the control: a walk that refuses
        // everything satisfies the assertions above and measures nothing.
        let _ = plant(&root, "docs/notes.md");
        std::fs::create_dir_all(root.join("docs/nested")).expect("creatable");
        let _ = plant(&root, "docs/nested/more.md");

        for shape in ["nested_clone", "nested_worktree", "nested_bare"] {
            assert!(
                nested_repository_shape(&root.join(shape)),
                "`{shape}` is another repository and the predicate did not recognise it. \
                 Whatever is inside it -- another checkout's `login_ui.rs` at a commit \
                 predating the `concat!` split -- is then read back and reported as a \
                 violation of THIS tree"
            );
        }
        for kept in [
            "docs",
            "docs/nested",
            "buildout",
            "src/vault_window",
            // The two forgeries. `mkdir .git` and `touch .git` both passed
            // the old `.exists()` predicate, and each was a one-command,
            // silent opt-out for the directory it sat in -- measured on the
            // GIT path, which is the one that ships on a checkout.
            "docs/scratch",
            "docs/touched",
        ] {
            assert!(
                !nested_repository_shape(&root.join(kept)),
                "control: `{kept}` is not a repository and the predicate said it was. The \
                 assertions above are then satisfied by a predicate that refuses everything, \
                 and the fallback walk measures nothing. `docs/scratch` holds an EMPTY `.git` \
                 directory and `docs/touched` an EMPTY `.git` file: the day `.exists()` comes \
                 back, either one deletes its directory from the scan, on both paths, with \
                 the suite green. `buildout` and `src/vault_window` both carry a planted \
                 `CACHEDIR.TAG`: the day that tag comes back as a directory-level opt-out, \
                 one empty file deletes a subtree of `src/` from this scan again"
            );
        }
        assert!(
            !bytes.is_empty(),
            "control: the artifact fixture is empty, so nothing below is about build output"
        );
    }

    /// **The pin on [`tracked_files`]'s answer to "what does this repository
    /// own", built against a real throwaway repository rather than against
    /// this one.**
    ///
    /// Everything this function decides is invisible on a clean checkout: the
    /// untracked union has nothing untracked to add, the gitignore filter has
    /// nothing ignored to drop, and the ancestor shape filter has no nested
    /// repository to refuse. Delete any of the three and the suite stays
    /// green -- which is exactly the failure mode the round before this one
    /// shipped, where four of six reported kills turned out to be inert in
    /// the configuration every number was measured in.
    ///
    /// So a repository with all five shapes in it is CONSTRUCTED, and the
    /// listing is compared as a set. Five claims, and the ordinary tracked
    /// file is the control on all of them:
    ///
    /// * a committed file is listed;
    /// * a **staged but uncommitted** file is listed (`git ls-files` is
    ///   index-based, so this is a property of the index, not of `HEAD`);
    /// * an **untracked, not-ignored** file is listed -- this is the
    ///   write-then-compile-before-`git add` loop, and it was out of scope
    ///   until now;
    /// * a **gitignored** file is NOT listed;
    /// * an untracked path beside an EMPTY `.git` directory IS listed. That
    ///   was the C1 hole: the ancestor filter's whole predicate was
    ///   `.git.exists()`, so `mkdir .git` next to an untracked
    ///   probe-bearing file turned a measured FAILURE into a full green
    ///   with no control firing, on the GIT path, which is the one that
    ///   ships on a checkout;
    /// * an untracked path under a nested bare repository is NOT listed but
    ///   IS reported in `refused`, which the scan turns into a failure --
    ///   the previous round applied that hardening to the fallback only.
    ///   `git ls-files --others` enumerates that repository file by
    ///   file. An untracked build output under a name `.gitignore` does not
    ///   mention IS listed, and that is the change this round makes: it used
    ///   to be dropped on `CACHEDIR.TAG`, which cargo does not reliably write
    ///   and anyone can plant. It is dropped on its bytes instead, in the
    ///   scan, which searches its bytes like any other file's.
    ///
    /// If `git` cannot be run this test FAILS rather than returning green;
    /// see the assertion below.
    #[test]
    fn tracked_files_lists_what_this_repository_owns_and_nothing_else() {
        /// Removes the directory on the way out, however this test leaves.
        struct Scratch(std::path::PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        fn git(root: &std::path::Path, args: &[&str]) -> bool {
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .is_ok_and(|o| o.status.success())
        }

        let root = std::env::temp_dir().join(format!(
            "deskwarden-owns-pin-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _scratch = Scratch(root.clone());
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("the scratch directory is creatable");

        // **No silent return.** If `git` cannot be run here, every claim
        // below -- the untracked union, the gitignore boundary, the
        // nested-repository filter -- goes inert and this test reports `ok`
        // having measured nothing. That is the exact failure mode it was
        // written to fix, re-created one level up, so it is a failure.
        assert!(
            git(&root, &["init", "-q", "."]),
            "`git init` failed in a fresh temporary directory. Every assertion in this test is \
             about what `git ls-files` decides; without git it measures nothing at all, and \
             returning green for that is the silent pass this test exists to prevent. Run the \
             suite where git works."
        );

        let write = |rel: &str, body: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("has a parent")).expect("creatable");
            std::fs::write(&path, body).expect("writable");
            path
        };

        // **The fixture's `.gitignore` names `target/` UNANCHORED, because
        // this repository's does** (`.gitignore:3`), and every claim below
        // about force-adding depends on it. It used to name
        // `ignored_scratch/` and nothing else, which made the two `git add
        // -f` calls under `target/` INERT -- a plain `git add` behaved
        // identically -- so the fixture reproduced "a tracked file under
        // `target/`" while its comments described "`.gitignore` names both
        // `target/` directories but governs `--others` only". That is the
        // failure mode this module keeps deleting: a comment describing state
        // the fixture does not have. The line is here so the mechanism is
        // real, and it makes `docs/target/notes.rs` ignored too, which is the
        // production divergence itself rather than a story about it.
        std::fs::write(root.join(".gitignore"), "ignored_scratch/\ntarget/\n")
            .expect("writable");
        let committed = write("src/committed.rs", "// committed");
        let committed_two = write("docs/committed.md", "// committed, and not a `.rs`");
        assert!(
            git(&root, &["add", ".gitignore", "src/committed.rs", "docs/committed.md"]),
            "the add ran"
        );
        assert!(
            git(&root, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "base"]),
            "the commit ran"
        );
        let gitignore = root.join(".gitignore");
        let staged = write("src/staged.rs", "// staged, never committed");
        assert!(git(&root, &["add", "src/staged.rs"]), "the second add ran");
        // The one this round exists for.
        let untracked = write("src/scratch_untracked.txt", "// written, not yet added");
        let untracked_two = write("docs/scratch_untracked.md", "// likewise, outside `src/`");
        // Ignored, and therefore not ours to police.
        write("ignored_scratch/note.txt", "// ignored");
        // A nested BARE repository. `--others` enumerates it file by file.
        // Its files are not listed -- and, since this round, not dropped in
        // silence either: the repository comes back in `refused` and the
        // scan fails naming it.
        std::fs::create_dir_all(root.join("nested_bare/objects")).expect("creatable");
        std::fs::create_dir_all(root.join("nested_bare/refs")).expect("creatable");
        write("nested_bare/HEAD", "ref: refs/heads/main");
        write("nested_bare/objects/loose", "// not our bytes");
        // **The C1 finding, on the path it lived on.** An untracked file
        // beside an EMPTY `.git` directory. `git ls-files --others` lists
        // it; the old ancestor filter, whose whole predicate was
        // `.git.exists()`, dropped it -- so `mkdir .git` was a one-command,
        // silent opt-out for any untracked subtree, measured green. It is
        // listed.
        let untracked_three = write("src/scratch/notes_untracked.rs", "// no repository above me");
        std::fs::create_dir_all(root.join("src/scratch/.git")).expect("creatable");
        // A nested CLONE, which `git ls-files --others` collapses to a
        // single DIRECTORY entry rather than listing its files. It
        // therefore never reached the ancestor test and was dropped by the
        // `is_file` guard -- silently, a whole working tree out of scope.
        // Measured: `git init` in `docs/vendored/` with a probe-bearing
        // `lib.rs` in it SURVIVED on the git path while the fallback
        // refused it.
        // It has to be a REAL one: with a hand-built `.git` holding only a
        // `HEAD`, git does not collapse the directory, `--others` lists the
        // file, and the ancestor test catches it -- so the branch that
        // handles the collapsed form goes inert and deleting it is a
        // measured SURVIVED.
        write("vendored/lib.rs", "// another checkout's bytes");
        assert!(git(&root.join("vendored"), &["init", "-q", "."]), "the nested `git init` ran");
        assert!(git(&root.join("vendored"), &["add", "lib.rs"]), "the nested add ran");
        assert!(
            git(
                &root.join("vendored"),
                &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "nested"]
            ),
            "the nested commit ran"
        );
        // A build output under a name `.gitignore` does not name. It IS
        // listed -- `CACHEDIR.TAG` is no longer a directory-level opt-out,
        // because it is forgeable and cargo does not reliably write it --
        // and it is scanned, because the scan is a byte-window search with
        // no encoding rule left in it. The two `target/` directories
        // `.gitignore` DOES name never reach here at all.
        let tag = write("buildout/CACHEDIR.TAG", "Signature: 8a477f597d28d172");
        let rlib = write("buildout/deskwarden.rlib", "// listed and scanned");
        // **Build output that is TRACKED.** This is the claim the change it
        // pins shipped with zero assertions behind it. `.gitignore` governs
        // `--others`, not `git ls-files`, so a force-added file under either
        // `target/` is tracked and IS offered here -- and if it is not
        // filtered, the git path FAILS on bytes the fallback (the
        // release-tarball configuration, which `git archive` ships that same
        // file into) never even lists. The two configurations then return
        // OPPOSITE verdicts on identical bytes, with the silent one shipping.
        // Both literal paths are covered, because the predicate is a
        // two-term disjunction and one term is free to be deleted.
        let forced_root = write("target/forced.rs", "// force-added under the root `target/`");
        assert!(git(&root, &["add", "-f", "target/forced.rs"]), "the forced root add ran");
        let forced_crate =
            write("deskwarden/target/forced.rs", "// force-added under the crate `target/`");
        assert!(
            git(&root, &["add", "-f", "deskwarden/target/forced.rs"]),
            "the forced crate add ran"
        );
        // The control on both, and it is what keeps the two assertions above
        // from being satisfied by a filter that drops everything tracked, or
        // by one that matches the NAME `target` at any depth -- which would
        // make `mkdir target` the directory-level opt-out this module has
        // spent three rounds removing. A tracked sibling whose path merely
        // starts with the same letters stays listed.
        let kept_sibling = write("deskwarden/targeted/keep.rs", "// beside `target/`, not under it");
        assert!(
            git(&root, &["add", "-f", "deskwarden/targeted/keep.rs"]),
            "the sibling add ran"
        );
        // And the second control, which is the one that catches a NAME
        // match rather than the two literal paths. `docs/target/` is not
        // Cargo's build directory for anything and must stay in scope.
        //
        // It is FORCE-ADDED, and that is not decoration: the fixture's
        // `.gitignore` names `target/` unanchored, exactly as this
        // repository's does, so `docs/target/` is ignored and `--others
        // --exclude-standard` would never offer this path. Force-adding it
        // makes it TRACKED -- and a tracked path is this repository's by
        // git's own decision, so it is listed and scanned no matter what
        // directory it sits in. That is the whole of the build-output story
        // now: gitignored means untracked means never offered, and
        // force-added means tracked means scanned. There is no directory
        // list anywhere in this file to disagree with git about it.
        let kept_depth = write("docs/target/notes.rs", "// a directory named `target`, not one");
        assert!(git(&root, &["add", "-f", "docs/target/notes.rs"]), "the depth control add ran");

        // **The mechanism, MEASURED rather than described.** Three of the
        // comments above turn on "`.gitignore` names `target/` unanchored, so
        // these paths are ignored and `-f` is what makes them tracked". Until
        // this round the fixture's `.gitignore` said `ignored_scratch/` and
        // nothing else, every `-f` was inert, a plain `git add` behaved
        // identically, and the comments described a mechanism the fixture did
        // not have. Asking git settles it: if these stop being ignored, the
        // force-adds stop meaning anything and the comments go stale again,
        // silently. The sibling is the control -- `target/` must not swallow
        // `targeted/`, or the divergence being pinned is not a divergence.
        for ignored in ["target/forced.rs", "deskwarden/target/forced.rs", "docs/target/notes.rs"] {
            assert!(
                git(&root, &["check-ignore", "-q", "--no-index", ignored]),
                "the fixture's `.gitignore` does not ignore `{ignored}`, so `git add -f` on it \
                 is inert and this fixture reproduces `a tracked file under `target/`` rather \
                 than the production shape its comments describe: `.gitignore` names both \
                 build directories, governs `--others` only, and a force-added path under one \
                 is therefore TRACKED and offered to `tracked_files` anyway"
            );
        }
        assert!(
            !git(&root, &["check-ignore", "-q", "--no-index", "deskwarden/targeted/keep.rs"]),
            "control: the fixture's `.gitignore` swallows `deskwarden/targeted/`, so the \
             sibling control is ignored rather than merely un-excluded and it stops \
             distinguishing a path filter from a name match"
        );

        let mut refused: Vec<std::path::PathBuf> = Vec::new();
        let listed: std::collections::BTreeSet<std::path::PathBuf> =
            tracked_files(&root, &mut refused)
                .expect("the listing is not empty")
                .into_iter()
                .collect();
        let expected: std::collections::BTreeSet<std::path::PathBuf> = [
            gitignore,
            committed,
            committed_two,
            staged,
            untracked,
            untracked_two,
            untracked_three,
            tag,
            rlib,
            kept_sibling.clone(),
            kept_depth.clone(),
            forced_root.clone(),
            forced_crate.clone(),
        ]
        .into_iter()
        .collect();
        // **Force-added build output is TRACKED, and tracked is scanned.**
        // This assertion used to run the other way: `tracked_files` carried
        // two literal build-output paths and dropped these two, so that the
        // git listing would agree with a directory walk that could not read
        // `.gitignore`. The walk is gone and so are the literals, and with
        // them the last directory-name list in this file. What is left is
        // git's own answer and only that: `.gitignore` governs `--others`,
        // not `git ls-files`, so a force-added path under `target/` is
        // tracked, is listed, and is scanned.
        //
        // That is the LOUD direction, and it is the one worth having. The
        // failure it produces is "this crate's own artifact carries the probe
        // because compiling a `concat!` is what produces it" -- a false
        // positive that names a file and tells you to un-force-add it. The
        // direction it replaces was a SILENT one: a source file force-added
        // under `target/` was invisible to the scan in both configurations.
        for forced in [&forced_root, &forced_crate] {
            assert!(
                listed.contains(forced),
                "`tracked_files` did not list {forced:?}, a TRACKED file under Cargo's build \
                 directory. Nothing in this crate is entitled to drop a tracked path: git has \
                 already decided it is this repository's. A filter here that takes it out is a \
                 directory-name list coming back, and a directory-name list is the surface six \
                 rounds of this test were lost on. Gitignored build output never reaches this \
                 function at all, because `--others --exclude-standard` does not offer it."
            );
        }
        assert!(
            listed.contains(&kept_depth),
            "control: `tracked_files` dropped {kept_depth:?}, a tracked source file in a \
             directory that merely happens to be NAMED `target`. Nothing in this function \
             is entitled to drop a tracked path, and a name match would make `mkdir \
             target` beside any file the one-command directory opt-out this module spent \
             four rounds removing."
        );
        assert!(
            listed.contains(&kept_sibling),
            "control: `tracked_files` dropped {kept_sibling:?}, which is a tracked source \
             file beside `target/` and not under it. This is then satisfied by a filter \
             that drops tracked files wholesale, or by one matching the NAME `target` -- \
             and a name match makes `mkdir target` anywhere in the tree the one-command \
             directory opt-out this module removed."
        );
        assert_eq!(
            listed, expected,
            "`tracked_files` did not name exactly the files this repository owns. An entry              missing from the left is a file the probe scan no longer polices -- an untracked              source under `src/` is the one this round added. An extra entry is another              repository's or a build output's bytes being reported as this tree's, which is              the false positive the git bound was introduced to remove"
        );
        // And the nested repository is REFUSED, not dropped. This is the
        // half of the previous round's fix that was applied to the fallback
        // and not to the git path, which is where the C1 hole was.
        assert_eq!(
            refused,
            [root.join("nested_bare"), root.join("vendored")],
            "`tracked_files` did not hand back exactly the nested repositories whose untracked \
             files it declined to attribute. An empty list is the silent skip this round \
             removes -- the scan asserts this is empty and fails naming it, so a nested \
             repository dropped here is a subtree that leaves the scan with nothing said. \
             `vendored` missing is the nested-CLONE form of it, which git hands back as one \
             directory entry rather than as its files and which the `is_file` guard used to \
             swallow, measured SURVIVED on the git path. \
             `src/scratch` appearing is the other direction: an EMPTY `.git` is not a \
             repository and must not red the suite either"
        );

        // And the empty answer is `None`, not `Some(vec![])`. A directory that
        // is not a checkout of anything must send the scan to its FALLBACK
        // rather than hand it a listing of nothing to police. Measured: with
        // `(!listed.is_empty()).then_some(listed)` weakened to `Some(listed)`
        // the whole suite stayed green in both configurations, because every
        // other assertion here is about a listing that is not empty. The one
        // thing that would have caught it is the scan's file-count control,
        // and leaning on a control in another test to police this function's
        // contract is leaning on an accident.
        assert!(
            tracked_files(&root.join("ignored_scratch"), &mut Vec::new()).is_none(),
            "`tracked_files` answered `Some(..)` for a directory it listed nothing for. The \r
             scan takes that as the tree's file list and never reaches its fallback, so a \r
             tree it cannot enumerate is policed by scanning no files at all"
        );
    }

    /// **The pin on the fix above, stated where it can be checked directly.**
    ///
    /// The whole reason a source-linting test could produce a probe hit is that
    /// the probe's bytes were sitting in a file those tests read. `concat!`
    /// removes that, and nothing else in the module would notice it being put
    /// back: re-joining the fragments into one literal compiles, passes every
    /// other test here, and reintroduces an intermittent failure of whichever
    /// probe test libtest happens to schedule alongside a source reader.
    ///
    /// So the property is asserted over the **tree** rather than over this file,
    /// and over every file rather than a list -- a new file that pastes the
    /// probe in is caught the day it is added.
    ///
    /// **And "the tree" means the repository, not `src/`.** This walked `src/`
    /// and `*.rs` while its own doc claimed the tree, which is the narrower of
    /// the two readings dressed as the wider one -- the exact shape of claim
    /// this module exists to catch. `build.rs`, `examples/`, `installer/`,
    /// `docs/`, the `*.md` files, `Cargo.lock`, `.github/` and `.claude/` are
    /// all read by something at some point, and a probe pasted into any of them
    /// is a probe a test can allocate. So the scan covers the whole repository
    /// and reads BYTES rather than text, so that a binary -- the installers in
    /// `installer/` are two of them -- is scanned rather than skipped by a
    /// `read_to_string` that would fail on it.
    ///
    /// **And "the repository" means the files this repository OWNS, which is
    /// `git ls-files` and not `read_dir` from the root.** This used to walk the
    /// directory tree and skip three names -- `target/`, `.git/`,
    /// `.superpowers/` -- while explicitly refusing a blanket dot-directory
    /// skip on the grounds that it "would have taken `.github/` and `.claude/`
    /// with them, which are neither". `.claude/` is exactly the problem. On
    /// this user's checkout `.claude/worktrees/` holds two live sibling git
    /// worktrees belonging to other agents (beside two empty directories left
    /// by finished ones), parked on commits that
    /// predate the `concat!` split -- so the walk read another checkout's
    /// `login_ui.rs`, found the probe in it as a single literal, and failed
    /// this test 6 runs out of 6. Nothing was wrong with THIS tree.
    ///
    /// **And `/.claude/` in `.gitignore` is that skip, re-entering by another
    /// door -- say so rather than call it housekeeping.** The rule committed at
    /// `.gitignore:27` is a DIRECTORY-LEVEL OPT-OUT from this scan: it is the
    /// same shape -- "this name is not ours, do not look in it" -- that the six
    /// rounds above were spent deleting from *this file*, moved into a file
    /// git reads instead. It is not neutral because it is spelled in
    /// `.gitignore`; it is the exclusion predicate with a different owner.
    ///
    /// It is nevertheless defended, on three grounds that are checkable rather
    /// than rhetorical:
    ///
    /// * Nothing under `.claude/` is tracked, so it subtracts nothing from the
    ///   committed tree this test exists to police.
    /// * It governs the UNTRACKED enumeration only. A `git add -f` under
    ///   `.claude/` puts the file in the index, and all three producers above
    ///   -- `ls-files`, `ls-tree -r HEAD`, `ls-files --cached` -- then offer
    ///   it and it IS scanned. The opt-out cannot be used to hide a file this
    ///   repository actually owns.
    /// * It fixes a real red on a fresh clone rather than a hypothetical one:
    ///   agent tooling creates `.claude/worktrees/`, those are checkouts of
    ///   this repository parked on pre-`concat!` commits, and the scan printed
    ///   a path to what looks like a plaintext master password in a tree that
    ///   was not this one.
    ///
    /// The honest residual is stated once and not softened: an untracked probe
    /// under `.claude/` is invisible to this scan, at a cost of zero edits to
    /// this crate. Untracked is out of scope by construction here -- this test
    /// polices what the repository OWNS -- but `.claude/` is the one place
    /// where that scope boundary was drawn by a rule someone wrote for
    /// convenience rather than by git's own notion of ownership.
    ///
    /// A fourth skip name would have been the fourth widening of a filter, and
    /// this module's whole thesis is that a filter on the verdict cannot
    /// distinguish "not ours" from "ours, elsewhere". So the producer is
    /// removed instead: the list is what `git ls-files` says this repository
    /// tracks. Build output, `.git/` in any of its forms, gitignored scratch,
    /// and every nested worktree are then not skipped -- they were never in
    /// the list. Three further hazards close with it:
    ///
    /// * `read_dir(..).expect(..)` and `fs::read(..).expect(..)` panicked on
    ///   any locked or permission-denied file. Concurrent agents building
    ///   in-tree produce those.
    /// * An agent that points `CARGO_TARGET_DIR` at an in-repo directory NOT
    ///   literally named `target` planted this crate's own rlib -- which of
    ///   course carries the assembled probe, because that is what compiling a
    ///   `concat!` produces -- inside the walk. (On the git path only; see the
    ///   fallback section below, which is where that hazard actually lived.)
    /// * In a worktree `.git` is a **file**, not a directory, so the `is_dir`
    ///   skip did not fire on it and it was read rather than skipped.
    ///
    /// **There is no fallback, and that is this round's change.** If `git`
    /// cannot be run, or runs and reports failure, or lists nothing, this test
    /// PANICS naming itself rather than falling back to a directory walk. The
    /// walk is gone, and so are the four layers that existed to keep its
    /// build-output exclusion honest: `BUILD_OUTPUT_DIRS`, `build_output_dir`,
    /// the `.gitignore` reader and the two asserts over it.
    ///
    /// The reason is measured rather than argued. That exclusion list was the
    /// only surface this test ever lost on. Six rounds hardened it and six
    /// mutations defeated it, each one the previous round's defect a level up
    /// -- shared list, positional snapshot, `let`-bound closure, single
    /// `.gitignore` binding -- and the last of them (`X7`, three edits, no
    /// assert and no rendering touched) put a probe in a tracked directory and
    /// went UNSEEN in debug and in release. Every one of those holes lived in
    /// the GITLESS configuration, where nothing can second-guess a constant in
    /// this file; on the git path the tracked-file controls killed all of them.
    ///
    /// So the surface is deleted rather than defended. `git ls-files` reports
    /// exactly what is tracked, `--others --exclude-standard` adds what is
    /// present and not ignored, and build output -- gitignored by
    /// `.gitignore:2` and `:3` -- is in neither. There is no list of directory
    /// names left in this file to widen.
    ///
    /// **The new edit cost, measured rather than claimed.** There is no
    /// directory list, but there is still one lever: a `git` invocation takes
    /// arguments, and git argument syntax includes PATHSPECS. That lever is
    /// named here rather than left to be found, and it is priced at THREE
    /// coordinated edits because the file list is the union of three separately
    /// written invocations -- `ls-files` plus `--others`, `ls-tree -r HEAD`, and
    /// `ls-files --cached` -- each at its own call site, with a named
    /// cross-check between the first two.
    ///
    /// **Three call sites, but only TWO distinct questions**, and the earlier
    /// wording here ("three independent producers") overclaimed. `ls-files`
    /// defaults to `--cached`, so the first and third invocations ask git the
    /// same thing in two spellings; `ls-tree -r HEAD` asks a different one (the
    /// commit, not the index). What the three-edit floor rests on is that a
    /// pathspec is written per `Command`, so the same question asked twice
    /// still has to be corrupted twice -- the floor is real and it is measured.
    /// The INDEPENDENCE is not, and is not claimed any more: corrupting the
    /// index question at both of its sites plus the tree question is three
    /// edits over two questions, not over three. Measured on an export with a real
    /// probe in the tracked `deskwarden/examples/ui_preview.rs`: one edit
    /// (a pathspec on `ls-files`) is KILLED and inert; two edits (that plus an
    /// `ls-tree` rewritten to a positive pathspec, since `ls-tree` refuses
    /// exclude magic) SURVIVED the whole suite with the probe unseen before the
    /// third enumeration existed, and is KILLED with it. Three is a LOWER
    /// BOUND: it says fewer cannot, not that three suffices.
    ///
    /// **What that costs, disclosed rather than implied.** A `git archive`
    /// export, a release tarball or a `cargo package` staging directory cannot
    /// run this test at all -- it fails loudly there. That is a real
    /// regression in reach and it is accepted: no `cargo package`, `cargo
    /// publish`, `cargo vendor` or source-tarball step exists anywhere in this
    /// repository, `.github/workflows/release.yml` is the only CI and uses
    /// `actions/checkout@v5` with a real `.git`, and the release ships an
    /// `.exe` and an installer rather than source. The gitless configuration
    /// is a measurement convenience, and a measurement convenience is not
    /// worth the one surface this test keeps losing on. Isolated measurement
    /// is still available: export, then `git init && git add -A && git
    /// commit`.
    ///
    /// So there is no directory-level opt-out any more, and no directory-level
    /// exclusion either.
    ///
    /// * Build output is out of scope because `.gitignore` names it and git
    ///   therefore never offers it -- git's answer, not a guess, and not a
    ///   literal path list in this file. A file FORCE-ADDED under `target/` is
    ///   tracked and IS scanned; that direction is loud and it is intended.
    ///   The round that instead replaced the marker with "scan a file only if
    ///   its bytes are valid UTF-8" traded one forgeable exclusion for
    ///   another: it red-lined a tracked `.ps1` regenerated as UTF-16LE by
    ///   PowerShell's own defaults, and it let a probe-bearing file survive in
    ///   both configurations behind three appended 0xff bytes and a name off
    ///   a twelve-item extension list. The scan is a byte-window search and
    ///   never needed UTF-8.
    /// * A tracked file passes through **no** filter at all. Git has already
    ///   decided it is this repository's; a marker planted next to it cannot
    ///   un-decide that.
    /// * The repository shapes still exist, because a nested clone's working
    ///   files really are another checkout's bytes -- but on **neither**
    ///   path are they an exclusion any more. Each is a hard failure: the
    ///   producer hands back what it declined to attribute and this test
    ///   fails naming it. The previous round did that in the fallback only
    ///   and left the git path filtering untracked entries in silence, which
    ///   is where the whole opt-out then lived: `mkdir docs/scratch/.git`
    ///   next to a probe-bearing untracked file took a measured FAILURE to a
    ///   full green, and a real `git init` in `docs/vendored/` did the same
    ///   through a different door, because git collapses a nested clone to
    ///   one directory entry that the `is_file` guard swallowed. Neither
    ///   works now, and an EMPTY `.git` is not a repository at all; see
    ///   [`nested_repository_shape`] and [`tracked_files`]. Without git the
    ///   instrument cannot tell whose bytes those are, and both available
    ///   guesses are wrong in a way that has been measured, so it says so
    ///   instead of running a weaker scan quietly.
    ///
    /// What remains asymmetric is only what git alone can know -- `.gitignore`,
    /// and the fact that a tracked file is owned by definition -- so in
    /// fallback mode a gitignored scratch file that has nothing to do with
    /// this repository is still scanned. That direction is the safe one: it
    /// can only report bytes that really are sitting in the tree being
    /// measured.
    ///
    /// **And "owns" is wider than "tracks".** `git ls-files` is index-based:
    /// a staged-but-uncommitted file IS listed (verified), but a file written
    /// and not yet `git add`ed is NOT -- so a probe-bearing
    /// `deskwarden/src/scratch_untracked.txt` survived this test, which the
    /// old directory walk had caught. Write-then-compile-before-`git add` is
    /// exactly the loop this repository is developed in, so the list is the
    /// union of `git ls-files` with `git ls-files --others
    /// --exclude-standard`. What that costs: another agent's uncommitted new
    /// files enter this test's scope in a shared working tree, and a scratch
    /// file someone drops in without gitignoring it is scanned. Both are the
    /// point rather than the price -- a file sitting in the tree is a file a
    /// test can read back -- and gitignored scratch stays out, so
    /// `target/`, `.superpowers/` and the rest are untouched.
    ///
    /// **And this test takes [`PROBE_LOCK`] before it reads anything.** It is
    /// the largest reader of bytes in the crate and it arms nothing, so on any
    /// tree where a violation *does* exist it allocates and -- during the
    /// unwind of its own assertion -- frees probe-bearing blocks on a bare
    /// libtest thread while another probe test may be armed. That is the exact
    /// original flake shape, and it would have been reintroduced by the fix
    /// for it. [`hold_the_probe_lock`] is the documented hatch for a test that
    /// touches probe plaintext without arming; this is one.
    ///
    /// Controls, in order: **`root` being this crate's own repository, by
    /// absolute-path identity against a separately spelled
    /// `env!("CARGO_MANIFEST_DIR")`** -- the first thing checked, before a
    /// file is listed, because every producer and every cross-check below
    /// reads that one binding and a single edit to it made all of them agree
    /// about another checkout; nothing was refused unwalked; the file count;
    /// this file being reached, **named by whole absolute path rather than by
    /// the `ends_with("login_ui.rs")` suffix any other checkout also
    /// supplies**; at least one NON-`.rs` file being reached -- without which
    /// a list that had quietly gone back to `src/**/*.rs` would satisfy the
    /// others; at least one file **inside** the crate directory, which is the
    /// load-bearing half, and at least one outside it, which is the half a
    /// foreign tree satisfies for free and which therefore cannot stand
    /// alone; no
    /// file whose extension says source text being skipped as not-UTF-8, which
    /// is the only way the byte rule could be turned back into an opt-out; and
    /// finally the two counts over what was actually opened and scanned.
    #[test]
    fn no_source_file_in_this_crate_contains_the_assembled_probe() {
        // Before a single byte is read: this test allocates the probe whenever
        // the property it checks is violated, and it never arms. See the doc.
        hold_the_probe_lock();

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the crate has a parent directory")
            .to_path_buf();
        // **`root` is the tenth shared input, and it used to cost ONE edit.**
        //
        // Every producer below reads this one binding: `tracked_files(&root,
        // ..)`, `git -C &root ls-tree -r HEAD`, `git -C &root ls-files
        // --cached`. Both readers, both `metadata` walks and the post-scan
        // re-check are all downstream of it. The claim that hiding a tracked
        // file "requires all three producers corrupted in lockstep" was
        // therefore false: point the ONE binding at another checkout and all
        // three agree perfectly about the wrong tree.
        //
        // Measured, both with the real probe committed in the tracked
        // `deskwarden/examples/ui_preview.rs` and going UNSEEN:
        //
        // * `.to_path_buf().parent().unwrap().join("exp")`, a clean sibling
        //   export -- SURVIVED the full lib suite in debug AND release.
        // * `.to_path_buf().join(".claude/worktrees/w1")`, a clean sibling
        //   checkout INSIDE this repository -- SURVIVED, and `git status`
        //   stayed clean, because `/.claude/` is gitignored. The decoy tree
        //   was invisible to review and to the scan at once.
        //
        // Every control held over the substituted tree, because every control
        // was written as a SUFFIX test (`ends_with("login_ui.rs")`) or as a
        // NEGATION (`any(|p| !p.starts_with(crate_dir))`) -- and another
        // checkout satisfies a suffix test, while a tree outside
        // `CARGO_MANIFEST_DIR` satisfies the negation *by being outside it*.
        // The negation was not merely blind to the substitution; it was
        // ENTAILED by it.
        //
        // So `root` is now pinned by ABSOLUTE-PATH IDENTITY, immediately, to
        // the compiled-in location of this crate. `root` is the parent of the
        // crate directory, so re-joining the crate's own directory name must
        // reproduce `CARGO_MANIFEST_DIR` exactly -- not by suffix, not by
        // containment, by equality of the whole path.
        //
        // That equality is `Path::eq`, which compares `components()`, so it
        // is COMPONENT-WISE and not byte-wise: a trailing separator, a
        // doubled separator and a `.` component are all absorbed, while `..`
        // is preserved as a component and therefore never absorbed. No
        // path-arithmetic dressing of a decoy -- `root/./sub/`, `root//sub`,
        // `root/sub/` -- can make an unequal path compare equal here, and
        // none can make the real one compare unequal either. If any comment
        // in this file claims this comparison is byte-wise, it is wrong.
        //
        // **Why the two `env!` expansions are written out separately, and
        // must stay that way.** The obvious tidy-up is
        //
        //     let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        //     let root = crate_dir.parent().unwrap().to_path_buf();
        //
        // and it would REINTRODUCE the exact defect this round exists to
        // close, one level up: `crate_dir` becomes a single binding that
        // feeds both the tree that is scanned and the yardstick it is checked
        // against, so one edit moves them together and the assertion below
        // passes over the decoy. `env!` is expanded at each site by the
        // compiler and there is no binding between them, so the pin costs an
        // attacker TWO edits at two sites. Do not factor these together.
        //
        // **The ELEVENTH shared input was NOT what the last round guessed.**
        // It guessed `CARGO_MANIFEST_DIR`, the deepest and most expensive
        // vector there is; the eleventh was `crate_dir`, one `let` in this
        // same function, added by the very commit that wrote the warning
        // above -- two edits, no `Command`, no pathspec, measured SURVIVED in
        // both profiles. It is written up in full at its old site below. The
        // lesson is that the next shared input is never the exotic one.
        //
        // **The TWELFTH shared input, named now rather than found later, is
        // `git` itself.** With `root` pinned and re-asserted and every
        // yardstick spelled at its own site, the remaining thing that ALL
        // FIVE enumerations -- `ls-files`, `ls-files --others`, `ls-tree -r
        // HEAD`, `ls-files --cached`, `status --porcelain -uall` -- take as a
        // single shared input is the ONE program resolved from the ONE name
        // `"git"` on `PATH`, running in an environment this test does not
        // control. A `git.exe` earlier on `PATH` that filters one path out of
        // every answer subtracts from all five at once.
        //
        // **That paragraph used to list five vectors here and call all five
        // undefendable. Measured, four of them were not, and four are now
        // closed.** "Cannot be defended" was doing the work of "was not
        // measured", which is the class this file keeps losing to:
        //
        // * `GIT_WORK_TREE` genuinely shifted the tree -- `git -C <export>
        //   rev-parse --show-toplevel` returned the PARENT of the export.
        //   Closed immediately below by pinning `--show-toplevel` against
        //   `root`: one invocation, one assert.
        // * `.git/info/exclude` and `core.excludesFile` each genuinely hid a
        //   file from BOTH `--others --exclude-standard` and `status -uall`.
        //   Closed after the third enumeration by `git check-ignore -v`, which
        //   NAMES the excluding file, plus an assert that every exclusion
        //   source is a tracked `.gitignore` under `root`.
        // * `GIT_DIR` alone and `GIT_CEILING_DIRECTORIES` shifted NOTHING.
        //   Under `git -C <absolute path>` -- which is how every invocation
        //   here is spelled -- `--show-toplevel` came back as the export with
        //   either one set. Listing them as live vectors was simply wrong, and
        //   the `--show-toplevel` pin covers them anyway if a future git
        //   changes its mind.
        //
        // What remains genuinely undefendable is the FIRST item only: a
        // wrapper `git.exe` earlier on `PATH`, and with it `~/.gitconfig`'s
        // `[alias]`/`[include]` machinery, which can rewrite what any of these
        // invocations means. That subtracts from all seven questions at once,
        // including the two added to close the other four, and it cannot be
        // caught by asking git anything -- a test that asks git what is in the
        // tree is only as honest as the git it asked, and re-deriving the
        // answer without git is the directory walk six rounds were spent
        // deleting. THAT is the disclosed boundary. The other four were an
        // oversight dressed as one.
        //
        // And `CARGO_MANIFEST_DIR` remains the deepest one. Both expansions
        // read the same compile-time value, so the pin does not prove `root`
        // is THIS repository -- it proves `root` is the parent of wherever
        // this crate's manifest sits when it is compiled. Move the manifest
        // and both sides move together and the assertion still passes. That
        // is deliberately not defended here: it cannot be done from inside
        // this file at all. It needs `Cargo.toml` moved or a workspace member
        // added, which is a structural change to the build, visible in a
        // diff in a way an expression buried at line seven thousand is not,
        // and defending it from in here would mean hard-coding a repository
        // name -- which breaks every fork and every worktree while stopping
        // nobody who is already editing the manifest. The boundary is stated
        // instead: this test polices the repository the crate is compiled
        // from, and whoever can move the crate can move what it polices.
        assert_eq!(
            root.join(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .file_name()
                    .expect("the crate directory has a name")
            ),
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
            "the_probe_scan_reads_the_wrong_tree: `root` is {root:?}, which is not the parent \
             of this crate's own compiled-in directory. Everything below -- both file \
             listings, the `ls-tree` cross-check, the `ls-files --cached` cross-check and the \
             post-scan re-check -- reads that one path, so a `root` pointing at ANY other \
             checkout makes all of them agree, unanimously, about a tree that is not this one. \
             A clean sibling export (or a clean checkout under the gitignored `.claude/`) then \
             passes the whole suite green while the real probe sits committed in this tree \
             unread. That is not a stale path to repair in place: it is the scan measuring \
             something other than the repository it claims to police."
        );
        // **`GIT_WORK_TREE`, closed -- the pin above is about `root`, this one
        // is about the tree git decides `root` NAMES.**
        //
        // `root` being this crate's own parent directory says nothing about
        // what `git -C <root>` will answer, and the two are not the same
        // question. Measured: with `GIT_WORK_TREE` set to the parent of an
        // export, `git -C <export> rev-parse --show-toplevel` returned the
        // PARENT, not the export -- every one of the five enumerations below
        // is then answered about a different working tree while `root` itself
        // is untouched and all three `assert_eq!`s on it pass. Zero source
        // edits, no diff.
        //
        // One invocation and one assert close it: whatever tree git thinks it
        // is standing in must BE `root`. Both sides go through
        // `canonicalize`, because `--show-toplevel` prints forward slashes,
        // resolves symlinks and normalises the drive letter while
        // `CARGO_MANIFEST_DIR` does none of those -- comparing the two raw
        // would be a false red on any checkout reached through a symlink or a
        // differently-cased path. `canonicalize` puts both into the same
        // verbatim form, so this compares trees rather than spellings.
        //
        // It is sound on a linked worktree (`--show-toplevel` is that
        // worktree's own root, which is what `root` is) and on a fork (no
        // repository name appears anywhere in it).
        //
        // **And the in-file comment below overstated its neighbours, which is
        // corrected there:** measured under `git -C <absolute path>`,
        // `GIT_DIR` alone and `GIT_CEILING_DIRECTORIES` shifted NOTHING --
        // `--show-toplevel` came back as the export in both cases. Only
        // `GIT_WORK_TREE` moved it.
        let toplevel_raw = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_else(|| {
                panic!(
                    "the_probe_scan_requires_git: `git rev-parse --show-toplevel` could not \
                     answer for {root:?}, so there is no way to tell which working tree the \
                     five enumerations below are actually about. This test does not run \
                     without git; see the panics below."
                )
            });
        let toplevel_seen = std::fs::canonicalize(&toplevel_raw);
        let toplevel_want = std::fs::canonicalize(&root);
        assert!(
            matches!(
                (toplevel_seen.as_deref(), toplevel_want.as_deref()),
                (Ok(seen), Ok(want)) if seen == want
            ),
            "the_probe_scan_reads_the_wrong_tree: `git -C {root:?}` reports its working tree \
             as {toplevel_raw:?} ({toplevel_seen:?}), which is not {root:?} \
             ({toplevel_want:?}). `root` can be pinned to this crate's own parent directory \
             and still name a tree git answers about differently -- `GIT_WORK_TREE` does \
             exactly that, at ZERO source edits, and every listing, cross-check and re-check \
             below would then agree unanimously about a tree that is not this repository. \
             Unset `GIT_WORK_TREE` and re-run."
        );

        // **The `--others` half's SECOND PRODUCER, asked FIRST.**
        //
        // Undisclosed asymmetry, found by attacking this design rather than
        // by review, and measured both ways: `ls-tree -r HEAD` and `ls-files
        // --cached` enumerate the COMMITTED and CACHED trees, so between them
        // they are two extra opinions about the TRACKED half of the listing
        // and ZERO opinions about the untracked half. But the scan does list
        // `--others --exclude-standard` today, so untracked files ARE inside
        // this test's property -- and with a real probe in an untracked,
        // non-ignored `docs/scratch_note.md` the unmutated test KILLED,
        // naming the file, while ONE edit appending `:!docs` to that single
        // `--others` invocation SURVIVED. A one-edit narrowing that no
        // control can see is a hole, not a scope statement.
        //
        // So the untracked half gets its own second opinion, at its own call
        // site with its own arguments and its own output format: `git status
        // --porcelain -z --untracked-files=all`, whose `?? ` records are the
        // same question ("what is here that git is not tracking and
        // `.gitignore` does not exclude?") put through a different plumbing
        // command. A pathspec added to either invocation alone is now both
        // loud (the assert below) and inert (the union below).
        //
        // **It is asked BEFORE the listing on purpose**, and that ordering is
        // the whole race handling. Other agents land commits in this tree
        // while this suite runs, so an untracked file can appear or vanish
        // mid-test. Asked first, a file CREATED between the two questions is
        // in the listing and not in this set, which the subset check below
        // does not mind; a file DELETED between them is in this set and not
        // in the listing, and the `is_file` filter here re-checked at use
        // drops it. Asked the other way round both directions would red.
        let untracked_raw = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["status", "--porcelain", "-z", "--untracked-files=all"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| o.stdout)
            .unwrap_or_else(|| {
                panic!(
                    "the_probe_scan_requires_git: `git status --porcelain` could not enumerate \
                     {root:?}, so the UNTRACKED half of the listing has no second opinion to \
                     be checked against and a pathspec added to the `--others` producer would \
                     be invisible. This test does not run without git; see the panic below."
                )
            });
        let untracked_second: Vec<std::path::PathBuf> = untracked_raw
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .filter_map(|record| {
                // `?? <path>`; every other status code is about a path git
                // already knows, which the committed and cached enumerations
                // speak for. `-z` means no quoting and no rename pairs among
                // the `??` records, so this is a plain three-byte prefix.
                let rest = record.strip_prefix(b"?? ".as_slice())?;
                std::str::from_utf8(rest).ok()
            })
            .map(|rel| root.join(rel))
            .collect();

        let mut refused: Vec<std::path::PathBuf> = Vec::new();
        let mut files = tracked_files(&root, &mut refused).unwrap_or_else(|| {
            panic!(
                "the_probe_scan_requires_git: `git ls-files` could not enumerate {root:?}, so \
                 the list of files this test polices could not be produced, and NOTHING was \
                 scanned. There is deliberately no directory-walk fallback any more. A walk \
                 needs an exclusion list for build output, and that list -- and the \
                 `.gitignore` cross-check propping it up -- was the entire attack surface of \
                 this test for six rounds: every defeat was a coordinated widening of it. \
                 Deriving the list from git excludes build output BY CONSTRUCTION, because \
                 build output is gitignored and therefore untracked, and leaves no directory \
                 list in this file for a mutation to widen. The price is stated here rather \
                 than paid silently: without git this property cannot be checked at all, and a \
                 test that cannot check its property says so instead of quietly checking a \
                 weaker one. Run the suite in a real checkout with `git` on `PATH`."
            )
        });
        // The listing does not get to guess. A nested repository holding
        // files this scan would otherwise have read is recorded rather than
        // dropped, and recorded is a FAILURE. Walking into one reports
        // another checkout's `login_ui.rs` as a violation of THIS tree;
        // skipping it lets a planted `.git` delete a whole subtree from the
        // scan. Neither is an answer, so this says so. See `tracked_files`.
        assert!(
            refused.is_empty(),
            "{} nested repository shape(s) hold files this scan was asked to police and cannot \
             attribute, the first of them {:?}. This is either an UNTRACKED entry `git ls-files \
             --others` listed as one directory standing for a whole working tree, or a TRACKED \
             GITLINK (`160000 commit <sha> <path>`) -- a nested clone committed as a submodule \
             reference, which every enumeration below would otherwise drop through its own \
             `is_file` filter, at ZERO source edits and with `git status` clean. The bytes \
             inside are not knowably this repository's, and the two guesses are both wrong in \
             a way that has been measured. Remove the nested repository, or commit what is \
             under it so git can attribute it.",
            refused.len(),
            refused.first()
        );
        assert!(
            files.len() > 30,
            "control: the walk found only {} files, so the check below is a check over nothing",
            files.len()
        );
        // **The pin, RE-ASSERTED now that the listing exists.** The one at the
        // top of this function is a statement about a VALUE at one instant;
        // this one closes the window between that instant and here, which is
        // precisely where the measured two-edit defeat put its `let root =
        // root.join(".claude").join("worktrees").join("w1");`. Its `env!` is
        // spelled out again rather than shared with the pin above or with the
        // controls below, for the reason this whole function is written the
        // way it is.
        assert_eq!(
            root.join(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .file_name()
                    .expect("the crate directory has a name")
            ),
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
            "the_probe_scan_reads_the_wrong_tree: `root` was this crate's own parent directory \
             when it was pinned above and is {root:?} now that the listing has been produced. \
             A `let root = ...` between the pin and here redirects the listing while leaving \
             the pin's own `assert_eq!` already run and satisfied, which is a MEASURED defeat \
             and not a hypothetical: two edits, no `Command` touched, no pathspec, a real \
             probe committed in this tree unseen through the full suite in debug and release"
        );

        // **`crate_dir` WAS the eleventh shared input, and it cost TWO edits.**
        //
        // The round that added the `root` pin warned, at length and
        // correctly, that factoring the two `env!` expansions in that pin
        // into one `let crate_dir` would reintroduce the defect one level up
        // -- and then it factored a THIRD `env!` into exactly that binding,
        // feeding all three of the controls it introduced or repaired from
        // one `let`. Measured, with a real probe committed in the tracked
        // `deskwarden/examples/ui_preview.rs` and a clean checkout at the
        // gitignored `<root>/.claude/worktrees/w1`: TWO edits -- `let root =
        // root.join(".claude").join("worktrees").join("w1");` immediately
        // after the pin's `assert_eq!`, and `let crate_dir = &root.join(
        // "deskwarden");` here -- SURVIVED debug filtered, debug full and
        // release full, with the probe unseen. Liveness at the identical site
        // KILLED. Neither edit touched a `Command` or a pathspec, so the
        // "three producers in lockstep" floor stated below was false at two.
        //
        // The pin asserts a VALUE, not a BINDING. `assert_eq!` runs once,
        // before a file is listed, and `let root = ...` one line later
        // shadows it for every producer, both readers, both `metadata` walks
        // and the post-scan re-check, with nothing looking at `root` again.
        // Two things close that window and both are done:
        //
        // * every yardstick below is spelled `env!("CARGO_MANIFEST_DIR")` AT
        //   ITS OWN USE SITE, so corrupting them costs one edit each rather
        //   than one edit total. Do not factor these back together, for the
        //   same reason the pin's own two expansions are unfactored;
        // * the pin is RE-ASSERTED after the listing is produced and again
        //   after both readers have finished, so a shadow rebind of `root`
        //   anywhere in this function is caught by the assertion it was
        //   inserted to get past rather than by luck.
        //
        // And the record the previous round left is corrected here: it
        // credited the new pin with killing the one-edit shadow rebinds.
        // Measured, it does not -- the ABSOLUTE-PATH `this_file` control just
        // below kills them on its own, because a pure `let root = ...` moves
        // the listing without moving the yardstick. The positive controls are
        // the load-bearing half; the pin is close to redundant against one
        // edit and earns its place only against the second one.
        //
        // This used to read `any(|p| p.ends_with("login_ui.rs"))`, and a
        // suffix is exactly what a substituted checkout supplies for free:
        // every other tree with this crate in it has a `login_ui.rs` too. The
        // file that declares the probe is identified by its WHOLE absolute
        // path instead, built from the compiled-in crate directory.
        let this_file =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("login_ui.rs");
        assert!(
            files.iter().any(|p| *p == this_file),
            "control: the listing does not contain {this_file:?} -- the file that declares the \
             probe, named by absolute path rather than by suffix. Either the listing dropped \
             it, or the listing is of a DIFFERENT checkout that merely also has a file whose \
             name ends `login_ui.rs`"
        );
        assert!(
            files.iter().any(|p| p.extension().is_some_and(|e| e != "rs")),
            "control: the walk reached no file that is not a `.rs` -- it has narrowed back to \
             the source directory, and the claim in this test's name is again wider than what \
             it measures"
        );
        // **Both directions, and the positive one is the load-bearing half.**
        //
        // Only the negation used to be asserted, and a substituted `root`
        // does not merely evade it -- it SATISFIES it, because a tree outside
        // `CARGO_MANIFEST_DIR` puts *every* listed file outside the crate
        // directory. The control that was supposed to prove the listing
        // reaches beyond the crate was the one the mutant passed most easily.
        // The positive half cannot be satisfied that way: a listing has a
        // file under this crate's own compiled-in directory only if it is a
        // listing of this crate's own tree.
        assert!(
            files.iter().any(|p| p.starts_with(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))),
            "control: NO file the listing reached is inside {:?}, this crate's own \
             compiled-in directory. The listing is not of this repository at all. Note that \
             the complementary assertion below -- that something lies OUTSIDE the crate \
             directory -- is satisfied automatically by any foreign tree, which is why it \
             cannot stand alone",
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        );
        assert!(
            files.iter().any(|p| !p.starts_with(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))),
            "control: every file the walk reached is inside the crate directory, so `docs/`, \
             the top-level `*.md` and `.github/` are outside what this test measures while its \
             name says the whole tree"
        );

        // **The second opinion, and it is git's own -- not a directory
        // walk's, and not this file's.**
        //
        // Deleting the exclusion list removed the six-round attack surface,
        // but it did not remove EVERY in-file lever over what gets scanned.
        // One remains and it is named here rather than left to be found: the
        // `git ls-files` invocation in `tracked_files` takes arguments, and
        // git argument syntax includes PATHSPECS. A single edit --
        // `.args(["ls-files", "-z", ":!deskwarden/examples"])` -- deletes a
        // tracked directory from the listing while satisfying every control
        // above, because the controls above are floors and a floor cannot
        // see a subtraction that leaves ninety files behind.
        //
        // So the listing is checked against a SECOND, independently spelled
        // question put to git: `git ls-tree -r HEAD`, which enumerates the
        // committed tree and takes its own arguments at its own call site.
        // Every committed path that is present as a file must be in `files`.
        // This is not a directory walk and it carries no exclusion list of
        // any kind -- there is nothing here to widen, only two separate
        // invocations to corrupt in lockstep. That is the honest new edit
        // cost, stated as a lower bound and not as "impossible": a pathspec
        // added to the producer alone is KILLED here; hiding a tracked
        // directory needs the producer edited AND this cross-check defeated,
        // and this cross-check is a named assert whose deletion is itself an
        // edit.
        //
        // It fails loudly rather than degrading if git cannot answer, for
        // the same reason the listing does.
        let committed_raw = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["ls-tree", "-r", "-z", "--name-only", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| o.stdout)
            .unwrap_or_else(|| {
                panic!(
                    "the_probe_scan_requires_git: `git ls-tree -r HEAD` could not enumerate \
                     {root:?}, so the listing this test scans has no second opinion to be \
                     checked against and a pathspec added to the producer would be invisible. \
                     This test does not run without git; see the panic above."
                )
            });
        let committed: Vec<std::path::PathBuf> = committed_raw
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .filter_map(|raw| std::str::from_utf8(raw).ok())
            .map(|rel| root.join(rel))
            .filter(|p| p.is_file())
            .collect();
        assert!(
            committed.len() > 30,
            "control: `git ls-tree -r HEAD` named only {} present files, so the cross-check \
             below is a cross-check over nothing",
            committed.len()
        );
        let unscanned: Vec<&std::path::PathBuf> =
            committed.iter().filter(|p| !files.contains(p)).collect();
        assert!(
            unscanned.is_empty(),
            "{} file(s) are committed to HEAD and present in the working tree but are NOT in \
             the list this test is about to scan, the first of them {:?}. The two enumerations \
             are two separately spelled questions put to git, so this is what a PATHSPEC added \
             to the `git ls-files` invocation in `tracked_files` looks like -- the one \
             remaining in-file lever over what gets scanned, and the reason it is not a \
             one-edit hole. If a file was legitimately removed from the index but left on \
             disk (`git rm --cached`), commit that removal; the scan will not quietly police \
             a smaller tree than the one that is committed.",
            unscanned.len(),
            unscanned.first()
        );

        // **The third enumeration, and why there are three.** Two producers
        // put the floor at two coordinated edits, and two was measured, not
        // guessed: a pathspec on `ls-files` plus a rewritten `ls-tree` whose
        // positive pathspec enumerates the tree's top level minus one
        // directory made a real probe in a real tracked file UNSEEN, whole
        // suite green. `ls-tree` will not take an exclude pathspec -- `git`
        // answers `pathspec magic not supported by this command` -- so that
        // second edit has to be a hand-written list of the repository's
        // layout, but it is still one edit and one edit is not a floor worth
        // reporting.
        //
        // So there is a third, at its own call site with its own arguments,
        // and it too is unioned in. Hiding a tracked file now requires all
        // three producers corrupted in lockstep, or one corrupted and the
        // other two deleted -- three edits either way, in the configuration
        // that ships, with no gitless configuration left to hide in.
        //
        // **That "three" was stated as a lower bound and it was FALSE, three
        // times over, and all three refutations are now closed.** It was
        // beaten at TWO edits by the `crate_dir` rebind (neither edit a
        // `Command`, neither a pathspec -- see the write-up at the controls
        // below), and at ZERO edits by a gitlink, which all three producers
        // dropped independently through their own `is_file` filters. The
        // round that fixed the gitlink then restated the floor at three and
        // was beaten at ZERO again, by the same gitlink with its `.git`
        // marker DELETED after the commit: the fix keyed on a working-tree
        // shape, and a working-tree shape can be removed. The floor for a
        // TRACKED file has therefore been 0, not 3, for two rounds running.
        // It is stated at three again only now that the refusal keys on the
        // INDEX MODE (`ls-files -s`, `ls-tree -r` without `--name-only`),
        // which no file operation can reach -- and it should be read as "no
        // route BELOW has been found cheaper than three", which is the same
        // sentence that was wrong the last two times. It was also silent about the
        // UNTRACKED half entirely: `ls-tree` and `ls-files --cached` speak
        // only for the tracked tree, so a pathspec on the `--others`
        // invocation was ONE edit and unobserved until `git status
        // --porcelain -uall` was added above as its second producer. A floor
        // in a comment is a claim, and every claim in this file has now been
        // beaten at least once. Read this one as "no route BELOW is cheaper
        // than three", not as a proof about routes nobody has tried.
        //
        // **That lower bound was FALSE for ten rounds, and it holds now only
        // because `root` is asserted.** All three producers take the
        // repository they enumerate from the SAME `let root` binding --
        // `tracked_files(&root, ..)`, `git -C &root ls-tree -r HEAD`, `git -C
        // &root ls-files --cached`. Independence of the three QUESTIONS buys
        // nothing when all three are asked about the same wrong TREE: one
        // edit to `root` pointed them at a clean sibling export -- and, worse,
        // at a clean checkout under the gitignored `.claude/`, invisible to
        // `git status` as well as to the scan -- and all three then agreed
        // perfectly while a probe committed in this tree went unread through
        // the full suite in debug AND release. Every control held over the
        // substituted tree, including the one asserting that some file lies
        // outside `CARGO_MANIFEST_DIR`, which a foreign tree satisfies by
        // construction. This floor is conditional on the tree being the right
        // one, and that condition is now an assertion at the `root` binding
        // rather than an assumption in this paragraph.
        //
        // These are three separate `Command`s, not one statement written
        // three times and not one binding read three times. That distinction
        // is the whole history of this test: the round before last was
        // defeated by shadow-rebinding a single `let`-bound closure, and the
        // last one by refilling a single `let ignore_text` that two
        // deliberately-different sites both read. There is no binding here
        // for a fourth edit to refill.
        let cached_raw = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["ls-files", "--cached", "--full-name", "-z"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| o.stdout)
            .unwrap_or_else(|| {
                panic!(
                    "the_probe_scan_requires_git: `git ls-files --cached` could not enumerate \
                     {root:?}. This test does not run without git; see the panic above."
                )
            });
        let cached: Vec<std::path::PathBuf> = cached_raw
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .filter_map(|raw| std::str::from_utf8(raw).ok())
            .map(|rel| root.join(rel))
            .filter(|p| p.is_file())
            .collect();
        assert!(
            cached.len() > 30 && cached.iter().any(|p| p.ends_with("login_ui.rs")),
            "control: the third enumeration named {} present files and {} the file that \
             declares the probe, so it is not a third opinion about anything",
            cached.len(),
            if cached.iter().any(|p| p.ends_with("login_ui.rs")) { "reached" } else { "missed" }
        );

        // **`.git/info/exclude` and `core.excludesFile`, closed -- and they
        // were the two cheapest holes left in the whole test.**
        //
        // Both halves of the untracked listing honour them: `--others
        // --exclude-standard` and `status --porcelain -uall` alike. So one
        // line in a file that is NOT tracked, that no diff shows and that no
        // control below can see, deletes an untracked probe-bearing file from
        // BOTH untracked producers at once -- the second opinion added last
        // round is not a second opinion about this at all, because it is
        // subtracted from too. Zero source edits.
        //
        // It looked undefendable and it is not, because git will name the
        // source: `git check-ignore -v <path>` prints
        // `.git/info/exclude:1:docs/secret.md<TAB>docs/secret.md` for the
        // local case and the configured file's ABSOLUTE path for the
        // `core.excludesFile` case. That turns "something invisible excluded a
        // file" into an ordinary assert of exactly the shape everything else
        // in this function has: the only thing allowed to exclude anything
        // from this tree is a TRACKED `.gitignore` inside `root`, which is a
        // file review sees and this scan reads.
        //
        // Two invocations. `status --porcelain -z -uall --ignored` names what
        // is excluded -- traditional `--ignored`, not `--ignored=matching`, so
        // a wholly-ignored directory collapses to ONE record and `target/` is
        // not enumerated file by file. Then one `check-ignore -v -z --stdin`
        // over that whole set, whose output is four NUL-separated fields per
        // record: source, line number, pattern, pathname. `check-ignore` exits
        // 1 when nothing it was given is ignored, which is a normal answer and
        // not a failure, so the status is not filtered on -- only the parse is.
        //
        // Measured, all four ways, each against a liveness pair with the same
        // probe and the exclusion removed: `.git/info/exclude` KILLED,
        // `core.excludesFile` KILLED, an UNTRACKED `docs/.gitignore` KILLED
        // (that one was not previously disclosed at all), and a TRACKED,
        // COMMITTED `docs/.gitignore` GREEN.
        //
        // **That last one is the boundary and it is not a bug.** A tracked
        // `.gitignore` line does take an untracked file out of this scan's
        // scope, at ONE edit -- but that edit is a committed line in a file
        // whose whole purpose is to say what this repository does not track,
        // and it shows up in a diff like any other. That is the difference
        // this assert is drawing: not "nothing may be excluded", but "nothing
        // may be excluded by something review cannot see". Cost on this tree:
        // ~1.2 s for the two calls over ~30 000 ignored paths, whose sole
        // exclusion source is the tracked root `.gitignore`.
        //
        // The two vectors this does NOT reach are stated rather than implied:
        // an exclusion that hides a file from the untracked listing AND from
        // `--ignored` at the same time is not reachable through the exclude
        // machinery (that machinery is what `--ignored` reports), but a
        // wrapper `git.exe` on `PATH` subtracts from this pair exactly as it
        // subtracts from the other five. See the boundary note above.
        let ignored_raw = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["status", "--porcelain", "-z", "--untracked-files=all", "--ignored"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| o.stdout)
            .unwrap_or_else(|| {
                panic!(
                    "the_probe_scan_requires_git: `git status --ignored` could not enumerate \
                     {root:?}, so there is no way to tell whether an untracked exclusion \
                     source outside this repository's tracked `.gitignore` files is hiding \
                     something from both untracked producers. This test does not run without \
                     git; see the panics above."
                )
            });
        // **The excluded set has TWO producers too, and the block below is no
        // longer conditional on either of them being non-empty.**
        //
        // The audit was wrapped in `if !ignored_paths.is_empty()`, so ANY
        // edit that emptied this one list -- dropping `--ignored` from the
        // arguments, mistyping the `"!! "` prefix -- skipped the whole
        // attribution audit rather than failing it, and skipping it is
        // exactly what an exclusion source wants. So a second, independently
        // spelled question is asked (`ls-files --others --ignored`, plumbing
        // rather than porcelain, with its own arguments at its own call site)
        // and the two are UNIONED. They do not collapse ignored directories
        // by the same rule -- measured on this tree, `status --ignored`
        // reports `.superpowers/sdd/*` file by file where `ls-files
        // --directory` collapses it to `.superpowers/` -- so this is not a
        // set-equality check, it is two overlapping views both of which must
        // attribute to a tracked `.gitignore`. Emptying one no longer empties
        // the audit.
        //
        // **Bytes, not `&str`.** These were `filter_map(from_utf8().ok())`,
        // which silently DROPPED any non-UTF-8 path -- a silent skip in a
        // test whose whole design is that nothing is skipped in silence, and
        // one that a deliberately non-UTF-8 filename could aim. `check-ignore
        // -z --stdin` takes bytes and gives them back, so nothing here needs
        // to decode a path at all.
        let ignored_second_raw = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args([
                "ls-files",
                "-z",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
                "--no-empty-directory",
            ])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| o.stdout)
            .unwrap_or_else(|| {
                panic!(
                    "the_probe_scan_requires_git: `git ls-files --others --ignored` could not \
                     enumerate {root:?}, so the excluded set has no second opinion and one \
                     edit emptying `git status --ignored` would skip the attribution audit \
                     entirely. This test does not run without git; see the panics above."
                )
            });
        // **The two producers are parsed into two SEPARATE bindings, and the
        // union is measured against both -- because the union used to be one
        // statement behind one guard, and that guard was the whole defence.**
        //
        // As written before, both parses were inlined into a single `for` over
        // `a.chain(b)` whose body was `if !ignored_paths.contains(&path) {
        // push }`. Deleting that one `!` -- ONE character, in a line that
        // reads like a de-duplication detail -- made the condition false for
        // every path, because the vector starts empty and `contains` is never
        // true on an empty vector. `ignored_paths` then stayed empty no matter
        // what EITHER producer said, and every check downstream passed
        // vacuously: an empty feed makes `check-ignore` exit 1 with empty
        // stdout (a normal answer, so the status assert passes), zero fields
        // is a whole number of four-field records, `0 == 0 * 4` holds, and
        // `chunks(4)` never iterates so `foreign` is empty. Measured with a
        // real probe in `docs/scratch_note.md` hidden by an UNTRACKED
        // `docs/.gitignore`: pristine KILLED at the `foreign.is_empty()`
        // assert, the one-character mutant SURVIVED the filtered run AND the
        // full suite, in debug and in `--release`, with the probe unseen.
        //
        // Note what that mutant actually deleted. The probe file is never
        // scanned even on pristine code -- it is untracked and excluded, so no
        // producer offers it to the scanner. ATTRIBUTION is the only thing in
        // this test that can report it at all. Emptying the attributed list
        // does not skip a scan; it deletes the sole detector.
        //
        // So: two bindings, two names, and the yardsticks below are taken from
        // the bindings rather than from the fed list. See the two asserts
        // immediately after the union, and the coverage assert at the audit.
        //
        // **What that `!` does now is TREE-DEPENDENT, and the attribution
        // above should be read as a measurement on THIS tree rather than as a
        // property of the control.** The de-duplication is now
        // `if seen_ignored.insert(path)`, so the mutant is an ADDED `!`, and
        // `!insert` pushes exactly the paths the two producers BOTH named --
        // the duplicates -- rather than nothing. On this tree that is 2 of
        // 29 776, so the union collapses and the two coverage asserts fire.
        // On a tree where the two producers agree completely, every path is a
        // duplicate, `ignored_paths` is COMPLETE, the coverage asserts pass
        // and it is `foreign.is_empty()` that kills instead. Both trees kill;
        // which control does it is not fixed, so do not read a green coverage
        // assert as evidence that the union is intact.
        let status_ignored: std::collections::HashSet<&[u8]> = ignored_raw
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .filter_map(|record| record.strip_prefix(b"!! ".as_slice()))
            .collect();
        let listed_ignored: std::collections::HashSet<&[u8]> = ignored_second_raw
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .collect();
        // **A `HashSet` for the de-duplication, not `Vec::contains`.** The
        // linear scan was O(n^2) over byte slices, and this repository ignores
        // 29 778 paths, so it was doing ~440 million slice comparisons.
        // Measured over exactly that list, taken from these two producers on
        // the real tree: 2.63 s unoptimized and 468 ms optimized, against
        // 47.9 ms and 5.0 ms hashed -- so it was material in both profiles,
        // and dominated the ~1.2 s the two `git` calls themselves cost. This
        // is a MEMBERSHIP-preserving change, and only that. `HashSet<&[u8]>`
        // hashes the slice CONTENTS, which is exactly what `Vec::contains`
        // compared, so no path is added and none is dropped -- the set
        // `ignored_paths` denotes is identical.
        //
        // **The ORDER is not, and an earlier revision of this comment claimed
        // a verification that cannot have happened.** It said "the same
        // paths, in the same first-seen order". `RandomState` reseeds per
        // PROCESS, so iterating `status_ignored` and `listed_ignored` yields a
        // different order in every run: measured over 12 paths, three runs,
        // three different orders. That order propagates to `ignored_paths`, to
        // the bytes fed to `check-ignore`, to the order of the child's records
        // and therefore to the order of the paths printed in the `foreign`
        // failure message below.
        //
        // It is behaviourally harmless -- every consumer here is
        // order-insensitive: the feed is a set of questions, the audit is a
        // set membership, and all four asserts compare lengths or emptiness.
        // But the practical consequence is worth stating for whoever reads a
        // red next: **the failure messages of the `foreign` and `unaudited`
        // asserts list their paths in an unstable order, so do not diff two
        // runs' output and conclude that the set changed.** Sort before
        // comparing, as `unaudited` already does for its own output.
        let mut seen_ignored: std::collections::HashSet<&[u8]> = std::collections::HashSet::new();
        let mut ignored_paths: Vec<&[u8]> = Vec::new();
        for path in status_ignored.iter().chain(listed_ignored.iter()).copied() {
            if seen_ignored.insert(path) {
                ignored_paths.push(path);
            }
        }
        // **The union is measured against each PART, and the parts are bound
        // where the union is not.** A union of two sets contains each of them,
        // so on honest code these hold by construction and cost nothing -- the
        // point is that the right-hand side comes from a binding the union
        // statement does not produce. Any edit inside the loop that drops
        // paths on the floor shrinks the left side ONLY, and is named here.
        // Both are vacuously true on a tree that ignores nothing (0 >= 0),
        // which the exported fixture tree is, so this is not a false red
        // there; the reviewer's "assert each part is non-empty" would be one.
        assert!(
            ignored_paths.len() >= status_ignored.len(),
            "control: `git status --ignored` named {} excluded path(s) in {root:?} but the \
             union fed to the attribution audit holds only {}. The union is what gets \
             attributed to a `.gitignore`; anything missing from it is excluded by a source \
             nothing checks. An empty union makes the whole \
             `.git/info/exclude` / `core.excludesFile` / untracked-`.gitignore` defence inert \
             while every check below passes vacuously.",
            status_ignored.len(),
            ignored_paths.len()
        );
        assert!(
            ignored_paths.len() >= listed_ignored.len(),
            "control: `git ls-files --others --ignored` named {} excluded path(s) in {root:?} \
             but the union fed to the attribution audit holds only {}. The second producer \
             exists so that emptying the first does not empty the audit; a union smaller than \
             it means the union statement, not either producer, is dropping paths.",
            listed_ignored.len(),
            ignored_paths.len()
        );
        {
            use std::io::Write as _;

            // **The path list goes through a FILE, not through a pipe, and
            // that is not a style choice.** This tree ignores ~30 000 paths
            // (`target/` and the gitignored agent directories), so
            // `check-ignore` answers with well over a megabyte -- far more
            // than a pipe buffer holds. Writing the list to the child's stdin
            // while it is writing that answer back deadlocks unless one side
            // is drained concurrently, and draining it concurrently means a
            // `std::thread::spawn`, which this crate's spawn census counts
            // exactly and rightly refuses to widen for a test's convenience.
            // A file has no buffer to fill, so `output()` is safe as written:
            // there is no stdin to write once the child is running.
            let feed_path = std::env::temp_dir()
                .join(format!("deskwarden-probe-scan-excluded-{}.lst", std::process::id()));
            let feed: Vec<u8> = ignored_paths
                .iter()
                .flat_map(|rel| rel.iter().copied().chain(std::iter::once(0u8)))
                .collect();
            std::fs::File::create(&feed_path)
                .and_then(|mut file| file.write_all(&feed))
                .unwrap_or_else(|error| {
                    panic!(
                        "the {} excluded path(s) in {root:?} could not be written to \
                         {feed_path:?} ({error}), so they cannot be traced back to the files \
                         that excluded them",
                        ignored_paths.len()
                    )
                });
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["check-ignore", "-v", "-z", "--stdin"])
                .stdin(std::fs::File::open(&feed_path).unwrap_or_else(|error| {
                    panic!("{feed_path:?} could not be re-opened as stdin ({error})")
                }))
                .stderr(std::process::Stdio::null())
                .output()
                .unwrap_or_else(|error| {
                    panic!(
                        "the_probe_scan_requires_git: `git check-ignore` could not be run \
                         ({error}), so the {} excluded path(s) in {root:?} cannot be traced \
                         back to the file that excluded them",
                        ignored_paths.len()
                    )
                });
            // Best-effort: a leftover list in the temp directory is litter,
            // not a hole, and failing the scan over it would be a false red.
            let _ = std::fs::remove_file(&feed_path);
            // **THE EXIT STATUS, CHECKED -- and this was the hole.**
            //
            // Nothing here looked at `out.status`, on the reasoning written
            // above that "`check-ignore` exits 1 when nothing it was given is
            // ignored, which is a normal answer and not a failure". That
            // sentence is true and the conclusion drawn from it was not: it
            // licensed ignoring EVERY nonzero exit, not just the one that
            // means "none". A `check-ignore` that FAILS prints nothing, and
            // empty stdout satisfied both guards below vacuously -- zero
            // records is a whole number of four-field records, and no records
            // means no foreign source. Measured at ONE edit: appending
            // `"--no-such-flag"` to the arguments above makes git exit 129
            // with empty stdout, and with a real probe in a file hidden by a
            // `.git/info/exclude` line the test came back `ok. 1 passed` with
            // the probe unseen, in debug AND in release, while the same
            // binary with the exclusion emptied was KILLED. On a clean tree
            // the mutant is green, so it needs no accompanying diff to look
            // innocent.
            //
            // Every other child in this test already panics on a nonzero
            // exit; this one had `unwrap_or_else` on the SPAWN error only.
            // And unlike the gitlink producers it has no second opinion about
            // its own answer: the entire `info/exclude` + `core.excludesFile`
            // + untracked-`.gitignore` defence hangs off this one child. It
            // also made the disclosed `git.exe`-wrapper boundary strictly
            // cheaper, because a wrapper only had to make this call FAIL, not
            // lie.
            //
            // Two asserts, because a total failure and a PARTIAL one are
            // different shapes and git produces both: confirmed at the git
            // level that `check-ignore` truncates its stdout mid-stream and
            // exits 128 on a bad path, so a half-written answer is real.
            // First the status: 0 means at least one given path is ignored, 1
            // means none of them is, and those two are the only answers this
            // parser is entitled to read.
            assert!(
                matches!(out.status.code(), Some(0 | 1)),
                "the_probe_scan_requires_git: `git check-ignore -v -z --stdin` exited {:?} for \
                 the {} excluded path(s) in {root:?}, which is neither 0 (some given path is \
                 ignored) nor 1 (none is). A failing `check-ignore` prints NOTHING, and an \
                 empty answer satisfies both of the checks below vacuously -- so this is the \
                 whole `.git/info/exclude` / `core.excludesFile` / untracked-`.gitignore` \
                 defence going inert while the test reports green. It is a failure, not an \
                 answer.",
                out.status.code(),
                ignored_paths.len()
            );
            // Four NUL-separated fields per record: source, line, pattern,
            // pathname. Anything that does not come in fours is a format this
            // parser does not understand, and an unparsed answer is not a
            // clean one -- so it is reported rather than skipped.
            let fields: Vec<&[u8]> = out.stdout.split(|b| *b == 0).collect();
            let fields = match fields.split_last() {
                // `-z` terminates every field, so the split leaves one
                // trailing empty piece; an empty answer is just that piece.
                Some((last, rest)) if last.is_empty() => rest.to_vec(),
                _ => fields,
            };
            assert!(
                fields.len() % 4 == 0,
                "control: `git check-ignore -v -z` produced {} NUL-separated fields for {} \
                 excluded path(s), which is not a whole number of four-field records. This \
                 parser does not understand the answer it got, and an answer it cannot read \
                 is not an answer that the exclusion sources were legitimate",
                fields.len(),
                ignored_paths.len()
            );
            // **And then the COUNT, because a partial answer is the shape
            // this command actually produces.** Every path fed in came from a
            // producer that already called it excluded, so `check-ignore`
            // owes exactly one record for each -- and a truncated answer,
            // which git writes when it dies mid-stream, would otherwise leave
            // the unreported tail unaudited while every remaining record
            // looked perfectly legitimate. This is the assert that catches a
            // 128-with-partial-stdout; the status assert above catches a
            // 129-with-nothing.
            assert!(
                fields.len() == ignored_paths.len() * 4,
                "the_probe_scan_requires_git: `git check-ignore -v -z` returned {} record(s) \
                 for the {} excluded path(s) it was given in {root:?}. Every path fed to it \
                 came from a producer that had already called it excluded, so a short answer \
                 is a TRUNCATED answer -- git writes exactly that, stdout cut mid-stream, when \
                 it dies part-way through -- and the paths it never got to are then excluded \
                 by a source nothing has checked. A partial answer is not an answer that the \
                 exclusion sources were legitimate.",
                fields.len() / 4,
                ignored_paths.len()
            );
            // The legitimate sources, by RELATIVE path and unfiltered by
            // `is_file`. Both of those used to be wrong in the same
            // direction, as false reds rather than as holes: the comparison
            // was `PathBuf` equality against `cached`, which is (a) filtered
            // by `.is_file()`, so deleting a tracked `.gitignore` from disk
            // while git still honours the committed copy made it "not
            // legitimate", and (b) lexically exact, so a tracked
            // `Docs/.gitignore` reported by `check-ignore` as `docs/.gitignore`
            // -- the same file on this platform -- red-lined the suite.
            // Comparing the relative spellings case-insensitively is right on
            // Windows, where this ships, and on a case-sensitive filesystem
            // it widens the accepted set only to sibling paths that git would
            // itself have to be tracking.
            let cached_rel: Vec<String> = cached_raw
                .split(|b| *b == 0)
                .filter(|s| !s.is_empty())
                .map(|raw| String::from_utf8_lossy(raw).replace('\\', "/").to_ascii_lowercase())
                .collect();
            // **The legitimacy rule is a NAMED PREDICATE, and it is exercised
            // on a fixture before it is trusted on the tree.**
            //
            // Everything else added to this test watches the audited set
            // SHRINK: `unaudited`, both coverage asserts, the record-count
            // assert and the status assert all fire when something reaches
            // `check-ignore` that should have and did not. Not one of them
            // watches the other direction -- an edit that WIDENS what counts
            // as a legitimate exclusion source. Measured: deleting the single
            // conjunct `&& tracked.iter().any(..)` below, with a real probe in
            // an untracked `docs/scratch_note.md` hidden by an UNTRACKED
            // `docs/.gitignore`, SURVIVED the filtered run in debug AND in
            // `--release` with the probe unseen, while the identical site on
            // pristine code KILLED at `foreign.is_empty()` in both profiles.
            // Every control above stayed green, because nothing shrank: the
            // same 29 776 paths were fed, answered and attributed. Only the
            // VERDICT moved.
            //
            // So the verdict gets its own second opinion, at the point of
            // decision rather than as another whole-scan invariant. The four
            // asserts below call this predicate on a hand-built fixture whose
            // answers are known independently of any repository state, and
            // each one is chosen so that exactly ONE conjunct can produce it.
            // Deleting any conjunct turns one of them red immediately, on a
            // pristine tree, with no probe planted and nothing to notice.
            let fold = |text: &str| text.replace('\\', "/").to_ascii_lowercase();
            let legitimate = |source: &str, tracked: &[String]| -> bool {
                let as_path = root.join(source);
                let source_rel = fold(source);
                let named_gitignore = std::path::Path::new(source)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| fold(name) == fold(".gitignore"));
                named_gitignore
                    && as_path.starts_with(&root)
                    && tracked.iter().any(|entry| *entry == source_rel)
            };
            // The fixture's "tracked" set is built HERE, not read from the
            // repository, and it deliberately contains all four spellings the
            // asserts ask about -- so membership cannot be what distinguishes
            // three of them, and the conjunct under test is the only thing
            // that can.
            let outside_root = root
                .parent()
                .expect("the repository root has a parent directory")
                .join("not-this-repository")
                .join(".gitignore");
            let outside_root = outside_root.to_string_lossy().into_owned();
            let fixture_tracked: Vec<String> = vec![
                fold("deskwarden/.gitignore"),
                fold(".git/info/exclude"),
                fold(&outside_root),
            ];
            // Liveness. Without this the predicate could be `false` and the
            // three rejections below would all pass vacuously.
            assert!(
                legitimate("deskwarden/.gitignore", &fixture_tracked),
                "control: the legitimacy predicate rejected a source that is named \
                 `.gitignore`, lies under {root:?} and is present in the tracked set it was \
                 given. It now rejects everything, so the three rejections below prove nothing \
                 and the whole audit is a formality."
            );
            // THE FINDING. `docs/.gitignore` is named correctly and lies under
            // `root`; the ONLY thing that can reject it is the requirement
            // that the source be a TRACKED file, and the fixture set does not
            // contain it. This is the assert that a deletion of that conjunct
            // turns red.
            assert!(
                !legitimate("docs/.gitignore", &fixture_tracked),
                "control: the legitimacy predicate ACCEPTED an untracked `.gitignore`. An \
                 untracked `.gitignore` is a file that exists on disk, appears in no commit \
                 and shows up in no diff, so accepting one as an exclusion source means any \
                 untracked probe-bearing file can be removed from every listing this test has \
                 for the cost of one two-line file that review never sees. The requirement \
                 that an exclusion source be tracked is the whole reason this audit exists."
            );
            // `.git/info/exclude` is IN the fixture set and under `root`, so
            // only the file-name test can reject it.
            assert!(
                !legitimate(".git/info/exclude", &fixture_tracked),
                "control: the legitimacy predicate ACCEPTED `.git/info/exclude`. It was handed \
                 to the predicate as a tracked path under the root precisely so that the file \
                 name is the only thing left to reject it -- and `.git/info/exclude` is the \
                 one-line, zero-diff, never-committed exclusion source this audit was written \
                 for."
            );
            // The absolute path is in the fixture set and is named
            // `.gitignore`, so only the containment test can reject it. This
            // is the shape `check-ignore` prints for a `core.excludesFile`.
            assert!(
                !legitimate(&outside_root, &fixture_tracked),
                "control: the legitimacy predicate ACCEPTED an absolute path outside {root:?}. \
                 `check-ignore` prints a `core.excludesFile` as the absolute path it was \
                 configured as, so containment is what distinguishes this repository's own \
                 ignore rules from a machine-wide setting nothing in this tree records."
            );
            let mut foreign: Vec<String> = Vec::new();
            let mut audited: std::collections::HashSet<&[u8]> = std::collections::HashSet::new();
            for record in fields.chunks(4) {
                audited.insert(record[3]);
                let source = String::from_utf8_lossy(record[0]).into_owned();
                let pathname = String::from_utf8_lossy(record[3]).into_owned();
                // The ONLY legitimate exclusion source: a `.gitignore` that is
                // itself a tracked file of this repository, named relative to
                // `root`. `.git/info/exclude` fails on the file name;
                // `core.excludesFile` fails on both -- `check-ignore` prints
                // it as the absolute path it was configured as, which is not a
                // path under `root` and is not in `cached` either. An
                // UNTRACKED `.gitignore` fails on `cached`, which is the case
                // that would otherwise be a one-file, no-diff bypass wearing a
                // legitimate name.
                // **Both comparisons now fold by the SAME rule, and the
                // residual is stated rather than left as an asymmetry.** They
                // disagreed: the file name was matched case-SENSITIVELY
                // against `".gitignore"` while `source_rel` was matched
                // case-INSENSITIVELY against `cached_rel`, so one half of this
                // predicate believed `Docs` and `docs` were different files
                // and the other half believed they were the same. Reading a
                // predicate should not require knowing which half you are in,
                // so `fold` is applied to both.
                //
                // The residual, plainly: on a CASE-SENSITIVE checkout, a
                // tracked `Docs/.gitignore` makes an untracked
                // `docs/.gitignore` legitimate. Folding the file name does not
                // close that -- only a case-SENSITIVE path comparison would,
                // and that is the comparison this block deliberately gave up,
                // because on Windows (where this crate ships, and where the
                // two ARE one file) git reports a tracked `Docs/.gitignore` as
                // `docs/.gitignore` and an exact comparison red-lines the
                // suite for nothing. A configuration macro cannot arbitrate it
                // either, because this
                // crate refuses configuration-dependent compilation outright
                // (`nothing_in_this_crate_is_compiled_differently_when_it_is_tested`),
                // and rightly, since a suite green about a different program
                // is the failure mode all of this exists to prevent. So the
                // cost of the residual is: two `.gitignore` files whose paths
                // differ only in case, on a filesystem that distinguishes
                // them, one of the two committed. This tree has exactly one
                // tracked `.gitignore`, so the set of spellings available to
                // borrow is empty.
                if !legitimate(&source, &cached_rel) {
                    foreign.push(format!("{source} (excluding {pathname})"));
                }
            }
            assert!(
                foreign.is_empty(),
                "{} path(s) in {root:?} are excluded from BOTH untracked producers by a source \
                 that is not a tracked `.gitignore` of this repository: {:?}. \
                 `--others --exclude-standard` and `status --untracked-files=all` both honour \
                 `.git/info/exclude` and `core.excludesFile`, and neither is a tracked file, \
                 so ONE line in one of them removes an untracked probe-bearing file from every \
                 listing this test has -- at zero source edits, leaving no diff, and with the \
                 second opinion added for the untracked half subtracted from as well. If this \
                 named a global `core.excludesFile` you genuinely want, move the pattern into \
                 this repository's tracked `.gitignore`, where review can see it.",
                foreign.len(),
                foreign
            );
            // **THE YARDSTICK THAT DOES NOT MOVE WITH THE FED LIST.**
            //
            // `fields.len() == ignored_paths.len() * 4` above is
            // self-referential and always was: `fields` is `check-ignore`'s
            // answer TO `ignored_paths`, so it compares the child's output
            // against the child's own input. It detects the child
            // under-ANSWERING -- a truncated stdout, which is why it stays --
            // and it cannot detect the producers under-REPORTING, because
            // shrinking the fed list shrinks both sides of it together. It is
            // the same family as `scanned == files.len()` and `closes_read ==
            // gates_seen`.
            //
            // These two are not. The left side is the set of pathnames the
            // audit above actually attributed; the right side is a producer's
            // OWN parse, bound before the union and never fed to anything. An
            // edit that shrinks what reaches `check-ignore` -- the deleted
            // `!`, a truncated union, a pathspec, a `.filter()` -- shrinks the
            // left side and leaves the right side exactly where it was, so it
            // is named here rather than passing vacuously. There is no single
            // binding both sides read.
            //
            // Vacuous on a tree that ignores nothing, which is correct: with
            // nothing excluded there is nothing to attribute.
            let mut unaudited: Vec<String> = status_ignored
                .iter()
                .chain(listed_ignored.iter())
                .filter(|path| !audited.contains(*path))
                .map(|path| String::from_utf8_lossy(path).into_owned())
                .collect();
            unaudited.sort();
            unaudited.dedup();
            assert!(
                unaudited.is_empty(),
                "{} path(s) that `git status --ignored` or `git ls-files --others --ignored` \
                 called excluded in {root:?} were never attributed to any exclusion source: \
                 {:?}. Every path either producer names as excluded must be traced back to a \
                 tracked `.gitignore`, and one that never reached `git check-ignore` was not \
                 traced back to anything -- the audit reported green about a set it never \
                 examined. This assert takes its expectation from the producers' own parses \
                 rather than from the list that was fed to `check-ignore`, precisely so that \
                 an edit shrinking the fed list cannot shrink the expectation with it.",
                unaudited.len(),
                unaudited
            );
        }

        // **And the second opinion is UNIONED IN, not merely compared.** The
        // assert above detects a corrupted producer; this makes a corrupted
        // producer INERT. A pathspec on the `git ls-files` invocation alone
        // now removes nothing from the scan, because everything committed is
        // added back here -- so that single edit is both loud and useless,
        // and deleting the assert above on its own accomplishes nothing at
        // all. Every remaining route to hiding a tracked file has to corrupt
        // BOTH enumerations, at two separately written call sites, in
        // lockstep. That cost is stated in this test's doc and it is a lower
        // bound, not a proof.
        for path in committed.iter().chain(cached.iter()) {
            if !files.contains(path) {
                files.push(path.clone());
            }
        }

        // The untracked half's second opinion, checked and then unioned in on
        // exactly the same terms as the tracked half's two. `is_file` is
        // evaluated HERE rather than when the records were parsed, so a file
        // `git status` named and something else has since removed is simply
        // gone rather than a red; and a nested repository, which `status`
        // reports as one directory record, is not a file and is therefore
        // spoken for by `refused` above rather than by this list.
        let untracked_present: Vec<&std::path::PathBuf> =
            untracked_second.iter().filter(|path| path.is_file()).collect();
        let unlisted_untracked: Vec<&std::path::PathBuf> =
            untracked_present.iter().copied().filter(|path| !files.contains(path)).collect();
        assert!(
            unlisted_untracked.is_empty(),
            "{} untracked, non-ignored file(s) are present in the working tree and were named \
             by `git status --porcelain --untracked-files=all` but are NOT in the list this \
             test is about to scan, the first of them {:?}. The two enumerations are two \
             separately spelled questions put to git about the same half of the tree, so this \
             is what a PATHSPEC added to the `git ls-files --others --exclude-standard` \
             invocation in `tracked_files` looks like -- until this cross-check existed that \
             single edit was invisible, because `ls-tree -r HEAD` and `ls-files --cached` both \
             speak only for the tracked half",
            unlisted_untracked.len(),
            unlisted_untracked.first()
        );
        for path in untracked_present.iter().copied() {
            if !files.contains(path) {
                files.push(path.clone());
            }
        }

        // **Three needles over the same bytes, and no encoding DECISION.**
        // The byte search catches a probe in any encoding whose bytes for
        // ASCII are ASCII -- UTF-8, Latin-1, Windows-1252 -- and misses one
        // in any encoding that is not, of which the reachable case on
        // Windows is UTF-16. Measured: `docs/u16.md` written as a BOM plus
        // the probe in UTF-16LE was GREEN in both configurations (96 listed,
        // 96 scanned, no hit), while the byte-identical content as UTF-8 was
        // one hit. The scan worked; the encoding hid the file.
        //
        // That is not hypothetical here, because it lands on the file the
        // previous round cited as its own motivating example: the TRACKED
        // `deskwarden/installer/bootstrap-bw.ps1`, which Windows PowerShell
        // 5.1's `>`, `Out-File` and `Set-Content -Encoding Unicode`
        // regenerate as UTF-16LE BY DEFAULT. The old design red-lined that
        // file loudly for its encoding; this one scanned it and would have
        // found nothing in it, ever -- a loud false positive traded for a
        // silent true negative on the same file.
        //
        // So the same window search is run twice more, over the probe's
        // UTF-16LE and UTF-16BE transcodings. This adds two byte patterns,
        // not an encoding rule: nothing here inspects a file to decide what
        // it is, so the `is_source_text`/`MUST_BE_TEXT` shape three rounds
        // were spent removing is not on its way back. What it does NOT cover
        // is stated rather than implied: an encoding that is neither
        // ASCII-transparent nor UTF-16 -- UTF-32, EBCDIC, a compressed or
        // encrypted container -- still hides the bytes, as does any splitting
        // of the literal across a window boundary, which is what
        // `concat!` is FOR and is the remedy this test recommends.
        let probe_utf16le: Vec<u8> = PROBE.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let probe_utf16be: Vec<u8> = PROBE.encode_utf16().flat_map(u16::to_be_bytes).collect();
        let needles: [(&'static str, &[u8]); 3] = [
            ("as bytes", PROBE.as_bytes()),
            ("encoded UTF-16LE", &probe_utf16le),
            ("encoded UTF-16BE", &probe_utf16be),
        ];
        // **Control over the TABLE, not over the locals beside it.**
        //
        // The previous spelling of this control compared `probe_utf16le`,
        // `probe_utf16be` and `PROBE.as_bytes()` to *each other*, and never
        // once mentioned `needles`. That left the table -- the single binding
        // both readers below take their needles from, and therefore the input
        // this whole test is *about* -- unpoliced, and the hole was one edit
        // wide and measured rather than argued: rewrite line one of the table
        // to `("as bytes", &probe_utf16le)` and the scan searches the UTF-16LE
        // transcoding twice and the raw bytes never. With a real probe
        // committed as a literal in the tracked
        // `deskwarden/examples/ui_preview.rs`, that single edit came back
        // `2247 passed; 0 failed` with the probe UNSEEN, in debug AND in
        // release. Every equality in the old control still held, because not
        // one of them was about the table -- the same shape as the seven
        // rounds before it, a shared input one level up from where the
        // controls were pointing.
        //
        // So the entries themselves are checked, and each is checked to be the
        // transcoding its own label claims. `PROBE` is ASCII, which is what
        // makes the two UTF-16 forms checkable against `PROBE.as_bytes()`
        // *without* going back through `encode_utf16` -- so this control does
        // not merely restate the two lines above it, it re-derives them by a
        // second route and compares.
        assert!(PROBE.is_ascii(), "control: the probe is not ASCII, so the byte-level checks \
             below are not the transcodings they claim to check");
        assert!(
            needles[0].1 == PROBE.as_bytes()
                && needles[1].1 == probe_utf16le.as_slice()
                && needles[2].1 == probe_utf16be.as_slice(),
            "control: the needle table does not hold the three transcodings of the probe that \
             its labels say it holds, so both readers below are searching for something that \
             is not the probe. This is the exact one-edit hole the previous control could not \
             see: it compared the locals and never the table"
        );
        assert!(
            needles[1].1.len() == PROBE.len() * 2
                && needles[2].1.len() == PROBE.len() * 2
                && needles[1]
                    .1
                    .chunks(2)
                    .zip(PROBE.as_bytes())
                    .all(|(pair, byte)| pair.len() == 2 && pair[0] == *byte && pair[1] == 0)
                && needles[2]
                    .1
                    .chunks(2)
                    .zip(PROBE.as_bytes())
                    .all(|(pair, byte)| pair.len() == 2 && pair[0] == 0 && pair[1] == *byte),
            "control: the second and third needles are not the UTF-16LE and UTF-16BE \
             transcodings of the probe, checked byte by byte against `PROBE` itself rather \
             than against the `encode_utf16` call that produced them"
        );
        // And they are three DIFFERENT byte strings of non-zero length.
        // Without this, two transcodings that silently collapsed to the same
        // bytes -- or to an empty window, which `windows(0)` would make match
        // nothing at all -- would look like three passes while being one.
        assert!(
            !needles[0].1.is_empty()
                && needles[0].1 != needles[1].1
                && needles[1].1 != needles[2].1
                && needles[0].1 != needles[2].1,
            "control: the needle table is not three distinct non-empty byte strings, so the \
             three passes below are not three passes"
        );

        let mut scanned = 0usize;
        let mut consumed_total: u64 = 0;
        let mut unreadable: Vec<String> = Vec::new();
        let mut scanned_this_file = false;
        for file in &files {
            // **Bytes, no extension list, and no encoding rule.** This is a
            // byte-window search; it never needed UTF-8. Every listed file is
            // opened and searched, the icon and the four `.ttf` fonts and the
            // two built installers included -- so the "five binary assets are
            // listed and then not scanned" cost the previous round disclosed
            // is gone, a UTF-16LE `.ps1` regenerated by PowerShell 5.1 is
            // scanned rather than reported, and appending three `\xff` bytes
            // to a file no longer takes it out of scope. See
            // the git-only listing for the measurements behind all three.
            //
            // **A read that FAILS is a FAILURE, named.** It used to be
            // skipped, on the reasoning that a tracked file can be momentarily
            // locked by an editor, an antivirus or a concurrent build, with
            // "the counters below" said to stop that tolerance from becoming a
            // silent pass. They did not, and the hole needed ZERO edits to the
            // source: plant the real probe in the tracked
            // `deskwarden/examples/ui_preview.rs`, then deny read on that ONE
            // file (`icacls ... /deny *S-1-1-0:(R)`), and the run is
            // `1 passed; 0 failed` with the probe unseen -- because the floor
            // was `scanned + 5 >= files.len()` and a floor cannot see a
            // subtraction of five. On Windows an editor lock, an AV hold or an
            // ACL is enough, and up to five files could take that exit
            // unremarked. The same hole cost ONE edit from the source side --
            // an `.and_then` on this very statement returning `Err` for one
            // file name -- and that mutation SURVIVED the whole suite in debug
            // and in release with a committed probe in the tree.
            //
            // So this is now exactly as loud as `the_probe_scan_requires_git`:
            // a file this test was asked to police and could not read is a
            // property it could not check, and a test that cannot check its
            // property says so instead of quietly checking a weaker one. The
            // remedy for a real lock is to re-run, not to tolerate it.
            let (consumed, found) = match scan_for_needles(file, &needles) {
                Ok(result) => result,
                Err(error) => {
                    unreadable.push(format!("{} ({error})", file.display()));
                    continue;
                }
            };
            scanned += 1;
            // By absolute path, not by suffix: `ends_with("login_ui.rs")` is
            // satisfied by ANY checkout's copy of this file, so under a
            // substituted `root` this control reported that "the one file
            // this test exists to police was policed" while the real file
            // went unread. `this_file` is built from `CARGO_MANIFEST_DIR`
            // and is checked against the listing above, so a mutation that
            // moves it to a decoy tree reds that control instead.
            scanned_this_file |= *file == this_file;
            assert!(
                found.is_none(),
                "{} contains the assembled probe ({}). Any test that reads this tree back now \
                 allocates a copy of it and frees it on its own libtest thread, and the global \
                 scan cannot tell those bytes from an armed test's own -- which is a measured, \
                 intermittent false positive on this crate's security tests. Split the literal \
                 with `concat!`. If this named a BUILD ARTIFACT, it is not a finding about the \
                 source: a compiled artifact of this crate carries the assembled probe because \
                 that is what compiling a `concat!` produces. Point `CARGO_TARGET_DIR` outside \
                 the repository, or at `target/`, which `.gitignore` already names -- git \
                 therefore never offers such a file to this scan, and this scan does not sniff \
                 at any file's bytes to guess whose they are, because every sniff tried so far \
                 was also an evasion",
                file.display(),
                found.unwrap_or("")
            );

            // **The evidence, per file, against a source the reader cannot
            // fabricate.** `consumed` is what the read loop actually pulled
            // off the disk; `len()` here is a separate `stat` this test makes
            // for itself. A `scan_for_needles` that returns early -- for one
            // file name, for a size, for anything -- reports fewer bytes than
            // the file has and is named right here. The hit assert above has
            // already run, so a short count from a genuine hit cannot reach
            // this line.
            let on_disk = std::fs::metadata(file)
                .unwrap_or_else(|error| {
                    panic!(
                        "{} was scanned but could not be measured ({error}), so the byte \
                         evidence below cannot be checked against anything",
                        file.display()
                    )
                })
                .len();
            assert!(
                consumed == on_disk,
                "{} is {on_disk} bytes on disk but the scan consumed only {consumed} of them, \
                 so the probe could be sitting in the {} bytes that were never looked at. This \
                 is the control the previous round did not have: `scanned == files.len()` \
                 counted LOOP TRIPS, and a `scan_for_needles` that returns `Ok(None)` without \
                 reading anything satisfies a trip count for free -- measured, that one line \
                 SURVIVED in both profiles with a real committed probe unseen. A byte total \
                 checked against `std::fs::metadata` cannot be satisfied for free, because \
                 this number comes from a syscall the scan function never makes",
                file.display(),
                on_disk.saturating_sub(consumed)
            );
            consumed_total += consumed;
        }

        // The controls above are over the LIST. This is over what was
        // actually opened and scanned, so a tree in which every read failed
        // -- or in which only this file's read did -- is a failure and not a
        // green run. There is no longer any second, quieter category: a
        // listed file is either scanned or it failed to open, and the
        // not-source bucket that the `.bak` evasion hid in does not exist.
        // Every listed file must be scanned. NOT "all but five": that
        // tolerance was a zero-edit hole (see the read loop above), and both
        // controls here are now equalities rather than floors, because a floor
        // cannot see a subtraction.
        assert!(
            unreadable.is_empty(),
            "{} listed file(s) could not be OPENED, so the probe could be sitting in one of \
             them unread and this scan cannot say otherwise. This is not tolerated for the \
             same reason `the_probe_scan_requires_git` is not: a check that could not run over \
             part of the tree is not a check that passed over it. Measured: with the probe \
             planted in a tracked file and read denied on that one file, the old \
             five-file tolerance returned a full green. If a lock is genuinely transient -- an \
             editor, an antivirus, a concurrent build -- close it and re-run. The files: {:?}",
            unreadable.len(),
            unreadable
        );
        assert!(
            scanned == files.len() && scanned > 30,
            "control: {scanned} of the {} listed files were scanned, so the check above is a \
             check over a fraction of the tree",
            files.len()
        );
        assert!(
            scanned_this_file,
            "control: the file that declares the probe was listed but was not scanned, so the \
             one file this test exists to police was not policed"
        );

        // **And the same evidence in aggregate.** The per-file equality above
        // lives inside the loop, so it is only reached by a file the loop
        // reached; this one is computed from `files` afresh and catches a file
        // that was listed and then skipped entirely. It is deliberately a
        // second `metadata` pass rather than a sum accumulated alongside
        // `consumed_total`, so that the two sides of this equality come from
        // two different walks of the list.
        let expected_total: u64 = files
            .iter()
            .map(|file| {
                std::fs::metadata(file)
                    .unwrap_or_else(|error| {
                        panic!(
                            "{} is in the list this test scanned but could not be measured \
                             ({error})",
                            file.display()
                        )
                    })
                    .len()
            })
            .sum();
        assert!(
            consumed_total == expected_total && expected_total > 0,
            "the {} listed files hold {expected_total} bytes between them but the scan \
             consumed {consumed_total}, a shortfall of {}. Every listed file must be read \
             end to end; a byte that was never read is a byte the probe could be in. Unlike \
             `scanned` this is not a count of loop trips, and unlike anything the read loop \
             reports about itself it is measured by `std::fs::metadata` here, which is why a \
             silent-success return inside `scan_for_needles` cannot satisfy it",
            files.len(),
            expected_total.saturating_sub(consumed_total)
        );

        // **A SECOND READER, separately written, over the same files.**
        //
        // The byte total above closes the *silent* success return -- a bare
        // `return Ok((0, None))` at the top of `scan_for_needles` is now named,
        // with the file and the shortfall. It does NOT close a return that
        // LIES about the count, and that was measured rather than assumed:
        // `return Ok((std::fs::metadata(path)?.len(), None))`, one line, keeps
        // `unreadable` empty, `scanned` full, the per-file equality satisfied
        // and the aggregate satisfied, with a real committed probe unseen --
        // SURVIVED. Any quantity a single reader reports about itself can be
        // fabricated by that reader; the only thing a reader cannot fake is
        // another reader.
        //
        // So the bytes are searched twice, by two loops written out separately,
        // the way the file list is asked for by two separately written
        // `Command`s rather than one builder called twice. This one is not a
        // call to `scan_for_needles` and shares no line with it: its own
        // `File`, its own buffer at a DIFFERENT size (64 KiB, so a needle at a
        // 1 MiB boundary is mid-buffer here and split there, and vice versa),
        // its own carry computed from the same needles, its own byte total.
        // Corrupting the scan now costs TWO coordinated edits at two call
        // sites -- and either one alone is both loud and inert, because the
        // byte totals of the two passes are asserted equal to each other and
        // to the `metadata` sum.
        //
        // The cost is honest: every listed file is read twice. On this tree
        // that is ~10 MB and unmeasurable; on a force-added multi-gigabyte
        // artifact it doubles the disclosed 9.75 s release / 40.5 s debug.
        // Memory does not double -- this loop is chunked for the same reason
        // the other is, and there is no size predicate here either, because a
        // size predicate is the exact shape six rounds were spent deleting.
        //
        // **AND ITS OWN NEEDLES, which is this round's change.** As shipped,
        // the two "independent" readers both iterated `needles` and both
        // `max()`ed their carry out of it, so the two loops shared the one
        // binding the search is *about*: corrupt that table's first entry and
        // BOTH readers dutifully searched for the wrong bytes, agreed with
        // each other perfectly, agreed with both `metadata` walks, and passed.
        // Two readers over one needle table are one reader. The disclosed
        // "two coordinated edits, one in each loop" floor was, measured, ONE.
        //
        // So this loop builds its own table from `PROBE` by a DIFFERENT route:
        // an interleave over `PROBE.as_bytes()` rather than a fold over
        // `encode_utf16`, which is sound because `PROBE` is ASCII (asserted
        // above). The two tables are then asserted equal -- and note that the
        // assert is belt and the loop below is braces: even with this
        // cross-check deleted, this reader still searches the RIGHT bytes and
        // still names a file the first reader was steered past. What the two
        // readers now share is `PROBE` itself and the file list, and nothing
        // else; the file list already has three independently spelled
        // producers above, and `PROBE` is pinned by
        // `the_probe_is_the_string_this_test_was_built_around`.
        const VERIFY_CHUNK: usize = 64 << 10;
        let verify_utf16le: Vec<u8> =
            PROBE.as_bytes().iter().flat_map(|byte| [*byte, 0u8]).collect();
        let verify_utf16be: Vec<u8> =
            PROBE.as_bytes().iter().flat_map(|byte| [0u8, *byte]).collect();
        let verify_needles: [(&'static str, &[u8]); 3] = [
            ("as bytes", PROBE.as_bytes()),
            ("encoded UTF-16LE", &verify_utf16le),
            ("encoded UTF-16BE", &verify_utf16be),
        ];
        assert!(
            verify_needles.len() == needles.len()
                && verify_needles.iter().zip(needles.iter()).all(|(mine, theirs)| {
                    mine.0 == theirs.0 && mine.1 == theirs.1 && !mine.1.is_empty()
                }),
            "the two readers are not searching for the same thing: the first reader's needle \
             table is {:?} and the second reader's, derived from `PROBE` by a separate route, \
             is {:?}. One of the two tables has been steered off the probe",
            needles.iter().map(|(label, bytes)| (label, bytes.len())).collect::<Vec<_>>(),
            verify_needles.iter().map(|(label, bytes)| (label, bytes.len())).collect::<Vec<_>>()
        );
        let carry = verify_needles
            .iter()
            .map(|(_, bytes)| bytes.len())
            .max()
            .unwrap_or(0)
            .saturating_sub(1);
        assert!(
            carry >= PROBE.len(),
            "control: the second reader's carry is {carry} bytes, which is shorter than the \
             probe itself, so it could not span any boundary at all"
        );
        let mut verified_total: u64 = 0;
        for file in &files {
            use std::io::Read as _;

            let mut handle = std::fs::File::open(file).unwrap_or_else(|error| {
                panic!("{} could not be re-opened for the second pass ({error})", file.display())
            });
            let mut tail: Vec<u8> = Vec::with_capacity(carry + VERIFY_CHUNK);
            let mut buffer = vec![0u8; VERIFY_CHUNK];
            loop {
                let read = handle.read(&mut buffer).unwrap_or_else(|error| {
                    panic!("{} could not be re-read for the second pass ({error})", file.display())
                });
                if read == 0 {
                    break;
                }
                verified_total += read as u64;
                tail.extend_from_slice(&buffer[..read]);
                for (label, needle) in &verify_needles {
                    assert!(
                        needle.is_empty()
                            || !tail.windows(needle.len()).any(|window| window == *needle),
                        "{} contains the assembled probe ({label}), and it took a SECOND, \
                         separately written reader to see it -- the first pass reported this \
                         file clean. Whatever `scan_for_needles` is doing, it is not searching \
                         this file. Split the literal with `concat!`; if this named a build \
                         artifact, point `CARGO_TARGET_DIR` outside the repository",
                        file.display()
                    );
                }
                let keep = tail.len().min(carry);
                let drop_to = tail.len() - keep;
                tail.drain(..drop_to);
            }
        }
        assert!(
            verified_total == expected_total && verified_total == consumed_total,
            "the two readers did not cover the same bytes: the first consumed {consumed_total}, \
             the second {verified_total}, and `std::fs::metadata` says the {} listed files hold \
             {expected_total}. Two passes that disagree mean one of them skipped something, and \
             a skipped byte is a byte the probe could be in",
            files.len()
        );

        // **And the LIST is checked again, after both readers have finished.**
        //
        // Found by attacking this round's own design rather than by review.
        // The three producers above are cross-checked against each other at
        // the moment the list is BUILT, which polices the producers and not
        // the binding they produce. `files` is itself a shared input -- of
        // both readers AND of both `metadata` walks -- so a single
        // `files.retain(..)` or `files.remove(..)` inserted anywhere between
        // that cross-check and this line subtracts a file from all four at
        // once, and every equality above is then satisfied over the smaller
        // list for free: `scanned == files.len()` holds, both per-file byte
        // totals hold, both aggregates hold, and the two readers agree
        // perfectly about a tree with the probe-bearing file removed from it.
        // `scanned_this_file` pins only THIS file; any other tracked file
        // could go. That is the same shape as every defeat in this test's
        // history -- controls pointed at the derivation while the shared
        // binding one level up went unasserted -- and it was one edit.
        //
        // So the same question is asked after the fact, against paths captured
        // from separately spelled `git` invocations before the scan began. The
        // window that edit lived in is closed; the honest cost of hiding a
        // tracked file is back to corrupting all three producers in lockstep.
        let dropped: Vec<&std::path::PathBuf> = committed
            .iter()
            .chain(cached.iter())
            .chain(untracked_present.iter().copied())
            .filter(|path| !files.contains(path))
            .collect();
        assert!(
            dropped.is_empty(),
            "{} committed file(s) were in the list when the three enumerations were \
             cross-checked against each other and are NOT in it now that both readers have \
             finished, the first of them {:?}. The list is a shared input of both readers and \
             of both `metadata` walks, so one subtraction from it satisfies every byte \
             equality above over a smaller tree, silently",
            dropped.len(),
            dropped.first()
        );

        // **And the pin a THIRD time, after everything has been read.** The
        // two above cover the window before the listing and the window
        // between the listing and the controls; this one covers the rest of
        // the function, so there is no line in it at which a `let root = ...`
        // survives to the end. Same reason the list is re-checked after the
        // readers rather than only when it was built: an assertion is about
        // the instant it runs, and the shared input outlives the instant.
        assert_eq!(
            root.join(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .file_name()
                    .expect("the crate directory has a name")
            ),
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
            "the_probe_scan_reads_the_wrong_tree: `root` is {root:?} now that both readers \
             have finished, which is not the parent of this crate's own compiled-in \
             directory. Everything this test just read, cross-checked and re-checked was read \
             from a tree that is not this repository"
        );
    }

    /// **The chunk carry, pinned.** [`scan_for_needles`] reads in 1 MiB chunks
    /// and carries `max_needle_len - 1` bytes of each chunk into the next, so a
    /// probe lying across a chunk boundary is still found. That carry was
    /// correct when it shipped and NOTHING TESTED IT: deleting it outright
    /// (`.saturating_sub(1) * 0`) SURVIVED the whole 2243-test suite in debug
    /// and in release, and with a real probe committed at a boundary offset the
    /// scan test itself came back `ok. 1 passed` with the probe unseen. The
    /// liveness pair at the identical site -- same mutant, probe at offset 0 --
    /// was KILLED, so the mutation was reachable and simply unobserved.
    ///
    /// The tracked tree has no file big enough to have a 1 MiB boundary in it,
    /// which is exactly why this needs a fixture rather than a committed one.
    /// The fixture is written to the OS temp directory and deleted before the
    /// assertions run -- **never into the tracked tree**, because a committed
    /// file with the probe at a boundary would be a file containing the probe,
    /// which is the property this module exists to deny. The needle comes from
    /// [`PROBE`], so this source file does not contain it whole either.
    ///
    /// What is covered: the probe straddling the first 1 MiB boundary; the
    /// same for the UTF-16LE transcoding, whose 74 bytes are what makes the
    /// carry `max()` over all three needles rather than the first one's length;
    /// the probe at the file's very first byte; the probe at its last bytes;
    /// and a clean file of the same size, which is the liveness control that
    /// stops all of the above from being four passes over a function that
    /// answers `Some` unconditionally. Every case also asserts the byte total,
    /// so the evidence the scan now relies on is pinned here too.
    #[test]
    fn the_chunk_carry_finds_a_probe_that_straddles_a_chunk_boundary() {
        // This test writes the assembled probe into heap buffers. See the doc
        // on `hold_the_probe_lock`; it must not race an armed allocator watch.
        hold_the_probe_lock();

        const CHUNK: usize = 1 << 20;
        const SIZE: usize = 2 << 20;

        let probe_utf16le: Vec<u8> = PROBE.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let probe_utf16be: Vec<u8> = PROBE.encode_utf16().flat_map(u16::to_be_bytes).collect();
        // The same three needles the scan uses, so `overlap` here is the same
        // 73 bytes it is there.
        let needles: [(&'static str, &[u8]); 3] = [
            ("as bytes", PROBE.as_bytes()),
            ("encoded UTF-16LE", &probe_utf16le),
            ("encoded UTF-16BE", &probe_utf16be),
        ];

        let plant = |offset: usize, needle: &[u8]| -> Vec<u8> {
            let mut bytes = vec![b'.'; SIZE];
            bytes[offset..offset + needle.len()].copy_from_slice(needle);
            bytes
        };

        let dir = std::env::temp_dir().join(format!(
            "deskwarden-chunk-carry-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("the OS temp directory is writable");

        let run = |name: &str, body: Vec<u8>| -> (usize, u64, Option<&'static str>) {
            let path = dir.join(name);
            std::fs::write(&path, &body).expect("the fixture is writable");
            let (consumed, found) =
                scan_for_needles(&path, &needles).expect("the fixture is readable");
            (body.len(), consumed, found)
        };

        // Straddling the first chunk boundary: 10 bytes before it, the rest
        // after. Without the carry the two halves are never in one window.
        let straddle = run("straddle.bin", plant(CHUNK - 10, PROBE.as_bytes()));
        // The 74-byte UTF-16LE needle split across the same boundary. A carry
        // sized from the FIRST needle rather than the longest would be 36 bytes
        // and would miss this one while still passing the case above.
        let straddle_utf16 = run("straddle-utf16le.bin", plant(CHUNK - 37, &probe_utf16le));
        // The first byte of the file: found in the first chunk, before any
        // carry exists. This is the liveness half -- it is what the deleted
        // carry still finds, so a case that fails here is a broken fixture and
        // not a broken carry.
        let first = run("first.bin", plant(0, PROBE.as_bytes()));
        // The last bytes of the file, in a final short read.
        let last = run("last.bin", plant(SIZE - PROBE.len(), PROBE.as_bytes()));
        // The control: same size, no probe anywhere.
        let clean = run("clean.bin", vec![b'.'; SIZE]);

        // Deleted BEFORE the assertions, so a failure does not leave a file
        // holding the assembled probe behind in the temp directory.
        std::fs::remove_dir_all(&dir).expect("the fixture directory is removable");

        assert!(
            clean.2.is_none(),
            "control: a 2 MiB file with no probe in it was reported as containing one ({:?}), \
             so the four cases below are not evidence of anything",
            clean.2
        );
        // **The byte total a HIT must report, computed here from the offset
        // this test planted at.** `scan_for_needles` returns the moment a
        // window matches, so on a hit `consumed` is the end of the chunk in
        // which the needle completes, capped at the file: `CHUNK` for the
        // probe at byte 0, `SIZE` for the other three. What used to stand
        // below was `consumed == size || found.is_some()`, asserted nine lines
        // under `assert!(found.is_some(), ..)` -- a disjunction whose right
        // half this test had *just proved true* on all four cases, so it was
        // unconditionally satisfied and could not fail. It was a dead control
        // wearing the word "control", of exactly the kind this module's other
        // comments are about. This is an equality against a number the test
        // computes rather than one the reader reports, so an early return, a
        // short read, or a `consumed` fabricated from `metadata` is named.
        let stop_after = |offset: usize, len: usize| -> u64 {
            let end = offset + len;
            (end.next_multiple_of(CHUNK).min(SIZE)) as u64
        };
        for (label, (size, consumed, found), expected) in [
            (
                "straddling the first 1 MiB chunk boundary",
                straddle,
                stop_after(CHUNK - 10, PROBE.len()),
            ),
            (
                "straddling that boundary as UTF-16LE",
                straddle_utf16,
                stop_after(CHUNK - 37, probe_utf16le.len()),
            ),
            ("at the file's first byte", first, stop_after(0, PROBE.len())),
            (
                "at the file's last bytes",
                last,
                stop_after(SIZE - PROBE.len(), PROBE.len()),
            ),
        ] {
            assert!(
                found.is_some(),
                "a probe {label} was NOT found in a {size}-byte fixture. The reader chunks at \
                 1 MiB and carries `max_needle_len - 1` bytes forward for exactly this case; \
                 measured, deleting that carry SURVIVED the entire suite, which is why this \
                 test exists. A tracked file large enough to have a chunk boundary in it would \
                 hide the probe across that boundary from the whole scan"
            );
            assert!(
                consumed == expected,
                "control: the fixture {label} is {size} bytes and the reader must stop at the \
                 end of the 1 MiB chunk the needle completes in, which is {expected} bytes, \
                 but it reported {consumed}"
            );
        }
        assert!(
            clean.1 == clean.0 as u64 && clean.0 == SIZE,
            "control: the clean 2 MiB fixture reported {} of {} bytes consumed, so the byte \
             evidence the scan asserts against is not what this reader actually reads",
            clean.1,
            clean.0
        );
    }

    /// **The probe itself, pinned** -- the last input the two readers of
    /// `no_source_file_in_this_crate_contains_the_assembled_probe` still share.
    ///
    /// Whether to build this took an argument, so the argument is written down
    /// rather than the conclusion. `PROBE` is not the guard; it is the guard's
    /// INPUT, and that is the whole distinction. Editing `"password"` to
    /// `"passwordX"` inside the `concat!` that declares it is ONE edit and reds
    /// NOTHING: the file scan, its second reader and the chunk-carry test all
    /// draw their needle from `PROBE`, so all three agree perfectly with each
    /// other about the wrong string. Measured, with a real probe committed as a
    /// literal in the tracked `deskwarden/examples/ui_preview.rs`: `2247
    /// passed; 0 failed`, probe UNSEEN. And in a diff that edit reads as a
    /// rename.
    ///
    /// Deleting a test is also one edit. The difference is legibility: a
    /// deletion is visible AS a deletion at review time, and a swapped string
    /// literal is not. This control converts a SILENT one-edit defeat into a
    /// LOUD two-edit one. That is its entire claim, and it does not claim to
    /// survive its own deletion -- nothing here does, or can.
    ///
    /// It pins by DIGEST rather than by a second spelling of the same words,
    /// deliberately. A reconstruction such as `concat!("deskwarden-drop-",
    /// "probe-master-", "password")` would be defeated by the SAME
    /// find-and-replace over `"password"` that defeats the original: one
    /// mechanical operation, not two independent edits, so it would raise the
    /// cost by nothing and would be theatre. A hex digest is not hit by that
    /// replace.
    #[test]
    fn the_probe_is_the_string_this_test_was_built_around() {
        // Nothing here puts the probe's bytes on the heap -- `Sha256::digest`
        // buffers into its own state and the hex string is not the probe -- but
        // the lock is cheap and this module's convention is that anything
        // naming `PROBE` takes it.
        hold_the_probe_lock();

        assert!(
            PROBE.len() == 37,
            "the probe is {} bytes, not the 37 every scan in this module was measured against",
            PROBE.len()
        );
        use sha2::Digest as _;
        let digest = format!("{:x}", sha2::Sha256::digest(PROBE.as_bytes()));
        assert!(
            digest == "94f3994cc703054bbed55ac331f24ed69cfe7fa9c4b5297c8cecf1bff52587e0",
            "the probe hashes to {digest}, which is not the value this test pins. `PROBE` is \
             the input from which every probe scan in this module derives its needle, so \
             changing it changes what all of them look for WITHOUT reddening any of them -- \
             measured, that one edit inside the `concat!` left a real committed probe unseen \
             with the whole suite green. If the change is deliberate, update this digest in \
             the same commit and say why in the message. If it is not, this is the test that \
             was supposed to notice."
        );
    }

    /// A heap copy of [`PROBE`], built *before* any watch is armed so that the
    /// temporaries of building it are never what a test observes.
    fn probe_password() -> String {
        String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8")
    }

    /// A form with the probe in **every** `String` field, not only `password`.
    ///
    /// The two `Drop` tests below drop the whole form, so what they can see is
    /// bounded by what `Drop` wipes. While that impl zeroized `password` alone,
    /// a copy of the master password in any other field was released to the
    /// allocator in the clear on both of those exits with the whole suite
    /// green. Filling all four is what makes the other three `zeroize` calls in
    /// `impl Drop for LoginForm` load-bearing: delete any one of them and the
    /// field it wiped frees its buffer with the probe still in it.
    ///
    /// The exhaustive destructuring in
    /// `a_successful_sign_in_wipes_the_password_while_the_window_stays_open` is
    /// what stops a newly added `String` field from quietly missing this list:
    /// it is a compile error there until someone decides.
    fn form_with_the_probe_in_every_string_field() -> LoginForm {
        LoginForm {
            server_choice: ServerChoice::default(),
            server_url: probe_password(),
            email: probe_password(),
            password: probe_password(),
            reveal_password: false,
            enable_hello: false,
            error: Some(probe_password()),
        }
    }

    /// **The exit the `close_on_success: false` host never takes.** A window
    /// the user typed a master password into and then closed: no answer ever
    /// arrives, so `apply_auth_result` never runs and `Drop` is the only thing
    /// standing between the plaintext and the allocator.
    #[test]
    fn a_closed_window_does_not_release_the_master_password_in_the_clear() {
        let bare = probe_password();
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "control: an ordinary String's plaintext went past the allocator unnoticed, so the \
             assertion below is about an instrument that sees nothing"
        );

        let form = form_with_the_probe_in_every_string_field();
        assert!(
            !plaintext_reached_the_allocator(move || drop(form)),
            "a LoginForm released the plaintext master password to the allocator -- this is the \
             exit taken by every user who typed a password and then closed the window. Every \
             String field of the form holds the probe here, so this fires for a copy left in \
             `server_url`, `email` or `error` just as it does for `password` itself"
        );
    }

    /// The other exit no answer reaches: an unwind out of the event loop.
    ///
    /// The deliberate panic prints through the default hook. Replacing the hook
    /// would be a process-global change and this suite runs its tests in
    /// parallel, so the noise is left in rather than raced over.
    #[test]
    fn an_unwind_does_not_release_the_master_password_in_the_clear() {
        let bare = probe_password();
        assert!(
            plaintext_reached_the_allocator(move || {
                let _ = std::panic::catch_unwind(AssertUnwindSafe(move || {
                    let _held = bare;
                    panic!("deliberate: unwinding past a live master password");
                }));
            }),
            "control: an ordinary String unwound past the allocator unnoticed, so the assertion \
             below is about an instrument that sees nothing"
        );

        let form = form_with_the_probe_in_every_string_field();
        assert!(
            !plaintext_reached_the_allocator(move || {
                let _ = std::panic::catch_unwind(AssertUnwindSafe(move || {
                    let _held = form;
                    panic!("deliberate: unwinding past a live master password");
                }));
            }),
            "an unwind out of the login window released the plaintext master password (from \
             any String field of the form -- all four hold the probe here)"
        );
    }

    /// **The leak this module was written for, in the shape the single-window
    /// host runs it.**
    ///
    /// `app_window` builds the login frame with `close_on_success: false` and
    /// move-captures it -- `LoginForm` and all -- into a window that goes on to
    /// be the spinner and then the vault. Its success handling takes the token
    /// out of the shared cell and advances the stage; it drops nothing. So on
    /// that host `Drop` does not run until the user quits the app, and anything
    /// left in `form.password` when the token is produced lives for the entire
    /// vault session.
    ///
    /// This drives that shape: apply a successful answer, take the token, drop
    /// nothing -- and then look at the form that is still alive.
    #[test]
    fn a_successful_sign_in_wipes_the_password_while_the_window_stays_open() {
        // **The positive control this test spent three revisions without.**
        // Its two siblings each have one; this one did not, which is the
        // structural reason both vacuities below survived for as long as they
        // did. Everything measured here is measured with an instrument that
        // has just been shown to see a plain `String`'s buffer go past.
        let bare = probe_password();
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "control: an ordinary String's plaintext went past the allocator unnoticed, so \
             every assertion below is about an instrument that sees nothing"
        );

        let mut form = LoginForm::default();
        form.password = probe_password();

        // Built before the watch is armed, so this String's own allocation is
        // nothing the measurement below is about.
        let answer = Ok("session-token".to_string());

        // **The stated property, measured directly: signing in does not
        // release the plaintext to the allocator.**
        //
        // This used to be measured only after the fact, on the buffer left in
        // the field -- which says nothing at all about a wipe that *frees* the
        // plaintext-bearing allocation instead of overwriting it. Each of
        // `form.password = String::new()`,
        // `mem::replace(&mut form.password, String::new())` and
        // `clear(); shrink_to_fit()` does exactly that; each is a leak lasting
        // the whole vault session on the `close_on_success: false` host; and
        // each left the entire suite green. The free happened in here, before
        // anything was armed, and the `mem::take` afterwards then yielded a
        // zero-capacity String whose drop deallocates nothing, so the
        // assertion at the end of this test passed vacuously. The third
        // variant is the one to fear -- `clear()` plus `shrink_to_fit()` reads
        // as an honest tidy-up.
        //
        // Arming around the call is free of unrelated traffic because the `Ok`
        // arm allocates and frees nothing of its own: it assigns `None` over an
        // `error` that is already `None`, moves the token out of `answer`, and
        // zeroizes in place.
        let mut outcome = None;
        assert!(
            !plaintext_reached_the_allocator(|| {
                outcome = Some(apply_auth_result(answer, &mut form));
            }),
            "the successful sign-in released the plaintext master password to the allocator. A \
             wipe that REPLACES the field (`= String::new()`, `mem::replace`, or `clear()` \
             followed by `shrink_to_fit()`) frees the buffer the plaintext is in rather than \
             overwriting it, and the freed bytes are still the password"
        );

        let Some(AuthOutcome::Succeeded(token)) = outcome else {
            panic!("a successful `bw unlock` produced no token, so this test proves nothing");
        };
        // The ordering the wipe depends on: the token is already out before
        // anything touches the password.
        assert_eq!(token, "session-token", "the produced token did not survive the wipe");

        // Nothing has dropped `form`, and on this host nothing will.
        assert!(
            form.password.is_empty(),
            "the master password is still in the form after a successful sign-in; on the \
             single-window host nothing drops the form until the app quits, so this is the \
             plaintext living for the whole vault session: {:?}",
            form.password
        );

        // ...and it was wiped, not merely truncated: the bytes are gone from
        // the allocation the String still owns. This is the in-place half --
        // `zeroize()` replaced by `clear()`, `truncate(0)` or `drain(..)`
        // leaves the plaintext sitting in a buffer that is freed later, when
        // the form finally dies.
        //
        // **Why the buffers are taken out of the form first.** This assertion
        // used to measure `drop(form)`, which runs `impl Drop for LoginForm`
        // -- and that zeroizes. The closure being measured performed the very
        // wipe it was trying to detect, so it could not fail for the reason it
        // states. `mem::take` moves each String -- the same allocation,
        // untouched -- out from under `Drop`, and `mem::forget` then makes sure
        // nothing else wipes them before the frees that are being watched.
        //
        // **And EVERY String field is taken, not just `password`.** With only
        // `password` taken, `mem::forget` meant the other three were never
        // freed at all, so no probe could see them even in principle: adding
        // `form.server_url = form.password.clone();` to the success arm left
        // the whole suite green. The exhaustive pattern below is the guard
        // against that returning -- it binds nothing, and exists so that adding
        // a field to `LoginForm` fails to compile here until whoever added it
        // decides whether it belongs in `stolen`. (By reference, because
        // `LoginForm` has a `Drop` impl and nothing can be moved out of it.)
        let LoginForm {
            server_choice: _,
            server_url: _,
            email: _,
            password: _,
            reveal_password: _,
            enable_hello: _,
            error: _,
        } = &form;
        let stolen = [
            std::mem::take(&mut form.password),
            std::mem::take(&mut form.server_url),
            std::mem::take(&mut form.email),
            form.error.take().unwrap_or_default(),
        ];
        std::mem::forget(form);

        // Control on the measurement, and the second thing closing the
        // reallocating wipes: a `stolen[0]` with no capacity is a `String`
        // whose drop frees nothing at all, so `!plaintext_reached_the_allocator`
        // would hold for the emptiest possible reason. A real wipe leaves the
        // allocation in place and only overwrites what is in it.
        assert!(
            stolen[0].capacity() >= PROBE.len(),
            "control: the password buffer taken out of the form has capacity {} -- the \
             allocation the probe would have found was already freed inside \
             `apply_auth_result`, so dropping this String deallocates nothing and the assertion \
             below cannot fail",
            stolen[0].capacity()
        );

        assert!(
            !plaintext_reached_the_allocator(move || drop(stolen)),
            "a String field of the form still holds the plaintext master password. If it is \
             `password`, the successful sign-in only emptied the String's length and the \
             plaintext is still in the allocation, reaching the allocator when the form finally \
             dies. If it is any other field, a copy of the master password was made into it and \
             lives exactly as long"
        );
    }
}

/// The seam [`build_login_frame`] opened, held from the source.
///
/// None of it can be observed by running anything: [`run_login_flow_for`]
/// blocks on a real winit event loop and opens a real OS window, so no test in
/// this crate calls it. What the split newly makes possible to get wrong is
/// the `close_on_success` gate -- a window that records a token and then does
/// not close is a window the user is stuck in forever, having signed in
/// successfully, and every existing test in this file passes through it.
#[cfg(test)]
mod login_frame_host_tests {
    fn source() -> &'static str {
        include_str!("login_ui.rs")
    }

    /// Everything before the first `#[cfg(test)]`. Split with `concat!` so the
    /// marker exists in the binary but appears in this file only where the
    /// real attributes are -- otherwise this needle would find ITSELF, above
    /// all the production code, and every slice below would be empty.
    ///
    /// **What is below the cut is invisible to every guard that uses this**,
    /// and the length check here does not change that: it holds for any file
    /// with a test module at all. The region below the cut is held instead by
    /// `nothing_but_gated_test_modules_lives_below_the_guards_cut`, which
    /// walks it in full and requires it to be test modules and nothing else.
    fn production() -> &'static str {
        let source = source();
        let end = source
            .find(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file");
        let production = &source[..end];
        assert!(
            !production.is_empty() && production.len() < source.len(),
            "control: the slice is empty or is the whole file, so it is not the production \
             half of anything"
        );
        production
    }

    /// The builder's body: from its signature to the host that follows it.
    fn builder_body() -> &'static str {
        let production = production();
        let at = production
            .find(concat!("pub fn build_login", "_frame("))
            .expect("no `build_login_frame` in this file");
        let end = production
            .find(concat!("pub fn run_login_flow", "_for("))
            .expect("no `run_login_flow_for` in this file");
        assert!(
            at < end,
            "control: `build_login_frame` is expected above `run_login_flow_for`"
        );
        &production[at..end]
    }

    /// The own-event-loop host's body: from its signature to the fatal wrapper.
    fn host_body() -> &'static str {
        let production = production();
        let at = production
            .find(concat!("pub fn run_login_flow", "_for("))
            .expect("no `run_login_flow_for` in this file");
        let end = production
            .find(concat!("pub fn run_login", "_flow("))
            .expect("no `run_login_flow` in this file");
        assert!(at < end, "control: the fatal wrapper is expected below the host");
        &production[at..end]
    }

    #[test]
    fn the_login_frame_is_built_without_opening_a_window() {
        // The CALL, not the words: this file's prose says "run_ui_native" in
        // doc comments, and a needle matching those would fail here for the
        // wrong reason.
        const CALL: &str = concat!("eframe::run_ui_", "native(");
        assert!(
            !builder_body().contains(CALL),
            "`build_login_frame` opens its own event loop, so `app_window` calling it would \
             nest one native event loop inside another -- which eframe cannot do, and which \
             is the entire reason this function exists separately from `run_login_flow_for`."
        );
        // Paired positive control: the loop really is in this file, in the
        // host, so the negative above is about WHERE it is rather than about a
        // needle that never matches anything.
        assert!(
            host_body().contains(CALL),
            "`run_login_flow_for` no longer opens a window at all, so nothing in this crate \
             shows the sign-in card outside the single startup window"
        );
        assert_eq!(
            production().matches(CALL).count(),
            1,
            "expected exactly one event loop in this file"
        );
    }

    #[test]
    fn the_own_window_host_closes_on_a_produced_token_and_reads_it_back() {
        let host = host_body();
        assert!(
            host.contains(concat!("handles.take_", "token()")),
            "`run_login_flow_for` never reads the token cell, so a completed sign-in reports \
             `None` -- which its two fatal callers treat as 'the user closed the window' and \
             end the process over: {host:?}"
        );
        // `true` for `close_on_success`, named by the comment that sits
        // immediately above the argument, because a bare `true,` in an
        // argument list names nothing and `pre_styled` is a `bool` two lines
        // above it.
        assert!(
            host.contains(concat!("// ...and a produced token is the end of the window,")),
            "the comment naming `run_login_flow_for`'s `close_on_success` argument is gone, \
             so the two bare `bool`s in this call are unlabelled and can be swapped silently: \
             {host:?}"
        );
    }

    #[test]
    fn recording_a_token_and_closing_the_window_are_separable() {
        let builder = builder_body();
        let record = concat!("*token_for_", "closure.borrow_mut() = Some(session_token);");
        let close = concat!("send_viewport_cmd(egui::ViewportCommand::", "Close);");
        let at = builder
            .find(record)
            .expect("the login frame no longer records the token it produced");
        let rest = &builder[at + record.len()..];
        // The window-ending command must be INSIDE the gate, not beside it.
        // Bounded to the region between recording the token and the next
        // statement block rather than a fixed byte window, which would either
        // overrun into unrelated arms or stop short of a reworded gate.
        let gate = concat!("if close_on_", "success {");
        let gate_at = rest
            .find(gate)
            .expect("the `close_on_success` gate is gone: the single window now closes the \
                     moment sign-in succeeds, which is the three-window flicker this change \
                     removed");
        let close_at = rest.find(close).expect(
            "nothing closes the login window when it produces a token, so \
             `run_login_flow_for` -- which returns only when its window closes -- never \
             returns and the app hangs on a SUCCESSFUL sign-in",
        );
        assert!(
            gate_at < close_at,
            "the close is not inside the `close_on_success` gate, so the single startup \
             window closes itself the instant the user signs in"
        );
        // Positive control: the two needles are really distinct positions in
        // a region that contains both, rather than one needle matching twice.
        assert_ne!(gate_at, close_at);
        assert_eq!(builder.matches(close).count(), 1, "expected one close command");
    }
}

/// **`bw status` is bounded, and the bound is a decision that can be read.**
///
/// The defect: `check_bw_status_details` is a bare `Command::output()`, and
/// the only thing that claimed to cover it was
/// `app_window::WORKING_DEADLINE` -- which bounds the WINDOW. When it fired
/// the `bw` child was still running and the user had watched a frozen spinner
/// through the whole budget.
///
/// Last module in the file on purpose: every source-position guard above
/// slices at the FIRST `#[cfg(test)]`, so a test module introduced higher up
/// would silently empty `production()` and vacate them all.
#[cfg(test)]
mod status_deadline_tests {
    use super::*;
    use std::sync::mpsc::Sender;
    use std::time::{Duration, Instant};

    fn answered(email: &str) -> BwStatusDetails {
        BwStatusDetails {
            status: BwStatus::Unlocked,
            user_email: Some(email.to_string()),
            server_url: Some("https://vault.example".to_string()),
        }
    }

    /// Control for every negative below: an answer that arrives inside the
    /// budget is passed straight through, so a `status_details_within` that
    /// simply always returned "unknown" would fail here rather than looking
    /// like a working timeout.
    #[test]
    fn an_answer_inside_the_budget_is_reported_verbatim() {
        let got = status_details_within(Duration::from_secs(30), |tx: Sender<BwStatusDetails>| {
            tx.send(answered("someone@example.test")).unwrap();
        });
        assert_eq!(got.status, BwStatus::Unlocked);
        assert_eq!(got.user_email.as_deref(), Some("someone@example.test"));
        assert_eq!(got.server_url.as_deref(), Some("https://vault.example"));
    }

    /// **The defect, as behaviour.** A `bw status` that never answers must
    /// cost the caller its budget and not a second more. Before the bound the
    /// only limit was the child's own, which is none.
    #[test]
    fn a_status_that_never_answers_costs_the_budget_and_not_the_child_s_lifetime() {
        let budget = Duration::from_millis(150);
        let started = Instant::now();
        let got = status_details_within(budget, |tx: Sender<BwStatusDetails>| {
            // Holds its sender for far longer than the budget, exactly as a
            // wedged `bw` child does. Never joined -- that is the point: the
            // caller must not be waiting on it.
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(20));
                let _ = tx.send(answered("late@example.test"));
            });
        });
        let waited = started.elapsed();

        assert_eq!(
            got, unknown_status_details(),
            "a `bw status` that did not answer must read as the same 'we do not know' a failed \
             spawn produces, not as a second unauthenticated-looking state"
        );
        assert!(
            waited >= budget,
            "the wait returned before its own budget ({waited:?} < {budget:?}), so this proves \
             nothing about a bound -- it would pass against a function that never waited at all"
        );
        assert!(
            waited < Duration::from_secs(10),
            "the caller waited {waited:?} on a sender that does not answer for 20s, so the \
             budget is not bounding anything and the spinner is frozen for the child's \
             lifetime -- the whole defect"
        );
    }

    /// A worker that dies without sending -- the panicked-thread case -- must
    /// be the same "unknown", and must NOT cost the full budget: `recv_timeout`
    /// reports `Disconnected` the moment the sender drops.
    #[test]
    fn a_worker_that_dies_without_answering_is_unknown_immediately() {
        let started = Instant::now();
        let got = status_details_within(Duration::from_secs(30), |tx: Sender<BwStatusDetails>| {
            drop(tx);
        });
        assert_eq!(got, unknown_status_details());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a dead worker made the caller sit out the whole budget rather than being noticed \
             when its sender dropped"
        );
    }

    /// The budget is honoured as given rather than clamped to some internal
    /// favourite: two different budgets must produce two different waits.
    #[test]
    fn a_longer_budget_really_does_wait_longer() {
        let wait_for = |budget: Duration| {
            let started = Instant::now();
            let _ = status_details_within(budget, |tx: Sender<BwStatusDetails>| {
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(20));
                    let _ = tx.send(answered("late@example.test"));
                });
            });
            started.elapsed()
        };
        let short = wait_for(Duration::from_millis(50));
        let long = wait_for(Duration::from_millis(600));
        assert!(
            long > short,
            "the budget is ignored: {long:?} for a 600ms budget is no longer than {short:?} for \
             a 50ms one, so `status_details_within` is waiting on something of its own choosing"
        );
    }

    /// **The wiring.** The pure decision above is worth nothing if the
    /// production call does not apply it. Deleting `STATUS_DEADLINE` from
    /// `check_bw_status_details_bounded`, or unwrapping it back to a bare
    /// `check_bw_status_details()`, fails here.
    #[test]
    fn the_bounded_form_applies_the_deadline_to_a_spawned_status() {
        let source: &str = include_str!("login_ui.rs");
        let body = source
            .split_once(concat!("pub fn check_bw_status_details_bo", "unded()"))
            .expect("the bounded form must still exist")
            .1
            .split_once(concat!("#[cfg(", "test)]"))
            .expect("a test module must still follow the production code")
            .0;
        assert!(
            body.len() < source.len(),
            "control: the split isolated a region rather than keeping the whole file"
        );
        assert!(
            body.contains(concat!("status_details_wi", "thin(STATUS_DEADLINE,")),
            "the bounded form no longer applies `STATUS_DEADLINE` -- so `bw status` is untimed \
             again and `app_window::WORKING_DEADLINE`'s third term is a fiction once more"
        );
        assert!(
            body.contains(concat!("thread::sp", "awn(")),
            "the bounded form no longer runs `bw status` on a thread, so `recv_timeout` has \
             nothing to time out ON and the bound cannot fire"
        );
    }

    /// The number itself, argued rather than asserted equal to itself.
    #[test]
    fn the_status_deadline_is_generous_against_a_real_bw_status_and_cheap_against_the_watchdog() {
        // ~2.39s measured warm (see `main`'s `account_details_source`).
        assert!(
            STATUS_DEADLINE >= Duration::from_secs(24),
            "{STATUS_DEADLINE:?} is less than ten times a measured `bw status`; a slow machine \
             would lose the toolbar's account name routinely"
        );
        assert!(
            STATUS_DEADLINE < crate::bw_serve::BACKEND_OP_TIMEOUT,
            "the account name is now budgeted as generously as starting the backend itself -- \
             but its failure only blanks a label, while the watchdog this is charged to can \
             throw away the whole sign-in"
        );
    }
    // -----------------------------------------------------------------
    // The region BELOW the cut -- the half no source guard in this file
    // reads.
    // -----------------------------------------------------------------

    /// The `cfg` attribute that makes a module test-only, split so this
    /// constant is not itself one and so it cannot be found by a guard
    /// looking for the real attributes.
    const BELOW_CUT_GATE: &str = concat!("#[cfg(", "test)]");

    /// The literal the source guards in this file cut the file at. Split for
    /// the same reason: an unsplit copy would BE the first occurrence, and
    /// every production slice in this file would come back empty.
    const BELOW_CUT_MARKER: &str = BELOW_CUT_GATE;

    /// Column-0 lines that live below the cut but are the CONTENTS OF A
    /// STRING LITERAL rather than source. Each is controlled below: it must
    /// still occur in this file exactly once, so a stale entry here cannot
    /// quietly widen the hole this test exists to close.
    const BELOW_CUT_STRING_LINES: &[&str] = &[
        concat!("ERROR bitwarden_crypto::keys::master_key: error=The decryption ", "operation failed"),
        concat!("ERROR bitwarden_core::client::internal: error=Cryptography error, The decryption ", "operation failed"),
        concat!("ERROR bitwarden_core::key_management::crypto: error=Cryptography error, The decryption ", "operation failed"),
        concat!("Cryptography error, The decryption ", "operation failed\";"),
    ];

    /// `true` for `mod NAME {`, `pub mod NAME {` and `pub(crate) mod NAME {`,
    /// and for nothing else. Deliberately exact rather than a `starts_with`:
    /// `mod x { fn escape() {} }` on one line is not a module opener as far
    /// as this walk is concerned, and must fail it.
    fn below_cut_is_module_opener(line: &str) -> bool {
        let t = line.strip_prefix("pub(crate) ").unwrap_or(line);
        let t = t.strip_prefix("pub ").unwrap_or(t);
        let Some(rest) = t.strip_prefix("mod ") else {
            return false;
        };
        let Some(name) = rest.strip_suffix(" {") else {
            return false;
        };
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }

    /// What this file's below-the-cut region is walked under.
    ///
    /// The walk itself is [`crate::below_cut::walk`] and is NOT written here.
    /// It used to be, inline in the guard below, in one of fifteen
    /// near-identical copies -- which is how the escaped-quote off-by-one in
    /// the brace matcher reached three files at once, and how the byte-offset
    /// close check that the shared walk carries failed to reach this file at
    /// all. This copy had the line walk and no close check, so a payload that
    /// closes the last test module with an INDENTED brace, plants a `pub fn`
    /// at file scope after it and rebalances with a column-0 `}` walked
    /// straight through: measured SURVIVING the whole suite at 2223 lib / 217
    /// bin / 0 failed / 0 warnings in both profiles, with the planted `pub fn`
    /// shipping in the lib's DEBUG LLVM IR. What the copies really disagreed
    /// about is this struct's worth of text, so that is what stayed local.
    ///
    /// `is_module_opener` is this file's OWN [`below_cut_is_module_opener`]
    /// and not [`crate::below_cut::is_module_opener`], deliberately: the
    /// `modules == column_zero_module_openers(..)` control below compares the
    /// walk's count against the other instance, so a one-edit widening of
    /// either predicate desynchronizes the two and reds the suite. Pointing
    /// the walk at the shared predicate would have made both sides move
    /// together and thrown that property away.
    const BELOW_CUT_RULES: crate::below_cut::WalkRules = crate::below_cut::WalkRules {
        // The region handed to the walk begins AT the marker, so the walk
        // starts unarmed and the first module's own gate is the one that
        // arms it -- nothing outside the region is taken on trust.
        gate: BELOW_CUT_GATE,
        gated_at_start: false,
        // This file's walk compared the TRIMMED line against the gate, so
        // that is what is preserved here rather than quietly strengthened.
        gate_at_column_zero: false,
        is_module_opener: below_cut_is_module_opener,
        string_lines: BELOW_CUT_STRING_LINES,
        top_level_item_note: "Every source guard in this file slices at the first test gate and reads only what is above it, so an item down here is read by none of them: it can duplicate a call site pinned at exactly one, or reintroduce a construct banned by name, and the suite stays green.",
        ungated_module_note: "A `pub(crate) mod ext { .. }` written down here is the same escape, one `mod` deep.",
    };

    /// `(visited, modules, closes, depth)` for the region below this file's
    /// cut, by the one shared walk.
    fn walk_below_the_cut(source: &str) -> (usize, usize, usize, usize) {
        let cut = source
            .find(BELOW_CUT_MARKER)
            .expect("the cut marker is checked by the caller");
        crate::below_cut::walk(&source[cut..], &BELOW_CUT_RULES)
    }

    /// **Below the cut there is nothing but test-only modules, and the cut is
    /// where every guard in this file believes it is.**
    ///
    /// This file guards a great deal of behaviour by slicing its own source at
    /// the first `cfg(test)` attribute and counting needles in the half ABOVE that cut.
    /// Two things can silently empty those guards, and neither of them changes
    /// a single guard's own text:
    ///
    /// 1. **Anything appended below the test modules is invisible to all of
    ///    them.** It is not counted, not forbidden and not read. Confirmed by
    ///    mutation, in this crate, tonight: a second production item appended
    ///    under the test module of `main.rs` -- a duplicate startup handoff,
    ///    the exact defect a guard pins at "exactly one" -- left the whole
    ///    binary suite green with zero warnings. The bottom of a file is a
    ///    natural place to add a helper, and until this test existed nothing
    ///    stopped it.
    /// 2. **The cut can move UPWARDS.** The slice is a `find` of a literal, so
    ///    that literal appearing in a comment or a string above the real test
    ///    modules truncates the production half and blinds every guard to
    ///    everything after it. This file already contains that literal in prose
    ///    elsewhere; the guards survive it today only because the marker they
    ///    match is spelled to avoid it, which is an accident of spelling and
    ///    not a check.
    ///
    /// So the whole region from the cut to EOF is walked and required to be a
    /// sequence of `#[cfg(test)]`-gated, column-0 module blocks and nothing
    /// else, and the cut itself is pinned against a production anchor that must
    /// still be found (a positive control) immediately above it.
    ///
    /// This is a source-analysis test, which is the class that has failed in
    /// this codebase repeatedly, so every part of it carries its own control:
    /// the anchor's occurrence count, the module count, the close count, the
    /// number of lines actually visited, and the string-literal exceptions.
    /// A walk that visited nothing would fail on all five.
    #[test]
    fn nothing_but_gated_test_modules_lives_below_the_guards_cut() {
        let source = include_str!("login_ui.rs");

        // 1. The cut lands where the guards think it does.
        let cut = source.find(BELOW_CUT_MARKER).unwrap_or_else(|| {
            panic!(
                "{BELOW_CUT_MARKER:?} is not in this file at all -- every source guard here \
                 slices at it, and a slice that cannot be made is a guard that reads nothing"
            )
        });
        assert!(
            cut > 0 && source.as_bytes()[cut - 1] == b'\n',
            "the cut landed in the MIDDLE of a line, so the marker was matched inside a \
             comment or a string literal rather than at a real declaration; that truncates \
             the production half and blinds every source guard in this file to everything \
             below the truncation point"
        );

        // 2. Positive control on where the cut is: the production half must
        //    still reach the LAST production item in the file. If the marker
        //    were matched earlier than the real test modules, this anchor
        //    would fall below the cut instead of just above it.
        const LAST_PRODUCTION_ITEM: &str = concat!("pub fn run_login_flow(account: Option<(&Path, &Account)>, ", "first_run: bool) -> String {");
        assert_eq!(
            source.matches(LAST_PRODUCTION_ITEM).count(),
            1,
            "control: {LAST_PRODUCTION_ITEM:?} is not in this file exactly once, so it no \
             longer pins anything -- repoint it at the last production item above the test \
             modules"
        );
        let anchor = source.find(LAST_PRODUCTION_ITEM).expect("counted just above");
        assert!(
            anchor < cut,
            "the last production item this control knows about is BELOW the cut, which means \
             the cut moved up and the production half every guard in this file reads is \
             truncated"
        );
        assert!(
            cut - anchor < 4_000,
            "the cut is more than 4000 bytes past the last production item this control knows \
             about: either production was appended below the anchor (repoint the anchor) or \
             the cut moved down"
        );

        // 3. The walk, run over an LF copy of this file and a CRLF copy of
        //    the same text, which must agree. Built BOTH ways rather than
        //    compared against the bytes on disk on purpose: this repository
        //    stores LF blobs and only `core.autocrlf=true` makes the working
        //    tree CRLF, so a control that asserted "this file is CRLF" would
        //    itself be a check that passes on one machine and fails on Linux
        //    CI.
        let lf = source.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        assert_ne!(
            lf, crlf,
            "control: the two copies are the same string, so comparing the walk over them \
             compares it with itself -- this file has no line endings at all"
        );
        let as_lf = walk_below_the_cut(&lf);
        let as_crlf = walk_below_the_cut(&crlf);
        assert_eq!(
            as_lf, as_crlf,
            "the walk gives a different answer on an LF copy of this file than on a CRLF \
             one, so something in it is sensitive to line endings"
        );
        // And the file as it really is on disk, whichever of the two that is.
        let as_on_disk = walk_below_the_cut(source);
        assert!(
            as_on_disk == as_lf || as_on_disk == as_crlf,
            "this file's line endings are mixed: the walk over it agrees with neither the \
             all-LF nor the all-CRLF copy of its own text"
        );
        let (visited, modules, closes, depth) = as_on_disk;

        // 4. The walk is not vacuous, and it finished.
        assert!(
            visited > 100,
            "control: the walk visited only {visited} lines below the cut, which is not a test \
             module's worth -- the slice is empty or nearly so and this test proves nothing"
        );
        assert_eq!(
            depth, 0,
            "a test module below the cut is never closed by a column-0 `}}`, so the walk ran \
             off the end of the file inside it and stopped inspecting top-level lines"
        );
        assert_eq!(
            modules, 12,
            "the number of top-level test modules below the cut changed. That is fine -- but \
             this count is the control that proves the walk really visited them, so update it \
             deliberately rather than loosening it"
        );
        assert_eq!(
            closes, modules,
            "control: every module the walk opened must also have been closed at column 0"
        );

        // The opener count, cross-checked against a SECOND instance of the
        // opener predicate. `column_zero_module_openers` uses
        // `below_cut::is_module_opener`; the walk used this file's own
        // `below_cut_is_module_opener`. Widening either one alone
        // desynchronizes them and fails here, which is the property that
        // sharing a single predicate would have cost.
        assert_eq!(
            modules,
            crate::below_cut::column_zero_module_openers(&source[cut..]),
            "the walk opened {modules} modules but there are {} column-0 gated module openers \
             below the cut -- the walk's opener predicate and \
             `below_cut::is_module_opener` no longer agree",
            crate::below_cut::column_zero_module_openers(&source[cut..])
        );

        // Controls on the walk itself. Without these it could be a no-op that
        // visits lines and asserts nothing.
        let appended = format!("{source}\npub fn sneaked() {{}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&appended)).is_err(),
            "control: the walk accepted a `pub fn` appended below the test modules, which is \
             the exact mutation it exists to catch"
        );
        // An INDENTED top-level item, which a column-0-only filter would miss.
        // The payload is an indented, GATED module opener and not a `struct`:
        // a struct is refused whether or not indentation is checked, because
        // it is not a module opener either way, so it would leave the
        // indentation rule unmeasured. This shape the opener predicate
        // accepts, so only the indentation rule can refuse it -- and the
        // trailing column-0 `}` makes the payload one the walk would
        // otherwise ACCEPT, so deleting the rule reds this control.
        let indented =
            format!("{source}\n{BELOW_CUT_MARKER}\n    mod sneaked_indented {{\n}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&indented)).is_err(),
            "control: the walk accepted an INDENTED, gated module opener appended below the \
             test modules, which a column-0-only filter would miss"
        );
        // A column-0 line INSIDE the last test module that this file does not
        // name in its string-literal allowance. The line is planted by
        // dropping the file's final column-0 `}` and writing it back after
        // the payload, so the braces still balance and the module's real
        // close is still the last line -- the ONLY thing that refuses it is
        // the allowance being an exact list rather than a permission.
        let without_final_brace = source
            .replace("\r\n", "\n")
            .strip_suffix("}\n")
            .expect("this file ends with a column-0 closing brace")
            .to_owned();
        let unlisted = format!("{without_final_brace}zz_not_source\n}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&unlisted)).is_err(),
            "control: the walk accepted a column-0 line inside a test module that this file's \
             string-literal allowance does not name, so the allowance is a permission and not \
             a list"
        );
        // Liveness control at the IDENTICAL site: the same planting, with a
        // line this file's allowance DOES name, is accepted. So the refusal
        // above is about the allowance and not about the planting having
        // broken the region.
        let listed = format!("{without_final_brace}{}\n}}\n", BELOW_CUT_STRING_LINES[0]);
        let cut_of_listed = listed
            .find(BELOW_CUT_MARKER)
            .expect("the marker survives the planting");
        assert!(
            crate::below_cut::try_walk(&listed[cut_of_listed..], &BELOW_CUT_RULES).is_ok(),
            "control: the walk refuses the planted region even when the planted line IS named \
             in the allowance, so the refusal above is not measuring the allowance"
        );
        let ungated = format!("{source}\nmod shipped {{\n}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&ungated)).is_err(),
            "control: the walk accepted an UNGATED module below the cut, which ships"
        );

        // And the one the line walk could not catch, which is why this file
        // stopped carrying its own: this file's own text with its last module
        // closed by an INDENTED brace, a `pub fn` at file scope after it, and
        // a column-0 `}` further down to rebalance the count. Perfectly
        // balanced source, no lexer trick -- every payload line is indented,
        // so the `depth == 1` branch skips it and the walk ends with
        // `closes == modules` and `depth == 0`. Measured SURVIVING the whole
        // suite here at 2223 lib / 217 bin / 0 failed / 0 warnings, and
        // shipping three times over in the lib's DEBUG LLVM IR. Only the
        // byte-offset close check the shared walk carries kills it.
        let balanced = format!(
            "{without_final_brace}    }}\n    pub fn sneaked(x: u64) -> u64 {{ x }}\n    \
             #[allow(dead_code)]\n    mod filler {{\n}}\n"
        );
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&balanced)).is_err(),
            "control: the walk accepted this file's last test module closed by an INDENTED \
             brace with a `pub fn` at file scope after it. That is the payload the byte-offset \
             close check exists for, and it is once again invisible"
        );
        for known in BELOW_CUT_STRING_LINES {
            assert_eq!(
                source.matches(known).count(),
                1,
                "control: the string-literal exception {known:?} is not in this file exactly \
                 once, so it is stale and is widening this check for nothing"
            );
        }
    }
}

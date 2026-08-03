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
    let mut cmd = match crate::bw_path::bw_command_in(data_dir) {
        Ok(cmd) => cmd,
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
        .unwrap_or(BwStatusDetails {
            status: BwStatus::Unauthenticated,
            user_email: None,
            server_url: None,
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
    let mut cmd = crate::bw_path::bw_command_in(data_dir)?;
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
fn run_bw_with_password(
    args: &[&str],
    password: &str,
    data_dir: Option<&Path>,
) -> Result<String, String> {
    let mut cmd = crate::bw_path::bw_command_in(data_dir)?;
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

/// Gap between a field's label and the field itself.
const LABEL_GAP: f32 = 7.0;
/// Gap between one label+field group and the next (and before the action
/// button). Deliberately much larger than [`LABEL_GAP`] -- roughly 3x -- so
/// each label reads as bound to the field under it rather than floating
/// between two of them.
const GROUP_GAP: f32 = 22.0;

/// Diameter of the in-flight spinner beside Continue.
///
/// Deliberately *not* [`theme::BUTTON_HEIGHT`]. egui allocates a `Spinner`
/// as a square and draws a ring of `size / 2 - 2` radius inside it, so
/// matching the button's 32px box gave a 28px ring — the same size as the
/// button by measurement, but visibly heavier beside it, because what the
/// eye weighs the button by is its 13px label rather than its bounding box.
/// Sized against the label instead; `ui.horizontal`'s `Align::Center` keeps
/// it centred on the taller button, and the row's height is still the
/// button's, so nothing reflows when it appears.
const AUTH_SPINNER_SIZE: f32 = 20.0;

/// What the custom titlebar asked for this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromeAction {
    None,
    Minimize,
    Close,
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
/// content, [`ChromeMetrics::LOGIN`]'s sizing, and `maximizable: false` --
/// this window is fixed-size, so ▢ stays the inert, ghosted affordance it
/// has always been. See that function for the shared implementation.
pub fn draw_window_chrome(ui: &mut egui::Ui, title: &str) -> ChromeAction {
    draw_window_chrome_with_extra(ui, title, ChromeMetrics::LOGIN, false, |_ui| {})
}

/// Same as [`draw_window_chrome`], but calls `extra_content` to draw
/// additional widgets in the bar between the title and the window controls
/// (used by the vault window's toolbar buttons: Lock, the account avatar,
/// and Sync), paints the bar/mark/title per `metrics` (see
/// [`ChromeMetrics`]), and -- when `maximizable` is true -- makes the ▢
/// control a real, clickable maximize/restore toggle instead of the
/// permanently-ghosted affordance both windows used to show unconditionally.
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

    // Close (✕).
    let close_rect = control(0);
    let close = ui.interact(close_rect, ui.id().with("chrome-close"), Sense::click());
    if close.hovered() {
        ui.painter()
            .rect_filled(close_rect, CornerRadius::ZERO, theme::CANVAS);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let c = close_rect.center();
    ui.painter().line_segment(
        [c + egui::vec2(-4.5, -4.5), c + egui::vec2(4.5, 4.5)],
        glyph_stroke,
    );
    ui.painter().line_segment(
        [c + egui::vec2(-4.5, 4.5), c + egui::vec2(4.5, -4.5)],
        glyph_stroke,
    );
    if close.clicked() {
        action = ChromeAction::Close;
    }

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
pub fn round_window_corners(window_title: &str) {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;

    unsafe {
        let hwnd: HWND = match FindWindowW(None, &HSTRING::from(window_title)) {
            Ok(hwnd) if !hwnd.is_invalid() => hwnd,
            _ => return,
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

            if status == BwStatus::Unauthenticated {
                if form.server_choice == ServerChoice::SelfHosted {
                    theme::field_label(ui, "Server URL");
                    ui.add_space(LABEL_GAP);
                    theme::text_field(ui, &mut form.server_url, false);
                    ui.add_space(GROUP_GAP);
                }
                theme::field_label(ui, "Email");
                ui.add_space(LABEL_GAP);
                theme::text_field(ui, &mut form.email, false);
                ui.add_space(GROUP_GAP);
            }

            // 3h's label row: field name left, account email right, so the
            // user can see whose vault they're about to open.
            ui.horizontal(|ui| {
                theme::field_label(ui, "Master password");
                if let Some(email) = account_email {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(email).size(11.0).color(theme::TEXT_GHOST));
                    });
                }
            });
            ui.add_space(LABEL_GAP);
            theme::password_field(ui, &mut form.password, &mut form.reveal_password);
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
                    ui.add(
                        egui::Spinner::new()
                            .size(AUTH_SPINNER_SIZE)
                            .color(theme::BLUE),
                    );
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
fn profile_dir_for(account: Option<(&Path, &Account)>) -> Option<PathBuf> {
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
pub fn run_login_flow_for(
    account: Option<(&Path, &Account)>,
    first_run: bool,
) -> Option<String> {
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
    // error text), so any fixed height either clips or leaves a gap.
    let mut window_height = 0.0f32;

    // Outcome of an in-flight `bw login`/`bw unlock` (see `spawn_auth`),
    // polled non-blockingly each frame. `auth_in_progress` gates every
    // control that could start a second one and drives the spinner beside
    // Continue.
    let (auth_tx, auth_rx) = std::sync::mpsc::channel::<Result<String, String>>();
    let mut auth_in_progress = false;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            // Starting size only; grown/shrunk to fit below.
            .with_inner_size([470.0, 588.0])
            // The design's window is a fixed composition; there is nothing
            // in it that grows, so resizing only breaks the layout.
            .with_resizable(false)
            .with_maximize_button(false)
            // The titlebar is the design's own (white, mark, ghost
            // controls), drawn by draw_window_chrome; the native frame
            // can't be themed into it.
            .with_decorations(false)
            // The taskbar icon (there is no native titlebar to show one);
            // eframe windows don't inherit the exe's icon resource.
            .with_icon(theme::window_icon()),
        ..Default::default()
    };

    let mut styled = false;

    let _ = eframe::run_ui_native("Log in to Deskwarden", options, move |ui, _frame| {
        if !styled {
            // egui applies a new font set at the *start* of the next frame,
            // not the one that calls set_fonts -- drawing Archivo-styled
            // text in this same frame would look up a family that doesn't
            // exist yet and panic. Skip drawing this frame; the real UI
            // starts on the next one, once the fonts are actually live.
            theme::paint_window_background(ui);
            theme::apply(ui.ctx());
            round_window_corners("Log in to Deskwarden");
            styled = true;
            ui.ctx().request_repaint();
            return;
        }

        match draw_window_chrome(ui, "Log in to Deskwarden") {
            ChromeAction::Close => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
            ChromeAction::Minimize => ui
                .ctx()
                .send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            ChromeAction::None => {}
        }

        egui::Frame::new()
            // Deeper at the bottom: the footer row's dropdown otherwise
            // sits too close to the window edge (3h pads the body 30px).
            .inner_margin(Margin {
                left: 26,
                right: 26,
                top: 24,
                bottom: 30,
            })
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());

                // Non-blocking: the worker reports here and the window keeps
                // painting (and animating its spinner) until it does.
                if let Ok(result) = auth_rx.try_recv() {
                    auth_in_progress = false;
                    match result {
                        Ok(session_token) => {
                            *token_for_closure.borrow_mut() = Some(session_token);
                            form.error = None;
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        Err(e) => {
                            // Raw CLI output to the log (it's the only
                            // diagnostic channel this console-less binary
                            // has), one actionable line to the window.
                            log::warn!("bw login/unlock failed: {e}");
                            form.error = Some(friendly_auth_error(&e));
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
                // footer is pinned above. Rounded up to whole pixels so a
                // sub-pixel wobble can't ping-pong the window size.
                let wanted = (flow_bottom + FOOTER_RESERVE).ceil();
                if (wanted - window_height).abs() > 0.5 {
                    window_height = wanted;
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                            470.0, wanted,
                        )));
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

                            // Handed to the worker, then wiped from the form
                            // immediately -- the buffer has served its
                            // purpose here either way, and not leaving a
                            // second live copy sitting in the widget while
                            // the CLI runs is strictly better. On failure
                            // this also clears the field, which the user has
                            // to retype anyway.
                            spawn_auth(
                                auth_tx.clone(),
                                args,
                                form.password.clone(),
                                enroll_hello,
                                profile_dir.clone(),
                            );
                            form.password.zeroize();
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
                            if let Some((config_dir, id)) = &hello_scope {
                                hello::unenroll_for(config_dir, id);
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
            });
    });

    // `None` means the user closed the window with the X button rather than
    // completing the flow. What that *costs* is the caller's to decide, and
    // is decided in exactly one place: [`run_login_flow`].
    let produced = token.borrow_mut().take();
    produced
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

    /// `run_login_flow_for`'s body, from its declaration to the fatal
    /// wrapper's -- the same slice `only_the_fatal_entry_point_can_end_the_
    /// process` uses, named once so the two cannot disagree.
    fn window_body() -> &'static str {
        let body = SOURCE
            .split_once("pub fn run_login_flow_for(")
            .expect("the cancellable entry point must exist")
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
            .split_once("pub fn run_login_flow_for(")
            .expect("the cancellable entry point must exist")
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

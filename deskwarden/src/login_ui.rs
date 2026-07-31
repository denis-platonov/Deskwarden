use crate::bw_path::bw_command;
use crate::hello::{self, HelloState};
use crate::theme;
use eframe::egui::{self, Color32, CornerRadius, Margin, Pos2, RichText, Sense, Stroke, Vec2};
use std::cell::RefCell;
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
    let mut cmd = match bw_command() {
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
    bw_status_stdout(None)
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
pub fn bw_logout() -> Result<(), String> {
    let output = bw_command()?
        .arg("logout")
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

/// Points the Bitwarden CLI at a self-hosted server.
///
/// Returns `Err` rather than panicking: a typo in a self-hosted URL is
/// ordinary user error and belongs inline in the login window (the same way
/// `run_bw_with_password` failures already are), not as a process-killing
/// panic with a Rust backtrace.
pub fn configure_server(url: &str) -> Result<(), String> {
    let output = bw_command()?
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
/// Bitwarden-signed (`bw_path::bw_command`), never a freshly-resolved one:
/// this is the single call site that hands over the master password, so it
/// must not be able to pick up a `bw.exe` that appeared after that check.
fn run_bw_with_password(args: &[&str], password: &str) -> Result<String, String> {
    let mut cmd = bw_command()?;
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
    if show_panel {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let half = (ui.available_width() - 30.0) / 2.0;
            hairline_segment(ui, half);
            ui.label(RichText::new("or").size(11.0).color(theme::TEXT_GHOST));
            hairline_segment(ui, half);
        });
        ui.add_space(10.0);

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
fn spawn_auth(
    tx: std::sync::mpsc::Sender<Result<String, String>>,
    args: Vec<String>,
    mut password: String,
    enroll_hello: bool,
) {
    std::thread::spawn(move || {
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let result = run_bw_with_password(&arg_refs, &password);

        if result.is_ok() && enroll_hello {
            match hello::enroll(&password) {
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
/// the resulting session token.
pub fn run_login_flow() -> String {
    let details = check_bw_status_details();

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
    let mut hello_state = hello::state();

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
                                Some(url) => match configure_server(&url) {
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
                            let enroll_hello = form.enable_hello
                                && hello_state.available
                                && !hello_state.enrolled;

                            // Handed to the worker, then wiped from the form
                            // immediately -- the buffer has served its
                            // purpose here either way, and not leaving a
                            // second live copy sitting in the widget while
                            // the CLI runs is strictly better. On failure
                            // this also clears the field, which the user has
                            // to retype anyway.
                            spawn_auth(auth_tx.clone(), args, form.password.clone(), enroll_hello);
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
                        match hello::unlock_password() {
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
                                    false,
                                );
                                form.error = None;
                                auth_in_progress = true;
                            }
                            Err(e) => {
                                log::warn!("Windows Hello quick unlock failed: {e}");
                                form.error = Some(e);
                                // A failed open deletes the blob (see
                                // hello::unlock_password); re-probe so the
                                // panel disappears rather than erroring
                                // forever.
                                hello_state = hello::state();
                            }
                        }
                    }
                    Some(LoginAction::LogOut) => match bw_logout() {
                        Ok(()) => {
                            log::info!("logged out at the user's request; showing sign-in");
                            // A sealed master password for an account the
                            // CLI no longer knows is a liability: drop the
                            // enrollment with the account.
                            hello::unenroll();
                            hello_state = hello::state();
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

    let produced = token.borrow_mut().take();
    match produced {
        Some(session_token) => session_token,
        None => {
            // The user closed the window with the X button rather than
            // completing the flow. There is nothing sensible to continue with
            // -- every downstream operation needs a session -- so exit
            // cleanly with a logged reason instead of a raw panic backtrace.
            log::error!("login window was closed without producing a session token; exiting");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

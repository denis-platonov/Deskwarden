use crate::bw_path::bw_command;
use crate::hello::{self, HelloState};
use crate::theme;
use eframe::egui::{self, CornerRadius, Margin, RichText, Sense, Stroke};
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

/// What the custom titlebar asked for this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromeAction {
    None,
    Minimize,
    Close,
}

/// Draws 3h's window chrome — the design's titlebar is a custom white bar
/// (15×17 mark, 12px title, ghost window controls), which the native
/// Windows frame cannot be themed into, so the window runs frameless and
/// this paints the chrome: full-window background, a 40px draggable
/// titlebar, a hairline under it, and — ▢ ✕ controls (▢ inert: the window
/// is fixed-size). Reserves the titlebar's space in `ui`'s layout; window
/// rounding comes from DWM (see [`round_window_corners`]).
pub fn draw_window_chrome(ui: &mut egui::Ui, title: &str) -> ChromeAction {
    let mut action = ChromeAction::None;
    let full = ui.max_rect();
    let bar = egui::Rect::from_min_max(full.min, egui::Pos2::new(full.max.x, full.min.y + 40.0));

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

    // Left: the mark and the title (3h: 15×17 mark, 12px 600 title).
    let mark_rect = egui::Rect::from_min_size(
        egui::Pos2::new(bar.min.x + 14.0, bar.center().y - 8.5),
        egui::Vec2::new(15.0, 17.0),
    );
    theme::paint_mark(ui.painter(), mark_rect);
    ui.painter().text(
        egui::Pos2::new(mark_rect.right() + 10.0, bar.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::new(12.0, egui::FontFamily::Name(theme::SEMIBOLD.into())),
        theme::TEXT_SECONDARY,
    );

    // Right: the three 40px control zones. Glyphs are drawn, not typed, so
    // they can't fall through to a fallback font's rendition.
    let control = |i: usize| {
        egui::Rect::from_min_max(
            egui::Pos2::new(bar.max.x - 40.0 * (i + 1) as f32, bar.min.y + 1.0),
            egui::Pos2::new(bar.max.x - 40.0 * i as f32, bar.max.y - 1.0),
        )
    };
    let glyph_stroke = Stroke::new(1.2, theme::TEXT_FAINT);

    // Close (✕).
    let close_rect = control(0);
    let close = ui.interact(close_rect, ui.id().with("chrome-close"), Sense::click());
    if close.hovered() {
        ui.painter()
            .rect_filled(close_rect, CornerRadius::ZERO, theme::CANVAS);
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

    // Maximize (▢) — drawn but inert: the window is fixed-size, so the
    // affordance is shown ghosted with no hover or click.
    let max_rect = control(1);
    ui.painter().rect_stroke(
        egui::Rect::from_center_size(max_rect.center(), egui::Vec2::splat(9.0)),
        CornerRadius::ZERO,
        Stroke::new(1.2, theme::TEXT_GHOST),
        egui::StrokeKind::Middle,
    );

    // Minimize (—).
    let min_rect = control(2);
    let minimize = ui.interact(min_rect, ui.id().with("chrome-min"), Sense::click());
    if minimize.hovered() {
        ui.painter()
            .rect_filled(min_rect, CornerRadius::ZERO, theme::CANVAS);
    }
    let m = min_rect.center();
    ui.painter().line_segment(
        [m + egui::vec2(-4.5, 0.0), m + egui::vec2(4.5, 0.0)],
        glyph_stroke,
    );
    if minimize.clicked() {
        action = ChromeAction::Minimize;
    }

    // Everything left of the controls drags the window.
    let drag_zone =
        egui::Rect::from_min_max(bar.min, egui::Pos2::new(bar.max.x - 120.0, bar.max.y));
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
) -> Option<LoginAction> {
    let mut action = None;

    // Brand lockup (3h: 38×44 mark, 25px wordmark, 10px tag).
    ui.horizontal(|ui| {
        theme::mark(ui, 44.0);
        ui.add_space(2.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(theme::bold("Deskwarden", 25.0).color(theme::INK));
            ui.label(theme::semibold("FILLS NATIVE WINDOWS", 10.0).color(theme::TEXT_FAINT));
        });
    });

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
            // 3h's card gap: 12px between the label row, the field, and the
            // action row.
            ui.spacing_mut().item_spacing.y = 12.0;

            if status == BwStatus::Unauthenticated {
                if form.server_choice == ServerChoice::SelfHosted {
                    theme::field_label(ui, "Server URL");
                    theme::text_field(ui, &mut form.server_url, false);
                }
                theme::field_label(ui, "Email");
                theme::text_field(ui, &mut form.email, false);
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
            theme::password_field(ui, &mut form.password, &mut form.reveal_password);

            // Quick-unlock opt-in, offered until enrolled. Enrollment
            // happens on the next successful password unlock, so the
            // password being sealed is one that provably works.
            if hello.available && !hello.enrolled {
                ui.checkbox(
                    &mut form.enable_hello,
                    RichText::new("Unlock with Windows Hello next time")
                        .size(12.0)
                        .color(theme::TEXT_MUTED),
                );
            }

            if theme::primary_button(ui, "Continue", Some("↵")).clicked() {
                action = Some(LoginAction::Submit);
            }
        });

    if let Some(err) = &form.error {
        ui.add_space(6.0);
        ui.label(RichText::new(err).size(12.0).color(theme::ERROR));
    }

    // 3h's alternative path: the "or" divider and the Windows Hello panel,
    // once quick unlock is enrolled.
    if hello.available && hello.enrolled && status != BwStatus::Unauthenticated {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let half = (ui.available_width() - 30.0) / 2.0;
            hairline_segment(ui, half);
            ui.label(RichText::new("or").size(11.0).color(theme::TEXT_GHOST));
            hairline_segment(ui, half);
        });
        ui.add_space(10.0);

        if hello_panel(ui) {
            action = Some(LoginAction::HelloUnlock);
        }
        if ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::H)) {
            action = Some(LoginAction::HelloUnlock);
        }
    }

    // Enter submits from anywhere in the form, same as clicking Continue --
    // 3h's Continue carries the ↵ affordance.
    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        action = Some(LoginAction::Submit);
    }

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

/// A fixed-width horizontal hairline, for the "or" divider.
fn hairline_segment(ui: &mut egui::Ui, width: f32) {
    let (rect, _) =
        ui.allocate_exact_size(egui::Vec2::new(width.max(0.0), 1.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, theme::HAIRLINE);
}

/// The 3h Windows Hello panel: blue-washed row with a padlock tile, "Use
/// Windows Hello", and the CTRL+H chip. Returns true when clicked.
fn hello_panel(ui: &mut egui::Ui) -> bool {
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
                    ui.label(
                        RichText::new("Face, fingerprint, or PIN")
                            .size(11.0)
                            .color(theme::TEXT_MUTED),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let chip = RichText::new("CTRL+H")
                        .size(10.0)
                        .family(egui::FontFamily::Monospace)
                        .color(theme::BLUE);
                    egui::Frame::new()
                        .fill(theme::CARD)
                        .corner_radius(CornerRadius::same(5))
                        .inner_margin(Margin::symmetric(7, 3))
                        .show(ui, |ui| {
                            ui.label(chip);
                        });
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

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([470.0, 560.0])
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
            .inner_margin(Margin::symmetric(26, 24))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                let action = draw_login_window(
                    ui,
                    status,
                    account_email.as_deref(),
                    &host,
                    hello_state,
                    &mut form,
                );

                let mut done = false;

                match action {
                    Some(LoginAction::Submit) => {
                        // The server must be configured before `bw login` --
                        // it's global CLI config. A bad or missing
                        // self-hosted URL is inline UI error, not a panic:
                        // bail out of this submit and let the user correct
                        // it.
                        let server_configured = if status == BwStatus::Unauthenticated {
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

                        if server_configured {
                            let result = match status {
                                BwStatus::Unauthenticated => run_bw_with_password(
                                    &["login", &form.email, "--raw"],
                                    &form.password,
                                ),
                                BwStatus::Locked | BwStatus::Unlocked => {
                                    run_bw_with_password(&["unlock", "--raw"], &form.password)
                                }
                            };

                            // Enrollment happens only on success and only
                            // before the wipe below: the password being
                            // sealed for Windows Hello is the one that just
                            // provably opened the vault. A failed enrollment
                            // is logged, not surfaced -- the unlock itself
                            // succeeded and the window is about to close.
                            if result.is_ok()
                                && form.enable_hello
                                && hello_state.available
                                && !hello_state.enrolled
                            {
                                match hello::enroll(&form.password) {
                                    Ok(()) => log::info!("Windows Hello quick unlock enrolled"),
                                    Err(e) => log::warn!(
                                        "could not enroll Windows Hello quick unlock: {e}"
                                    ),
                                }
                            }

                            // The master password has served its purpose
                            // either way: wipe the buffer instead of leaving
                            // it live in memory for the rest of the process's
                            // lifetime. On failure this also clears the
                            // field, which the user has to retype anyway.
                            form.password.zeroize();

                            match result {
                                Ok(session_token) => {
                                    *token_for_closure.borrow_mut() = Some(session_token);
                                    form.error = None;
                                    done = true;
                                }
                                Err(e) => {
                                    log::warn!("bw login/unlock failed: {e}");
                                    form.error = Some(e);
                                }
                            }
                        }
                    }
                    Some(LoginAction::HelloUnlock) => {
                        match hello::unlock_password() {
                            Ok(password) => {
                                match run_bw_with_password(&["unlock", "--raw"], &password) {
                                    Ok(session_token) => {
                                        *token_for_closure.borrow_mut() = Some(session_token);
                                        form.error = None;
                                        done = true;
                                    }
                                    Err(e) => {
                                        log::warn!("bw unlock via Windows Hello failed: {e}");
                                        form.error = Some(e);
                                    }
                                }
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

                if done {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
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
}

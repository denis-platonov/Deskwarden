use crate::bw_path::bw_command;
use crate::theme;
use eframe::egui::{self, Margin, RichText, Rounding, Stroke};
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

pub fn check_bw_status() -> BwStatus {
    check_bw_status_with_session(None)
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
    let mut cmd = match bw_command() {
        Ok(cmd) => cmd,
        Err(e) => {
            log::error!("cannot run `bw status`: {e}");
            return BwStatus::Unauthenticated;
        }
    };
    cmd.arg("status");
    if let Some(token) = session_token {
        cmd.env("BW_SESSION", token);
    }

    match cmd.output() {
        Ok(output) => parse_bw_status(&String::from_utf8_lossy(&output.stdout)),
        Err(e) => {
            log::error!(
                "failed to run `bw status` from the verified Bitwarden CLI path \
                 (see bw_path::resolve_bw_exe for where that path comes from): {e}"
            );
            BwStatus::Unauthenticated
        }
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

/// Opens a blocking egui window that shows a server-choice + email field
/// when `check_bw_status()` is `Unauthenticated`, or just a password field
/// when `Locked`/`Unlocked`; runs `bw login`/`bw unlock` accordingly and
/// returns the resulting session token.
pub fn run_login_flow() -> String {
    let status = check_bw_status();

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

    let mut self_hosted = false;
    let mut server_url = String::new();
    let mut email = String::new();
    let mut password = String::new();
    let mut error: Option<String> = None;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([420.0, 520.0]),
        ..Default::default()
    };

    let mut styled = false;

    let _ = eframe::run_simple_native("Log in to Deskwarden", options, move |ctx, _frame| {
        if !styled {
            // egui applies a new font set at the *start* of the next frame,
            // not the one that calls set_fonts -- drawing Archivo-styled
            // text in this same frame would look up a family that doesn't
            // exist yet and panic. Skip drawing this frame; the real UI
            // starts on the next one, once the fonts are actually live.
            theme::apply(ctx);
            styled = true;
            ctx.request_repaint();
            return;
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(theme::CANVAS)
                    .inner_margin(Margin::symmetric(28.0, 24.0)),
            )
            .show(ctx, |ui| {
                // Brand lockup (design 3g): mark beside the wordmark and tag.
                ui.horizontal(|ui| {
                    theme::mark(ui, 40.0);
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(theme::bold("Deskwarden", 24.0).color(theme::INK));
                        ui.label(
                            theme::semibold("FILLS NATIVE WINDOWS", 9.5).color(theme::TEXT_FAINT),
                        );
                    });
                });

                ui.add_space(16.0);

                // 3b's language: matches are counted but never named until the
                // vault opens -- unlocking is what this window is for.
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
                ui.label(theme::bold(title, 17.0).color(theme::INK));
                ui.label(RichText::new(subtitle).size(12.0).color(theme::TEXT_FAINT));

                ui.add_space(10.0);

                egui::Frame::none()
                    .fill(theme::CARD)
                    .rounding(Rounding::same(10.0))
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .inner_margin(Margin::same(16.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 6.0;

                        if status == BwStatus::Unauthenticated {
                            ui.checkbox(&mut self_hosted, "Self-hosted server");
                            if self_hosted {
                                theme::field_label(ui, "Server URL");
                                theme::text_field(ui, &mut server_url, false);
                            }
                            theme::field_label(ui, "Email");
                            theme::text_field(ui, &mut email, false);
                        }

                        theme::field_label(ui, "Master password");
                        theme::text_field(ui, &mut password, true);
                    });

                if let Some(err) = &error {
                    ui.add_space(6.0);
                    ui.label(RichText::new(err).size(12.0).color(theme::ERROR));
                }

                ui.add_space(12.0);

                let mut done = false;

                // Enter submits from anywhere in the form, same as clicking
                // Continue -- the design's fields all carry ↵ affordances.
                let submitted = theme::primary_button(ui, "Continue", Some("Enter")).clicked()
                    || ctx.input(|i| i.key_pressed(egui::Key::Enter));

                if submitted {
                    // A bad self-hosted URL is inline UI error, not a panic: bail
                    // out of this Continue click and let the user correct it.
                    let server_configured = if status == BwStatus::Unauthenticated
                        && self_hosted
                        && !server_url.is_empty()
                    {
                        match configure_server(&server_url) {
                            Ok(()) => true,
                            Err(e) => {
                                log::warn!("bw config server failed: {e}");
                                error = Some(e);
                                false
                            }
                        }
                    } else {
                        true
                    };

                    if server_configured {
                        let result = match status {
                            BwStatus::Unauthenticated => {
                                run_bw_with_password(&["login", &email, "--raw"], &password)
                            }
                            BwStatus::Locked | BwStatus::Unlocked => {
                                run_bw_with_password(&["unlock", "--raw"], &password)
                            }
                        };

                        // The master password has served its purpose either way:
                        // wipe the buffer instead of leaving it live in memory for
                        // the rest of the process's lifetime. On failure this also
                        // clears the field, which the user has to retype anyway.
                        password.zeroize();

                        match result {
                            Ok(session_token) => {
                                *token_for_closure.borrow_mut() = Some(session_token);
                                error = None;
                                done = true;
                            }
                            Err(e) => {
                                log::warn!("bw login/unlock failed: {e}");
                                error = Some(e);
                            }
                        }
                    }
                }

                if done {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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
}

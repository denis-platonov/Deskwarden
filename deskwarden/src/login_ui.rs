use eframe::egui;
use std::cell::RefCell;
use std::process::Command;
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
/// checks a cached token before trusting it. Spawn failure -- typically the
/// CLI not being on PATH -- is logged and reported as `Unauthenticated`
/// rather than panicking the whole app.
pub fn check_bw_status_with_session(session_token: Option<&str>) -> BwStatus {
    let mut cmd = Command::new("bw");
    cmd.arg("status");
    if let Some(token) = session_token {
        cmd.env("BW_SESSION", token);
    }

    match cmd.output() {
        Ok(output) => parse_bw_status(&String::from_utf8_lossy(&output.stdout)),
        Err(e) => {
            log::error!(
                "failed to run `bw status` (is the Bitwarden CLI installed and on PATH?): {e}"
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
    let output = Command::new("bw")
        .args(["config", "server", url])
        .output()
        .map_err(|e| {
            format!("failed to run `bw config server` (is the Bitwarden CLI on PATH?): {e}")
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
fn run_bw_with_password(args: &[&str], password: &str) -> Result<String, String> {
    let mut cmd = Command::new("bw");
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
        viewport: egui::ViewportBuilder::default().with_inner_size([360.0, 320.0]),
        ..Default::default()
    };

    let _ = eframe::run_simple_native("Log in to deskwarden", options, move |ctx, _frame| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("deskwarden");

            if status == BwStatus::Unauthenticated {
                ui.checkbox(&mut self_hosted, "Self-hosted server");
                if self_hosted {
                    ui.label("Server URL");
                    ui.text_edit_singleline(&mut server_url);
                }
                ui.label("Email");
                ui.text_edit_singleline(&mut email);
            }

            ui.label("Master password");
            ui.add(egui::TextEdit::singleline(&mut password).password(true));

            if let Some(err) = &error {
                ui.colored_label(egui::Color32::RED, err);
            }

            let mut done = false;

            if ui.button("Continue").clicked() {
                // A bad self-hosted URL is inline UI error, not a panic: bail
                // out of this Continue click and let the user correct it.
                let server_configured =
                    if status == BwStatus::Unauthenticated && self_hosted && !server_url.is_empty()
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
        assert_eq!(
            parse_bw_status(r#"{"status":"locked"}"#),
            BwStatus::Locked
        );
    }

    #[test]
    fn treats_unauthenticated_and_unparseable_output_as_unauthenticated() {
        assert_eq!(
            parse_bw_status(r#"{"status":"unauthenticated"}"#),
            BwStatus::Unauthenticated
        );
        assert_eq!(parse_bw_status(""), BwStatus::Unauthenticated);
        assert_eq!(parse_bw_status("command not found"), BwStatus::Unauthenticated);
    }
}

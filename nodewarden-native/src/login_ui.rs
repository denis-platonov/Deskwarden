use eframe::egui;
use std::cell::RefCell;
use std::process::Command;
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BwStatus {
    Unauthenticated,
    Locked,
    Unlocked,
}

pub fn check_bw_status() -> BwStatus {
    let output = Command::new("bw")
        .args(["status"])
        .output()
        .expect("failed to run `bw status` (is the Bitwarden CLI installed and on PATH?)");
    let text = String::from_utf8_lossy(&output.stdout);
    if text.contains("\"status\":\"unlocked\"") {
        BwStatus::Unlocked
    } else if text.contains("\"status\":\"locked\"") {
        BwStatus::Locked
    } else {
        BwStatus::Unauthenticated
    }
}

pub fn configure_server(url: &str) {
    let status = Command::new("bw")
        .args(["config", "server", url])
        .status()
        .expect("failed to run `bw config server` (is the Bitwarden CLI installed and on PATH?)");
    if !status.success() {
        panic!("`bw config server {url}` failed");
    }
}

/// Runs `bw` with the given args plus a password supplied via an
/// environment variable (`--passwordenv`), never as a bare CLI argument --
/// a bare-argument password would be visible to other processes/users
/// via the OS process list.
fn run_bw_with_password(args: &[&str], password: &str) -> Result<String, String> {
    let mut cmd = Command::new("bw");
    cmd.args(args);
    cmd.args(["--passwordenv", "NODEWARDEN_BW_PASSWORD"]);
    cmd.env("NODEWARDEN_BW_PASSWORD", password);
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

    let _ = eframe::run_simple_native("Log in to nodewarden", options, move |ctx, _frame| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("nodewarden-native");

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
                if status == BwStatus::Unauthenticated && self_hosted && !server_url.is_empty() {
                    configure_server(&server_url);
                }

                let result = match status {
                    BwStatus::Unauthenticated => {
                        run_bw_with_password(&["login", &email, "--raw"], &password)
                    }
                    BwStatus::Locked | BwStatus::Unlocked => {
                        run_bw_with_password(&["unlock", "--raw"], &password)
                    }
                };

                match result {
                    Ok(session_token) => {
                        *token_for_closure.borrow_mut() = Some(session_token);
                        done = true;
                    }
                    Err(e) => error = Some(e),
                }
            }

            if done {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    });

    let session_token = token
        .borrow_mut()
        .take()
        .expect("login flow closed without producing a session token");
    session_token
}

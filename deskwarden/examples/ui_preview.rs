//! Visual preview of the themed windows.
//!
//! Interactive:
//!
//! ```text
//! cargo run --example ui_preview            # the autofill overlay (design 2a)
//! cargo run --example ui_preview -- --login # the login/unlock window (design 3h)
//! ```
//!
//! The overlay closes on Enter/Esc/✕; the login preview just draws (its
//! Continue does nothing here -- no `bw` is spawned from a preview).
//!
//! Self-screenshotting (for reviewing the design implementation without a
//! human at the keyboard):
//!
//! ```text
//! cargo run --example ui_preview -- --screenshot
//! cargo run --example ui_preview -- --login --screenshot
//! ```
//!
//! renders the surface, saves it to `target/ui_preview_overlay.png` /
//! `target/ui_preview_login.png`, and exits.
//!
//! Both modes render the exact draw functions the app ships
//! (`overlay_ui::draw_overlay_card`, `login_ui::draw_login_window`), not
//! copies, so what this shows is what the real app shows. The pickers aren't
//! previewed: their windows are plain compositions of the same theme
//! widgets, and they need a live vault to have anything to list.

use deskwarden::hello::HelloState;
use deskwarden::login_ui::{self, BwStatus, LoginForm};
use deskwarden::{overlay_ui, theme};
use eframe::egui::{self, Margin};
use std::path::PathBuf;

fn main() -> eframe::Result {
    let screenshot = std::env::args().any(|a| a == "--screenshot");
    let signin = std::env::args().any(|a| a == "--signin");
    let login = signin || std::env::args().any(|a| a == "--login");

    let viewport = if login {
        // The real login window's size and chrome (login_ui::run_login_flow).
        egui::ViewportBuilder::default()
            .with_inner_size([470.0, 560.0])
            .with_icon(theme::window_icon())
    } else {
        egui::ViewportBuilder::default()
            .with_inner_size([396.0, 164.0])
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_icon(theme::window_icon())
    };
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let png_name = if signin {
        "target/ui_preview_signin.png"
    } else if login {
        "target/ui_preview_login.png"
    } else {
        "target/ui_preview_overlay.png"
    };

    eframe::run_native(
        "Deskwarden preview",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(Preview {
                login,
                signin,
                form: LoginForm::default(),
                screenshot_path: screenshot.then(|| PathBuf::from(png_name)),
                frames: 0,
                styled: false,
            }))
        }),
    )
}

struct Preview {
    /// Which surface this run previews: the login window or the overlay.
    login: bool,
    /// In login mode: preview the sign-in (unauthenticated) state instead of
    /// the unlock state, with the server dropdown and Hello opt-in visible.
    signin: bool,
    /// Form state for the login preview (typing works; Continue doesn't).
    form: LoginForm,
    /// `Some` in --screenshot mode: where the PNG goes.
    screenshot_path: Option<PathBuf>,
    frames: u32,
    /// Whether the theme has been applied yet. Done on the first update
    /// frame, not in the creation context, for the same reason as the real
    /// windows (see login_ui): eframe re-applies its own style after
    /// creation, and egui font sets go live a frame after `set_fonts`.
    styled: bool,
}

impl eframe::App for Preview {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.styled {
            theme::apply(ctx);
            self.styled = true;
            ctx.request_repaint();
            return;
        }
        self.frames += 1;

        if self.login {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::none()
                        .fill(theme::WINDOW_BG)
                        .inner_margin(Margin::symmetric(26.0, 24.0)),
                )
                .show(ctx, |ui| {
                    // Sample data mirroring the 3h mock (unlock: Hello shown
                    // as enrolled so the panel renders; sign-in: available
                    // but unenrolled so the opt-in and server dropdown
                    // render); actions are ignored -- a preview must never
                    // spawn `bw` or pop Hello.
                    let (status, email, hello) = if self.signin {
                        (
                            BwStatus::Unauthenticated,
                            None,
                            HelloState {
                                available: true,
                                enrolled: false,
                            },
                        )
                    } else {
                        (
                            BwStatus::Locked,
                            Some("a.novak@ledgerline.com"),
                            HelloState {
                                available: true,
                                enrolled: true,
                            },
                        )
                    };
                    let _ = login_ui::draw_login_window(
                        ui,
                        status,
                        email,
                        "vault.ledgerline.eu",
                        hello,
                        &mut self.form,
                    );
                });
        } else {
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(ctx, |ui| {
                    // The preview closes on the dismiss ✕ too, so the
                    // affordance can actually be clicked here rather than
                    // only looked at.
                    if overlay_ui::draw_overlay_card(
                        ui,
                        "ledgerline.exe",
                        "Ledgerline",
                        Some("a.novak@ledgerline.com"),
                    ) == overlay_ui::OverlayAction::Dismiss
                    {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
        }

        if let Some(path) = self.screenshot_path.clone() {
            // A couple of warm-up frames first, so fonts and layout have
            // settled before the capture.
            if self.frames == 3 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
            }
            let captured = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(image) = captured {
                save_png(&path, &image).expect("could not write the screenshot PNG");
                println!("wrote {}", path.display());
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            // Keep frames coming: a hidden/idle window repaints lazily, and
            // the screenshot round-trip needs the pump to keep turning.
            ctx.request_repaint();
        }

        if !self.login
            && ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter))
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn save_png(path: &PathBuf, image: &egui::ColorImage) -> Result<(), Box<dyn std::error::Error>> {
    let [w, h] = image.size;
    let mut data = Vec::with_capacity(w * h * 4);
    for p in &image.pixels {
        data.extend_from_slice(&p.to_array());
    }
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&data)?;
    Ok(())
}

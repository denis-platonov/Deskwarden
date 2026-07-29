//! Visual preview of the themed autofill overlay (design 2a).
//!
//! Interactive:
//!
//! ```text
//! cargo run --example ui_preview
//! ```
//!
//! opens the overlay card with sample data; Enter or Esc closes it.
//!
//! Self-screenshotting (for reviewing the design implementation without a
//! human at the keyboard):
//!
//! ```text
//! cargo run --example ui_preview -- --screenshot
//! ```
//!
//! renders the card, saves it to `target/ui_preview_overlay.png`, and exits.
//!
//! This renders the exact `overlay_ui::draw_overlay_card` the app ships (not
//! a copy), so what this shows is what a real match shows. The login and
//! picker windows aren't previewed here: they run real `bw` commands (and
//! `login_ui::run_login_flow` exits the process when closed without a
//! token), which a preview must not do.

use deskwarden::{overlay_ui, theme};
use eframe::egui;
use std::path::PathBuf;

fn main() -> eframe::Result {
    let screenshot = std::env::args().any(|a| a == "--screenshot");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([396.0, 164.0])
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top(),
        ..Default::default()
    };

    eframe::run_native(
        "Deskwarden preview",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(Preview {
                screenshot_path: screenshot.then(|| PathBuf::from("target/ui_preview_overlay.png")),
                frames: 0,
            }))
        }),
    )
}

struct Preview {
    /// `Some` in --screenshot mode: where the PNG goes.
    screenshot_path: Option<PathBuf>,
    frames: u32,
}

impl eframe::App for Preview {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frames += 1;

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                overlay_ui::draw_overlay_card(
                    ui,
                    "ledgerline.exe",
                    "Ledgerline",
                    Some("a.novak@ledgerline.com"),
                );
            });

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

        if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter)) {
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

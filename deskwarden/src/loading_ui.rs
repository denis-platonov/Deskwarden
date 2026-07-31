//! A small spinner window shown while a background thread does slow,
//! non-interactive startup work (currently: waiting for `bw serve` to become
//! ready after login).
//!
//! Without this, the gap between the login window closing and the tray icon
//! appearing -- up to ~28s on a cold `bw serve` start -- showed nothing from
//! Deskwarden at all. Whatever else happened to be on screen (a terminal, in
//! more than one report) filled that silence, reading as "the app opened an
//! empty terminal" rather than "the app is still starting up".

use crate::theme;
use eframe::egui::{self, Margin};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Receiver;

/// Shows a "Deskwarden" window with a spinner and `message` until `rx`
/// yields a value, then closes and returns it.
///
/// `rx` is expected to be the receiving half of a channel whose sending half
/// was handed to a `std::thread::scope`d worker thread computing `T` --
/// scoped rather than a bare `std::thread::spawn`, so the worker can borrow
/// data (e.g. `&VaultBridge`) from the caller's stack without needing it to
/// be `'static`. `rx` itself has no such borrow (an `mpsc::Receiver<T>` is
/// self-contained), so it's fine to move into this window's own `'static`
/// closure regardless of where it was created.
///
/// Panics if `rx` disconnects without ever sending a value -- that means the
/// worker thread panicked, which is a bug to surface loudly, not paper over.
pub fn show_while<T: Send + 'static>(message: &str, rx: Receiver<T>) -> T {
    let result: Rc<RefCell<Option<T>>> = Rc::new(RefCell::new(None));
    let result_for_closure = result.clone();
    let message = message.to_string();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([320.0, 150.0])
            .with_resizable(false)
            .with_icon(theme::window_icon()),
        ..Default::default()
    };

    let mut styled = false;

    let _ = eframe::run_ui_native("Deskwarden", options, move |ui, _frame| {
        if !styled {
            // egui applies a new font set at the *start* of the next frame,
            // not the one that calls set_fonts -- drawing Archivo-styled
            // text in this same frame would look up a family that doesn't
            // exist yet and panic. Skip drawing this frame; the real UI
            // starts on the next one, once the fonts are actually live.
            theme::paint_window_background(ui);
            theme::apply(ui.ctx());
            styled = true;
            ui.ctx().request_repaint();
            return;
        }

        if let Ok(value) = rx.try_recv() {
            *result_for_closure.borrow_mut() = Some(value);
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::CANVAS)
                    .inner_margin(Margin::same(24)),
            )
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(18.0);
                    theme::mark(ui, 32.0);
                    ui.add_space(14.0);
                    ui.add(egui::Spinner::new().size(22.0).color(theme::BLUE));
                    ui.add_space(10.0);
                    ui.label(theme::semibold(&message, 13.0).color(theme::TEXT_SECONDARY));
                });
            });

        if result_for_closure.borrow().is_some() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            // No user input drives this window, so without an explicit
            // repaint request it would sit static between rx polls instead
            // of animating the spinner / noticing the channel has a value.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(80));
        }
    });

    let value = result.borrow_mut().take();
    value.expect("loading window closed without a result from its background worker")
}

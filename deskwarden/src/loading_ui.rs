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

/// Named rather than inlined at the `run_ui_native` call, because
/// `foreground::raise_window` finds this window BY this title -- one
/// declaration means the two cannot drift apart.
const WINDOW_TITLE: &str = "Deskwarden";

/// Shows a "Deskwarden" window with a spinner and `message` until `rx`
/// yields a value, then closes and returns `Some(value)`.
///
/// `rx` is expected to be the receiving half of a channel whose sending half
/// was handed to a worker thread computing `T`. Both current call sites
/// (`main::wait_for_vault_ready_with_spinner` and
/// `picker_ui::pick_vault_item`) *detach* that worker with a bare
/// `std::thread::spawn`, giving it owned data (a `VaultBridge` clone, an
/// `Arc<VaultCache>` clone) rather than borrowing from the caller's stack.
/// That is deliberate: a `std::thread::scope`d worker -- which is what both
/// sites used to do -- forces the caller to block until the worker finishes
/// even after this function has already returned `None` because the user
/// closed the window, which made "the user can close this" a lie (review
/// 12). Detached, the worker simply finishes on its own and its result is
/// dropped unheard. `rx` itself is self-contained (an `mpsc::Receiver<T>`
/// borrows nothing), so it is fine to move into this window's own `'static`
/// closure regardless of where it was created.
///
/// Returns `None` if the window closed without `rx` ever yielding a value --
/// either the user closed it via the title bar's X or Alt+F4 (this is a
/// normal decorated window; nothing about "loading" makes it modal or
/// un-closable), or the worker thread disconnected the channel without
/// sending (e.g. it panicked). Review 11's Critical: this used to
/// `.expect()` on exactly that case, which meant a user closing this spinner
/// while `pick_vault_item`'s populate ran panicked the main thread, and
/// `main.rs`'s panic hook only logs -- so the process unwound out of `main`
/// and the tray icon, hotkey, and autofill all vanished with it. Every
/// caller must now decide for itself what "the user closed this" means
/// (abandon quietly, treat as a failure, etc.) rather than that decision
/// being made for it by a crash.
pub fn show_while<T: Send + 'static>(message: &str, rx: Receiver<T>) -> Option<T> {
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

    let _ = eframe::run_ui_native(WINDOW_TITLE, options, move |ui, _frame| {
        if !styled {
            // egui applies a new font set at the *start* of the next frame,
            // not the one that calls set_fonts -- drawing Archivo-styled
            // text in this same frame would look up a family that doesn't
            // exist yet and panic. Skip drawing this frame; the real UI
            // starts on the next one, once the fonts are actually live.
            theme::paint_window_background(ui);
            theme::apply(ui.ctx());
            // The OS window exists by this first painted frame (the same
            // hook `round_window_corners` uses), and this is where it is
            // brought to the front. See `foreground`: a refusal from Windows
            // flashes the taskbar button rather than being ignored.
            crate::foreground::raise_window(WINDOW_TITLE);
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
    value
}

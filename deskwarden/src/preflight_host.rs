//! **The window that hosts the 4b preflight.**
//!
//! `vault_window::preflight` had a tested `draw` and no window: the safety
//! property was live -- a gated fill was refused by
//! `preflight::dispatch_with` and the user was told through
//! `injector::sequence::Notifier` -- but the confirmation the design asks for
//! never appeared, so the *hold to send* never existed and neither did "copy
//! instead".
//!
//! This file is the window, and it is deliberately the thinnest thing that can
//! be one. It decides nothing:
//!
//! * it does not compute a verdict (`PreflightState::new` did, before it was
//!   handed over);
//! * it does not decide when a hold is long enough (`preflight::draw` does);
//! * and it is not the gate. Its `Some(PreflightAction::Send)` only means the
//!   caller may go and *ask* `dispatch_with`, which describes the foreground
//!   again and refuses on its own terms. See
//!   [`crate::vault_window::preflight::SendGate::confirm`].
//!
//! Same shape as [`crate::overlay_ui::show_prompt_overlay`]: a blocking
//! `eframe::run_native` on the thread that was about to fill, with the answer
//! read back out of an `Rc<RefCell<_>>` once the window is gone. That is what
//! makes the confirmation happen *before* anything is typed rather than
//! alongside it.

use crate::theme;
use crate::vault_window::preflight::{self, PreflightAction, PreflightState};
use eframe::egui;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use zeroize::Zeroizing;

/// One preflight at a time per process. A second one would be a second window
/// asking about a foreground that the first one is already standing in front
/// of.
static PREFLIGHT_OPEN: AtomicBool = AtomicBool::new(false);

pub const PREFLIGHT_WIDTH: f32 = 420.0;
pub const PREFLIGHT_HEIGHT: f32 = 380.0;

/// The window's own title. Never shown (the card is undecorated) but it is
/// what Win32 knows this window as.
pub const PREFLIGHT_TITLE: &str = "Deskwarden";

pub fn preflight_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([PREFLIGHT_WIDTH, PREFLIGHT_HEIGHT])
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_icon(theme::window_icon()),
        ..Default::default()
    }
}

/// Shows the preflight and blocks until the user answers.
///
/// `copy_payload` is the value "Copy instead" puts on the clipboard. It is the
/// only secret this window is handed, it is never drawn -- the step list was
/// built by `detail_edit::step_rows` with the eye shut, and this function
/// never touches `state.rows` -- and it is wiped when the window closes.
///
/// Nothing in this crate can call it: it opens a real window. Its whole body
/// is the two lines below plus the `eframe::App` impl, and every decision it
/// forwards to is tested where that decision lives.
pub fn show_preflight(
    state: PreflightState,
    copy_payload: Zeroizing<String>,
) -> Option<PreflightAction> {
    if PREFLIGHT_OPEN.swap(true, Ordering::SeqCst) {
        log::warn!(
            "a preflight was requested while one is already open in this process; refusing \
             rather than stacking a second confirmation over the same foreground"
        );
        // **`None`, which the caller reads as "do not send".** A second
        // confirmation that answered `Send` because it could not be shown
        // would be the exact inversion of what this window is for.
        return None;
    }

    let answer: Rc<RefCell<Option<PreflightAction>>> = Rc::new(RefCell::new(None));
    let app = PreflightApp { state, copy_payload, answer: answer.clone() };
    let _ = eframe::run_native(
        PREFLIGHT_TITLE,
        preflight_options(),
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    );

    PREFLIGHT_OPEN.store(false, Ordering::SeqCst);
    let answered = *answer.borrow();
    answered
}

struct PreflightApp {
    state: PreflightState,
    copy_payload: Zeroizing<String>,
    answer: Rc<RefCell<Option<PreflightAction>>>,
}

impl eframe::App for PreflightApp {
    /// Transparent behind the card, for the same reason the overlay is: the
    /// rounded corners would otherwise sit in a visible rectangle.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // **Every frame, unconditionally.** The hold accumulates `stable_dt`
        // across frames, and egui only repaints on input by default -- a key
        // held down produces no events at all after the first, so without this
        // the hold would stall at whatever the first frame credited it with
        // and the send would never arrive.
        ctx.request_repaint();

        if let Some(action) = preflight::draw(ui, &mut self.state) {
            if action == PreflightAction::CopyInstead {
                // The clipboard is the one place this value is allowed to go,
                // and it goes there because the user asked for it in
                // preference to typing.
                crate::clipboard::copy_secret(&self.copy_payload);
            }
            *self.answer.borrow_mut() = Some(action);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reentrancy guard's refusal is `None`, and `None` must be a refusal
    /// everywhere it is read.
    ///
    /// Driven through the real function: `PREFLIGHT_OPEN` is set by hand, so
    /// the early return is taken before `run_native` is reached and no window
    /// is opened. Flipping the early return to `Some(PreflightAction::Send)`
    /// -- the shape a "well, it was already confirmed once" reading would take
    /// -- fails here.
    #[test]
    fn a_second_preflight_refuses_rather_than_confirming() {
        PREFLIGHT_OPEN.store(true, Ordering::SeqCst);
        let answered = show_preflight(
            crate::vault_window::preflight::PreflightState::new(
                crate::injector::target::SendTarget {
                    title: "SAP Logon 760 - Sign in".into(),
                    image_name: "saplogon.exe".into(),
                    pid: 7412,
                    class_name: "SAPFEWndClass".into(),
                    focused_is_masked: true,
                },
                "saplogon.exe",
                "{PASSWORD}",
                &crate::key_sequence::ResolveSource {
                    username: "",
                    password: "",
                    custom: Vec::new(),
                    totp: &crate::vault_window::detail::TotpState::NoSecret,
                },
            ),
            Zeroizing::new(String::new()),
        );
        PREFLIGHT_OPEN.store(false, Ordering::SeqCst);
        assert_eq!(answered, None, "a second preflight answered something other than a refusal");
    }

    /// The card has to be big enough to hold what 4b says, or the hold
    /// affordance is below the fold on a surface whose whole job is to be
    /// read before a password is typed.
    #[test]
    fn the_card_is_sized_for_the_surface_it_hosts() {
        assert!(PREFLIGHT_WIDTH >= 380.0 && PREFLIGHT_HEIGHT >= 320.0);
    }
}

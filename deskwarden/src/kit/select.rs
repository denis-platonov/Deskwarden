//! Text the mouse can select: the detail pane's values.
//!
//! The owner asked for every value on the detail pane to be selectable --
//! "not just with copy button like it is now but user can select with a mouse
//! any characters and copy like a portion of it". egui already knows how
//! ([`LabelSelectionState`]: drag a range, double-click a word, Ctrl+C), but
//! only for text that goes through a `Label`; this app paints most of its
//! values as raw galleys, and `theme::apply` switches `selectable_labels` off
//! for the whole app. So selection is **opted into per value**, here, and a
//! value that is a secret opts in only while it is showing.
//!
//! **A masked value is not selectable, and that is the security rule of this
//! module.** The dots on a hidden password are drawn by [`paint`] / [`galley`]
//! with `selectable == false`, which is the painter call they always were:
//! no widget, no cursor, nothing for a drag to land on. Selecting dots could
//! otherwise copy the secret behind them, because egui copies the *label's*
//! text and a label may hold more than it shows. The caller decides, from the
//! same `revealed` flag that picked the dots, so the two cannot disagree.
//!
//! **A selection copy is protected like the copy button's.** egui writes a
//! selection to the clipboard by emitting `OutputCommand::CopyText`, which
//! eframe hands to the OS as a plain `CF_UNICODETEXT` -- no history
//! exclusion, no cloud-sync exclusion, no expiry. [`protect_copies`]
//! installs a plugin that takes those commands out of the frame's output
//! before eframe sees them and hands the text to the app's own clipboard
//! writer instead (`clipboard::copy_secret` in production).

use eframe::egui::{
    self, text_selection::LabelSelectionState, Color32, Context, FullOutput, Galley, Id,
    OutputCommand, Pos2, Rect, Response, Sense, Stroke, Ui,
};
use std::sync::Arc;
use zeroize::Zeroizing;

/// The sense a selectable value's response needs: egui's selection starts
/// from a drag, and a click must still be reported (it is not a drag).
pub fn sense() -> Sense {
    Sense::click_and_drag()
}

/// Paint `galley` at `pos`: as selectable text when `selectable`, as the
/// plain painter call it always was when not.
///
/// `response` must have been made with [`sense`] when `selectable` is true.
/// The colour is the fallback colour of [`egui::Painter::galley`], unchanged.
pub fn paint(ui: &Ui, response: &Response, pos: Pos2, galley: Arc<Galley>, color: Color32, selectable: bool) {
    if selectable {
        LabelSelectionState::label_text_selection(ui, response, pos, galley, color, Stroke::NONE);
    } else {
        ui.painter().galley(pos, galley, color);
    }
}

/// A painted galley made selectable in place: the rect it occupies becomes a
/// widget (`id`), and the galley is drawn through egui's selection state.
/// Not selectable: the plain painter call, **no widget at all**, and `None`.
pub fn galley(ui: &Ui, id: Id, pos: Pos2, galley: Arc<Galley>, color: Color32, selectable: bool) -> Option<Response> {
    if !selectable {
        ui.painter().galley(pos, galley, color);
        return None;
    }
    let response = ui.interact(Rect::from_min_size(pos, galley.size()), id, sense());
    paint(ui, &response, pos, galley, color, true);
    Some(response)
}

/// What a frame's selection copies are handed to.
struct ProtectedCopy {
    write: fn(&str),
}

fn armed_id(ctx: &Context) -> Id {
    Id::new(("selection-copy-armed", ctx.viewport_id()))
}

impl egui::Plugin for ProtectedCopy {
    fn debug_name(&self) -> &'static str {
        "protected-selection-copy"
    }

    /// Just before the frame's output goes to eframe: every `CopyText` of an
    /// armed frame is taken out and written by the app's own writer, so
    /// eframe never writes the unprotected copy as well.
    fn output_hook(&mut self, ctx: &Context, output: &mut FullOutput) {
        let id = armed_id(ctx);
        let armed = ctx.data_mut(|d| d.remove_temp::<bool>(id)).unwrap_or(false);
        if !armed {
            return;
        }
        let commands = std::mem::take(&mut output.platform_output.commands);
        for command in commands {
            match command {
                OutputCommand::CopyText(text) => {
                    let text = Zeroizing::new(text);
                    if !text.is_empty() {
                        (self.write)(text.as_str());
                    }
                }
                other => output.platform_output.commands.push(other),
            }
        }
    }
}

/// Route this frame's egui copies (a selection's Ctrl+C) through `write`
/// instead of eframe's own clipboard. Called by every frame that draws
/// selectable values; the first call on a context installs `write`, later
/// calls only re-arm the frame.
pub fn protect_copies(ctx: &Context, write: fn(&str)) {
    ctx.add_plugin(ProtectedCopy { write });
    let id = armed_id(ctx);
    ctx.data_mut(|d| d.insert_temp(id, true));
}

/// Drop whatever is selected on this context: another item's pane is about
/// to be drawn over the same widget ids.
pub fn clear_selection(ctx: &Context) {
    ctx.plugin::<LabelSelectionState>().lock().clear_selection();
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{pos2, vec2};
    use std::cell::RefCell;

    thread_local! {
        static PROTECTED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn recorder(text: &str) {
        PROTECTED.with(|p| p.borrow_mut().push(text.to_string()));
    }

    fn written() -> Vec<String> {
        PROTECTED.with(|p| p.borrow().clone())
    }

    const AT: Pos2 = pos2(20.0, 20.0);

    /// One frame: the value drawn at [`AT`], selectable or not.
    fn run(ctx: &Context, events: Vec<egui::Event>, selectable: bool) -> FullOutput {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(300.0, 100.0))),
                events,
                ..Default::default()
            },
            |ui| {
                protect_copies(ui.ctx(), recorder);
                let g = ui.painter().layout_no_wrap("hunter2".into(), egui::FontId::proportional(16.0), Color32::BLACK);
                let _ = galley(ui, Id::new("value"), AT, g, Color32::BLACK, selectable);
            },
        )
    }

    fn button(at: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
    }

    /// Drag across the whole value, then press Ctrl+C. Returns every frame's
    /// plain `CopyText` strings -- what eframe would have written itself.
    fn drag_over_it_and_copy(selectable: bool) -> Vec<String> {
        let ctx = crate::kit::testing::ctx();
        let from = pos2(AT.x + 1.0, AT.y + 8.0);
        let to = pos2(AT.x + 200.0, AT.y + 8.0);
        let mut plain = Vec::new();
        for events in [
            vec![],
            vec![egui::Event::PointerMoved(from), button(from, true)],
            vec![egui::Event::PointerMoved(to)],
            vec![egui::Event::PointerMoved(to), button(to, false)],
            vec![egui::Event::Copy],
            vec![],
        ] {
            let output = run(&ctx, events, selectable);
            plain.extend(output.platform_output.commands.into_iter().filter_map(|c| match c {
                OutputCommand::CopyText(t) => Some(t),
                _ => None,
            }));
        }
        plain
    }

    #[test]
    fn a_revealed_value_selects_and_its_copy_goes_through_the_protected_writer() {
        let plain = drag_over_it_and_copy(true);
        assert!(plain.is_empty(), "eframe was left a plain CopyText to write unprotected: {plain:?}");
        assert_eq!(written(), vec!["hunter2".to_string()], "the selection did not reach the protected writer");
    }

    #[test]
    fn a_masked_value_is_not_selectable_and_yields_no_text() {
        let plain = drag_over_it_and_copy(false);
        assert!(plain.is_empty(), "dragging over masked text produced a copy: {plain:?}");
        assert!(written().is_empty(), "dragging over masked text reached the clipboard writer: {:?}", written());
    }

    #[test]
    fn a_masked_value_registers_no_widget_for_a_drag_to_land_on() {
        let ctx = crate::kit::testing::ctx();
        let _ = run(&ctx, vec![], false);
        let _ = run(&ctx, vec![], false);
        assert!(
            ctx.read_response(Id::new("value")).is_none(),
            "a masked value has a widget; a drag over it could begin a selection"
        );
    }

    #[test]
    fn a_frame_that_did_not_arm_the_guard_leaves_egui_copies_alone() {
        let ctx = crate::kit::testing::ctx();
        let _ = run(&ctx, vec![], true);
        // A frame of some other screen: it copies through egui as before.
        let output = ctx.run_ui(egui::RawInput::default(), |ui| ui.ctx().copy_text("path".into()));
        assert!(
            output.platform_output.commands.iter().any(|c| matches!(c, OutputCommand::CopyText(t) if t == "path")),
            "an unarmed frame's copy was swallowed"
        );
        assert!(written().is_empty());
    }
}

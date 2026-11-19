//! The vault window's "Edit folder" modal (rename + delete), opened from a
//! sidebar folder row's edit-pencil icon. A dedicated small file rather than
//! folded into `sidebar.rs` or `mod.rs`: it owns its own state and drawing,
//! and neither of those files needs to know how it's built internally --
//! `vault_window::mod` only needs [`FolderEditState`] and
//! [`draw_folder_edit_modal`]'s returned [`FolderEditAction`].

use crate::theme;
use eframe::egui::{self, RichText};

/// The modal's own per-open state: which folder is being edited, the name
/// draft (seeded from the folder's current name, edited in place), and a
/// two-click delete confirm. The delete confirm is a simple bool rather than
/// `vault_window::mod`'s timed `confirm_click` pattern -- the modal already
/// requires a deliberate "open this folder's editor" step before Delete is
/// even reachable, so the fast-double-click concern that motivated the timed
/// version elsewhere doesn't apply here to the same degree, but a bare
/// single click still shouldn't delete outright.
pub struct FolderEditState {
    pub folder_id: String,
    pub name: String,
    delete_armed: bool,
    /// Set by the caller when a save or delete comes back an error.
    ///
    /// Without somewhere to put it, a failed action logged a warning and
    /// left the modal sitting open exactly as it was -- indistinguishable,
    /// from the outside, from the click not having registered at all.
    pub error: Option<String>,
}

impl FolderEditState {
    pub fn new(folder_id: String, name: String) -> Self {
        Self {
            folder_id,
            name,
            delete_armed: false,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderEditAction {
    None,
    Save,
    Delete,
    Cancel,
}

/// The dialog's id; its scrim is `"folder-edit-scrim"`.
pub const ID: &str = "folder-edit";

/// The id of the name field, for tests.
pub fn name_id() -> egui::Id {
    egui::Id::new((ID, "name"))
}

/// Draws the folder editor as a v2 dialog and reports what was pressed. Pure
/// view plus the one bit of state (`delete_armed`) the red button needs: the
/// caller performs the rename or the delete against `VaultBridge` and closes
/// the dialog in response to the returned action.
///
/// **The close mark, the scrim and Esc are Cancel and nothing else.** This
/// dialog also has a Save that writes and a Delete that, once armed, destroys
/// a folder; a dismissal that resolved to either would mean something
/// different here than on every other dialog.
pub fn draw_folder_edit_modal(ctx: &egui::Context, state: &mut FolderEditState) -> FolderEditAction {
    use crate::kit::controls::{self, ButtonKind, ButtonSize};
    use crate::kit::modal::{Dialog, ModalOutcome};
    let mut action = FolderEditAction::None;
    let (outcome, ()) = Dialog::new(ID, 480.0).title("Edit folder").show(ctx, |ui| {
        let width = ui.available_width();
        ui.spacing_mut().item_spacing.y = 7.0;
        ui.label(
            RichText::new("Folder name")
                .font(egui::FontId::new(14.0, egui::FontFamily::Name(theme::BOLD.into())))
                .color(theme::TEXT_SECONDARY),
        );
        crate::kit::input::text(ui, name_id(), &mut state.name, "", false, width);
        ui.label(RichText::new("Use \"/\" to nest, e.g. Work/Clients").size(13.0).color(theme::TEXT_FAINT));
        if let Some(error) = &state.error {
            ui.add(egui::Label::new(RichText::new(error).size(14.0).color(theme::ERROR)).wrap());
        }
        ui.add_space(11.0);
        let size = ButtonSize { height: 48.0, pad_x: 18.0, radius: 14, font_px: 16.0 };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let delete_label = if state.delete_armed { "Confirm delete" } else { "Delete" };
            if controls::button(ui, ButtonKind::Danger, size, delete_label, None).clicked() {
                if state.delete_armed {
                    action = FolderEditAction::Delete;
                } else {
                    state.delete_armed = true;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if controls::button(ui, ButtonKind::Primary, size, "Save", None).clicked() {
                    action = FolderEditAction::Save;
                }
                if controls::button(ui, ButtonKind::Outline, size, "Cancel", None).clicked() {
                    action = FolderEditAction::Cancel;
                }
            });
        });
    });
    if action == FolderEditAction::None && outcome == ModalOutcome::Closed {
        action = FolderEditAction::Cancel;
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Pos2, Rect, Vec2};

    /// The window the modal is centred in. Tall enough that the card fits
    /// with room around it, so "a point outside the card" is a real point.
    const BODY: Vec2 = Vec2::new(900.0, 700.0);

    /// Every string this frame painted, with where it landed, and every line
    /// it stroked.
    #[derive(Default)]
    struct Painted {
        texts: Vec<(String, Rect)>,
        /// Strokes, for the one control on this card that paints no words:
        /// the dismiss ✕ in the title line. A test that could only see text
        /// could not tell a card with a mark from one without.
        segments: Vec<[Pos2; 2]>,
    }

    impl Painted {
        fn strings(&self) -> Vec<&str> {
            self.texts.iter().map(|(t, _)| t.as_str()).collect()
        }

        fn has(&self, label: &str) -> bool {
            self.texts.iter().any(|(t, _)| t == label)
        }

        /// The one rect painting `label`, or a failure naming everything that
        /// *was* painted -- which is what turns "the button is gone" into a
        /// readable message rather than a silent click into empty space.
        fn rect_of(&self, label: &str) -> Rect {
            let found: Vec<Rect> = self
                .texts
                .iter()
                .filter(|(t, _)| t == label)
                .map(|(_, r)| *r)
                .collect();
            assert_eq!(
                found.len(),
                1,
                "expected exactly one {label:?} in the folder modal, found {}; painted: {:?}",
                found.len(),
                self.strings()
            );
            found[0]
        }
    }

    fn walk(shape: &egui::Shape, painted: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => painted.texts.push((
                text.galley.text().to_string(),
                Rect::from_min_size(text.pos, text.galley.size()),
            )),
            egui::Shape::LineSegment { points, .. } => painted.segments.push(*points),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, painted);
                }
            }
            _ => {}
        }
    }

    fn raw_input(events: &[egui::Event]) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, BODY)),
            events: events.to_vec(),
            ..Default::default()
        }
    }

    /// A context with `theme::apply`'s fonts live. The two throwaway frames
    /// are the ones every other harness in this crate runs: a font set
    /// registered during a frame is only usable from the start of the next.
    fn styled_context() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw_input(&[]), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(raw_input(&[]), |_ui| {});
        ctx
    }

    fn frame(
        ctx: &egui::Context,
        state: &mut FolderEditState,
        events: &[egui::Event],
    ) -> (FolderEditAction, Painted) {
        let mut action = FolderEditAction::None;
        let output = ctx.run_ui(raw_input(events), |ui| {
            action = draw_folder_edit_modal(ui.ctx(), state);
        });
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        (action, painted)
    }

    /// A full press-and-release, which is what egui needs before it will
    /// report `Response::clicked` -- a press alone is not a click.
    fn click(pos: Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    fn escape() -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]
    }

    fn state() -> FolderEditState {
        FolderEditState::new("fld-7".into(), "Work/Clients".into())
    }

    /// Runs the modal idle until it is painting, and hands back what it
    /// painted -- which is what every "click the button named X" test needs
    /// before it can aim.
    ///
    /// **Two frames, and the second one is not optional.** An `egui::Area`
    /// the context has never seen runs a *sizing pass* on its first frame:
    /// that `Ui` is invisible and the whole card tessellates to nothing, so a
    /// harness that read the first frame would see an empty screen and
    /// conclude every button was missing. Both halves are asserted here so
    /// the day egui stops doing that is a loud failure in one place rather
    /// than an off-by-one frame in a dozen tests.
    fn opened(ctx: &egui::Context, state: &mut FolderEditState) -> Painted {
        let (sizing, blank) = frame(ctx, state, &[]);
        assert_eq!(
            sizing,
            FolderEditAction::None,
            "the modal reported an action on a frame with no input at all"
        );
        assert!(
            blank.texts.is_empty(),
            "the sizing pass painted after all; this warm-up no longer describes what egui \
             does, and the frame counts in these tests may be off by one"
        );

        let (action, painted) = frame(ctx, state, &[]);
        assert_eq!(
            action,
            FolderEditAction::None,
            "the modal reported an action on a frame with no input at all"
        );
        assert!(
            !painted.texts.is_empty(),
            "the modal painted nothing at all on a frame it was drawn in"
        );
        painted
    }

    // -- the state ----------------------------------------------------------

    #[test]
    fn new_seeds_the_draft_from_the_folder_and_arms_nothing() {
        // The name is seeded, not blanked: this modal renames an existing
        // folder, and a `String::new()` here would silently offer to rename
        // every folder to "" for anyone who pressed Save without typing.
        let state = FolderEditState::new("fld-7".into(), "Work/Clients".into());
        assert_eq!(state.folder_id, "fld-7");
        assert_eq!(
            state.name, "Work/Clients",
            "the name draft was not seeded from the folder"
        );
        assert!(
            !state.delete_armed,
            "a freshly opened modal is already armed to delete"
        );
        assert_eq!(state.error, None);
    }

    // -- what the frame shows -----------------------------------------------

    #[test]
    fn the_open_modal_shows_the_folders_name_and_all_three_answers() {
        let ctx = styled_context();
        let mut state = state();
        let painted = opened(&ctx, &mut state);

        for expected in [
            "Edit folder",
            "Folder name",
            "Work/Clients",
            "Delete",
            "Save",
            "Cancel",
        ] {
            assert!(
                painted.has(expected),
                "the modal did not paint {expected:?}; painted: {:?}",
                painted.strings()
            );
        }
        assert!(
            !painted.has("Confirm delete"),
            "the modal opened already showing the delete confirmation"
        );
    }

    // -- the dismiss ✕ --------------------------------------------------------

    #[test]
    fn an_error_the_caller_set_is_on_screen() {
        // The whole reason `error` exists: without it a failed save left the
        // modal sitting open exactly as it was, indistinguishable from the
        // click never having registered.
        let ctx = styled_context();
        let mut state = state();
        state.error = Some("A folder named Work/Clients already exists.".into());
        let painted = opened(&ctx, &mut state);
        assert!(
            painted.has("A folder named Work/Clients already exists."),
            "the caller's error is nowhere on screen; painted: {:?}",
            painted.strings()
        );
    }

    #[test]
    fn no_error_paints_no_error_line() {
        // The positive control for the test above: if the modal painted some
        // fixed string there always, that test would pass with `state.error`
        // ignored entirely.
        let ctx = styled_context();
        let mut state = state();
        let painted = opened(&ctx, &mut state);
        assert!(
            !painted.has("A folder named Work/Clients already exists."),
            "an error line appeared with no error set"
        );
    }

    // -- the answers --------------------------------------------------------

    #[test]
    fn clicking_save_asks_the_caller_to_save() {
        let ctx = styled_context();
        let mut state = state();
        let save = opened(&ctx, &mut state).rect_of("Save");

        let (action, _) = frame(&ctx, &mut state, &click(save.center()));
        assert_eq!(
            action,
            FolderEditAction::Save,
            "clicking Save did not ask for a save; the button is decoration"
        );
    }

    #[test]
    fn clicking_cancel_cancels() {
        let ctx = styled_context();
        let mut state = state();
        let cancel = opened(&ctx, &mut state).rect_of("Cancel");

        let (action, _) = frame(&ctx, &mut state, &click(cancel.center()));
        assert_eq!(
            action,
            FolderEditAction::Cancel,
            "clicking Cancel did not cancel"
        );
    }

    #[test]
    fn escape_cancels() {
        // The same rule the discard confirmation encodes: Escape is the
        // reflex for "get this off my screen", so it must resolve to the
        // answer that destroys nothing. Delete is behind two pointer clicks
        // precisely so a keypress can never reach it.
        let ctx = styled_context();
        let mut state = state();
        let _ = opened(&ctx, &mut state);

        let (action, _) = frame(&ctx, &mut state, &escape());
        assert_eq!(
            action,
            FolderEditAction::Cancel,
            "Escape did not cancel the folder modal"
        );
    }

    #[test]
    fn escape_while_the_delete_is_armed_still_only_cancels() {
        // Escape must not be able to answer the destructive question, even
        // once the first Delete click has armed it.
        let ctx = styled_context();
        let mut state = state();
        let delete = opened(&ctx, &mut state).rect_of("Delete");
        let _ = frame(&ctx, &mut state, &click(delete.center()));
        assert!(
            state.delete_armed,
            "control: the first Delete click did not arm anything"
        );

        let (action, _) = frame(&ctx, &mut state, &escape());
        assert_eq!(
            action,
            FolderEditAction::Cancel,
            "Escape over an armed delete answered something other than cancel"
        );
    }

    #[test]
    fn escape_does_not_overwrite_an_answer_the_pointer_already_gave() {
        // A save and an Escape landing in the same frame must not silently
        // become a cancel: the `action == None` guard in front of the Escape
        // branch is what stops it, and deleting that guard reds this.
        let ctx = styled_context();
        let mut state = state();
        let save = opened(&ctx, &mut state).rect_of("Save");

        let mut events = click(save.center());
        events.extend(escape());
        let (action, _) = frame(&ctx, &mut state, &events);
        assert_eq!(
            action,
            FolderEditAction::Save,
            "an Escape in the same frame as a Save click threw the save away"
        );
    }

    // -- the empty name -----------------------------------------------------

    #[test]
    fn an_empty_name_is_still_reported_as_a_save() {
        // Pinning what this file actually does, so the next reader does not
        // assume a guard that is not here: the modal does NOT validate. An
        // empty box saves, and it is `vault_window::mod`'s job to refuse and
        // hand back `state.error`. If validation ever moves in here, this
        // test is the one that must be rewritten deliberately rather than a
        // caller quietly renaming a folder to "".
        let ctx = styled_context();
        let mut state = FolderEditState::new("fld-7".into(), String::new());
        let save = opened(&ctx, &mut state).rect_of("Save");

        let (action, _) = frame(&ctx, &mut state, &click(save.center()));
        assert_eq!(action, FolderEditAction::Save);
        assert!(
            state.name.is_empty(),
            "control: the name under test was not empty"
        );
    }

    // -- the two-click delete -----------------------------------------------

    #[test]
    fn one_delete_click_arms_and_deletes_nothing() {
        // The rule the file's own doc claims: "a bare single click still
        // shouldn't delete outright". Collapse the two-click ladder into a
        // straight `action = Delete` and this reds.
        let ctx = styled_context();
        let mut state = state();
        let delete = opened(&ctx, &mut state).rect_of("Delete");

        let (action, _) = frame(&ctx, &mut state, &click(delete.center()));
        assert_eq!(
            action,
            FolderEditAction::None,
            "one click on Delete deleted the folder"
        );

        let (idle, armed) = frame(&ctx, &mut state, &[]);
        assert_eq!(idle, FolderEditAction::None);
        assert!(
            armed.has("Confirm delete"),
            "the armed button does not say so, so the second click is unannounced; painted: {:?}",
            armed.strings()
        );
        assert!(
            !armed.has("Delete"),
            "the button still reads as an unarmed Delete"
        );
    }

    #[test]
    fn the_second_delete_click_deletes() {
        let ctx = styled_context();
        let mut state = state();
        let delete = opened(&ctx, &mut state).rect_of("Delete");

        let _ = frame(&ctx, &mut state, &click(delete.center()));
        let (_, armed) = frame(&ctx, &mut state, &[]);
        let confirm = armed.rect_of("Confirm delete");

        let (action, _) = frame(&ctx, &mut state, &click(confirm.center()));
        assert_eq!(
            action,
            FolderEditAction::Delete,
            "the confirmed delete never asked the caller to delete anything"
        );
    }

    // -- the name is editable -----------------------------------------------

    #[test]
    fn typing_in_the_box_edits_the_name_the_save_will_carry() {
        // The modal's entire purpose. A read-only box would leave every Save
        // renaming the folder to what it was already called, and every other
        // test here would still pass.
        let ctx = styled_context();
        let mut state = FolderEditState::new("fld-7".into(), String::new());
        let painted = opened(&ctx, &mut state);
        // The box is the gap between its label and the nesting hint under it.
        // Aimed from two painted rects rather than a guessed offset, so a
        // layout change moves the click with it instead of missing silently.
        let label = painted.rect_of("Folder name");
        let hint = painted.rect_of("Use \"/\" to nest, e.g. Work/Clients");
        assert!(
            hint.top() > label.bottom(),
            "control: the hint is not below the label, so the gap between them is not the box"
        );
        let into_box = Pos2::new(label.center().x, (label.bottom() + hint.top()) / 2.0);
        let _ = frame(&ctx, &mut state, &click(into_box));
        let _ = frame(&ctx, &mut state, &[egui::Event::Text("Archive".into())]);

        assert_eq!(
            state.name, "Archive",
            "typing into the folder-name box did not reach the draft the caller saves"
        );
    }
    /// The v2 dialog's close mark and its scrim both cancel, and neither can
    /// reach Save or Delete -- armed or not.
    #[test]
    fn the_close_mark_and_the_scrim_only_cancel() {
        use crate::kit::testing;
        for armed in [false, true] {
            for use_mark in [true, false] {
                let ctx = testing::ctx();
                let mut st = state();
                st.delete_armed = armed;
                let screen = egui::vec2(1280.0, 820.0);
                for t in [0.0, 0.1, 0.2] {
                    let _ = testing::frame(&ctx, t, screen, vec![], |ui| {
                        let _ = draw_folder_edit_modal(ui.ctx(), &mut st);
                    });
                }
                let at = if use_mark {
                    ctx.read_response(crate::kit::modal::close_id(ID)).expect("the close mark").rect.center()
                } else {
                    egui::pos2(6.0, 410.0)
                };
                let mut action = FolderEditAction::None;
                let _ = testing::frame(&ctx, 0.3, screen, testing::click_at(at), |ui| {
                    action = draw_folder_edit_modal(ui.ctx(), &mut st);
                });
                assert_eq!(action, FolderEditAction::Cancel, "armed {armed}, mark {use_mark}");
            }
        }
    }
}

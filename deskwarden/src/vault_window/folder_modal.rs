//! The vault window's "Edit folder" modal (rename + delete), opened from a
//! sidebar folder row's edit-pencil icon. A dedicated small file rather than
//! folded into `sidebar.rs` or `mod.rs`: it owns its own state and drawing,
//! and neither of those files needs to know how it's built internally --
//! `vault_window::mod` only needs [`FolderEditState`] and
//! [`draw_folder_edit_modal`]'s returned [`FolderEditAction`].

use crate::theme;
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke};

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

/// Draws the modal as a centered card over a dimmed scrim covering the whole
/// window. Pure view plus the tiny bit of state (`delete_armed`) the confirm
/// button needs -- the caller performs the actual rename/delete against
/// `VaultBridge` and closes the modal in response to the returned action.
pub fn draw_folder_edit_modal(ctx: &egui::Context, state: &mut FolderEditState) -> FolderEditAction {
    let mut action = FolderEditAction::None;

    // Dimmed scrim: a full-window click-catcher (so a click outside the
    // card can't reach whatever is behind it) painted at a low alpha, on the
    // `Foreground` layer so it sits above the sidebar/list/detail panels
    // regardless of draw order.
    egui::Area::new(egui::Id::new("folder-edit-scrim"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let screen = ctx.content_rect();
            ui.allocate_response(screen.size(), egui::Sense::click());
            ui.painter()
                .rect_filled(screen, CornerRadius::ZERO, egui::Color32::from_black_alpha(90));
        });

    // Centred on the frame it opens and wherever the user has dragged it
    // after that -- `theme::movable_modal` replaces the `.anchor(CENTER_CENTER,
    // ZERO)` that used to pin this card, and the handle below is its title
    // line. The offset resets when the modal closes, so the next open is
    // centred again.
    theme::movable_modal(ctx, egui::Area::new(egui::Id::new("folder-edit-modal")))
        .show(ctx, |ui| {
            theme::modal_drag_handle(ui, theme::MODAL_PLAIN_HEADER_HEIGHT);
            let framed = egui::Frame::new()
                .fill(theme::CARD)
                .corner_radius(CornerRadius::same(10))
                .stroke(Stroke::new(1.0, theme::BORDER))
                .inner_margin(Margin::same(20))
                .show(ui, |ui| {
                    ui.set_width(320.0);
                    // The title's own rectangle goes back out to the caller,
                    // which is the line the dismiss ✕ is hung on once this
                    // frame has measured the card. See `theme::modal_corner_mark`.
                    let title =
                        ui.label(theme::bold("Edit folder", 15.0).color(theme::INK)).rect;
                    ui.add_space(14.0);
                    theme::field_label(ui, "Folder name");
                    ui.add_space(6.0);
                    theme::text_field(ui, &mut state.name, false);
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Use \"/\" to nest, e.g. Work/Clients")
                            .size(11.0)
                            .color(theme::TEXT_GHOST),
                    );
                    if let Some(error) = &state.error {
                        ui.add_space(8.0);
                        ui.label(RichText::new(error).size(12.0).color(theme::ERROR));
                    }
                    ui.add_space(18.0);
                    ui.horizontal(|ui| {
                        let delete_label = if state.delete_armed { "Confirm delete" } else { "Delete" };
                        let delete_clicked = ui
                            .add(
                                egui::Button::new(RichText::new(delete_label).size(12.0).color(theme::ERROR))
                                    .fill(theme::CARD)
                                    .stroke(Stroke::new(1.0, theme::ERROR))
                                    .corner_radius(CornerRadius::same(7))
                                    .min_size(egui::vec2(0.0, 32.0)),
                            )
                            .clicked();
                        if delete_clicked {
                            if state.delete_armed {
                                action = FolderEditAction::Delete;
                            } else {
                                state.delete_armed = true;
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::primary_button(ui, "Save", None).clicked() {
                                action = FolderEditAction::Save;
                            }
                            ui.add_space(8.0);
                            if theme::secondary_button(ui, "Cancel").clicked() {
                                action = FolderEditAction::Cancel;
                            }
                        });
                    });
                    title
                });
            // The ✕ in the card's corner, drawn last so it is on top of both
            // the card's own contents and the drag strip under them.
            //
            // **It is Cancel and nothing else.** This card has two other
            // answers -- a Save that writes and a Delete that, once armed,
            // destroys a folder -- and a dismiss control that resolved to
            // either of those would be a gesture meaning something different
            // here than on every other card in the app. It is exactly what
            // Escape does at the bottom of this function, which is the rule:
            // the mark is a second surface for a gesture that already exists,
            // never a third outcome. A folder half-renamed in the box is
            // thrown away either way, and that is already true of the Cancel
            // button beside Save.
            if theme::modal_corner_mark(ui, framed.response.rect, framed.inner) {
                action = FolderEditAction::Cancel;
            }
        });

    // Esc cancels, same as every other transient overlay in this app.
    if action == FolderEditAction::None && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
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

    /// Where the dismiss ✕'s two arms cross, found from the paint.
    ///
    /// By geometry rather than by a computed rectangle, so the test cannot be
    /// satisfied by a mark this file *thinks* it placed: the two arms are the
    /// only diagonals this card strokes at all.
    fn dismiss_mark(painted: &Painted) -> Pos2 {
        let arms: Vec<&[Pos2; 2]> = painted
            .segments
            .iter()
            .filter(|[a, b]| {
                (b.x - a.x).abs() > 0.5 && ((b.x - a.x).abs() - (b.y - a.y).abs()).abs() < 0.01
            })
            .collect();
        assert_eq!(
            arms.len(),
            2,
            "the card strokes {} diagonals; the dismiss ✕ is exactly two, and a card with no \
             mark on it is the defect this test exists for",
            arms.len()
        );
        let centre = arms[0][0].lerp(arms[0][1], 0.5);
        assert!(
            (arms[1][0].lerp(arms[1][1], 0.5) - centre).length() < 0.01,
            "the two arms do not cross, so this is not a ✕"
        );
        centre
    }

    /// **The mark is drawn, on the title's own line, at the app's one corner
    /// inset.**
    ///
    /// The inset is asserted against `theme::MODAL_CLOSE_INSET` and the card's
    /// own geometry rather than against a number typed here, because the whole
    /// point of the shared primitive is that this card cannot have an inset of
    /// its own. Get this wrong and the mark drifts card by card, which is the
    /// state the ✕ pass was opened to end.
    #[test]
    fn the_title_line_carries_the_dismiss_mark_at_the_shared_inset() {
        let ctx = styled_context();
        let mut state = state();
        let painted = opened(&ctx, &mut state);

        let title = painted.rect_of("Edit folder");
        let at = dismiss_mark(&painted);

        // The card's right-hand edge, taken from the rectangle egui really
        // laid the modal's `Area` out in -- not re-derived here from the
        // column width and the frame's padding, which is exactly the
        // arithmetic `theme::modal_corner_mark` refuses to trust.
        let card = egui::AreaState::load(&ctx, egui::Id::new("folder-edit-modal"))
            .expect("the modal has never been drawn")
            .rect();
        let box_right = at.x + theme::CLOSE_MARK_HIT / 2.0;
        assert!(
            (card.right() - box_right - theme::MODAL_CLOSE_INSET).abs() < 0.5,
            "the mark's hit box ends {} points inside the card's edge, not \
             `theme::MODAL_CLOSE_INSET`'s {}",
            card.right() - box_right,
            theme::MODAL_CLOSE_INSET
        );
        assert!(
            (at.y - title.center().y).abs() < 1.0,
            "the mark sits at y={} and the title's line is centred on {}; a mark centred on \
             `MODAL_PLAIN_HEADER_HEIGHT` instead floats above the words it belongs beside",
            at.y,
            title.center().y
        );
    }

    /// **And pressing it cancels -- the same answer Escape gives.**
    ///
    /// The half a paint test cannot reach, and the half this app has shipped
    /// broken before. It is also the guard that the header's drag strip does
    /// not swallow the click: that strip covers `MODAL_PLAIN_HEADER_HEIGHT`,
    /// which is this whole title line, so a mark registered before it would be
    /// drawn, hovered, and dead. `theme::modal_drag_handle` records why.
    ///
    /// The answer is `Cancel` and could not be Save or Delete: the mark is a
    /// second surface for a gesture this card already had, never a third
    /// outcome. The control below is that Escape reaches the same one.
    #[test]
    fn pressing_the_dismiss_mark_cancels_exactly_as_escape_does() {
        let ctx = styled_context();
        let mut state = state();
        let at = dismiss_mark(&opened(&ctx, &mut state));

        let (action, _) = frame(&ctx, &mut state, &click(at));
        assert_eq!(
            action,
            FolderEditAction::Cancel,
            "the ✕ at {at:?} did not cancel the folder modal"
        );

        // The control: Escape, on a freshly opened card, reaches the same
        // answer -- so the mark is the existing gesture and not a new one.
        let keyed_ctx = styled_context();
        let mut keyed = FolderEditState::new("fld-7".into(), "Work/Clients".into());
        let _ = opened(&keyed_ctx, &mut keyed);
        let (by_key, _) = frame(&keyed_ctx, &mut keyed, &escape());
        assert_eq!(
            by_key, action,
            "the ✕ and Escape give this card different answers"
        );
    }

    /// **The mark does not answer the armed delete.** Arming Delete and then
    /// pressing the ✕ must leave the folder alone -- the corner mark is the
    /// way OUT of a card, and a card whose way out could destroy a folder is
    /// the accident the two-click arm exists to prevent.
    #[test]
    fn the_dismiss_mark_over_an_armed_delete_still_only_cancels() {
        let ctx = styled_context();
        let mut state = state();
        let _ = opened(&ctx, &mut state);
        state.delete_armed = true;
        let at = dismiss_mark(&frame(&ctx, &mut state, &[]).1);

        let (action, _) = frame(&ctx, &mut state, &click(at));
        assert_eq!(
            action,
            FolderEditAction::Cancel,
            "the ✕ over an armed delete answered something other than cancel"
        );
    }

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
    fn a_click_that_hits_the_scrim_answers_nothing() {
        // The positive control for both tests above: the scrim is a
        // full-window click-catcher, and if any click anywhere produced an
        // answer they would pass with every button deleted. It also pins the
        // scrim's own rule -- clicking away from this card must NOT be read
        // as one of the answers, none of which is safe to guess.
        let ctx = styled_context();
        let mut state = state();
        let card = opened(&ctx, &mut state).rect_of("Edit folder");

        let away = Pos2::new(card.center().x, card.top() - 120.0);
        let (action, _) = frame(&ctx, &mut state, &click(away));
        assert_eq!(
            action,
            FolderEditAction::None,
            "a click on the scrim answered the modal"
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
}

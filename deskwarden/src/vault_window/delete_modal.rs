//! The vault window's **delete confirmation** modal, opened from an item
//! row's right-click menu and from the detail pane's kebab.
//!
//! A dedicated small file for [`super::icon_modal`]'s reason: it owns its own
//! state and drawing, and neither `item_list.rs` nor `mod.rs` needs to know
//! how it is built -- `vault_window::mod` needs [`DeleteConfirmState`],
//! [`draw_delete_modal`] and the [`DeleteConfirmAction`] that comes back, and
//! nothing else.
//!
//! # Why a modal, and what it replaced
//!
//! A two-click confirmation: the first click on Delete re-labelled the entry
//! "Delete? Click to confirm" and armed a three-second window, a second click
//! inside it went through. It was chosen over a native `MessageBox` (which
//! blocks egui's event loop) and it worked, but it asks the user to
//! understand a state rather than a question -- and it asks it in the place
//! least able to hold one. Both doors into it are menus. A menu that stays
//! open to show its own armed entry is a menu that has stopped behaving like
//! a menu, and one that closes has hidden the state it just entered; the
//! kebab went red to cover the second case, which is a third thing to learn.
//!
//! A modal says what will happen in a sentence, names the item it will happen
//! to, and offers two buttons that are different from each other. It also has
//! somewhere to put the answer when the vault refuses the write -- the
//! two-click path had nowhere, so a failed delete disarmed itself and looked
//! exactly like a click that never registered.
//!
//! The folder × in the sidebar keeps its two-click confirmation. It is a
//! different object with a different consequence (a folder delete leaves its
//! items alone), it sits inline in a rail rather than behind a menu, and
//! changing it was not asked for.
//!
//! # What it does NOT do
//!
//! It writes to no vault and reads none. It reports which button was
//! pressed, exactly as [`super::icon_modal`] does, so that the delete itself
//! stays in the one place that already knows how to invalidate the three
//! lists this window keeps.

use crate::theme;
use crate::vault_bridge::VaultItem;
use eframe::egui::{self, RichText};

/// Which of the two deletes is being confirmed.
///
/// The two are not the same question and must not share a sentence: one moves
/// an item somewhere it can be fetched back from, the other is the only
/// irreversible thing this window does. A single `bool` here would be a
/// parameter whose two values differ by a word in a string; a named pair is
/// what the caller reads at its own call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteKind {
    /// The ordinary Delete on a live item: `delete_item`, no `permanent`
    /// flag, so the item lands in the Trash.
    ToTrash,
    /// `purge_item` on an item already in the Trash. Gone.
    Forever,
}

impl DeleteKind {
    /// The modal's heading.
    fn heading(self) -> &'static str {
        match self {
            DeleteKind::ToTrash => "Delete item",
            DeleteKind::Forever => "Delete forever",
        }
    }

    /// What pressing the red button will do, in one sentence.
    ///
    /// **The recoverable one says where it goes; the permanent one says it
    /// cannot be undone**, and neither says both. "It moves to the Trash,
    /// where you can restore it, but if you empty the Trash it is gone" is
    /// two facts in one breath and the reader keeps the wrong one.
    fn body(self) -> &'static str {
        match self {
            DeleteKind::ToTrash => {
                "It moves to the Trash. You can restore it from there, on this or any \
                 other Bitwarden client."
            }
            DeleteKind::Forever => {
                "This cannot be undone. The item and everything on it is removed from the \
                 vault on every device."
            }
        }
    }

    /// The red button's words.
    ///
    /// They repeat the heading rather than saying "Delete" in both cases: the
    /// button is the last thing read before the click, and a button reading
    /// "Delete" under a heading reading "Delete forever" is the one place a
    /// reader could still think the Trash is catching it.
    fn confirm_label(self) -> &'static str {
        match self {
            DeleteKind::ToTrash => "Delete",
            DeleteKind::Forever => "Delete forever",
        }
    }
}

/// The modal's per-open state.
pub struct DeleteConfirmState {
    /// Which item is being deleted. The id and not the item, for
    /// [`super::icon_modal::IconPickState`]'s reason: the modal is open
    /// across frames, a sync can replace the snapshot under it, and the
    /// handler looks the item up fresh when it acts.
    pub item_id: String,
    /// The row the card shows: the item's tile, its name and its username,
    /// copied at the moment the modal opened.
    ///
    /// **Still a copy and still not the item**, which is the rule above
    /// applied to the rest of what is drawn rather than an exception to it --
    /// see [`super::ModalSubject`], which owns that argument and the
    /// texture-handle half of it.
    pub subject: super::ModalSubject,
    /// Which delete this is.
    pub kind: DeleteKind,
    /// The refusal to show when the vault write fails.
    ///
    /// Set by the CALLER, like [`super::icon_modal::IconPickState::error`].
    /// This is the half the two-click confirmation could not do at all: a
    /// delete that failed disarmed itself and left the menu looking like a
    /// click that never landed.
    pub error: Option<String>,
}

impl DeleteConfirmState {
    /// Opens the modal for one item, with `icon` the caller's
    /// `icons.textures.get(item.id.as_str())`.
    ///
    /// **Takes the item and copies out of it here, rather than taking the
    /// three or four strings the card shows.** Four call sites open this
    /// modal -- two row commands and two detail-pane actions -- and each of
    /// them already holds the item; a constructor spelling out what a subject
    /// line consists of would spell it out four times, and the next field the
    /// line grows would be added to three of them.
    pub fn new(item: &VaultItem, icon: Option<&egui::TextureHandle>, kind: DeleteKind) -> Self {
        Self {
            item_id: item.id.clone(),
            subject: super::ModalSubject::of(item, icon),
            kind,
            error: None,
        }
    }
}

/// What the modal is asking `vault_window::mod` to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteConfirmAction {
    None,
    /// Go through with it. Carries nothing: the state the handler needs is
    /// still in [`DeleteConfirmState`], which the handler owns.
    Confirm,
    Cancel,
}

/// How wide the card is. Unchanged from the plain card this replaced: the
/// sentence under the subject line is the longest thing on it, and 340 is the
/// width at which the permanent delete's two lines break where they read
/// best.
const CARD_WIDTH: f32 = 340.0;

/// Draws the modal as a centred card over a dimmed scrim and reports what was
/// pressed.
///
/// **The card is [`theme::modal_card`]'s**, which is the app's one modal
/// shape: a coloured header band, a white body, and a footer whose two
/// answers split its width. This file no longer lays out a card of its own,
/// and [`super::icon_modal`] no longer lays out a second one -- see that
/// function's own doc for why the layout moved into `theme`.
///
/// **The header carries [`theme::ERROR`] and the warning triangle**, because
/// the accent is how the card says what kind of question it is before a word
/// of it is read. It is the same red the confirm button wears, and the same
/// red this app already spends on exactly this meaning (the kebab's Delete
/// words, the sidebar's folder ×).
///
/// **Esc cancels and Enter does nothing.** Every other overlay in this app
/// takes Esc, so this one does too; the affirmative key is deliberately
/// missing, because a modal that a stray Return can dismiss into a delete is
/// a modal that made the accident it was put there to prevent slightly more
/// convenient.
pub fn draw_delete_modal(
    ctx: &egui::Context,
    state: &mut DeleteConfirmState,
) -> DeleteConfirmAction {
    let mut action = DeleteConfirmAction::None;

    // The `Area` is declared HERE rather than inside `theme::modal_scrim`,
    // and the id is a literal on purpose: `item_list::MODAL_SCRIM_AREAS` is
    // walked for exactly this declaration, and a scrim it cannot see is a
    // modal the item list's arrow keys steer straight through.
    theme::modal_scrim(ctx, egui::Area::new(egui::Id::new("delete-confirm-scrim")));

    let press = theme::modal_card(
        ctx,
        egui::Area::new(egui::Id::new("delete-confirm-modal")),
        theme::ModalCard {
            accent: theme::ERROR,
            glyph: theme::ModalGlyph::Warning,
            title: state.kind.heading(),
            width: CARD_WIDTH,
            dismiss: "Cancel",
        },
        |ui| {
            // The row this is about, drawn as the results list draws it --
            // its own icon, its own name, its own username. The menu that
            // opened this is gone from the screen by now, and a confirmation
            // that did not say which item it was about would be one click
            // away from deleting the wrong credential; two logins at the same
            // bank differ by their username and nothing else, which is why
            // the second line is not decoration.
            state.subject.draw(ui);
            ui.add_space(12.0);
            // Wrapped, not truncated: this is the sentence that says what the
            // button does, and half of it is worse than none.
            ui.add(
                egui::Label::new(
                    RichText::new(state.kind.body()).size(12.0).color(theme::TEXT_MUTED),
                )
                .wrap(),
            );

            if let Some(error) = &state.error {
                ui.add_space(10.0);
                ui.add(
                    egui::Label::new(RichText::new(error).size(12.0).color(theme::ERROR)).wrap(),
                );
            }
        },
        |ui| theme::destructive_button(ui, state.kind.confirm_label()),
    );
    if press.confirmed {
        action = DeleteConfirmAction::Confirm;
    }
    // Second, so that the cautious answer wins a frame in which the pointer
    // somehow reported both. Nothing in egui delivers two clicks in one
    // frame today; this costs nothing and the other order costs a delete.
    if press.dismissed {
        action = DeleteConfirmAction::Cancel;
    }

    // Esc cancels, same as every other transient overlay in this app.
    if action == DeleteConfirmAction::None && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = DeleteConfirmAction::Cancel;
    }

    action
}

// **No `write_failed_sentence` here, unlike [`super::icon_modal`].** The
// wording for a refused delete already exists and is already the better
// wording: `vault_window::mod`'s `list_command_failure_message` says which
// backend answer caused it AND names the state the item is actually in
// ("It's still in your vault." / "It's still in the trash."), which is the
// question a user has when a delete does not appear to have happened. A
// second sentence here would be a second vocabulary for the same event, and
// the one that drifts.
#[cfg(test)]
mod tests {
    use super::*;

    /// One vault item, with whatever the test needs on it.
    ///
    /// `item_type` and `username` are both arguments because the subject line
    /// reads both: the username is its second line, and the kind is what
    /// decides which mark the tile falls back to when no favicon has been
    /// fetched.
    fn item(name: &str, username: Option<&str>, item_type: Option<i64>) -> VaultItem {
        VaultItem {
            id: "i1".into(),
            name: name.into(),
            fields: vec![],
            login: username.map(|u| crate::vault_bridge::LoginData {
                username: Some(u.to_string()),
                password: None,
                totp: None,
                uris: vec![],
                other: serde_json::Map::new(),
            }),
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    /// A login with a username, which is the row every test here opens
    /// against unless it is testing one of the other shapes.
    fn login() -> VaultItem {
        item("Ledgerline", Some("a.novak@ledgerline.com"), Some(1))
    }

    /// **The two deletes never share a sentence.** Every string this modal
    /// can show differs between the two kinds, which is the whole reason
    /// `DeleteKind` is an enum and not a `bool` threaded into one format
    /// string. If a future edit collapses any pair of these, the reader of
    /// the permanent delete gets the recoverable one's reassurance.
    #[test]
    fn the_permanent_delete_says_something_different_everywhere_it_speaks() {
        assert_ne!(DeleteKind::ToTrash.heading(), DeleteKind::Forever.heading());
        assert_ne!(DeleteKind::ToTrash.body(), DeleteKind::Forever.body());
        assert_ne!(
            DeleteKind::ToTrash.confirm_label(),
            DeleteKind::Forever.confirm_label()
        );
    }

    /// **The permanent one says it cannot be undone, and the other one does
    /// not say it can't.** Recorded as words rather than argued: this is the
    /// one fact the modal exists to deliver, and it is deliverable only by
    /// the sentence.
    #[test]
    fn only_the_permanent_delete_claims_to_be_permanent() {
        assert!(
            DeleteKind::Forever.body().contains("cannot be undone"),
            "the permanent delete's sentence stopped saying so: {:?}",
            DeleteKind::Forever.body()
        );
        assert!(
            DeleteKind::ToTrash.body().contains("restore"),
            "the recoverable delete stopped saying the item can be restored: {:?}",
            DeleteKind::ToTrash.body()
        );
        assert!(
            !DeleteKind::ToTrash.body().contains("cannot be undone"),
            "the recoverable delete claims to be permanent"
        );
    }

    /// **The button repeats the heading's word.** A button reading "Delete"
    /// under a heading reading "Delete forever" is the one place a reader
    /// could still believe the Trash is catching it.
    #[test]
    fn the_permanent_buttons_words_carry_forever_too() {
        assert!(DeleteKind::Forever.confirm_label().contains("forever"));
        assert!(!DeleteKind::ToTrash.confirm_label().contains("forever"));
    }

    /// **A fresh state carries no error.** The field is the caller's to
    /// fill; opening with one already in it would show a refusal for a write
    /// that has not been attempted.
    #[test]
    fn a_freshly_opened_modal_shows_no_refusal() {
        let state = DeleteConfirmState::new(&login(), None, DeleteKind::ToTrash);
        assert_eq!(state.error, None);
        assert_eq!(state.item_id, "i1");
        assert_eq!(state.subject.name(), "Ledgerline");
        assert_eq!(state.kind, DeleteKind::ToTrash);
    }
    // -- what it paints, and what a click on it reports ----------------------

    /// The window the modal is centred in. `icon_modal`'s harness, and its
    /// constant.
    const BODY: eframe::egui::Vec2 = eframe::egui::Vec2::new(900.0, 700.0);

    /// Every string this frame painted, with where it landed, and every
    /// rectangle under them.
    #[derive(Default)]
    struct Painted {
        texts: Vec<(String, egui::Rect)>,
        rects: Vec<eframe::egui::epaint::RectShape>,
        /// Every stroked line. The header band carries two controls that paint
        /// no words at all -- the warning bang's upright bar, and the two
        /// diagonals of the dismiss ✕ -- and a harness that could only see
        /// text could not tell a card with a way out of its corner from one
        /// without.
        segments: Vec<[egui::Pos2; 2]>,
    }

    impl Painted {
        fn strings(&self) -> Vec<&str> {
            self.texts.iter().map(|(t, _)| t.as_str()).collect()
        }

        /// Where each PICTURE landed.
        ///
        /// A favicon reaches the shape list as an ordinary `Shape::Rect`
        /// carrying a texture in its `brush` -- `egui::Image::paint_at` with
        /// a corner radius takes that path rather than emitting a mesh -- so
        /// the texture id is what tells a drawn image from every plain
        /// rectangle on the card. Nothing else here paints one, which makes
        /// this the only way a test can see that the subject line is drawing
        /// the item's real picture instead of its monogram.
        fn images(&self) -> Vec<egui::Rect> {
            self.rects
                .iter()
                .filter(|r| r.fill_texture_id() != egui::TextureId::default())
                .map(|r| r.rect)
                .collect()
        }

        /// The one band that runs the card's whole width in `fill`.
        ///
        /// The width is part of the question: the confirm button is filled in
        /// the same red as the header it sits under, and only the bands run
        /// edge to edge.
        fn band(&self, fill: egui::Color32) -> egui::Rect {
            let found: Vec<egui::Rect> = self
                .rects
                .iter()
                // **`CARD_WIDTH` plus the border on each side.** The bands are
                // painted out over the card's border rather than stopping
                // inside it, so that none of the card's own fill shows past
                // them -- see `theme::MODAL_BAND_BLEED`, which is the fix for
                // "there is some white line along the curve".
                .filter(|r| {
                    r.fill == fill
                        && (r.rect.width() - (CARD_WIDTH + 2.0 * theme::MODAL_BAND_BLEED))
                            .abs()
                            < 0.5
                })
                .map(|r| r.rect)
                .collect();
            assert_eq!(
                found.len(),
                1,
                "expected one full-width band filled {fill:?}, found {}; the card painted {:?}",
                found.len(),
                self.rects.iter().map(|r| (r.fill, r.rect.width())).collect::<Vec<_>>()
            );
            found[0]
        }

        /// The LOWEST rect painting `label`.
        ///
        /// The buttons are the bottom row of the card, and for
        /// `DeleteKind::Forever` the confirm button says exactly what the
        /// heading says -- on purpose, so the last words read before the
        /// click are the ones that mean "forever". [`Painted::rect_of`]
        /// therefore cannot name that button, and picking the first match
        /// would aim a click at the heading and report no action at all,
        /// which reads as "the button is inert" rather than "the test
        /// clicked the wrong thing".
        fn lowest_rect_of(&self, label: &str) -> egui::Rect {
            self.texts
                .iter()
                .filter(|(t, _)| t == label)
                .map(|(_, r)| *r)
                .max_by(|a, b| a.top().total_cmp(&b.top()))
                .unwrap_or_else(|| {
                    panic!(
                        "the delete modal never painted {label:?}; it painted: {:?}",
                        self.strings()
                    )
                })
        }
    }

    fn walk(shape: &egui::Shape, painted: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => painted.texts.push((
                text.galley.text().to_string(),
                egui::Rect::from_min_size(text.pos, text.galley.size()),
            )),
            egui::Shape::Rect(rect) => painted.rects.push(rect.clone()),
            egui::Shape::LineSegment { points, .. } => painted.segments.push(*points),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, painted);
                }
            }
            _ => {}
        }
    }

    thread_local! {
        /// The context's clock, a tenth of a second per frame.
        ///
        /// **Without it every colour on this card comes back wrong.** An
        /// `egui::Area` fades itself in over `Style::animation_time`, and the
        /// fade is an opacity applied to every shape the layer emits -- so a
        /// frame taken while it is a quarter of the way in reports the header
        /// band's red as a premultiplied dark brown. A headless context's
        /// clock does not advance on its own, and a tenth of a second is
        /// comfortably longer than the animation, so the card is fully opaque
        /// one frame after it appears.
        ///
        /// Thread-local because the test harness gives each test its own
        /// thread and its own context; all that is asked of the number is
        /// that it goes up.
        static CLOCK: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
    }

    fn raw_input(events: &[egui::Event]) -> egui::RawInput {
        let time = CLOCK.with(|clock| {
            clock.set(clock.get() + 0.1);
            clock.get()
        });
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, BODY)),
            time: Some(time),
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
        state: &mut DeleteConfirmState,
        events: &[egui::Event],
    ) -> (DeleteConfirmAction, Painted) {
        let mut action = DeleteConfirmAction::None;
        let output = ctx.run_ui(raw_input(events), |ui| {
            action = draw_delete_modal(ui.ctx(), state);
        });
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        (action, painted)
    }

    /// A full press-and-release, which is what egui needs before it will
    /// report `Response::clicked` -- a press alone is not a click.
    fn click(pos: egui::Pos2) -> Vec<egui::Event> {
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

    /// Runs the modal idle until it is painting, and hands back what it
    /// painted -- `icon_modal::tests::opened`, including the reason the
    /// SECOND frame is not optional: an `egui::Area` the context has never
    /// seen runs a sizing pass on its first frame, which tessellates to
    /// nothing.
    fn opened(ctx: &egui::Context, state: &mut DeleteConfirmState) -> Painted {
        let (sizing, blank) = frame(ctx, state, &[]);
        assert_eq!(
            sizing,
            DeleteConfirmAction::None,
            "the modal reported an action on a frame with no input at all"
        );
        assert!(
            blank.texts.is_empty(),
            "the sizing pass painted after all; this warm-up no longer describes what egui \
             does, and the frame counts in these tests may be off by one"
        );
        // Two painting frames, not one: the first is the one the card appears
        // on and therefore the one its fade-in starts on, and a card read
        // halfway through that fade reports every fill premultiplied down
        // toward transparent. See `CLOCK`.
        let (action, _) = frame(ctx, state, &[]);
        assert_eq!(action, DeleteConfirmAction::None);
        let (action, painted) = frame(ctx, state, &[]);
        assert_eq!(action, DeleteConfirmAction::None);
        assert!(!painted.texts.is_empty(), "the modal painted nothing at all");
        painted
    }

    /// **The header band is the destructive red, and the heading is in it.**
    /// The accent is how the card says what kind of question it is before a
    /// word of it is read, and this is the one modal in the window whose
    /// question destroys something -- so it wears the same red as the button
    /// that carries it out, and the same red this app already spends on
    /// exactly this meaning everywhere else.
    #[test]
    fn the_header_band_is_the_destructive_colour_and_carries_the_heading() {
        for kind in [DeleteKind::ToTrash, DeleteKind::Forever] {
            let ctx = styled_context();
            let mut state = DeleteConfirmState::new(&login(), None, kind);
            let painted = opened(&ctx, &mut state);

            let band = painted.band(theme::ERROR);
            assert!(
                // The band's own height plus the point it bleeds upward over
                // the card's border -- see `theme::MODAL_BAND_BLEED`.
                (band.height() - (theme::MODAL_HEADER_HEIGHT + theme::MODAL_BAND_BLEED))
                    .abs()
                    < 0.5,
                "{kind:?}: the red band is {} tall, so it is not the header",
                band.height()
            );
            let heading = painted
                .texts
                .iter()
                .find(|(t, r)| t == kind.heading() && band.contains_rect(*r))
                .map(|(_, r)| *r);
            assert!(
                heading.is_some(),
                "{kind:?}: {:?} is not painted inside the header band at {band:?}; the card \
                 painted {:?}",
                kind.heading(),
                painted.texts
            );
            // And the footer really is the third band, so this card is the
            // frame's card and not a plain one that happens to have a red
            // rectangle on it.
            let footer = painted.band(theme::CARD_TINT);
            assert!(
                footer.top() > band.bottom(),
                "the footer band at {footer:?} is not below the header at {band:?}"
            );
        }
    }

    /// **It names the item.** The menu that opened it is gone from the
    /// screen by the time it paints, and a confirmation that did not say
    /// which item it was about would be one click away from deleting the
    /// wrong credential.
    #[test]
    fn it_paints_the_heading_the_item_and_both_buttons() {
        for kind in [DeleteKind::ToTrash, DeleteKind::Forever] {
            let ctx = styled_context();
            let mut state = DeleteConfirmState::new(&login(), None, kind);
            let painted = opened(&ctx, &mut state);
            // The username joined this list when the subject line became the
            // item's row. Two logins at the same bank differ by it and by
            // nothing else, so a card naming only "Ledgerline" is a card that
            // cannot tell the reader which of the two is about to go.
            for expected in [
                kind.heading(),
                "Ledgerline",
                "a.novak@ledgerline.com",
                kind.confirm_label(),
                "Cancel",
            ] {
                assert!(
                    painted.strings().contains(&expected),
                    "{kind:?}: the modal never painted {expected:?}; it painted: {:?}",
                    painted.strings()
                );
            }
        }
    }

    /// **The subject line is the item's ROW, tile and all.**
    ///
    /// Three branches, because the tile has three: the item's favicon when
    /// the icon cache has one, the item's KIND mark when it does not, and the
    /// name's monogram when the kind has no mark of its own. That branch is
    /// `vault_window::ModalSubject`'s and it is the whole point of this
    /// change -- the card used to draw a monogram for every item, including
    /// the ones the reader had just picked out of the list BY their logo.
    ///
    /// The favicon half is asserted as a painted `Shape::Image`, because a
    /// texture is the one thing on this card with no string and no fill to
    /// find it by; the fallbacks are asserted as the absence of that image
    /// plus, for the monogram, its two letters.
    #[test]
    fn the_subject_line_is_the_items_row() {
        // With a picture: the item's own artwork, and no monogram under it.
        let ctx = styled_context();
        let texture = ctx.load_texture(
            "delete-modal-test-icon",
            egui::ColorImage::from_rgba_unmultiplied([2, 2], &[255u8; 16]),
            egui::TextureOptions::default(),
        );
        let mut state =
            DeleteConfirmState::new(&login(), Some(&texture), DeleteKind::ToTrash);
        let painted = opened(&ctx, &mut state);
        assert_eq!(
            painted.images().len(),
            1,
            "the card painted {} pictures; the item's favicon is the only one it draws",
            painted.images().len()
        );
        assert!(
            !painted.strings().contains(&"LE"),
            "the card drew the name's monogram over an item that has a favicon: {:?}",
            painted.strings()
        );
        let tile = painted.images()[0];
        let name = painted.lowest_rect_of("Ledgerline");
        assert!(
            tile.right() <= name.left(),
            "the tile at {tile:?} is not in front of the name at {name:?}"
        );

        // With no picture and no kind mark -- a login -- the monogram.
        let ctx = styled_context();
        let mut state = DeleteConfirmState::new(&login(), None, DeleteKind::ToTrash);
        let painted = opened(&ctx, &mut state);
        assert!(
            painted.images().is_empty(),
            "the card painted a picture for an item the icon cache has none for"
        );
        assert_eq!(
            theme::initials("Ledgerline"),
            "LE",
            "the monogram this test looks for is not the one `theme` would draw"
        );
        assert!(
            painted.strings().contains(&"LE"),
            "a login with no favicon lost its monogram tile; it painted: {:?}",
            painted.strings()
        );

        // With no picture and a kind that HAS a mark -- a secure note --
        // neither: `kind_mark::avatar` strokes the note rather than typing
        // two letters, exactly as the row does.
        let ctx = styled_context();
        let note = item("Recovery codes", None, Some(2));
        let mut state = DeleteConfirmState::new(&note, None, DeleteKind::ToTrash);
        let painted = opened(&ctx, &mut state);
        assert!(
            painted.images().is_empty(),
            "the card painted a picture for a secure note the cache has none for"
        );
        assert!(
            !painted.strings().contains(&"RC"),
            "a secure note got the name's monogram instead of the kind mark the row draws; \
             it painted: {:?}",
            painted.strings()
        );
        assert!(
            painted.strings().contains(&"Recovery codes"),
            "the note's name is missing, so the negative above proves nothing: {:?}",
            painted.strings()
        );
    }

    /// **The red button reports a confirm, and the grey one a cancel.**
    /// Clicked at the coordinates the modal really painted, so this fails if
    /// either button stops being drawn, stops being hit-testable, or starts
    /// reporting the other one's answer -- which is the mistake that would
    /// make Cancel delete.
    #[test]
    fn each_button_reports_its_own_answer() {
        for kind in [DeleteKind::ToTrash, DeleteKind::Forever] {
            for (label, expected) in [
                (kind.confirm_label(), DeleteConfirmAction::Confirm),
                ("Cancel", DeleteConfirmAction::Cancel),
            ] {
                let ctx = styled_context();
                let mut state = DeleteConfirmState::new(&login(), None, kind);
                let painted = opened(&ctx, &mut state);
                let at = painted.lowest_rect_of(label).center();
                let (action, _) = frame(&ctx, &mut state, &click(at));
                assert_eq!(
                    action, expected,
                    "{kind:?}: clicking {label:?} reported {action:?}"
                );
            }
        }
    }

    /// **Esc cancels, and it is the only key that answers.** Every other
    /// transient overlay in this app takes Esc; the affirmative key is
    /// deliberately missing, because a modal a stray Return can dismiss into
    /// a delete has made the accident it was put there to prevent slightly
    /// more convenient. Both halves, so this cannot pass against a modal
    /// that answers every keystroke.
    #[test]
    fn escape_cancels_and_enter_does_nothing() {
        let ctx = styled_context();
        let mut state = DeleteConfirmState::new(&login(), None, DeleteKind::Forever);
        let _ = opened(&ctx, &mut state);

        let enter = vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }];
        let (on_enter, _) = frame(&ctx, &mut state, &enter);
        assert_eq!(
            on_enter,
            DeleteConfirmAction::None,
            "Return answered the delete confirmation"
        );

        let (on_escape, _) = frame(&ctx, &mut state, &escape());
        assert_eq!(on_escape, DeleteConfirmAction::Cancel, "Esc did not cancel");
    }

    /// **A refusal the caller wrote in is painted.** The field is the whole
    /// reason a failed delete is no longer indistinguishable from a click
    /// that never registered, and a field nothing draws would leave it
    /// exactly as indistinguishable.
    #[test]
    fn a_refusal_written_into_the_state_reaches_the_card() {
        let ctx = styled_context();
        let mut state = DeleteConfirmState::new(&login(), None, DeleteKind::ToTrash);
        let clean = opened(&ctx, &mut state);
        let sentence = "Couldn't delete this. It's still in your vault.";
        assert!(!clean.strings().contains(&sentence));

        state.error = Some(sentence.to_string());
        let (_, painted) = frame(&ctx, &mut state, &[]);
        assert!(
            painted.strings().contains(&sentence),
            "the refusal never reached the card; it painted: {:?}",
            painted.strings()
        );
    }

    /// **The scrim is up, and it is the one `item_list::MODAL_SCRIM_AREAS`
    /// names.** That list is what gates this window's keyboard navigation,
    /// so a scrim drawn under a different id is a modal the arrow keys steer
    /// straight through.
    #[test]
    fn the_scrim_is_the_one_the_keyboard_gate_looks_for() {
        let ctx = styled_context();
        let mut state = DeleteConfirmState::new(&login(), None, DeleteKind::ToTrash);
        let _ = opened(&ctx, &mut state);
        assert!(
            ctx.memory(|m| m.areas().is_visible(&egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("delete-confirm-scrim"),
            ))),
            "the delete modal draws no scrim under the id the keyboard gate watches"
        );
    }

    /// **The header's dismiss ✕ cancels, and cannot delete.**
    ///
    /// This file gained no line for that mark: it is on
    /// [`theme::modal_card`]'s header band, and the band reports through
    /// [`theme::ModalPress::dismissed`] -- the same flag the Cancel button
    /// sets, which this function already turns into
    /// [`DeleteConfirmAction::Cancel`]. That is the whole argument for putting
    /// the mark on the frame rather than on each card, and it is exactly why
    /// the claim needs testing HERE: the card whose confirm destroys a vault
    /// item is the one where a crossed wire would cost something, and no
    /// assertion in `theme` can see what this file does with the flag.
    ///
    /// Driven on both kinds, because `Forever` is the one with nothing behind
    /// it -- a permanent delete answered by accident is not in the trash.
    #[test]
    fn the_header_dismiss_mark_cancels_and_never_deletes() {
        for kind in [DeleteKind::ToTrash, DeleteKind::Forever] {
            let ctx = styled_context();
            let mut state = DeleteConfirmState::new(&login(), None, kind);
            let painted = opened(&ctx, &mut state);

            // The mark, found from the paint: two diagonals
            // `theme::CLOSE_MARK_SPAN` across. The only other stroke on this
            // card is the warning bang's upright bar, which is not one.
            let arms: Vec<&[egui::Pos2; 2]> = painted
                .segments
                .iter()
                .filter(|[a, b]| {
                    ((b.x - a.x).abs() - theme::CLOSE_MARK_SPAN).abs() < 0.01
                        && ((b.y - a.y).abs() - theme::CLOSE_MARK_SPAN).abs() < 0.01
                })
                .collect();
            assert_eq!(
                arms.len(),
                2,
                "{kind:?}: the header carries {} arms the size of a ✕; it needs exactly two",
                arms.len()
            );
            let at = arms[0][0].lerp(arms[0][1], 0.5);
            assert!(
                (arms[1][0].lerp(arms[1][1], 0.5) - at).length() < 0.01,
                "{kind:?}: the two arms do not cross, so this is not a ✕"
            );

            let (action, _) = frame(&ctx, &mut state, &click(at));
            assert_eq!(
                action,
                DeleteConfirmAction::Cancel,
                "{kind:?}: the header's ✕ at {at:?} did not cancel -- and the one thing it must \
                 never do is answer the question this card is asking"
            );

            // The pairing: Escape gives the same answer, so the mark is the
            // gesture this card already had and not a second one.
            let ctx = styled_context();
            let mut fresh = DeleteConfirmState::new(&login(), None, kind);
            let _ = opened(&ctx, &mut fresh);
            let (by_key, _) = frame(&ctx, &mut fresh, &escape());
            assert_eq!(by_key, action, "{kind:?}: the ✕ and Escape disagree");
        }
    }
}

//! The vault window's **"Select icon"** modal, opened from an item row's
//! right-click menu.
//!
//! A dedicated small file rather than folded into `item_list.rs` or `mod.rs`,
//! for [`super::folder_modal`]'s reason: it owns its own state and drawing,
//! and neither of those files needs to know how it is built --
//! `vault_window::mod` needs [`IconPickState`], [`draw_icon_modal`] and the
//! [`IconPickAction`] that comes back, and nothing else.
//!
//! # Why a modal and not a submenu
//!
//! One of the two routes needs the user to TYPE something. A context menu can
//! hold a submenu of choices; it cannot hold a text field, and egui's menu
//! layer closes on the first click outside itself, so a field inside one would
//! lose focus to its own scrollbar. The file route could have lived on the
//! menu alone -- but then the two halves of one decision would sit in two
//! places, and the user who opened the URL box to discover they would rather
//! browse for a file would have to close it and go back to the menu.
//!
//! # What it does NOT do
//!
//! It opens no dialog, reads no file, makes no request and writes to no vault.
//! It reports which button was pressed. `IFileOpenDialog::Show` is modal and
//! pumps its own message loop, so calling it from in here would re-enter
//! egui's frame -- the same rule `EditAction::PickAppFile` and
//! `TotpAddAction::OpenImage` already follow, and the reason both of those are
//! actions rather than calls.

use crate::item_icon::{IconChoice, IconRefusal};
use crate::theme;
use crate::vault_bridge::VaultItem;
use eframe::egui::{self, RichText};

/// The modal's per-open state.
pub struct IconPickState {
    /// Which item is being given a picture. The id and not the item: the
    /// modal is open across frames, a sync can replace the snapshot under it,
    /// and the handler looks the item up fresh when it acts -- the same rule
    /// `RowCommand::RefreshIcon` follows for the domain it forgets.
    pub item_id: String,
    /// The row the card shows: the item's tile, its name and its username,
    /// copied at the moment the modal opened.
    ///
    /// Copied for the reason above and for one more that is this card's own:
    /// the alternative -- looking the item up to draw its subject line --
    /// would put a vault read in a draw closure. See [`super::ModalSubject`],
    /// which is shared with [`super::delete_modal`].
    ///
    /// **The tile it draws is the icon the item has NOW**, which on this card
    /// is the thing being changed. That is deliberate: "Select icon" opened
    /// on an item that already has a picture should show which picture it is
    /// replacing. It does not follow the choice being made in the box below,
    /// because nothing has been fetched yet -- see this file's doc on why no
    /// request is made here.
    pub subject: super::ModalSubject,
    /// The URL box. Seeded with the item's chosen URL when it has one, so
    /// "the address I set is slightly wrong" is an edit rather than a
    /// retype -- and empty for an item whose choice is a stored picture,
    /// because there is no address to show and putting a base64 blob in a
    /// text field would be a box the user cannot read or fix.
    pub url: String,
    /// The refusal to show, from [`IconRefusal::sentence`] or from a failed
    /// vault write.
    ///
    /// Set by the CALLER, like `FolderEditState::error`: without somewhere to
    /// put it, a failed pick logged a warning and left the modal sitting open
    /// exactly as it was, which from the outside is indistinguishable from
    /// the click never having registered.
    pub error: Option<String>,
}

impl IconPickState {
    /// Opens the modal for `item`, seeded from whatever it already has.
    ///
    /// `icon` is the caller's `icons.textures.get(item.id.as_str())` -- the
    /// picture the row is showing right now, which is what the subject line
    /// draws. `existing` stays a separate argument rather than being read off
    /// the item here, because it is the item's *stored choice* and not its
    /// rendered picture: the two disagree for an item whose chosen URL has
    /// never answered, and the box has to show the address that was set even
    /// when nothing was fetched from it.
    pub fn new(
        item: &VaultItem,
        icon: Option<&egui::TextureHandle>,
        existing: Option<&IconChoice>,
    ) -> Self {
        Self {
            item_id: item.id.clone(),
            subject: super::ModalSubject::of(item, icon),
            url: existing.and_then(IconChoice::url).unwrap_or_default().to_string(),
            error: None,
        }
    }
}

/// What the modal is asking `vault_window::mod` to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconPickAction {
    None,
    /// Open the shell's file dialog and, if it answers, store what it points
    /// at. The path is NOT chosen here -- see this module's doc.
    ChooseFile,
    /// Store the address in the box. Carries it, already trimmed, so the
    /// handler cannot act on a different string than the one that was
    /// validated below.
    UseUrl(String),
    Cancel,
}

/// The one hint under the URL box, and the one under the button.
///
/// The formats are named from [`crate::file_picker::ICON_FILTER_SPEC`] rather
/// than typed out, so the sentence and the dialog's own filter cannot come to
/// disagree -- a user told "PNG or ICO" over a dialog showing only PNG files
/// concludes the app is broken, and they are half right.
fn file_hint() -> String {
    format!(
        "Any picture Deskwarden can read ({}). It is shrunk to 64x64 and stored on the item, so \
         it syncs to your other devices.",
        crate::file_picker::ICON_FILTER_SPEC
    )
}

/// The URL box's hint.
const URL_HINT: &str = "A direct link to an image, e.g. https://example.com/logo.png. The \
                        address is stored; the picture is fetched when the item is shown.";

/// How wide the card is. Unchanged from the plain card this replaced: two
/// hints of three lines each set the height, and 360 is the width at which
/// neither of them runs to four.
const CARD_WIDTH: f32 = 360.0;

/// The heading, which is now the header band's words rather than the body's
/// first line.
const CARD_TITLE: &str = "Select icon";

/// Draws the modal as a centred card over a dimmed scrim and reports what was
/// pressed.
///
/// **The card is [`theme::modal_card`]'s**, which is the app's one modal
/// shape: a coloured header band, a white body, and a footer whose two
/// answers split its width. This file and [`super::delete_modal`] each used
/// to assemble that card by hand, and the layout moved into `theme` when the
/// two were brought into line -- a third hand-assembled copy is what the move
/// was made to prevent.
///
/// **The header is [`theme::BLUE`], and there was no precedent to copy.**
/// The design this card follows is the send preflight's refusal, which is the
/// only card in the app with a coloured header band at all, and its accent is
/// [`theme::ERROR`] because a refusal is what it is. Nothing in this crate
/// draws that band for an ordinary question -- `prefs_ui`'s modal has a
/// header, but it is white with a hairline under it and a ✕ at its end, which
/// is a window title bar rather than an accent. So the colour is chosen here
/// rather than found: blue is this app's one non-alarming accent, it is what
/// [`theme::primary_button`] already fills the affirmative answer with, and
/// the pairing means the header and the button a user is being steered toward
/// are the same colour on every card that is not a warning.
///
/// **The URL is validated HERE, on the click, and a bad one never leaves this
/// function.** `item_icon::choice_from_url` is the shape check -- a scheme
/// this app fetches, and a host -- and its refusal goes straight into
/// [`IconPickState::error`] rather than being carried out to a handler that
/// would have to decide what to do with it. The reason is that the two
/// failures are different in kind: a mistyped address is something the user
/// can fix in the box they are looking at, and a vault write that fails is
/// not. Only the second needs to reach the caller.
///
/// **What it does NOT check is whether the URL answers.** That would be a
/// blocking HTTP request on the UI thread -- up to ten seconds of frozen
/// window under a modal, on a `favicon` deadline chosen for a background
/// thread -- or a whole in-flight state on this struct for a fetch the icon
/// loader is about to make anyway, one frame later, on the thread it belongs
/// on. So a URL that 404s is stored, the loader's fetch produces nothing, and
/// the item shows the icon it would have had otherwise. The choice is still
/// there to see and to fix, and "Use the automatic icon" is on the menu
/// beneath the entry that opened this.
pub fn draw_icon_modal(ctx: &egui::Context, state: &mut IconPickState) -> IconPickAction {
    let mut action = IconPickAction::None;

    // The `Area` is declared HERE rather than inside `theme::modal_scrim`,
    // and the id is a literal on purpose: `item_list::MODAL_SCRIM_AREAS` is
    // walked for exactly this declaration, and a scrim it cannot see is a
    // modal the item list's arrow keys steer straight through.
    theme::modal_scrim(ctx, egui::Area::new(egui::Id::new("icon-pick-scrim")));

    // **Greyed while the box is empty, rather than refusing on the click.**
    // An empty box is not a wrong answer -- it is a user who has not typed
    // yet, and painting a refusal at one is how a form nags
    // (`totp_add::Reading::Empty`'s rule). A box with something wrong IN it
    // does get the refusal, below.
    //
    // **Read before the body draws, where it used to be read after.** The
    // body and the footer are separate closures now, so the one that edits
    // the box and the one that reads it cannot both hold it; the answer is
    // worked out here and passed in by value. The only frame on which the two
    // orders disagree is the one the first character is typed on, and no
    // click can land on that frame anyway -- egui hit-tests a press against
    // the widget rects registered on the PREVIOUS frame, which is the very
    // thing `an_empty_box_reports_nothing_and_refuses_nothing` puts an idle
    // frame in the middle of itself for.
    let typed = !state.url.trim().is_empty();

    let press = theme::modal_card(
        ctx,
        egui::Area::new(egui::Id::new("icon-pick-modal")),
        theme::ModalCard {
            accent: theme::BLUE,
            // No mark. This is a question, not a refusal -- see the doc above.
            glyph: theme::ModalGlyph::None,
            title: CARD_TITLE,
            width: CARD_WIDTH,
            dismiss: "Cancel",
        },
        |ui| {
            // The row this is about, drawn as the results list draws it --
            // and on this card the tile is doing a second job, because the
            // picture in it is the one about to be replaced. The menu that
            // opened this is long gone from the screen by now, and a modal
            // that did not say which item it was about would be one click
            // away from putting a logo on the wrong credential.
            state.subject.draw(ui);

            ui.add_space(12.0);
            theme::field_label(ui, "Image file");
            ui.add_space(6.0);
            // **Still in the BODY, not in the footer.** The footer holds the
            // two answers to the card's own question, and this is a third
            // thing: it opens a dialog and leaves the modal standing. Putting
            // it beside "Use this address" would offer three buttons as if
            // they were alternatives of the same weight.
            if theme::secondary_button(ui, "Choose a file\u{2026}").clicked() {
                action = IconPickAction::ChooseFile;
            }
            ui.add_space(4.0);
            hint(ui, &file_hint());

            ui.add_space(16.0);
            theme::field_label(ui, "Or an image address");
            ui.add_space(6.0);
            theme::text_field(ui, &mut state.url, false);
            ui.add_space(4.0);
            hint(ui, URL_HINT);

            if let Some(error) = &state.error {
                ui.add_space(10.0);
                // Wrapped, not truncated: every sentence here explains a
                // refusal, and a truncated explanation is worse than none.
                // `move_error_band`'s rule one file over.
                ui.add(
                    egui::Label::new(RichText::new(error).size(12.0).color(theme::ERROR)).wrap(),
                );
            }
        },
        |ui| theme::primary_button_enabled(ui, "Use this address", None, typed),
    );

    // **The URL is still validated HERE**, one statement further down than it
    // used to be: the click now comes back from the frame rather than being
    // read inside it, and nothing else about the rule changed. A bad address
    // sets the refusal and leaves the modal standing.
    //
    // Only one of these three can have happened -- egui reports at most one
    // click per frame, and the file button is in the body while the other two
    // are the footer's own answers -- so the order they are read in is not
    // load-bearing.
    if press.confirmed {
        match crate::item_icon::choice_from_url(&state.url) {
            Ok(_) => action = IconPickAction::UseUrl(state.url.trim().to_string()),
            Err(why) => state.error = Some(why.sentence()),
        }
    }
    if press.dismissed {
        action = IconPickAction::Cancel;
    }

    // Esc cancels, same as every other transient overlay in this app.
    if action == IconPickAction::None && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = IconPickAction::Cancel;
    }

    action
}

/// One faint explanatory line under a control, wrapped.
fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(RichText::new(text).size(11.0).color(theme::TEXT_GHOST)).wrap(),
    );
}

/// The refusal a caller shows when the vault write itself fails.
///
/// Here rather than at the call site so that the modal's two failure
/// sentences -- the one [`IconRefusal`] produces and this one -- are written
/// in the same place and in the same voice. It deliberately does NOT include
/// the backend's error text: that is a `VaultError` for the log, and pasting
/// a transport failure into a modal tells the user nothing they can act on.
pub fn write_failed_sentence() -> String {
    "That icon could not be saved to the vault. Check your connection and try again \u{2014} the \
     item still has the icon it had before."
        .to_string()
}

/// A refusal, as a sentence, for the caller that has an [`IconRefusal`] and
/// nowhere better to turn it into copy.
///
/// A one-line pass-through, and it exists so that `vault_window::mod` never
/// has to name `IconRefusal`'s variants: the file route's outcome arrives
/// there as a `Result` and leaves as a string.
pub fn refusal_sentence(why: IconRefusal) -> String {
    why.sentence()
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Pos2, Rect, Vec2};

    /// The window the modal is centred in. `folder_modal`'s harness
    /// throughout this module, and its constant: tall enough that the card
    /// fits with room around it.
    const BODY: Vec2 = Vec2::new(900.0, 700.0);

    /// Every string this frame painted, with where it landed, and every
    /// rectangle under them.
    #[derive(Default)]
    struct Painted {
        texts: Vec<(String, Rect)>,
        rects: Vec<eframe::egui::epaint::RectShape>,
        segments: Vec<[Pos2; 2]>,
    }

    impl Painted {
        fn strings(&self) -> Vec<&str> {
            self.texts.iter().map(|(t, _)| t.as_str()).collect()
        }

        /// The one band that runs the card's whole width in `fill`.
        ///
        /// The width is part of the question: the blue accent is also the
        /// fill of the affirmative button under it, and only the bands run
        /// edge to edge.
        fn band(&self, fill: egui::Color32) -> Rect {
            let found: Vec<Rect> = self
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

        fn has(&self, label: &str) -> bool {
            self.texts.iter().any(|(t, _)| t == label)
        }

        /// The one rect painting `label`, or a failure naming everything that
        /// *was* painted -- which is what turns "the button is gone" into a
        /// readable message rather than a silent click into empty space.
        fn rect_of(&self, label: &str) -> Rect {
            let found: Vec<Rect> =
                self.texts.iter().filter(|(t, _)| t == label).map(|(_, r)| *r).collect();
            assert_eq!(
                found.len(),
                1,
                "expected exactly one {label:?} in the icon modal, found {}; painted: {:?}",
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
        /// band's blue as a premultiplied navy. A headless context's clock
        /// does not advance on its own, and a tenth of a second is
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
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, BODY)),
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
        state: &mut IconPickState,
        events: &[egui::Event],
    ) -> (IconPickAction, Painted) {
        let mut action = IconPickAction::None;
        let output = ctx.run_ui(raw_input(events), |ui| {
            action = draw_icon_modal(ui.ctx(), state);
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

    /// One vault item. A login with a username, because that is the row the
    /// subject line has both of its lines to draw for.
    fn item() -> VaultItem {
        VaultItem {
            id: "i1".into(),
            name: "Chase".into(),
            fields: vec![],
            login: Some(crate::vault_bridge::LoginData {
                username: Some("a.novak@chase.com".into()),
                password: None,
                totp: None,
                uris: vec![],
                other: serde_json::Map::new(),
            }),
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: Some(1),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    fn state() -> IconPickState {
        IconPickState::new(&item(), None, None)
    }

    /// Runs the modal idle until it is painting, and hands back what it
    /// painted -- `folder_modal::tests::opened`, including the reason the
    /// SECOND frame is not optional: an `egui::Area` the context has never
    /// seen runs a sizing pass on its first frame, which tessellates to
    /// nothing.
    fn opened(ctx: &egui::Context, state: &mut IconPickState) -> Painted {
        let (sizing, blank) = frame(ctx, state, &[]);
        assert_eq!(
            sizing,
            IconPickAction::None,
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
        assert_eq!(action, IconPickAction::None);
        let (action, painted) = frame(ctx, state, &[]);
        assert_eq!(action, IconPickAction::None);
        assert!(!painted.texts.is_empty(), "the modal painted nothing at all");
        painted
    }

    // -- the state ----------------------------------------------------------

    /// The box is seeded from a URL choice and left empty for a stored
    /// picture. The second half is the one worth pinning: a base64 payload in
    /// a text field is a box the user can neither read nor fix, and an
    /// accidental `to_field_value()` there would put one in.
    #[test]
    fn the_url_box_is_seeded_from_a_url_choice_and_only_from_one() {
        let url = IconChoice::Url { url: "https://example.com/logo.png".into() };
        let seeded = IconPickState::new(&item(), None, Some(&url));
        assert_eq!(seeded.url, "https://example.com/logo.png");
        assert_eq!(seeded.item_id, "i1");
        assert_eq!(seeded.error, None);

        let png = IconChoice::Png { png: "AAAA".into() };
        let from_png = IconPickState::new(&item(), None, Some(&png));
        assert_eq!(
            from_png.url, "",
            "the URL box was seeded from a stored picture, so it holds base64"
        );
        assert_eq!(state().url, "", "the box is empty for an item with no choice at all");
    }

    // -- what it draws ------------------------------------------------------

    /// **The modal names the row it is about.** The menu that opened it is
    /// gone from the screen by the time it appears, and a picker that did not
    /// say which item it was for is one click from putting a logo on the
    /// wrong credential.
    #[test]
    fn the_card_names_the_item_and_both_routes() {
        let ctx = styled_context();
        let mut state = state();
        let painted = opened(&ctx, &mut state);
        // The username joined this list when the subject line became the
        // item's own row -- see `theme::modal_subject`. Two logins at the
        // same bank differ by it and by nothing else, and this card is one
        // click from putting a logo on whichever of them was not meant.
        for needle in [
            "Select icon",
            "Chase",
            "a.novak@chase.com",
            "Choose a file\u{2026}",
            "Use this address",
        ] {
            assert!(
                painted.has(needle),
                "the modal did not paint {needle:?}; painted: {:?}",
                painted.strings()
            );
        }
    }

    /// **The header band is the ordinary accent, and it carries no warning
    /// mark.** This card asks a question -- it neither destroys anything nor
    /// refuses anything -- so it wears the blue the affirmative button under
    /// it wears, and the glyph slot the refusal card fills with a triangle
    /// stays empty. A red band here would be the app telling the user that
    /// choosing a picture is dangerous.
    #[test]
    fn the_header_band_is_the_ordinary_accent_and_carries_no_warning() {
        let ctx = styled_context();
        let mut state = state();
        let painted = opened(&ctx, &mut state);

        let band = painted.band(theme::BLUE);
        assert!(
            // The band's own height plus the point it bleeds upward over the
            // card's border -- see `theme::MODAL_BAND_BLEED`.
            (band.height() - (theme::MODAL_HEADER_HEIGHT + theme::MODAL_BAND_BLEED))
                .abs()
                < 0.5,
            "the blue band is {} tall, so it is not the header",
            band.height()
        );
        assert!(
            painted
                .texts
                .iter()
                .any(|(t, r)| t == CARD_TITLE && band.contains_rect(*r)),
            "{CARD_TITLE:?} is not painted inside the header band at {band:?}; the card painted \
             {:?}",
            painted.strings()
        );
        assert!(
            painted.rects.iter().all(|r| r.fill != theme::ERROR),
            "something on this card is painted in the destructive red"
        );
        assert!(
            painted
                .segments
                .iter()
                .all(|[a, b]| !band.contains(*a) || !band.contains(*b)),
            "the header drew a mark; this card is a question and has no warning to give"
        );
        // And the footer really is the third band, so this is the frame's
        // card and not a plain one with a blue rectangle on it.
        let footer = painted.band(theme::CARD_TINT);
        assert!(
            footer.top() > band.bottom(),
            "the footer band at {footer:?} is not below the header at {band:?}"
        );
    }

    // -- what it reports ----------------------------------------------------

    /// Escape closes it, like every other overlay in this window.
    #[test]
    fn escape_cancels() {
        let ctx = styled_context();
        let mut state = state();
        let _ = opened(&ctx, &mut state);
        let (action, _) = frame(&ctx, &mut state, &escape());
        assert_eq!(action, IconPickAction::Cancel);
    }

    #[test]
    fn cancel_cancels() {
        let ctx = styled_context();
        let mut state = state();
        let painted = opened(&ctx, &mut state);
        let (action, _) = frame(&ctx, &mut state, &click(painted.rect_of("Cancel").center()));
        assert_eq!(action, IconPickAction::Cancel);
    }

    /// **The file button asks the CALLER to open the dialog.** Nothing here
    /// may touch the shell -- see the module doc -- so the whole of this
    /// route from inside the modal is one action value.
    #[test]
    fn the_file_button_asks_for_the_dialog() {
        let ctx = styled_context();
        let mut state = state();
        let painted = opened(&ctx, &mut state);
        let at = painted.rect_of("Choose a file\u{2026}").center();
        let (action, _) = frame(&ctx, &mut state, &click(at));
        assert_eq!(action, IconPickAction::ChooseFile);
        assert_eq!(state.error, None, "opening a dialog is not a failure");
    }

    /// A good address leaves the modal as the trimmed string the caller
    /// writes -- and it is that string and not the box's raw contents, so the
    /// value that was validated is the value that is stored.
    #[test]
    fn a_good_address_is_reported_trimmed() {
        let ctx = styled_context();
        let mut state = state();
        state.url = "  https://example.com/logo.png \t".into();
        let painted = opened(&ctx, &mut state);
        let at = painted.rect_of("Use this address").center();
        let (action, _) = frame(&ctx, &mut state, &click(at));
        assert_eq!(action, IconPickAction::UseUrl("https://example.com/logo.png".into()));
        assert_eq!(state.error, None);
    }

    /// **A bad address never leaves this function**, and the modal stays open
    /// saying why. The two failures are different in kind: this one the user
    /// can fix in the box they are looking at, and a vault write that fails
    /// is not -- only the second needs to reach the caller.
    #[test]
    fn a_bad_address_is_refused_here_and_reported_in_the_card() {
        let ctx = styled_context();
        let mut state = state();
        state.url = "example.com/logo.png".into();
        let painted = opened(&ctx, &mut state);
        let at = painted.rect_of("Use this address").center();
        let (action, _) = frame(&ctx, &mut state, &click(at));
        assert_eq!(
            action,
            IconPickAction::None,
            "an address with no scheme was reported out to the writer"
        );
        assert_eq!(
            state.error.as_deref(),
            Some(IconRefusal::BadUrl.sentence().as_str()),
            "the refusal is not the one `item_icon` decided on"
        );
        // And it is on screen, not merely in the struct. `folder_modal`'s
        // lesson: a failure that only sets a field is indistinguishable from
        // the click never having registered.
        let (_, after) = frame(&ctx, &mut state, &[]);
        assert!(
            after.strings().iter().any(|t| t.contains("http://")),
            "the refusal was not painted; painted: {:?}",
            after.strings()
        );
    }

    /// **An empty box is not a wrong answer.** The button is unavailable
    /// rather than refusing on the click: painting a refusal at a user who
    /// has not typed yet is how a form nags (`totp_add::Reading::Empty`'s
    /// rule).
    #[test]
    fn an_empty_box_reports_nothing_and_refuses_nothing() {
        let ctx = styled_context();
        let mut state = state();
        let painted = opened(&ctx, &mut state);
        let at = painted.rect_of("Use this address").center();
        let (action, _) = frame(&ctx, &mut state, &click(at));
        assert_eq!(action, IconPickAction::None);
        assert_eq!(
            state.error, None,
            "an untouched box was told off; the button is supposed to be unavailable instead"
        );
        // The live control: the very same click on the very same button, with
        // something in the box, DOES report. Without this, a button that had
        // stopped working at all would pass the assertions above.
        //
        // **One idle frame between the typing and the click, and it is not
        // padding.** egui hit-tests a press against the widget rects
        // registered on the PREVIOUS frame, and on that frame this button was
        // the disabled one -- so a press landing on it in the same frame the
        // box first has text in it is not attributed to a clickable widget at
        // all. A real user types and then aims, which is exactly this.
        state.url = "https://example.com/logo.png".into();
        let _ = frame(&ctx, &mut state, &[]);
        let (action, _) = frame(&ctx, &mut state, &click(at));
        assert_eq!(
            action,
            IconPickAction::UseUrl("https://example.com/logo.png".into()),
            "the button does not work with anything in the box either, so the assertions \
             above are about a dead control"
        );
    }

    // -- copy ---------------------------------------------------------------

    /// The hint under the file button names the formats the dialog will
    /// actually show, because it is built from the dialog's own filter.
    #[test]
    fn the_file_hint_names_the_formats_the_dialog_offers() {
        let hint = file_hint();
        assert!(
            hint.contains(crate::file_picker::ICON_FILTER_SPEC),
            "the hint stopped being built from the dialog's filter: {hint}"
        );
        assert!(hint.contains("*.png") && hint.contains("*.ico"));
    }

    /// The two sentences a caller can show are distinct and neither is empty
    /// -- a write failure and a refusal must not read as the same event.
    #[test]
    fn the_two_caller_facing_sentences_are_not_the_same_sentence() {
        let failed = write_failed_sentence();
        assert!(!failed.is_empty());
        for why in [
            IconRefusal::Unreadable,
            IconRefusal::TooLarge,
            IconRefusal::NotAnImage,
            IconRefusal::TooBigToStore,
            IconRefusal::BadUrl,
        ] {
            assert_ne!(refusal_sentence(why), failed);
            assert!(!refusal_sentence(why).is_empty(), "{why:?} has no sentence");
        }
        // The write failure says the item is unchanged, which is the fact the
        // user most needs after a save that did not happen.
        assert!(failed.contains("before"), "the write-failure sentence: {failed}");
    }

    // -- source pins --------------------------------------------------------

    /// **The modal never writes to the vault and never opens a dialog.** Both
    /// belong to the action handler -- see the module doc -- and both would be
    /// re-entrancy bugs from inside a draw closure. A source pin because the
    /// thing being ruled out is a call that cannot be made in a test.
    #[test]
    fn nothing_here_shells_out_or_writes() {
        let source = include_str!("icon_modal.rs");
        let code = source.split("#[cfg(test)]").next().expect("the file has a test module");
        assert!(code.len() < source.len(), "the test-module split did nothing");
        for forbidden in [
            concat!("pick_icon_", "image("),
            concat!("read_icon_", "file("),
            concat!("update_", "item("),
            "std::fs::",
        ] {
            assert!(
                !code.contains(forbidden),
                "{forbidden:?} is called from the modal's own code, which runs inside a draw \
                 closure"
            );
        }
        // Positive control: a needle that IS there, so the absences above are
        // about this file and not about the split.
        assert!(code.contains("choice_from_url"));
    }
}

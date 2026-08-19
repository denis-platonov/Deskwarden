use crate::app::FillChoice;
use crate::theme;
use eframe::egui::{self, CornerRadius, Margin, RichText, Sense, Stroke};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set for the duration of a `show_prompt_overlay` call.
///
/// The normal single-instance flow can't call this re-entrantly (it's a
/// blocking call on the one main thread, which can't process another
/// foreground event until this one returns) -- but two Deskwarden processes
/// running at once (observed live: an old dev build left running alongside a
/// freshly relaunched one) both watch the same foreground events and would
/// each independently open their own overlay for the same match, stacking
/// two overlay windows. This guard can't stop a *second process*'s window
/// from opening, but it does stop this process from ever contributing a
/// second one, and turns any single-process re-entrancy this analysis missed
/// into a harmless no-op instead of a stuck duplicate window.
static OVERLAY_OPEN: AtomicBool = AtomicBool::new(false);

/// What the overlay shows about the matched vault item: enough for the user
/// to recognize *which* credentials are about to be filled (design 2a shows
/// the username with the item name under it), without ever putting the
/// password itself on screen.
pub struct OverlayMatch {
    pub item_name: String,
    pub username: Option<String>,
}

/// The overlay window's inner width, in points. Fixed: the card is a
/// frameless, always-on-top window with no title bar, so the user cannot
/// resize it and nothing inside it may depend on a width it does not have.
pub const OVERLAY_WIDTH: f32 = 396.0;

/// Vertical pitch of ONE choice row: the painted row tile plus the gap that
/// separates it from the next one.
///
/// Measured, not chosen. `a_row_occupies_exactly_one_row_height` renders real
/// cards at one and two rows and asserts the difference between the two
/// cards' painted content is exactly this — so a font, a padding or an avatar
/// size changing under us fails a test here instead of silently pushing the
/// last row out of a window nobody can scroll.
pub const ROW_HEIGHT: f32 = 50.0;

/// Everything in the card that is NOT a choice row: the outer margins, the
/// card stroke, the header strip, the two hairlines, the row container's own
/// padding and the footer strip.
///
/// **Measured, like [`ROW_HEIGHT`], and no longer derived from 164.0.**
/// `the_chrome_constant_is_the_chrome_the_card_actually_paints` lays real
/// cards out at one, two, three and four rows and reads back what egui says
/// each one needs (154, 204, 254, 304 points): the part that does not grow
/// with `n` is exactly [`MEASURED_CHROME`], 104.0.
///
/// `CHROME_HEIGHT` is that measurement plus [`CHROME_SLACK`]. The sum is
/// still 114.0, so the one-row window is still the 164.0 the overlay has
/// always shipped at — but the two halves of that number are now separately
/// checkable, and the half that describes the drawing is checked against the
/// drawing.
pub const CHROME_HEIGHT: f32 = 114.0;

/// The chrome the card actually paints, in points: the distance from the
/// window's top edge to the first row's top, plus the distance from the last
/// row's bottom to the bottom of the space egui allocates for the card
/// (footer strip, card stroke and the outer margin that holds the drop
/// shadow), less one [`ROW_GAP`] — because the gap lives inside `ROW_HEIGHT`
/// (see [`ROW_GAP`]) and would otherwise be counted twice.
///
/// This is the number a test can fail. It is asserted equal to the measured
/// value at all four row counts the overlay can show, so a font, a margin or
/// a header control changing size fails here rather than clipping a row off a
/// window that has no scrollbar.
pub const MEASURED_CHROME: f32 = 104.0;

/// How much taller than it needs to be the overlay window is.
///
/// The overlay has shipped at 396x164 since it was written; a one-row card
/// only needs 154. The 10 points are dead space at the bottom of the card.
///
/// They are kept, rather than reclaimed, because slack in this direction is
/// the safe direction: a window taller than its card wastes ten points, a
/// window shorter than its card loses a row off a frameless, always-on-top
/// surface with no scrollbar and no resize border. Shrinking a shipped window
/// is a visible change to every user and buys nothing this module needs.
///
/// It is asserted **exactly**, not as a `>= 0.0` bound: a one-sided bound
/// cannot tell 10 points of deliberate slack from 30 points of a header that
/// silently stopped being drawn.
pub const CHROME_SLACK: f32 = 10.0;

/// The overlay window's inner height for a card showing `rows` choice rows.
///
/// Pure arithmetic — no egui, no context, no fonts — because
/// `app::overlay_position` has to know how tall the window will be in order
/// to clamp it onto the monitor's work area *before* the window exists to
/// measure.
///
/// `rows.max(1)` because a card with no rows is not a shorter card: the
/// overlay always paints at least one row (with no choices it paints the
/// matched-credential row it has always painted), and a zero-row height would
/// clip that row's bottom off a window the user cannot scroll.
pub fn overlay_height(rows: usize) -> f32 {
    CHROME_HEIGHT + ROW_HEIGHT * rows.max(1) as f32
}

/// Opens the autofill overlay for `app_name`: a small, frameless,
/// always-on-top card (design 2a — "no chrome") with the Deskwarden header,
/// the matched credential row, and a keyboard-hint footer.
///
/// Returns `Some(choice)` — **which** row the user picked (clicked, or the
/// first row if they pressed Enter) — and `None` if they dismissed it (the
/// header's ✕, Esc, or closing the window).
///
/// `choices` are the rows to offer, in order; the **first** is the primary,
/// the one Enter takes and the one drawn in the selected treatment. An empty
/// slice paints the single matched-credential row the overlay has always
/// painted and answers [`FillChoice::Saved`] for it.
///
/// `matched` is `None` when the item couldn't be read back from the vault at
/// prompt time; the overlay still shows, it just can't name the credentials.
///
/// `anchor` is the top-left corner (screen pixels) to open the window at --
/// computed by the caller (`app::overlay_position`) from where the matched
/// field actually is, so the overlay reads as "next to the field" rather
/// than wherever the OS defaults a new window to. `None` falls back to
/// whatever the OS picks.
/// **The window `show_prompt_overlay` asks the OS for** in order to show
/// `choices` at `anchor`.
///
/// Extracted from `show_prompt_overlay` for one reason: `show_prompt_overlay`
/// calls `eframe::run_native`, which opens a real always-on-top window, so no
/// test in this crate may execute it — and the size it asks for was therefore
/// the one number in the overlay that nothing could observe. It could be a
/// literal `100.0`, or `overlay_height(1)` for a four-row card, and every
/// geometry test in this module stayed green, because those tests build their
/// own window out of `overlay_height` and then check the card against it. The
/// card was always fine. It was the *window* that was never looked at.
///
/// `NativeOptions` and `ViewportBuilder` are plain structs with public
/// fields, so a test can read `overlay_options(choices, None).viewport
/// .inner_size` and paint a real card into exactly that many points. That is
/// what `the_window_the_overlay_actually_asks_for_fits_the_card_it_will_draw`
/// does, and it is the only assertion in this module about the requested size
/// that is not built out of the number it is checking.
///
/// **It takes `choices`, not a row count**, deliberately: handing it the wrong
/// number is the whole bug, so the one caller is not given the opportunity.
/// `show_prompt_overlay`'s remaining share of the decision is the single line
/// that passes its own `choices` along.
///
/// Sized for the rows it was actually given, not for one: a window built for
/// one row that paints four clips the last three off a frameless card the user
/// cannot scroll. `overlay_height` floors at one row, so the empty and
/// one-choice cases are still 164.0.
pub fn overlay_options(choices: &[FillChoice], anchor: Option<(f32, f32)>) -> eframe::NativeOptions {
    options_for_rows(choices.len(), anchor)
}

/// [`overlay_options`] with the row count named directly.
///
/// **Private, and it stays private.** `overlay_options`' doc explains why the
/// public entry point takes the choice list rather than a count: handing it
/// the wrong number is the entire bug that function exists to prevent, so its
/// one caller is not given the opportunity to. The no-match card has no choice
/// list to count -- it has [`NO_MATCH_ROWS`], a constant checked against the
/// card it really draws -- so it needs this shape, and nothing outside this
/// module may have it.
fn options_for_rows(rows: usize, anchor: Option<(f32, f32)>) -> eframe::NativeOptions {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([OVERLAY_WIDTH, overlay_height(rows)])
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_icon(theme::window_icon());
    if let Some((x, y)) = anchor {
        viewport = viewport.with_position([x, y]);
    }
    eframe::NativeOptions {
        viewport,
        ..Default::default()
    }
}

pub fn show_prompt_overlay(
    app_name: &str,
    matched: Option<&OverlayMatch>,
    anchor: Option<(f32, f32)>,
    choices: &[FillChoice],
) -> Option<FillChoice> {
    if OVERLAY_OPEN.swap(true, Ordering::SeqCst) {
        log::warn!(
            "autofill overlay requested for {app_name} while one is already open in this \
             process; ignoring rather than stacking a second window"
        );
        return None;
    }

    let app_name = app_name.to_string();
    let (item_name, username) = match matched {
        Some(m) => (m.item_name.clone(), m.username.clone()),
        None => (String::new(), None),
    };

    // Same Rc<RefCell<_>> pattern as picker_ui::run_picker: the update
    // closure/app is 'static and must move-capture its state, so a plain
    // local bool can't be read back after the blocking call returns. A clone
    // of the Rc is moved in; the original is read here once the blocking
    // call returns (safe: same thread, no cross-thread sharing).
    let chosen: Rc<RefCell<Option<FillChoice>>> = Rc::new(RefCell::new(None));
    let choices = choices.to_vec();

    let options = overlay_options(&choices, anchor);

    let app = OverlayApp {
        app_name,
        item_name,
        username,
        choices,
        chosen: chosen.clone(),
    };

    // `run_native` rather than `run_simple_native`, because the frameless
    // card needs a transparent clear color behind its rounded corners, and
    // only a full `eframe::App` impl can override `clear_color`.
    let _ = eframe::run_native(
        "Deskwarden",
        options,
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    );

    OVERLAY_OPEN.store(false, Ordering::SeqCst);

    let answer = chosen.borrow().clone();
    answer
}

/// Opens design **3a** for `app_name`: the no-match card, at `anchor`.
///
/// Returns nothing. There is no item and therefore no choice: the card's only
/// outcomes are dismissal (the ✕, Esc, or closing the window) and dismissal,
/// which is why this is not `Option<FillChoice>` -- a return type with a
/// `Some` in it would be a promise this state cannot keep.
///
/// Shares [`OVERLAY_OPEN`] with [`show_prompt_overlay`], and deliberately: the
/// guard is about how many overlay windows this process has on screen, not
/// about which kind. Two states of one card stacking on each other is the same
/// defect as two copies of one state.
///
/// **It answers one bit now**, where it used to answer nothing: whether the
/// user clicked *New login*. That is not a promise about an item -- there
/// still is none, and no id, `FillChoice` or `OverlayMatch` crosses this
/// signature. It is the destination 3a's button did not have until 3c existed.
pub fn show_no_match_overlay(app_name: &str, anchor: Option<(f32, f32)>) -> NoMatchAnswer {
    if OVERLAY_OPEN.swap(true, Ordering::SeqCst) {
        log::warn!(
            "no-match overlay requested for {app_name} while one is already open in this \
             process; ignoring rather than stacking a second window"
        );
        return NoMatchAnswer::Dismissed;
    }

    let asked = Rc::new(RefCell::new(NoMatchAnswer::Dismissed));
    let app = NoMatchApp { app_name: app_name.to_string(), asked: Rc::clone(&asked) };
    let options = no_match_options(anchor);

    let _ = eframe::run_native(
        "Deskwarden",
        options,
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    );

    OVERLAY_OPEN.store(false, Ordering::SeqCst);

    let answer = *asked.borrow();
    answer
}

/// What 3a answered: nothing, or "open 3c".
///
/// A two-variant enum rather than a `bool`, so the call site in
/// `crate::app::no_match_arm` reads as the two states it is and cannot be
/// silently inverted by a `!`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoMatchAnswer {
    /// The ✕, Esc, or the window closing. Nothing follows.
    #[default]
    Dismissed,
    /// *New login* was clicked: open design 3c for this window.
    NewLogin,
}

struct NoMatchApp {
    app_name: String,
    asked: Rc<RefCell<NoMatchAnswer>>,
}

impl eframe::App for NoMatchApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // `no_match_keyboard_action`, not an `if` here, for the reason
        // `keyboard_action` exists: this function needs an `eframe::Frame` and
        // a real always-on-top window, so no test in this crate may execute
        // it, and "Esc dismisses" is not a claim that may live only where
        // nothing can check it.
        let keys = no_match_keyboard_action(EscapePressed::read(&ctx));
        let card = draw_no_match_card(ui, &self.app_name);

        let action = if keys == OverlayAction::None { card } else { keys };
        if action == OverlayAction::NewLogin {
            // Recorded BEFORE the close, so the answer survives the window:
            // `show_no_match_overlay` reads this cell after `run_native`
            // returns, and `run_native` returns because of this command.
            *self.asked.borrow_mut() = NoMatchAnswer::NewLogin;
        }
        let done = matches!(
            action,
            OverlayAction::Dismiss | OverlayAction::Fill(_) | OverlayAction::NewLogin
        );
        if done {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// What the keyboard does to the no-match card: **Esc dismisses, and that is
/// all there is.**
///
/// Enter is not read at all, and that is the point rather than an omission.
/// On the matched card Enter fills the primary row; here there is no row and
/// no item, so an Enter that did anything would be doing it with credentials
/// that do not exist. It takes only [`EscapePressed`], so there is no second
/// argument for a swap to hide in -- the defect `keys`' newtypes exist to stop
/// is not merely prevented here, it is unrepresentable.
fn no_match_keyboard_action(escape: EscapePressed) -> OverlayAction {
    if escape.pressed() {
        return OverlayAction::Dismiss;
    }
    OverlayAction::None
}

struct OverlayApp {
    app_name: String,
    item_name: String,
    username: Option<String>,
    choices: Vec<FillChoice>,
    chosen: Rc<RefCell<Option<FillChoice>>>,
}

/// The row Enter takes: the **primary**, which is the first.
///
/// A free function rather than an expression inside `ui`, because `ui` needs a
/// real egui context and nothing in the test suite may open a window — so the
/// keyboard's half of "which choice did the user pick" would otherwise be the
/// one half no test could reach. With no choices at all the overlay is the
/// card it has always been, whose one row is the item's saved sequence.
fn primary_choice(choices: &[FillChoice]) -> FillChoice {
    choices.first().cloned().unwrap_or(FillChoice::Saved)
}

impl eframe::App for OverlayApp {
    // Transparent behind the card: without this the window would clear to
    // the theme's opaque panel fill and the rounded corners would sit in a
    // visible rectangle.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // The overlay is keyboard-first (design 2a's footer: "↵ Fill · Esc
        // Dismiss"): Enter fills, Esc dismisses, no focus juggling needed.
        // The decision itself is `keyboard_action`, because this function
        // cannot be called by a test -- it needs an `eframe::Frame` and a real
        // window -- and "Esc dismisses" is not a claim that may live only
        // where nothing can check it.
        let keys = keyboard_action(
            EnterPressed::read(&ctx),
            EscapePressed::read(&ctx),
            &self.choices,
        );

        let card = draw_overlay_card_rows(
            ui,
            &self.app_name,
            &self.item_name,
            self.username.as_deref(),
            &self.choices,
        );

        let done = match if keys == OverlayAction::None { card } else { keys } {
            OverlayAction::Fill(choice) => {
                *self.chosen.borrow_mut() = Some(choice);
                true
            }
            OverlayAction::Dismiss => true,
            // Unreachable, and by construction rather than by discipline:
            // `draw_overlay_card_rows` never answers it -- only
            // `draw_no_match_card` does, through the label
            // `draw_notice_card` is handed. Closing the card is what a
            // matched overlay would have to do with an answer it cannot
            // produce, and it is the same thing `Dismiss` does.
            OverlayAction::NewLogin => true,
            OverlayAction::None => false,
        };

        if done {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// What the keyboard did to the card this frame.
///
/// **Esc answers [`OverlayAction::Dismiss`], never a fill.** It is the one
/// control the user reaches for to say "no" to a window that appeared over the
/// app they were typing in, and an Esc that answered `Some(primary)` would
/// type their password into whatever has focus. That is a behavioural claim
/// now rather than three statements inside `OverlayApp::ui`, which no test in
/// this crate may execute.
///
/// Enter outranks Esc when a frame somehow carries both, which is the
/// behaviour this had when the two were separate `if`s.
fn keyboard_action(
    enter: EnterPressed,
    escape: EscapePressed,
    choices: &[FillChoice],
) -> OverlayAction {
    if enter.pressed() {
        return OverlayAction::Fill(primary_choice(choices));
    }
    if escape.pressed() {
        return OverlayAction::Dismiss;
    }
    OverlayAction::None
}

/// The two keyboard facts the overlay acts on, each in a type of its own —
/// and each obtainable **only by reading its own key**.
///
/// This module exists for the whole of the overlay's second critical finding:
/// `keyboard_action(enter: bool, escape: bool, ..)` could have its two
/// arguments **swapped** at the one call site that matters, it would compile,
/// and every test in the crate stayed green — because that call site lives in
/// `OverlayApp::ui`, which needs an `eframe::Frame` and a real always-on-top
/// window and can therefore never be executed here. A swapped pair means
/// **Esc fills the user's password into the app they just refused.**
///
/// Distinct types make `keyboard_action(escape, enter, ..)` a type error.
/// That alone was not enough, and the review that followed said so: with a
/// `pub` tuple field the identical bug was one level out and still compiled —
///
/// ```text
/// keyboard_action(EnterPressed(EscapePressed::read(&ctx).0), .., ..)
/// ```
///
/// — so the fields are private to this module, and, because the call site is
/// in the *parent* module, private is enough to stop it there. `breach.rs`'s
/// `Prefix`/`BaseUrl` are the model: a newtype whose field anyone may fill is
/// a door with the frame left out.
///
/// The last hole a private field alone leaves is a constructor that takes the
/// bool anyway — `EnterPressed::new(EscapePressed::read(&ctx).pressed())`.
/// So **there is no such constructor, not even a `cfg(test)` one.** In safe
/// Rust the only way to make an `EnterPressed`, in production or in a test, is
/// [`EnterPressed::read`], which reads `egui::Key::Enter` and nothing else.
/// Reading one back out is unrestricted; it is *making* one that is closed.
///
/// **The limit of that, said out loud rather than left implied.** An earlier
/// draft of this note claimed `read` was the only way to obtain either type
/// *anywhere in the crate*, and that is more than is true. A
/// `std::mem::transmute::<bool, EnterPressed>` written outside this module
/// needs no field access, calls no constructor, and contains none of the
/// strings the guard below counts. Closing it would take a crate-wide
/// `forbid(unsafe_code)`, which this crate cannot have -- 82 `unsafe` sites,
/// and they are how it talks to Win32 at all. So the guarantee is the
/// **safe-Rust** one, and
/// [`the_key_newtypes_cannot_be_built_from_a_bare_bool`] pins the safe-Rust
/// surface exactly. That is the shape the bug had: the swap this pair exists
/// to stop was a one-token slip in safe code. A `transmute` to either type
/// would be a deliberate bypass and would read as one in review.
///
/// What that leaves is `read` itself asking about the wrong key — one line,
/// inside a function that takes a bare `egui::Context`, which a test **can**
/// build without opening a window. `each_key_reader_reads_the_key_it_is_named_after`
/// covers it, negatives included, and
/// `the_key_newtypes_cannot_be_built_from_a_bare_bool` pins the shape of this
/// module so the hole cannot be reopened silently.
pub mod keys {
    use eframe::egui;

    /// Whether **Enter** was pressed this frame. See the module note.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct EnterPressed(bool);

    /// Whether **Escape** was pressed this frame. See the module note.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct EscapePressed(bool);

    impl EnterPressed {
        /// The only way to obtain one, anywhere in the crate.
        pub fn read(ctx: &egui::Context) -> Self {
            Self(ctx.input(|i| i.key_pressed(egui::Key::Enter)))
        }

        /// Whether Enter was down. Reading is unrestricted.
        pub fn pressed(self) -> bool {
            self.0
        }
    }

    impl EscapePressed {
        /// The only way to obtain one, anywhere in the crate.
        pub fn read(ctx: &egui::Context) -> Self {
            Self(ctx.input(|i| i.key_pressed(egui::Key::Escape)))
        }

        /// Whether Escape was down. Reading is unrestricted.
        pub fn pressed(self) -> bool {
            self.0
        }
    }
}

pub use keys::{EnterPressed, EscapePressed};

/// What the user did to the overlay card on this frame.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OverlayAction {
    /// Nothing yet; keep the overlay up.
    #[default]
    None,
    /// Fill — carrying **which** row was clicked, because "a fill happened"
    /// and "this is what to type" are different facts and the caller needs
    /// the second one.
    Fill(FillChoice),
    /// Close without filling (the header's ✕ was clicked).
    Dismiss,
    /// The 3a card's *New login* button was clicked: close this card and open
    /// design **3c**, the save-a-new-login form.
    ///
    /// **Only [`draw_no_match_card`] can answer this**, and that is a property
    /// of the two cards rather than of discipline: [`draw_notice_card`] paints
    /// the button only when it is handed a label, and [`draw_locked_card`]
    /// hands it `None`. See [`NEW_LOGIN_LABEL`] for why a locked vault must
    /// not offer it.
    NewLogin,
}

/// Draws the overlay card itself — header (mark, wordmark, match count,
/// dismiss ✕), the matched credential row, and the keyboard-hint footer.
///
/// Public (rather than folded into `OverlayApp::update`) so the
/// `ui_preview` example can render the exact card the app ships, not a
/// re-implementation that could drift from it.
pub fn draw_overlay_card(
    ui: &mut egui::Ui,
    app_name: &str,
    item_name: &str,
    username: Option<&str>,
) -> OverlayAction {
    draw_overlay_card_rows(ui, app_name, item_name, username, &[])
}

/// [`draw_overlay_card`] with an explicit list of choice rows.
///
/// An empty `choices` paints the single matched-credential row the overlay has
/// always painted — which is what `draw_overlay_card` (and therefore the whole
/// of production, until step 5 wires a choice list through) asks for. A
/// non-empty `choices` paints one row per choice, labelled by
/// [`FillChoice::label`].
///
/// The card is sized for `overlay_height(choices.len())`; because
/// `overlay_height` floors at one row, the empty case and the one-choice case
/// are the same height, and that height is the overlay's historical 164.0.
///
/// This is a separate function rather than an extra parameter on
/// `draw_overlay_card` only because `draw_overlay_card`'s signature is what
/// the `ui_preview` example calls, and this step owns exactly one file.
pub fn draw_overlay_card_rows(
    ui: &mut egui::Ui,
    app_name: &str,
    item_name: &str,
    username: Option<&str>,
    choices: &[FillChoice],
) -> OverlayAction {
    let mut action = OverlayAction::None;

    let card = egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0, theme::BORDER_STRONG))
        .shadow(egui::epaint::Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: egui::Color32::from_black_alpha(36),
        })
        .outer_margin(Margin {
            left: 4,
            right: 12,
            top: 2,
            bottom: 20,
        });

    card.show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;

        // Header: mark, wordmark, match count, and the dismiss ✕. The ✕ is
        // the only mouse-operable way out of a `with_decorations(false)`
        // window — there is no title bar to close, and the footer's "Esc
        // Dismiss" is a label, not a control. It matters more than it looks:
        // this window is raised in response to *another* app being
        // foregrounded, which is exactly the situation Windows' foreground
        // lock refuses keyboard focus for, so Esc is not guaranteed to reach
        // us at all.
        egui::Frame::new()
            .inner_margin(Margin::symmetric(12, 9))
            .show(ui, |ui| {
                if theme::card_header_with_close(ui, &match_count_label(choices.len().max(1))) {
                    action = OverlayAction::Dismiss;
                }
            });
        theme::hairline(ui);

        // The choice rows. With no choices this is the single matched
        // credential row, in the selected treatment, exactly as before.
        egui::Frame::new()
            .inner_margin(Margin::same(6))
            .show(ui, |ui| {
                let (primary, secondary) = row_text(app_name, item_name, username);
                if choices.is_empty() {
                    if credential_row(ui, &primary, &primary, &secondary, true) {
                        action = OverlayAction::Fill(FillChoice::Saved);
                    }
                } else {
                    for (index, choice) in choices.iter().enumerate() {
                        // Between rows only: with one row the card is byte-
                        // for-byte the geometry it has always had, which is
                        // what makes `overlay_height(1) == 164.0` true of the
                        // drawing and not just of the arithmetic.
                        if index > 0 {
                            ui.add_space(ROW_GAP);
                        }
                        // The avatar keeps showing WHO is being filled, not
                        // what is being typed -- the label already says that,
                        // and initials of "Username + Tab + Password" would
                        // name nothing.
                        // `choice.clone()`, not `choices[0]` and not the
                        // index: the row that was clicked is the row that
                        // answers, or four rows are four ways to do one thing.
                        if credential_row(ui, &primary, &choice.label(), &secondary, index == 0) {
                            action = OverlayAction::Fill(choice.clone());
                        }
                    }
                }
            });

        // Footer: keyboard hints on the tinted strip.
        theme::hairline(ui);
        egui::Frame::new()
            .fill(theme::CARD_TINT)
            .corner_radius(CornerRadius {
                sw: 9,
                se: 9,
                ..CornerRadius::ZERO
            })
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                theme::footer_hints(ui, &[("Enter", "Fill"), ("Esc", "Dismiss")]);
            });
    });

    action
}

/// The number of choice rows whose [`overlay_height`] the no-match card is
/// sized by.
///
/// **A constant, and one that is checked against the card rather than chosen
/// for it.** The overlay is a frameless, always-on-top window with a hardcoded
/// inner size, no resize border, no title bar to drag and **no `ScrollArea`
/// anywhere**: a control past its bottom edge is not merely awkward, it is
/// unreachable. `f67bf42`'s message records three separate occasions on which
/// a text or layout change pushed a control out of this viewport, which is why
/// this card gets a measurement and not a guess.
///
/// It is `1` because the no-match card's body is the same shape as one
/// credential row -- two truncated text lines -- and
/// [`the_no_match_card_fits_the_window_it_asks_for`] both measures the real
/// card against `overlay_height(NO_MATCH_ROWS)` and asserts it does NOT fit
/// the next size down. A one-sided bound would let the body silently shrink to
/// nothing; a two-sided one would not.
pub const NO_MATCH_ROWS: usize = 1;

/// The window the no-match card asks the OS for.
///
/// Separate from [`overlay_options`] rather than sharing its choice-list
/// argument, because there is no choice list: the size comes from
/// [`NO_MATCH_ROWS`]. It is public for the same reason `overlay_options` is --
/// `show_no_match_overlay` calls `eframe::run_native` and no test here may
/// execute it, so the size it asks for would otherwise be the one number in
/// this card nothing could observe.
pub fn no_match_options(anchor: Option<(f32, f32)>) -> eframe::NativeOptions {
    options_for_rows(NO_MATCH_ROWS, anchor)
}

/// The two lines of the no-match card, as [`row_text`] is for the matched one.
///
/// Both lines are constants plus **one** user-controlled string, `app_name`,
/// which is `app::window_label`'s answer -- an executable name or a window
/// title, either of which a user (or the app they ran) chooses. The card's
/// height must not depend on it; see [`draw_no_match_card`].
fn no_match_text(app_name: &str) -> (String, String) {
    (
        format!("No saved login for {app_name}"),
        "Open Deskwarden to search the vault.".to_string(),
    )
}

/// Design **3a**: the card for a window that asks for a password and that
/// nothing in the vault matches.
///
/// The state this exists for used to be indistinguishable from a broken app:
/// focusing an unrecognised login window did nothing whatsoever, and a user
/// cannot tell "Deskwarden has nothing for this" from "Deskwarden is not
/// running". This says the first one.
///
/// **One of 3a's two drawn buttons is here and the other still is not.** 3a as
/// designed offers *Search vault* and *New login*. *New login* is drawn now,
/// because design 3c exists and is its destination -- see [`NEW_LOGIN_LABEL`]
/// and [`OverlayAction::NewLogin`]. *Search vault* is still not drawn, and for
/// the reason it never was: it would have to reach the vault window from
/// inside `main::process_foreground_event`, which holds none of the machinery
/// that opens one. A button on a frameless, always-on-top card that does
/// nothing when clicked is worse than no button -- it is the same "is this
/// thing working?" the card exists to answer, moved one click later. The
/// guidance for it stays in the body text, which now says only that: the
/// second line no longer offers to "add one", because the button does.
///
/// **No avatar, and two truncated lines.** The body is the same shape as one
/// [`credential_row`] minus the initials tile, and with the footer's button it
/// is [`overlay_height`]`(`[`NO_MATCH_ROWS`]`)` tall with five points to
/// spare -- six fewer than before the button, which is what `NO_MATCH_SLACK`
/// records. Both lines truncate inside
/// a text column of explicit width for exactly the reason
/// [`credential_row`]'s do: `app_name` is user-controlled, wrapping is what
/// made a one-row card 189pt tall in a 164pt window, and this window still
/// cannot scroll.
///
/// Public so the `ui_preview` example renders the card the app ships rather
/// than a re-implementation that could drift from it.
///
/// Answers [`OverlayAction::Dismiss`] when the header's ✕ is clicked, and
/// [`OverlayAction::None`] otherwise. It never answers `Fill`: there is no
/// item to fill from, so the variant is unreachable here by construction
/// rather than by discipline.
pub fn draw_no_match_card(ui: &mut egui::Ui, app_name: &str) -> OverlayAction {
    let (primary, secondary) = no_match_text(app_name);
    draw_notice_card(ui, NO_MATCH_LABEL, &primary, &secondary, Some(NEW_LOGIN_LABEL))
}

/// Design **3b**: the card for a window that asks for a password while the
/// vault is **locked**.
///
/// **This is a correction, not an addition.** Until it existed a locked vault
/// reached [`draw_no_match_card`], and the user was told "No saved login for
/// <app>" -- a statement about the contents of a vault this process cannot
/// read. `main`'s `stand_down_after_unlock` empties the match engine on every
/// lock, and says so in its own log line: "the app matches are cleared too, so
/// nothing can prompt to autofill until they are rebuilt". So while locked
/// *every* window is unmatched, including every window that does have a saved
/// login, and 3a asserted the opposite of the truth about each of them. A
/// surface whose entire purpose is to be believed about the vault was being
/// shown in the one state where it could not be.
///
/// **It claims nothing about whether a match exists**, and that is where it
/// departs from 3b as drawn. The drawing counts them ("3 logins for Ledgerline
/// Desktop"); this build cannot count them, because the engine that would is
/// exactly what the lock cleared -- and a number here would be the same lie in
/// the other direction. Nor does it offer Windows Hello or a PIN: neither
/// exists in this app, and a card offering an unlock it cannot perform is
/// worse than the silence it replaces.
///
/// What is left is the one thing that is both true and useful: Deskwarden is
/// locked, it therefore cannot answer for this app, and unlocking is what
/// changes that.
///
/// Same shape, same window and same height as [`draw_no_match_card`] (see
/// [`LOCKED_ROWS`]), because it is the same two-line body: they share
/// [`draw_notice_card`] rather than each spelling the card out.
///
/// Public so the `ui_preview` example renders the card the app ships.
pub fn draw_locked_card(ui: &mut egui::Ui, app_name: &str) -> OverlayAction {
    let (primary, secondary) = locked_text(app_name);
    draw_notice_card(ui, LOCKED_LABEL, &primary, &secondary, None)
}

/// The card [`draw_no_match_card`] and [`draw_locked_card`] share: a header
/// with a ✕, a two-line body, and an `Esc Dismiss` footer.
///
/// **One function rather than two copies**, because the two states differ only
/// in three strings -- and because the height evidence is a property of *this*
/// layout. Two copies would let one of them drift a point taller than the
/// window they both ask the OS for, in a viewport that cannot scroll; shared,
/// `the_no_match_card_fits_the_window_it_asks_for` and
/// `the_locked_card_fits_the_window_it_asks_for` measure the same code twice.
///
/// `primary` and `secondary` are borrowed and both labels `.truncate()`, for
/// the reason [`credential_row`]'s do: each carries one user-controlled string
/// (`app::window_label`'s answer), wrapping is what made a one-row card 189pt
/// tall in a 164pt window, and this window still cannot scroll.
/// **The two cards are no longer the same height, and `action` is why.** 3a's
/// footer carries a button and 3b's does not, so [`NO_MATCH_ROWS`] and
/// [`LOCKED_ROWS`] are separately measured against separately sized cards --
/// which is exactly the reason those two constants were written as separate
/// constants rather than as an alias in the first place.
fn draw_notice_card(
    ui: &mut egui::Ui,
    label: &str,
    primary: &str,
    secondary: &str,
    action: Option<&str>,
) -> OverlayAction {
    let mut action_taken = OverlayAction::None;
    let mut clicked = OverlayAction::None;

    let card = egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0, theme::BORDER_STRONG))
        .shadow(egui::epaint::Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: egui::Color32::from_black_alpha(36),
        })
        .outer_margin(Margin {
            left: 4,
            right: 12,
            top: 2,
            bottom: 20,
        });

    card.show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;

        // The same header as every other overlay state, with the count
        // replaced by what there is to count. The ✕ matters more here than
        // anywhere: this window is raised in response to ANOTHER app being
        // foregrounded, which is exactly the case Windows' foreground lock
        // refuses keyboard focus for -- so Esc may never reach us, and the ✕
        // is the only mouse-operable way out of a window with no title bar.
        egui::Frame::new()
            .inner_margin(Margin::symmetric(12, 9))
            .show(ui, |ui| {
                if theme::card_header_with_close(ui, label) {
                    action_taken = OverlayAction::Dismiss;
                }
            });
        theme::hairline(ui);

        egui::Frame::new()
            .inner_margin(Margin::same(6))
            .show(ui, |ui| {
                egui::Frame::new()
                    .fill(theme::CANVAS)
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::symmetric(10, 9))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        // The text column's width is explicit and both labels
                        // truncate. With no avatar and no Enter chip there is
                        // no lane to keep clear, so the column is the whole
                        // row -- but it is still BOUNDED, which is the half
                        // that stops `app_name` from growing the card.
                        let text_width = ui.available_width().max(1.0);
                        ui.vertical(|ui| {
                            ui.set_width(text_width);
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.add(
                                egui::Label::new(
                                    theme::semibold(primary, 13.0).color(theme::INK),
                                )
                                .truncate(),
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(secondary).size(11.0).color(theme::TEXT_FAINT),
                                )
                                .truncate(),
                            );
                        });
                    });
            });

        theme::hairline(ui);
        egui::Frame::new()
            .fill(theme::CARD_TINT)
            .corner_radius(CornerRadius {
                sw: 9,
                se: 9,
                ..CornerRadius::ZERO
            })
            // The strip is shorter when it carries the button, because the
            // button brings its own 4pt of vertical padding: matching the
            // hint-only strip's 8 would have made the 3a card 167pt in a 164pt
            // window, which on this surface is the Esc hint gone.
            .inner_margin(Margin::symmetric(12, if action.is_some() { 6 } else { 8 }))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                match action {
                    // 3a. The button is the whole of what changed here: until
                    // it had a destination it was deliberately not drawn (see
                    // `draw_no_match_card`), and a control on a frameless,
                    // always-on-top card that does nothing when clicked is
                    // worse than no control. It has one now.
                    Some(label) => {
                        ui.horizontal(|ui| {
                            if theme::row_button(ui, label).clicked() {
                                clicked = OverlayAction::NewLogin;
                            }
                            ui.add_space(8.0);
                            // Still one hint, not two: there is nothing for
                            // Enter to fill.
                            theme::footer_hints(ui, &[("Esc", "Dismiss")]);
                        });
                    }
                    // One hint, not two: there is nothing for Enter to fill, and a
                    // footer that offered `Enter Fill` on a card with nothing to
                    // fill would be the card contradicting itself.
                    None => theme::footer_hints(ui, &[("Esc", "Dismiss")]),
                }
            });
    });

    if clicked != OverlayAction::None {
        return clicked;
    }
    action_taken
}

/// What the no-match card's header says where the matched card counts matches.
///
/// A constant rather than a literal inside [`draw_no_match_card`] so that the
/// card's one claim -- there is nothing here -- is a string a test can name
/// and find in the painted output, rather than one it has to re-spell.
pub const NO_MATCH_LABEL: &str = "No match";

/// What 3a's one button says, and **the only card that may carry it**.
///
/// [`draw_locked_card`] hands [`draw_notice_card`] `None` here, deliberately:
/// design 3c ends in `VaultCache::create_item`, which is a write through
/// `bw serve` against an unlocked vault, so a *New login* button on the locked
/// card would be an offer the process cannot honour -- the same class of
/// defect as the locked card's own correction (a card claiming something about
/// a vault it cannot read). `the_locked_card_offers_no_new_login_button` reads
/// the painted glyphs of both cards rather than trusting the argument.
///
/// A constant for the reason [`NO_MATCH_LABEL`] is one: it is the string a
/// test finds in the painted output rather than one it re-spells.
pub const NEW_LOGIN_LABEL: &str = "New login";

/// What the locked card's header says.
///
/// A constant for the same reason [`NO_MATCH_LABEL`] is one, and **it must not
/// be that string**: the two cards are drawn by the same
/// [`draw_notice_card`], so the header is the only thing in the painted output
/// that tells them apart, and
/// `the_locked_card_says_nothing_about_whether_a_match_exists` reads it.
pub const LOCKED_LABEL: &str = "Vault locked";

/// The number of choice rows whose [`overlay_height`] the locked card is sized
/// by.
///
/// The same `1` as [`NO_MATCH_ROWS`] and **not an alias of it**. They are equal
/// today because the two cards share [`draw_notice_card`] and so lay out
/// identically; they are separate constants because the card each one sizes is
/// separately measured (`the_locked_card_fits_the_window_it_asks_for`), and a
/// `pub use` would make one card's growth invisible in the other's window.
/// `the_two_notice_cards_are_the_same_height` asserts the equality it is safe
/// to rely on, from the cards rather than from the constants.
pub const LOCKED_ROWS: usize = 1;

/// The window the locked card asks the OS for. [`no_match_options`]'s sibling,
/// public for the same reason: `show_locked_overlay` calls
/// `eframe::run_native`, which no test here may execute.
pub fn locked_options(anchor: Option<(f32, f32)>) -> eframe::NativeOptions {
    options_for_rows(LOCKED_ROWS, anchor)
}

/// The two lines of the locked card, as [`no_match_text`] is for 3a.
///
/// **Neither line says whether the vault has a login for `app_name`**, and
/// that is the whole correction: the process cannot know while locked, so it
/// says what it can know instead -- that it is locked, and that unlocking is
/// what answers the question. `the_locked_card_claims_nothing_about_a_match`
/// holds the two strings to that.
///
/// `app_name` is `app::window_label`'s answer, so it is user-controlled and
/// the card's height must not depend on it; see [`draw_notice_card`].
fn locked_text(app_name: &str) -> (String, String) {
    (
        "Deskwarden is locked".to_string(),
        format!("Unlock it to see whether the vault has a login for {app_name}."),
    )
}

/// Opens design **3b** for `app_name`: the locked card, at `anchor`.
///
/// [`show_no_match_overlay`]'s sibling, down to sharing [`OVERLAY_OPEN`] --
/// the guard is about how many overlay windows this process has on screen, not
/// about which kind, and 3a stacked on 3b is the same defect as two copies of
/// either.
///
/// Returns nothing, for the same reason 3a's does: there is no item, so there
/// is no choice, and an `Option<FillChoice>` here would be a promise this
/// state cannot keep.
pub fn show_locked_overlay(app_name: &str, anchor: Option<(f32, f32)>) {
    if OVERLAY_OPEN.swap(true, Ordering::SeqCst) {
        log::warn!(
            "locked overlay requested for {app_name} while one is already open in this \
             process; ignoring rather than stacking a second window"
        );
        return;
    }

    let app = LockedApp { app_name: app_name.to_string() };
    let options = locked_options(anchor);

    let _ = eframe::run_native(
        "Deskwarden",
        options,
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    );

    OVERLAY_OPEN.store(false, Ordering::SeqCst);
}

struct LockedApp {
    app_name: String,
}

impl eframe::App for LockedApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // `no_match_keyboard_action`, shared rather than copied: this card has
        // the same keyboard as 3a -- Esc dismisses, Enter is not read, because
        // there is nothing here for Enter to fill either. Both bodies are
        // unreachable from a test (they need a real always-on-top window), so
        // sharing the one decision function is what keeps "Esc dismisses"
        // checked for both.
        let keys = no_match_keyboard_action(EscapePressed::read(&ctx));
        let card = draw_locked_card(ui, &self.app_name);

        let done = matches!(
            if keys == OverlayAction::None { card } else { keys },
            OverlayAction::Dismiss | OverlayAction::Fill(_)
        );
        if done {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

// ---------------------------------------------------------------------------
// Design 3c: save a new login.
// ---------------------------------------------------------------------------

/// The number of choice rows whose [`overlay_height`] the save-a-new-login
/// card is sized by.
///
/// **By far the tallest state the overlay has**, and the one with the most to
/// lose: four rows and three controls, in a frameless, always-on-top window
/// with a hardcoded inner size, no title bar, no resize border and **no
/// `ScrollArea` anywhere**. A control past the bottom edge here is not the
/// dismiss *hint* 3a would lose -- it is *Save*, or the password field itself.
///
/// Measured and bounded on both sides by
/// [`the_save_login_card_fits_the_window_it_asks_for`], with the four
/// adversarial app-name fixtures: the card must fit
/// `overlay_height(SAVE_LOGIN_ROWS)` and must NOT fit one [`ROW_HEIGHT`] less,
/// and the slack between them is pinned exactly. A one-sided bound cannot tell
/// deliberate dead space from a row that stopped being drawn.
///
/// **It is `3`, and the card has four rows.** The two numbers are not the same
/// number and never were: this one is the argument to [`overlay_height`], i.e.
/// how many *choice-row pitches* of window the card needs on top of the
/// chrome, and 3c's rows are shorter than a choice row (30pt against 50pt)
/// while its header, hairlines and footer are the same. The card measures
/// 254pt; `overlay_height(3)` is 264 and `overlay_height(2)` is 214. Writing
/// `4` here because the card draws four rows would ask the OS for a 314pt
/// window and leave 60 points of empty card under the Save button.
pub const SAVE_LOGIN_ROWS: usize = 3;

/// The window the save-a-new-login card asks the OS for.
///
/// [`no_match_options`]'s sibling, public for the same reason:
/// [`show_save_login_overlay`] calls `eframe::run_native` and no test here may
/// execute it, so the size it asks for would otherwise be the one number in
/// this card nothing could observe.
pub fn save_login_options(anchor: Option<(f32, f32)>) -> eframe::NativeOptions {
    options_for_rows(SAVE_LOGIN_ROWS, anchor)
}

/// What the user did to the 3c card. **Three answers, and two of them are
/// silence for different lengths of time.**
///
/// Deliberately *not* a `bool` and deliberately not `Option<...>`:
/// [`Self::NotNow`] is silence today and [`Self::Never`] is silence forever,
/// and conflating them is the one bug on this card a user cannot undo without
/// finding a setting. `crate::app::route_save_answer` is where that
/// distinction is turned into two different effects, as a pure function.
///
/// **No secret reaches this type**, which is why it may derive `Debug`: the
/// password lives in [`SaveLoginForm`] and is moved out of it by
/// [`SaveLoginForm::into_answer`] into `crate::app::NewLogin`, never through
/// here. `debug_leak_guard` is the test that holds that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SaveLoginAction {
    /// Nothing yet; keep the card up.
    #[default]
    None,
    /// *Save*: create the login from what is in the form.
    Save,
    /// *Not now*, the header ✕, or Esc: do nothing, and ask again next time.
    NotNow,
    /// *Never for this app*: do nothing, **and do not ask about this app
    /// again**.
    Never,
}

/// The 3c card's editable state, and the only place the typed password lives.
///
/// # One row is pre-filled, and the card says so
///
/// The design draws all four rows pre-filled. This build fills exactly one of
/// them -- [`Self::app_name`] -- and that is not a shortfall to be papered
/// over, it is the honest limit of what can be known:
/// `crate::injector::ui_automation` exposes exactly one question about a
/// foreground window, `window_has_password_field`, which answers a `bool`.
/// There is no username reader; and **a password field's contents are not read
/// and must not be**, which is the whole reason this struct owns a
/// [`zeroize::Zeroizing`] buffer the *user* types into rather than a value
/// captured off the screen.
///
/// So the card is worded as the form it is -- *"Save a login for &lt;app&gt;"*
/// with empty fields to type into -- and not as a confirmation of a capture
/// that did not happen. A card that said "Save this login?" over blank fields
/// would be claiming to have seen the credential it is asking the user to
/// enter.
///
/// # Debug
///
/// Hand-written, because [`Self::password`] is a `Zeroizing<String>` and
/// `Zeroizing` **derives** `Debug` and prints the inner value -- it is not a
/// redacting wrapper. `debug_leak_guard` refuses a derived `Debug` on any type
/// that can reach one, and this is that type.
#[derive(Clone)]
pub struct SaveLoginForm {
    /// The app the window belongs to: `crate::app::window_label`'s answer.
    /// **The one pre-filled row**, and the item's name -- so the item the user
    /// gets is named after the thing they were signing in to.
    pub app_name: String,
    /// What the user typed in the Username row. Empty is allowed:
    /// `vault_bridge` omits a blank username rather than POSTing `""`.
    pub username: String,
    /// What the user typed in the Password row. `Zeroizing`, so it is wiped
    /// when the form is dropped.
    ///
    /// **Typed here, never captured.** See the struct's own doc.
    pub password: zeroize::Zeroizing<String>,
}

impl std::fmt::Debug for SaveLoginForm {
    /// Names the fields and prints the password as `<redacted>` -- the barrier
    /// `debug_leak_guard` propagates through, so a type that holds a
    /// `SaveLoginForm` is not itself flagged.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SaveLoginForm")
            .field("app_name", &self.app_name)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl SaveLoginForm {
    /// A blank form for `app_name`.
    pub fn new(app_name: &str) -> Self {
        Self {
            app_name: app_name.to_string(),
            username: String::new(),
            password: zeroize::Zeroizing::new(String::new()),
        }
    }
}

/// What the Folder row says, and **it is a statement rather than a picker.**
///
/// 3c as drawn offers a folder dropdown. This card does not, and the reason is
/// the same one every other decision in this module answers to: an
/// `egui::ComboBox` popup is painted inside this viewport, and this viewport is
/// a frameless, always-on-top window of a hardcoded height with nothing to
/// scroll. A folder list of any length would open into -- and past -- the
/// bottom edge, which is precisely the unreachable-control failure
/// [`SAVE_LOGIN_ROWS`] exists to prevent, except that a clipped *popup* is
/// invisible to a height measurement of the card.
///
/// So the row states where the item will go, truthfully and without a control:
/// the new login is created unfiled (`NewItem::login(.., None)`), and the vault
/// window's edit form -- which has a scrollable pane and the whole folder list
/// -- is where it is filed. `the_folder_row_states_where_the_item_goes` holds
/// this string against the `None` that
/// `crate::app::new_login_item` really passes.
pub const FOLDER_ROW_TEXT: &str = "No folder · file it in the vault window";

/// The Username row's placeholder, and **the load-bearing half of the card's
/// honesty about what it did and did not read.**
///
/// It is phrased as an instruction to the user (*"type ..."*), not as a
/// description of a value, because the field really is empty and really does
/// have to be typed: there is no username reader in
/// `crate::injector::ui_automation`.
pub const USERNAME_HINT: &str = "type the username you used";

/// The Password row's placeholder, and the sharper case of [`USERNAME_HINT`].
///
/// **A password field's contents are not read by this app and must not be.**
/// The design draws this row pre-filled with bullets and a *Reveal* link, and
/// this build does not, because there is nothing to reveal -- the value is
/// whatever the user types into this box, which is why
/// [`SaveLoginForm::password`] is a `Zeroizing` buffer rather than a captured
/// string. `the_card_does_not_imply_a_capture_it_did_not_make` reads both
/// hints out of the painted card.
pub const PASSWORD_HINT: &str = "type the password you used";

/// Design **3c**: the card that offers to save a login for a window the vault
/// has nothing for.
///
/// Four rows -- App, Username, Password, Folder -- and three controls: *Save*,
/// *Not now*, *Never for this app*. Reached from [`draw_no_match_card`]'s
/// *New login* button, which is the destination that button did not have when
/// 3a shipped.
///
/// **The App and Folder rows are inert and the other two are fields**, which
/// is the shape of what this process actually knows; see [`SaveLoginForm`] for
/// the App row and [`FOLDER_ROW_TEXT`] for the Folder row.
///
/// **The password row is a real password field** (`TextEdit::password(true)`)
/// over a [`zeroize::Zeroizing`] buffer. It is masked for the same reason
/// every other password box in this app is: this window opens over whatever
/// the user was doing, in front of whoever is behind them.
///
/// Every label truncates, for the reason [`draw_notice_card`]'s do: `app_name`
/// is user-controlled, wrapping is what made a one-row card 189pt tall in a
/// 164pt window, and this window still cannot scroll.
///
/// Public so the `ui_preview` example renders the card the app ships rather
/// than a re-implementation that could drift from it.
pub fn draw_save_login_card(ui: &mut egui::Ui, form: &mut SaveLoginForm) -> SaveLoginAction {
    let mut action = SaveLoginAction::None;

    let card = egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0, theme::BORDER_STRONG))
        .shadow(egui::epaint::Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: egui::Color32::from_black_alpha(36),
        })
        .outer_margin(Margin {
            left: 4,
            right: 12,
            top: 2,
            bottom: 20,
        });

    card.show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;

        egui::Frame::new()
            .inner_margin(Margin::symmetric(12, 7))
            .show(ui, |ui| {
                // The ✕ is `NotNow`, not `Never`: closing a card is the
                // weakest answer a user can give it, and reading it as
                // "forever" is the bug this card's three answers exist to
                // avoid.
                if theme::card_header_with_close(ui, SAVE_LOGIN_LABEL) {
                    action = SaveLoginAction::NotNow;
                }
            });
        theme::hairline(ui);

        egui::Frame::new()
            .inner_margin(Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = ROW_FIELD_GAP;

                save_login_row(ui, "App", |ui| {
                    ui.add(
                        egui::Label::new(
                            theme::semibold(&form.app_name, 12.0).color(theme::INK),
                        )
                        .truncate(),
                    );
                });
                save_login_row(ui, "Username", |ui| {
                    save_login_field(ui, &mut form.username, false, USERNAME_HINT);
                });
                save_login_row(ui, "Password", |ui| {
                    // `&mut form.password` deref-coerces to the `Zeroizing`'s
                    // OWN buffer, typed into in place: there is no second
                    // `String` for the value to be copied into and left in.
                    save_login_field(ui, &mut form.password, true, PASSWORD_HINT);
                });
                save_login_row(ui, "Folder", |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(FOLDER_ROW_TEXT).size(11.0).color(theme::TEXT_FAINT),
                        )
                        .truncate(),
                    );
                });
            });

        theme::hairline(ui);
        egui::Frame::new()
            .fill(theme::CARD_TINT)
            .corner_radius(CornerRadius {
                sw: 9,
                se: 9,
                ..CornerRadius::ZERO
            })
            .inner_margin(Margin::symmetric(12, 7))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    if theme::primary_button(ui, SAVE_LABEL, Some("Enter")).clicked() {
                        action = SaveLoginAction::Save;
                    }
                    ui.add_space(6.0);
                    if theme::secondary_button(ui, NOT_NOW_LABEL).clicked() {
                        action = SaveLoginAction::NotNow;
                    }
                    ui.add_space(6.0);
                    // A link, not a third button: it is the destructive-ish
                    // answer of the three (it is the only one that persists
                    // anything), and the design gives it the least weight.
                    if theme::link_label(ui, NEVER_LABEL, 11.0).clicked() {
                        action = SaveLoginAction::Never;
                    }
                });
            });
    });

    action
}

/// The vertical gap between two of 3c's four rows.
///
/// Its own constant rather than a literal, because it is multiplied by three
/// in the card's height and so is one of the numbers
/// `the_save_login_card_fits_the_window_it_asks_for` is really measuring.
const ROW_FIELD_GAP: f32 = 6.0;

/// The width of 3c's label column -- the design's `width: 80px`.
const LABEL_COLUMN: f32 = 74.0;

/// One of 3c's four rows: a fixed-width caption on the left and `body` filling
/// what is left.
///
/// The caption column is a **fixed** width rather than a sized-to-content one,
/// which is what keeps the four rows' bodies aligned with each other and, more
/// importantly, keeps the row's height independent of its caption.
fn save_login_row(ui: &mut egui::Ui, caption: &str, body: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.set_height(SAVE_ROW_HEIGHT);
        ui.scope(|ui| {
            ui.set_width(LABEL_COLUMN);
            ui.add(
                egui::Label::new(RichText::new(caption).size(11.0).color(theme::TEXT_FAINT))
                    .truncate(),
            );
        });
        body(ui);
    });
}

/// The height of one of 3c's four rows -- the design's `height: 32px` input
/// box, which is what sets the row's pitch whether or not the row holds one.
///
/// **Every row is this tall, including the two that hold no field.** That is
/// what makes the card's height a function of four rows rather than of what
/// happens to be in them, and it is why an `app_name` of any length cannot
/// grow it.
const SAVE_ROW_HEIGHT: f32 = 30.0;

/// 3c's text box: [`SAVE_ROW_HEIGHT`] tall, filling the row, masked when
/// `password`.
///
/// **A single line, always.** `TextEdit::singleline` cannot wrap, so a long
/// value scrolls horizontally inside the box instead of growing the card --
/// the same bound `draw_notice_card`'s `.truncate()` puts on its labels, and
/// for the same reason.
fn save_login_field(ui: &mut egui::Ui, value: &mut String, password: bool, hint: &str) {
    let width = ui.available_width();
    ui.add_sized(
        [width, SAVE_ROW_HEIGHT],
        egui::TextEdit::singleline(value)
            .password(password)
            .hint_text(RichText::new(hint).size(11.0).color(theme::TEXT_FAINT))
            .font(egui::FontId::proportional(12.0))
            .margin(Margin::symmetric(8, 5))
            .background_color(theme::CARD)
            .desired_width(width),
    );
}

/// What 3c's header says where the matched card counts matches.
pub const SAVE_LOGIN_LABEL: &str = "Save a login";

/// 3c's primary button.
pub const SAVE_LABEL: &str = "Save";

/// 3c's *silence today* answer.
pub const NOT_NOW_LABEL: &str = "Not now";

/// 3c's *silence forever* answer.
///
/// It names the app-scoped thing it does, rather than saying only "Never":
/// this is the one control on the card that writes to `settings.json`, and the
/// user has to be able to tell from the words which of the two silences they
/// are choosing. `the_two_silences_read_differently_on_the_card` holds the two
/// strings apart.
pub const NEVER_LABEL: &str = "Never for this app";

/// What the keyboard does to the 3c card.
///
/// **Esc is `NotNow`, and Enter is `Save`.** Esc must be the weakest of the
/// three answers for the reason the header ✕ is: a user swatting a card away
/// has not asked for a persistent setting, and "Never" is not undoable from
/// this surface. Nothing on this card is bound to a key that silences an app
/// forever.
///
/// Enter is read here, unlike on 3a and 3b, because here there **is** a
/// primary action -- and it is the one the design puts the `↵` chip on.
pub fn save_login_keyboard_action(
    escape: EscapePressed,
    enter: EnterPressed,
) -> SaveLoginAction {
    if escape.pressed() {
        SaveLoginAction::NotNow
    } else if enter.pressed() {
        SaveLoginAction::Save
    } else {
        SaveLoginAction::None
    }
}

/// Opens design **3c** for `app_name` at `anchor`, and answers what the user
/// decided together with what they typed.
///
/// `None` is not an answer -- it is "this process already has an overlay on
/// screen", the [`OVERLAY_OPEN`] refusal every other `show_*` here makes.
/// A user who dismisses the card answers [`SaveLoginAction::NotNow`], which is
/// a decision and is spelled as one.
///
/// The password comes back inside a [`zeroize::Zeroizing`] so that the one
/// copy that crosses this boundary is wiped when the caller drops it.
pub fn show_save_login_overlay(
    app_name: &str,
    anchor: Option<(f32, f32)>,
) -> Option<(SaveLoginAction, SaveLoginForm)> {
    if OVERLAY_OPEN.swap(true, Ordering::SeqCst) {
        log::warn!(
            "save-login overlay requested for {app_name} while one is already open in this \
             process; ignoring rather than stacking a second window"
        );
        return None;
    }

    let answered = Rc::new(RefCell::new((SaveLoginAction::NotNow, SaveLoginForm::new(app_name))));
    let app = SaveLoginApp { form: SaveLoginForm::new(app_name), answered: Rc::clone(&answered) };
    let options = save_login_options(anchor);

    let _ = eframe::run_native(
        "Deskwarden",
        options,
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    );

    OVERLAY_OPEN.store(false, Ordering::SeqCst);

    let answer = answered.borrow().clone();
    Some(answer)
}

struct SaveLoginApp {
    form: SaveLoginForm,
    answered: Rc<RefCell<(SaveLoginAction, SaveLoginForm)>>,
}

impl eframe::App for SaveLoginApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // The keyboard's decision is a function call, not an `if` here, for
        // the reason `keyboard_action` exists: this body needs a real
        // always-on-top window, so no test in this crate may execute it, and
        // "Esc is Not now, not Never" is not a claim that may live only where
        // nothing can check it.
        let keys = save_login_keyboard_action(
            EscapePressed::read(&ctx),
            EnterPressed::read(&ctx),
        );
        let card = draw_save_login_card(ui, &mut self.form);

        let action = if keys == SaveLoginAction::None { card } else { keys };
        if action != SaveLoginAction::None {
            *self.answered.borrow_mut() = (action, self.form.clone());
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// The two lines of the credential row: the recognizable identity on top
/// (username when known, item name otherwise) and context underneath.
fn row_text(app_name: &str, item_name: &str, username: Option<&str>) -> (String, String) {
    match (username, item_name.is_empty()) {
        (Some(u), false) => (u.to_string(), format!("{item_name} · fills {app_name}")),
        (Some(u), true) => (u.to_string(), format!("fills {app_name}")),
        (None, false) => (item_name.to_string(), format!("fills {app_name}")),
        (None, true) => ("Saved credentials".to_string(), format!("fills {app_name}")),
    }
}

/// Gap between two adjacent choice rows. Folded into [`ROW_HEIGHT`] rather
/// than into [`CHROME_HEIGHT`], because there are `n - 1` gaps for `n` rows
/// and `CHROME_HEIGHT` must not depend on `n`:
///
/// ```text
/// chrome + n*tile + (n-1)*gap  ==  (CHROME_HEIGHT) + n*(tile + gap)
/// ```
///
/// holds exactly when `CHROME_HEIGHT == chrome - gap`, i.e. when the gap
/// belongs to the row. That identity is what lets `overlay_height` be `a + b*n`
/// at all.
const ROW_GAP: f32 = 4.0;

/// The overlay header's match count, e.g. `"1 match"` / `"4 matches"`.
///
/// It was the literal `"1 match"` regardless of how many rows the card was
/// about to draw — correct only for as long as the overlay showed exactly
/// one, which is the thing the surrounding work exists to stop being true.
/// `rows` is the row count the card really paints, which is
/// `choices.len().max(1)` for the same reason `overlay_height` takes
/// `rows.max(1)`: an empty slice still paints one row.
fn match_count_label(rows: usize) -> String {
    if rows == 1 {
        "1 match".to_string()
    } else {
        format!("{rows} matches")
    }
}

/// The width kept clear at the right-hand end of every choice row for the
/// selected row's `Enter` chip.
///
/// The text column is sized `available - CHIP_LANE` rather than being allowed
/// to take the whole row, because truncation needs a bound and the bound must
/// leave the chip somewhere to be: a text column that ate the full width
/// would push the `Enter` chip off the right edge of a window with no
/// horizontal scrolling either.
///
/// It is reserved on **both** treatments, selected and not, so the two rows
/// truncate at exactly the same place — a row whose text lane changed width
/// when it became selected would re-truncate under the mouse.
///
/// Checked against the chip that is really painted, not chosen and left
/// alone: `the_enter_chip_has_a_lane_of_its_own_and_the_text_stops_short_of_it`
/// measures the painted chip and the painted text and asserts the chip fits
/// inside the lane and no glyph run reaches into it.
const CHIP_LANE: f32 = 56.0;

/// One choice row. `selected` renders the emphasized treatment (blue wash,
/// blue avatar, Enter chip); otherwise the neutral one.
///
/// **A row cannot grow the card, whatever its text says.** This is the
/// module's first critical finding. The row used to be content-sized: two
/// plain `ui.label`s that wrapped, in a card whose height is a fixed
/// `CHROME_HEIGHT + n * ROW_HEIGHT` and a window that is
/// `with_decorations(false)`, always-on-top, unresizable and has no scroll
/// area anywhere. `secondary` is `format!("{item_name} · fills {app_name}")`
/// — **two user-controlled strings** — and the geometry was measured off one
/// short fixture. Measured on the shipped code, a four-row card was 396pt
/// tall against a 314pt window with realistic names (82pt gone), 444pt with a
/// name that has no spaces to wrap at (130pt gone), and 348pt with CJK (34pt
/// gone). Even a ONE-row card overflowed its 164pt window at 177/189pt; the
/// 10pt of [`CHROME_SLACK`] had been absorbing the mild end of it.
///
/// Enlarging the window is not the fix — a vault item's name is arbitrarily
/// long, so there is no worst case to size for. So the two lines are
/// **truncated to one line each** inside a text column of an explicit width
/// ([`CHIP_LANE`]), which makes the row's height a function of the font and
/// nothing else, and leaves the card at exactly the size it has always been
/// for every string.
///
/// **Both `.truncate()` calls are load-bearing, and on different paths.**
/// `secondary` is user-controlled on every path. `primary` is not: with a
/// non-empty choice list it is [`FillChoice::label`], one of four compile-time
/// constants, and with an EMPTY one — which is what `draw_overlay_card` and
/// therefore the whole of production draws today — it is `row_text`'s first
/// value, i.e. the **username, or the item name when there is none**. That
/// asymmetry is why the primary's `.truncate()` was inert under test for a
/// while: every geometry fixture drove the choice-list path, so the top line
/// was always a constant and removing its bound changed nothing measurable.
/// The two halves are covered by two tests, deliberately:
/// `no_string_a_user_can_supply_makes_the_card_taller` (choice rows, hostile
/// secondary) and
/// `the_no_choices_card_production_draws_today_bounds_its_user_controlled_top_line`
/// (no choices, hostile primary).
///
/// `avatar_of` is the text the initials tile is built from, which is NOT
/// `primary` once a row is labelled by what it will type rather than by whose
/// credentials it will type. Both treatments are the same height by
/// construction — the row's height is the taller of the 28pt avatar and the
/// two text lines, and neither the wash nor the chip is on that path — and
/// `both_row_treatments_are_the_same_height` holds it there.
///
/// Returns true when clicked.
fn credential_row(
    ui: &mut egui::Ui,
    avatar_of: &str,
    primary: &str,
    secondary: &str,
    selected: bool,
) -> bool {
    let fill = if selected {
        theme::BLUE_WASH
    } else {
        theme::CANVAS
    };
    let row = egui::Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::avatar(ui, &theme::initials(avatar_of), 28.0, selected);
                ui.add_space(2.0);
                // The text column is given an EXPLICIT width and its two
                // labels TRUNCATE. Both halves are load-bearing; see
                // [`CHIP_LANE`] and the module note above.
                let text_width = (ui.available_width() - CHIP_LANE).max(1.0);
                ui.vertical(|ui| {
                    ui.set_width(text_width);
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.add(
                        egui::Label::new(theme::semibold(primary, 13.0).color(theme::INK))
                            .truncate(),
                    );
                    ui.add(
                        egui::Label::new(
                            RichText::new(secondary).size(11.0).color(theme::TEXT_FAINT),
                        )
                        .truncate(),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if selected {
                        theme::kbd_chip(ui, "Enter", true);
                    }
                });
            });
        });

    let response = row.response.interact(Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_leads_with_the_username_when_known() {
        let (primary, secondary) = row_text("ledgerline.exe", "Ledgerline", Some("a@b.com"));
        assert_eq!(primary, "a@b.com");
        assert_eq!(secondary, "Ledgerline · fills ledgerline.exe");
    }

    #[test]
    fn row_falls_back_to_the_item_name_without_a_username() {
        let (primary, secondary) = row_text("app.exe", "Postgres — Prod", None);
        assert_eq!(primary, "Postgres — Prod");
        assert_eq!(secondary, "fills app.exe");
    }

    #[test]
    fn row_still_says_something_when_the_item_could_not_be_read() {
        let (primary, secondary) = row_text("app.exe", "", None);
        assert_eq!(primary, "Saved credentials");
        assert_eq!(secondary, "fills app.exe");
    }
}

/// The overlay's geometry, measured off frames the card actually paints.
///
/// Why this exists at all: the overlay is a `with_decorations(false)`,
/// always-on-top window with a hardcoded inner size and NO scroll area
/// anywhere. A row that lands past the window's bottom edge is not merely
/// ugly -- there is no title bar to drag, no border to resize and nothing to
/// scroll, so the user cannot reach it by any means. Three separate times in
/// this codebase a text or layout change has pushed a control out of its
/// viewport. So the sizing lands *before* anything can produce more than one
/// row, and it lands with instruments that look at what was painted.
///
/// **Painted ink, not layout rects.** A galley's `rect` is where a run was
/// *placed*; text that laid out into zero rows, or was elided, or was drawn at
/// alpha 0, all have a perfectly healthy galley rect. So [`ink`] reads
/// `RowVisuals::mesh_bounds` (the tessellator's own bounds, whitespace
/// excluded) for text and `Shape::visual_bounding_rect` (which expands a rect
/// by its stroke and its blur) for everything else, and it resolves each
/// shape's actual painted colour so a fully transparent shape can be
/// discarded.
#[cfg(test)]
mod geometry_tests {
    use super::*;
    use crate::key_sequence::FieldRef;
    use eframe::egui::{epaint, Color32, Rect};

    // ---------------------------------------------------------------- ink

    /// One painted thing: where its ink actually landed, how opaque that ink
    /// is at its most opaque, and -- for text -- the characters that ink is.
    #[derive(Debug, Clone)]
    struct Ink {
        rect: Rect,
        alpha: u8,
        /// `Some` only for text; the glyphs of ONE laid-out row, in order.
        glyphs: Option<String>,
        /// `Some` only for `Shape::Rect`: its fill and corner radius, which is
        /// how a row tile is told apart from an avatar or a chip.
        tile: Option<(Color32, u8)>,
    }

    fn alpha_of(colors: &[Color32]) -> u8 {
        colors.iter().map(|c| c.a()).max().unwrap_or(0)
    }

    fn path_alpha(fill: Color32, stroke: &epaint::PathStroke) -> u8 {
        let stroke_alpha = match &stroke.color {
            epaint::ColorMode::Solid(c) => c.a(),
            // A UV-mapped stroke's colour is a function this test cannot
            // evaluate. Treat it as fully visible rather than as absent --
            // an instrument that assumes "invisible" is the failure mode
            // this whole module exists to avoid.
            epaint::ColorMode::UV(_) => 255,
        };
        if stroke.width <= 0.0 {
            fill.a()
        } else {
            fill.a().max(stroke_alpha)
        }
    }

    /// Walks one shape tree into [`Ink`].
    ///
    /// The match is EXHAUSTIVE -- no `_` arm. Every `epaint::Shape` variant is
    /// named, so a shape kind this walker has never seen is a compile error
    /// rather than a silently dropped row. The card is known to emit
    /// `Vec`, `Rect` (frames, avatars, chips, hairlines), `Text` (every label
    /// and both chips), `LineSegment` (the dismiss ✕, which is two strokes and
    /// not a glyph) and `Path` (the Deskwarden mark); the remaining arms are
    /// handled anyway so that they cannot become blind spots later.
    fn walk(shape: &egui::Shape, out: &mut Vec<Ink>) {
        let rect = shape.visual_bounding_rect();
        let mut plain = |alpha: u8| {
            out.push(Ink {
                rect,
                alpha,
                glyphs: None,
                tile: None,
            });
        };
        match shape {
            egui::Shape::Noop => {}
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            egui::Shape::Circle(c) => plain(alpha_of(&[c.fill, c.stroke.color])),
            egui::Shape::Ellipse(e) => plain(alpha_of(&[e.fill, e.stroke.color])),
            egui::Shape::LineSegment { stroke, .. } => plain(alpha_of(&[stroke.color])),
            egui::Shape::Path(p) => plain(path_alpha(p.fill, &p.stroke)),
            egui::Shape::QuadraticBezier(b) => plain(path_alpha(b.fill, &b.stroke)),
            egui::Shape::CubicBezier(b) => plain(path_alpha(b.fill, &b.stroke)),
            egui::Shape::Mesh(m) => {
                plain(m.vertices.iter().map(|v| v.color.a()).max().unwrap_or(0));
            }
            egui::Shape::Rect(r) => out.push(Ink {
                rect,
                alpha: alpha_of(&[r.fill, r.stroke.color]),
                glyphs: None,
                tile: Some((r.fill, r.corner_radius.nw)),
            }),
            egui::Shape::Text(t) => {
                // The tessellator draws NOTHING at all for these two, so
                // neither may be reported as painted ink.
                if t.galley.is_empty() || t.opacity_factor <= 0.0 {
                    return;
                }
                for placed in &t.galley.rows {
                    let row = &placed.row;
                    if row.visuals.mesh.is_empty() {
                        continue;
                    }
                    // Exactly the tessellator's own arithmetic:
                    // `row.visuals.mesh_bounds` translated by galley pos +
                    // row pos. Not `galley.rect`, which is where the run was
                    // placed rather than where its ink is.
                    let rect = row
                        .visuals
                        .mesh_bounds
                        .translate(t.pos.to_vec2() + placed.pos.to_vec2());
                    let alpha = row.visuals.mesh.vertices[row.visuals.glyph_vertex_range.clone()]
                        .iter()
                        .map(|v| {
                            let c = match t.override_text_color {
                                Some(o) => o,
                                None if v.color == Color32::PLACEHOLDER => t.fallback_color,
                                None => v.color,
                            };
                            if t.opacity_factor < 1.0 {
                                c.gamma_multiply(t.opacity_factor).a()
                            } else {
                                c.a()
                            }
                        })
                        .max()
                        .unwrap_or(0);
                    out.push(Ink {
                        rect,
                        alpha,
                        glyphs: Some(row.glyphs.iter().map(|g| g.chr).collect()),
                        tile: None,
                    });
                }
            }
            egui::Shape::Callback(_) => panic!(
                "the overlay card painted a backend callback; this walker cannot see inside \
                 one, and an instrument that cannot see a row is exactly what these tests \
                 exist to prevent"
            ),
        }
    }

    // -------------------------------------------------------------- frames

    /// A context with this app's fonts really installed. `theme::apply` takes
    /// effect at the START of the next frame, so the two warm-up frames are
    /// load-bearing, not defensive.
    fn styled_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(sized(overlay_height(1)), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(sized(overlay_height(1)), |_ui| {});
        ctx
    }

    fn sized(height: f32) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(OVERLAY_WIDTH, height),
            )),
            ..Default::default()
        }
    }

    const APP: &str = "ledgerline.exe";
    const ITEM: &str = "Ledgerline";
    const USER: &str = "ada@example.com";

    /// One set of strings the card can be asked to draw.
    ///
    /// The geometry above was measured off `APP`/`ITEM`/`USER` alone, and
    /// that is how the card came to overflow its own window by up to 130
    /// points: `item_name` and `app_name` are **user-controlled**, they are
    /// concatenated into the secondary line, and a short fixture is the one
    /// input for which a wrapping row happens not to wrap.
    #[derive(Debug, Clone, Copy)]
    struct Fixture {
        name: &'static str,
        app: &'static str,
        item: &'static str,
        user: &'static str,
    }

    /// The fixture the module's numbers were originally measured from. It
    /// stays, as the control: whatever the adversarial ones do, the card the
    /// overlay has always drawn must not move.
    const SHORT: Fixture = Fixture {
        name: "short",
        app: APP,
        item: ITEM,
        user: USER,
    };

    /// Realistic, and long: a real vault item in a real organisation, and the
    /// kind of address a corporate directory hands out. Nothing exotic — this
    /// is the case the shipped card lost 82 points to.
    const LONG: Fixture = Fixture {
        name: "long realistic names",
        app: "ledgerline-production-accounting-cluster-primary-host.exe",
        item: "Ledgerline Production Accounting Cluster — Primary Vault Entry",
        user: "ada.lovelace.administrator@ledgerline-production-accounting.example.com",
    };

    /// Wide glyphs: CJK is roughly twice the advance per character, so a name
    /// of unremarkable *length* is a line of unremarkable-looking text that
    /// does not fit.
    ///
    /// The `user` is long enough to wrap **on its own**, not merely once it has
    /// been concatenated into the secondary line. That distinction is the whole
    /// of the primary label's coverage: the username is what the top line paints
    /// on the no-choices path production draws today, and an 18-glyph one fits a
    /// 260pt column at 13pt, so it exercised nothing there. See
    /// `the_no_choices_card_production_draws_today_bounds_its_user_controlled_top_line`,
    /// whose control refuses a fixture that would not have wrapped.
    const CJK: Fixture = Fixture {
        name: "CJK",
        app: "銀行口座管理システム.exe",
        item: "銀行口座管理システム本番環境の管理者資格情報エントリ",
        user: "銀行口座管理システム本番環境管理者＠銀行口座管理システム.example.co.jp",
    };

    /// **Nothing to wrap at.** A word wrapper's escape hatch is a space; a
    /// single unbroken token has none, so this is the worst case for any fix
    /// that relies on wrapping rather than on a bound.
    const NO_SPACES: Fixture = Fixture {
        name: "no spaces",
        app: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.exe",
        item: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        user: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    };

    /// Every fixture the card must survive, the short one first.
    const FIXTURES: [Fixture; 4] = [SHORT, LONG, CJK, NO_SPACES];

    /// The four rows `app::fill_choices` can produce, in its own order.
    fn four_choices() -> Vec<FillChoice> {
        vec![
            FillChoice::UserTabPass,
            FillChoice::Just(FieldRef::Username),
            FillChoice::Just(FieldRef::Password),
            FillChoice::Just(FieldRef::Totp),
        ]
    }

    /// Paints a real card with `choices` into a window of `height`, and
    /// returns every painted thing with non-zero alpha.
    ///
    /// Zero-alpha shapes are DISCARDED here, at the source: a card whose rows
    /// are painted fully transparent must look to every assertion below
    /// exactly like a card with no rows, because that is what it looks like to
    /// the user.
    fn painted(choices: &[FillChoice], height: f32) -> Vec<Ink> {
        painted_as(SHORT, choices, height)
    }

    /// [`painted`], for a card drawn with `fixture`'s strings rather than the
    /// short ones.
    fn painted_as(fixture: Fixture, choices: &[FillChoice], height: f32) -> Vec<Ink> {
        painted_as_user(fixture, Some(fixture.user), choices, height)
    }

    /// [`painted_as`], with the username spelled out rather than assumed to be
    /// present.
    ///
    /// `OverlayApp::ui` passes `self.username.as_deref()`, so `None` is a value
    /// production ships, and on that arm the top line paints the ITEM NAME.
    /// Every painting helper here hard-coded `Some(..)`, which is why the
    /// fallback arm went unmeasured.
    fn painted_as_user(
        fixture: Fixture,
        user: Option<&str>,
        choices: &[FillChoice],
        height: f32,
    ) -> Vec<Ink> {
        let ctx = styled_ctx();
        let output = ctx.run_ui(sized(height), |ui| {
            draw_overlay_card_rows(ui, fixture.app, fixture.item, user, choices);
        });
        let mut ink = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut ink);
        }
        ink.retain(|i| i.alpha > 0);
        ink
    }

    /// Clicks the point `at` on a card drawn with `choices`, and returns what
    /// the card answered on the frame the button came back up.
    ///
    /// Two frames on ONE context, not one: egui decides a click on the release,
    /// and a press and a release squeezed into a single frame is not the
    /// gesture the user makes. The first frame is the press (whose answer is
    /// asserted to be `None` -- a card that "filled" on mouse-down would fill
    /// the row the user dragged off), the second the release.
    fn click_on(choices: &[FillChoice], at: egui::Pos2) -> OverlayAction {
        let height = overlay_height(choices.len());
        let ctx = styled_ctx();
        // Warm-up frame: the row Frames must have been laid out once before
        // their rects can be interacted with.
        let _ = ctx.run_ui(sized(height), |ui| {
            draw_overlay_card_rows(ui, APP, ITEM, Some(USER), choices);
        });

        let press = |down: bool| egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: down,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            ..sized(height)
        };

        let mut on_press = OverlayAction::None;
        let _ = ctx.run_ui(press(true), |ui| {
            on_press = draw_overlay_card_rows(ui, APP, ITEM, Some(USER), choices);
        });
        assert_eq!(
            on_press,
            OverlayAction::None,
            "the card answered on mouse-DOWN; a press the user drags away from is not a choice"
        );

        let mut on_release = OverlayAction::None;
        let _ = ctx.run_ui(press(false), |ui| {
            on_release = draw_overlay_card_rows(ui, APP, ITEM, Some(USER), choices);
        });
        on_release
    }

    /// The clickable tiles of the choice rows, top to bottom.
    ///
    /// A row is a full-width, 8pt-radius filled rect in one of the two row
    /// treatments. Nothing else in the card is any of those things: the card
    /// itself is radius 10, the footer strip 9, the avatar 7 and 28pt wide,
    /// the keyboard chips 4, and the hairlines 0. The frame's own rect IS the
    /// rect its `Response` interacts on, so this is the clickable rect and not
    /// a proxy for it.
    fn row_tiles(ink: &[Ink]) -> Vec<Rect> {
        let mut tiles: Vec<Rect> = ink
            .iter()
            .filter(|i| {
                matches!(i.tile, Some((fill, 8)) if fill == theme::BLUE_WASH || fill == theme::CANVAS)
                    && i.rect.width() > OVERLAY_WIDTH / 2.0
            })
            .map(|i| i.rect)
            .collect();
        tiles.sort_by(|a, b| a.top().total_cmp(&b.top()));
        tiles
    }

    /// The ink of the one laid-out row whose glyphs are exactly `text`.
    ///
    /// Asserts there is EXACTLY one, which is what makes "the label is on
    /// screen" a claim about the label the caller named: a label that laid out
    /// into zero rows, or was elided to "Username + Tab + Pass…", has no
    /// match here and fails rather than quietly matching something else.
    fn glyph_run(ink: &[Ink], text: &str) -> Rect {
        let hits: Vec<&Ink> = ink
            .iter()
            .filter(|i| i.glyphs.as_deref() == Some(text))
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "expected exactly one painted run reading {text:?}; found {} -- painted runs were {:?}",
            hits.len(),
            ink.iter()
                .filter_map(|i| i.glyphs.clone())
                .collect::<Vec<_>>()
        );
        hits[0].rect
    }

    fn window(height: f32) -> Rect {
        Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(OVERLAY_WIDTH, height))
    }

    /// True when `inner` is fully within `outer`, with a hair of tolerance for
    /// the pixel-rounding the tessellator does to text.
    fn fits(inner: Rect, outer: Rect) -> bool {
        inner.top() >= outer.top() - 0.5
            && inner.bottom() <= outer.bottom() + 0.5
            && inner.left() >= outer.left() - 0.5
            && inner.right() <= outer.right() + 0.5
    }

    // --------------------------------------------------------------- tests

    /// The number this whole step is not allowed to change.
    #[test]
    fn a_one_row_card_is_the_size_the_overlay_has_always_been() {
        assert_eq!(
            overlay_height(1),
            164.0,
            "the overlay has shipped at 396x164 since it was written; a one-row card \
             must still be exactly that"
        );
        // NOT `assert_eq!(CHROME_HEIGHT + ROW_HEIGHT, 164.0)`, which was a
        // tautology: `CHROME_HEIGHT` was *defined* as `164.0 - ROW_HEIGHT`, so
        // the two constants could have been (40, 124) or (240, -76) and it
        // would still have held. What 164.0 is made of is checked against the
        // drawing instead, in
        // `the_chrome_constant_is_the_chrome_the_card_actually_paints`.
        // A card with nothing to offer is not a shorter card. `rows.max(1)`.
        assert_eq!(overlay_height(0), 164.0);
        // Each further row costs exactly one row.
        assert_eq!(overlay_height(2), 164.0 + ROW_HEIGHT);
        assert_eq!(overlay_height(3), 164.0 + 2.0 * ROW_HEIGHT);
        assert_eq!(overlay_height(4), 164.0 + 3.0 * ROW_HEIGHT);
    }

    /// The load-bearing test. For one, two, three and four rows: every row's
    /// clickable rect and every row's painted label are inside the window that
    /// `overlay_height` sized.
    ///
    /// Both counters are the point. A walker that finds no tiles, or a loop
    /// that never runs, satisfies every "is inside" assertion vacuously --
    /// which is precisely how this codebase's instruments have gone blind
    /// before.
    #[test]
    fn every_choice_row_is_inside_the_window_for_one_two_three_and_four_rows() {
        let mut iterations = 0;
        for n in 1..=4usize {
            iterations += 1;
            let choices = &four_choices()[..n];
            let height = overlay_height(n);
            let ink = painted(choices, height);
            let tiles = row_tiles(&ink);

            assert_eq!(
                tiles.len(),
                n,
                "a {n}-row card painted {} row tiles; a card that draws fewer rows than it \
                 was handed passes every geometry assertion below for free",
                tiles.len()
            );

            for (index, tile) in tiles.iter().enumerate() {
                assert!(
                    fits(*tile, window(height)),
                    "row {index} of {n} has its clickable rect at {tile:?}, outside the \
                     {height}pt window -- and this window has no title bar, no resize \
                     border and no scroll area, so the user could never reach it"
                );
            }

            for (index, choice) in choices.iter().enumerate() {
                let label = choice.label();
                // The glyphs painted equal the label asked for: not elided,
                // not wrapped into a second row, not laid out into none.
                let painted_label = glyph_run(&ink, &label);
                assert!(
                    fits(painted_label, window(height)),
                    "the label {label:?} (row {index} of {n}) has ink at {painted_label:?}, \
                     outside the {height}pt window"
                );
                assert!(
                    fits(painted_label, tiles[index].expand(1.0)),
                    "the label {label:?} paints at {painted_label:?}, which is not inside \
                     its own row tile {:?} -- the label and the clickable rect have come \
                     apart",
                    tiles[index]
                );
            }
        }
        assert_eq!(iterations, 4, "the loop must have covered 1, 2, 3 and 4 rows");
    }

    /// A four-row card is the tallest the overlay can be (`fill_choices`
    /// yields at most four by construction), so it is the one that pushes the
    /// footer and the dismiss control hardest.
    #[test]
    fn the_dismiss_control_and_the_footer_stay_inside_a_four_row_card() {
        let height = overlay_height(4);
        let ink = painted(&four_choices(), height);
        let win = window(height);

        // The ✕ is two 1.3pt line segments, not a glyph -- U+2715 resolves to
        // nothing in this app's face, so `close_glyph` draws it. It is the
        // ONLY mouse-operable way out of a decorationless window.
        // Each arm spans 7.0pt (`arm = 3.5` either side of centre) and its
        // visual bounding rect adds half its 1.3pt stroke on each end: 8.3
        // square, measured off the painted shapes and matched by nothing else
        // in the card (the mark's four paths are 5.7 x 6.9).
        let arms: Vec<&Ink> = ink
            .iter()
            .filter(|i| {
                i.tile.is_none()
                    && i.glyphs.is_none()
                    && (i.rect.width() - 8.3).abs() < 0.5
                    && (i.rect.height() - 8.3).abs() < 0.5
            })
            .collect();
        assert_eq!(
            arms.len(),
            2,
            "expected the dismiss ✕'s two strokes; found {}",
            arms.len()
        );
        for arm in &arms {
            assert!(
                fits(arm.rect, win),
                "a stroke of the dismiss ✕ is at {:?}, outside the {height}pt window",
                arm.rect
            );
        }

        // The footer's keyboard hints.
        for hint in ["Enter Fill", "Esc Dismiss"] {
            let rect = glyph_run(&ink, hint);
            assert!(
                fits(rect, win),
                "the footer hint {hint:?} paints at {rect:?}, outside the {height}pt window"
            );
        }

        // The footer strip itself sits BELOW the last row, not over it.
        let last_row = *row_tiles(&ink).last().expect("a four-row card has rows");
        let footer = glyph_run(&ink, "Enter Fill");
        assert!(
            footer.top() >= last_row.bottom(),
            "the footer's hints paint at {footer:?}, which overlaps the last row {last_row:?}"
        );
    }

    #[test]
    fn two_rows_never_overlap() {
        let height = overlay_height(4);
        let tiles = row_tiles(&painted(&four_choices(), height));
        assert_eq!(tiles.len(), 4);
        let mut pairs = 0;
        for pair in tiles.windows(2) {
            pairs += 1;
            let (a, b) = (pair[0], pair[1]);
            assert!(
                b.top() >= a.bottom(),
                "adjacent row tiles {a:?} and {b:?} intersect; one row is painting over \
                 the other"
            );
        }
        assert_eq!(pairs, 3, "four rows have exactly three adjacent pairs");
    }

    /// `ROW_HEIGHT` is measured, not chosen: it is the pitch two real rows are
    /// actually painted at.
    #[test]
    fn a_row_occupies_exactly_one_row_height() {
        let tiles = row_tiles(&painted(&four_choices(), overlay_height(4)));
        assert_eq!(tiles.len(), 4);
        let mut pitches = 0;
        for pair in tiles.windows(2) {
            pitches += 1;
            let pitch = pair[1].top() - pair[0].top();
            assert!(
                (pitch - ROW_HEIGHT).abs() < 0.01,
                "two rows are painted {pitch}pt apart, but `overlay_height` grows the \
                 window by {ROW_HEIGHT}pt per row"
            );
        }
        assert_eq!(pitches, 3);
        for tile in &tiles {
            assert!(
                (tile.height() - (ROW_HEIGHT - ROW_GAP)).abs() < 0.01,
                "a row tile is {}pt tall; ROW_HEIGHT - ROW_GAP is {}",
                tile.height(),
                ROW_HEIGHT - ROW_GAP
            );
        }
    }

    /// `overlay_height`'s `CHROME_HEIGHT` term does not depend on `n`, which
    /// is only true if the inter-row gaps are inside `ROW_HEIGHT`. Measured
    /// as: the slack left under the last painted thing is the same at one row
    /// as at four.
    #[test]
    fn the_chrome_costs_the_same_at_one_row_as_at_four() {
        let bottom_slack = |n: usize, choices: &[FillChoice]| {
            let height = overlay_height(n);
            let ink = painted(choices, height);
            let footer = glyph_run(&ink, "Enter Fill");
            height - footer.bottom()
        };
        let one = bottom_slack(1, &four_choices()[..1]);
        let four = bottom_slack(4, &four_choices());
        assert!(
            (one - four).abs() < 0.01,
            "a one-row card leaves {one}pt under its footer and a four-row card {four}pt; \
             the chrome is not a constant, so `CHROME_HEIGHT + ROW_HEIGHT * n` is the \
             wrong shape"
        );
        assert!(
            one >= 0.0,
            "the footer is already {}pt past the bottom of a one-row window",
            -one
        );
    }

    /// Both row treatments must be the same height, or `ROW_HEIGHT` is a
    /// single number describing two different rows.
    #[test]
    fn both_row_treatments_are_the_same_height() {
        let ink = painted(&four_choices(), overlay_height(4));
        let tiles = row_tiles(&ink);
        assert_eq!(tiles.len(), 4);
        // Positive control: the two treatments really are DIFFERENT, so this
        // is a claim about two variants and not about one drawn four times.
        let fills: Vec<Color32> = ink
            .iter()
            .filter(|i| {
                matches!(i.tile, Some((_, 8))) && i.rect.width() > OVERLAY_WIDTH / 2.0
            })
            .map(|i| match i.tile {
                Some((fill, _)) => fill,
                None => unreachable!(),
            })
            .collect();
        assert_eq!(fills.len(), 4);
        assert_eq!(fills[0], theme::BLUE_WASH, "the first row is the selected one");
        assert!(
            fills[1..].iter().all(|f| *f == theme::CANVAS),
            "rows after the first are the neutral treatment; got {fills:?}"
        );
        let first = tiles[0].height();
        for tile in &tiles[1..] {
            assert!(
                (tile.height() - first).abs() < 0.01,
                "the selected row is {first}pt tall and a neutral one {}pt",
                tile.height()
            );
        }
    }

    /// The card with no choices -- which is every card production draws until
    /// step 5 -- is the card it has always been: one row, selected treatment,
    /// same height, inside 164.
    #[test]
    fn a_card_with_no_choices_is_still_the_one_row_card() {
        let ink = painted(&[], 164.0);
        let tiles = row_tiles(&ink);
        assert_eq!(tiles.len(), 1, "no choices means exactly one row, not none");
        assert!(fits(tiles[0], window(164.0)));
        let username = glyph_run(&ink, USER);
        assert!(fits(username, window(164.0)));
        // And it is byte-identical in geometry to the one-choice card's row.
        let one_choice = row_tiles(&painted(&four_choices()[..1], overlay_height(1)));
        assert_eq!(one_choice.len(), 1);
        assert!(
            (one_choice[0].height() - tiles[0].height()).abs() < 0.01
                && (one_choice[0].top() - tiles[0].top()).abs() < 0.01,
            "the choice row {:?} is not where the matched-credential row {:?} is",
            one_choice[0],
            tiles[0]
        );
    }

    // ------------------------------------------- which row answered, and how

    /// **Click row `i`, get choice `i`.** The whole point of the step: four
    /// rows that all answer `choices[0]` are four ways to do one thing, and
    /// look exactly like four working rows to a test that only asks whether
    /// a fill happened.
    #[test]
    fn each_row_answers_its_own_choice() {
        let choices = four_choices();
        let tiles = row_tiles(&painted(&choices, overlay_height(choices.len())));
        assert_eq!(
            tiles.len(),
            choices.len(),
            "the card lost a row before a single click was sent -- egui culls shapes that \
             fall outside the screen rect, so a pushed-out row comes back as nothing"
        );

        let mut answers = Vec::new();
        for (index, tile) in tiles.iter().enumerate() {
            match click_on(&choices, tile.center()) {
                OverlayAction::Fill(choice) => answers.push(choice),
                other => panic!("row {index} at {tile:?} answered {other:?}, not a fill"),
            }
        }

        assert_eq!(answers.len(), 4, "the loop must have clicked all four rows");
        assert_eq!(
            answers, choices,
            "row i must answer choice i, in the order the rows are drawn"
        );
        // Pairwise distinct: a mapping that answers `choices[0]` for every row
        // would satisfy a weaker per-row assertion against a fixture whose
        // rows happened to repeat.
        for (i, a) in answers.iter().enumerate() {
            for b in &answers[i + 1..] {
                assert_ne!(a, b, "two rows answered the same choice: {answers:?}");
            }
        }
    }

    /// Clicking nothing in particular answers nothing -- the control that
    /// makes the test above about the ROWS rather than about clicking.
    #[test]
    fn a_click_that_lands_on_no_row_answers_nothing() {
        let choices = four_choices();
        let tiles = row_tiles(&painted(&choices, overlay_height(choices.len())));
        assert_eq!(tiles.len(), 4);
        // The footer strip, below every row.
        let below = egui::pos2(OVERLAY_WIDTH / 2.0, tiles[3].bottom() + 12.0);
        assert!(below.y < overlay_height(4), "the probe is inside the window");
        assert_eq!(click_on(&choices, below), OverlayAction::None);
    }

    /// **Enter takes the PRIMARY row, which is the first one.**
    ///
    /// The fixture's first row is deliberately not the one any of the obvious
    /// wrong implementations would reach for -- not `Saved` (the no-choices
    /// fallback), not `UserTabPass` (the historical fill), and not the last
    /// row -- so `enter fills the password field` is a claim about position
    /// and not about which variant happens to be around.
    #[test]
    fn enter_takes_the_first_row() {
        let choices = vec![
            FillChoice::Just(FieldRef::Password),
            FillChoice::UserTabPass,
            FillChoice::Saved,
        ];
        // The fixture controls: the rows really do differ, so "the first" is a
        // distinguishable answer.
        assert_ne!(choices[0], choices[1]);
        assert_ne!(choices[0], choices[2]);
        assert_ne!(choices[0], *choices.last().unwrap());

        assert_eq!(primary_choice(&choices), FillChoice::Just(FieldRef::Password));
        // And through the keyboard, which is the path that actually reaches
        // the user: Enter, not a click, not the card.
        assert_eq!(
            action(true, false, &choices),
            OverlayAction::Fill(FillChoice::Just(FieldRef::Password))
        );
        // And with no choices at all -- the card production still draws --
        // Enter is the fill it has always been.
        assert_eq!(primary_choice(&[]), FillChoice::Saved);
        assert_eq!(
            action(true, false, &[]),
            OverlayAction::Fill(FillChoice::Saved)
        );
    }

    /// **Esc dismisses, and dismissing is not a fill.** An Esc that answered
    /// `Some(primary)` -- one line, and the same shape as the Enter arm right
    /// above it -- types the user's password into the app they just said no
    /// to. Nothing else in this module could see it: `OverlayApp::ui` needs a
    /// real window.
    #[test]
    fn escape_dismisses_and_answers_no_choice() {
        let choices = four_choices();
        assert_eq!(action(false, true, &choices), OverlayAction::Dismiss);
        assert_eq!(action(false, true, &[]), OverlayAction::Dismiss);
        // Not a fill of anything, spelled out so a `Fill` variant added later
        // cannot slip past an equality against one particular value.
        assert!(!matches!(
            action(false, true, &choices),
            OverlayAction::Fill(_)
        ));
        // The controls: the instrument does report fills, and reports nothing
        // when nothing was pressed.
        assert!(matches!(action(true, false, &choices), OverlayAction::Fill(_)));
        assert_eq!(action(false, false, &choices), OverlayAction::None);
        // Both at once: the fill wins, as it did when these were two `if`s.
        assert_eq!(
            action(true, true, &choices),
            OverlayAction::Fill(choices[0].clone())
        );
    }

    /// **The residue of the swap, closed.**
    ///
    /// [`EnterPressed`] and [`EscapePressed`] being distinct types makes
    /// `keyboard_action(escape, enter, ..)` a compile error, so the swap the
    /// review found cannot be written at the call site any more. What it
    /// leaves behind is one level further in: `EnterPressed::read` could ask
    /// the context about `egui::Key::Escape`, which compiles and puts the
    /// identical bug back — Esc filling the password.
    ///
    /// That is reachable, and this reaches it. A bare `egui::Context` opens
    /// no window and needs no `eframe::Frame`, so each reader is run over a
    /// real frame carrying a real key event, and asserted to answer for **its
    /// own** key and not the other one. The negative halves are the load-
    /// bearing ones: a reader that answered `true` for both keys would pass a
    /// positive-only test.
    #[test]
    fn each_key_reader_reads_the_key_it_is_named_after() {
        fn read(keys: &[egui::Key]) -> (bool, bool) {
            let (enter, escape) = keys_down(keys);
            (enter.pressed(), escape.pressed())
        }

        // Enter down: the Enter reader says yes, the Escape reader says no.
        assert_eq!(
            read(&[egui::Key::Enter]),
            (true, false),
            "the Enter frame was not read as Enter-and-only-Enter; a reader that \
             answers for the other key puts `keyboard_action`'s swapped-argument bug \
             back one level in, where Esc fills the password"
        );
        // Escape down: exactly the mirror.
        assert_eq!(
            read(&[egui::Key::Escape]),
            (false, true),
            "the Escape frame was not read as Escape-and-only-Escape"
        );
        // The control: a frame with no key at all is not read as either, so
        // neither reader can be a constant `true`.
        assert_eq!(
            read(&[]),
            (false, false),
            "a frame carrying no key press was read as one"
        );
        // And a third key is neither, so neither reader can be "any key".
        assert_eq!(
            read(&[egui::Key::Space]),
            (false, false),
            "Space was read as Enter or Escape"
        );
    }

    /// The two key readers, run over one real frame carrying `keys`.
    ///
    /// **This is the only way a test can obtain either newtype**, and
    /// deliberately so. The fields are private to `mod keys` and no
    /// constructor takes a `bool` — not even a `cfg(test)` one — so a test
    /// cannot manufacture an `EnterPressed(true)` that production has no way
    /// to produce, and the re-expression of the key swap that the review
    /// found (`EnterPressed(EscapePressed::read(&ctx).0)`) is unspellable in
    /// the test module too. A bare `egui::Context` opens no window and needs
    /// no `eframe::Frame`.
    fn keys_down(keys: &[egui::Key]) -> (EnterPressed, EscapePressed) {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            events: keys
                .iter()
                .map(|&key| egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::default(),
                })
                .collect(),
            ..sized(overlay_height(1))
        };
        let mut seen = None;
        let _ = ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            seen = Some((EnterPressed::read(&ctx), EscapePressed::read(&ctx)));
        });
        seen.expect("the frame body must have run")
    }

    /// `keyboard_action` driven from a real frame carrying (or not carrying)
    /// each of the two keys.
    ///
    /// The `assert_eq!` is a precondition, not a duplicate of
    /// `each_key_reader_reads_the_key_it_is_named_after`: without it a frame
    /// that silently carried no key would make every caller below assert
    /// against `(false, false)` and pass for the wrong reason.
    fn action(enter: bool, escape: bool, choices: &[FillChoice]) -> OverlayAction {
        let mut keys = Vec::new();
        if enter {
            keys.push(egui::Key::Enter);
        }
        if escape {
            keys.push(egui::Key::Escape);
        }
        let (enter_read, escape_read) = keys_down(&keys);
        assert_eq!(
            (enter_read.pressed(), escape_read.pressed()),
            (enter, escape),
            "the frame built for enter={enter}, escape={escape} was not read back as that, \
             so nothing below is testing the case it names"
        );
        keyboard_action(enter_read, escape_read, choices)
    }

    // ------------------------------------------- the window that is asked for

    /// The inner size `show_prompt_overlay` will hand `eframe::NativeOptions`
    /// for a card of `rows` rows.
    ///
    /// **Observed, not recomputed.** Nothing on this path mentions
    /// `overlay_height`: the number comes back out of the same
    /// `ViewportBuilder` that production puts into `NativeOptions`, so it
    /// changes when, and only when, the window the user gets changes.
    fn requested_inner_size(rows: usize) -> egui::Vec2 {
        overlay_options(&four_choices()[..rows.min(4)], None)
            .viewport
            .inner_size
            .expect("the overlay viewport must request an inner size at all")
    }

    /// Paints a real `rows`-row card into a window of exactly `height` points
    /// and reports what did not survive it, or `Ok(())`.
    ///
    /// The shared instrument behind both the load-bearing assertion and its
    /// positive control, so the control really does exercise the check it is
    /// controlling rather than a lookalike.
    ///
    /// Everything a user must be able to see and click is checked, not just
    /// the rows: the dismiss ✕ and the footer hints live BELOW the last row,
    /// so a window short by less than one row clips them while every row
    /// still fits. That is the failure a row-only check waves through.
    ///
    /// The row COUNT is the first thing asserted because egui culls shapes
    /// entirely outside the screen rect: a row pushed off the bottom comes
    /// back as *nothing at all*, and "every row I found is inside the window"
    /// is trivially true of a card that lost one.
    fn card_fits_in(rows: usize, height: f32) -> Result<(), String> {
        card_fits_in_with(SHORT, rows, height)
    }

    /// [`card_fits_in`] for one particular fixture's strings.
    fn card_fits_in_with(fixture: Fixture, rows: usize, height: f32) -> Result<(), String> {
        let choices = &four_choices()[..rows];
        let ink = painted_as(fixture, choices, height);
        let win = window(height);
        let tiles = row_tiles(&ink);
        if tiles.len() != rows {
            return Err(format!(
                "a {rows}-row card ({}) in a {height}pt window painted {} row tiles; the \
                 missing ones were culled for being off the window entirely",
                fixture.name,
                tiles.len()
            ));
        }
        for (index, tile) in tiles.iter().enumerate() {
            if !fits(*tile, win) {
                return Err(format!(
                    "row {index} of {rows} has its clickable rect at {tile:?}, outside the \
                     {height}pt window"
                ));
            }
        }
        // The footer hints and the ✕: painted last, lowest in the card, and
        // the only mouse-operable way out of a decorationless window.
        // Each hint is laid out as one galley, key and label together --
        // the same runs `the_chrome_costs_the_same_at_one_row_as_at_four`
        // measures the footer by.
        for text in ["Enter Fill", "Esc Dismiss"] {
            let hits: Vec<&Ink> = ink
                .iter()
                .filter(|i| i.glyphs.as_deref() == Some(text))
                .collect();
            if hits.len() != 1 {
                return Err(format!(
                    "expected exactly one painted run reading {text:?} in a {height}pt window \
                     for {rows} rows; found {} -- the footer is off the bottom of a window \
                     with no scrollbar",
                    hits.len()
                ));
            }
            if !fits(hits[0].rect, win) {
                return Err(format!(
                    "the footer hint {text:?} paints at {:?}, outside the {height}pt window",
                    hits[0].rect
                ));
            }
        }
        Ok(())
    }

    /// **The load-bearing assertion about the window itself, and the one this
    /// module did not have.**
    ///
    /// `show_prompt_overlay` calls `eframe::run_native`, so no test may run
    /// it; every other geometry test in this module therefore builds its own
    /// window out of `overlay_height` and checks the card against that. Set
    /// the real `with_inner_size` height to a literal `100.0` and all of them
    /// stay green, because the card is fine and the *window* is the thing
    /// nobody was looking at.
    ///
    /// This one starts from `overlay_viewport` — the same builder production
    /// hands to `NativeOptions` — reads the size back out of it, and paints a
    /// real card into exactly that many points. The screen rect is the
    /// requested size; the requested size is not the screen rect's source.
    /// Re-pointed at [`FIXTURES`], not just the short one. That is the second
    /// half of the finding this test was written for: it was well built and
    /// it proved the *fixture's* card fits, while the card the user's own
    /// item name produces was 82 points too tall for the same window.
    #[test]
    fn the_window_the_overlay_actually_asks_for_fits_the_card_it_will_draw() {
        let mut checked = 0;
        for fixture in FIXTURES {
            for rows in 1..=4usize {
                let requested = requested_inner_size(rows);
                assert_eq!(
                    requested.x, OVERLAY_WIDTH,
                    "the overlay asked for a {}pt-wide window",
                    requested.x
                );
                if let Err(why) = card_fits_in_with(fixture, rows, requested.y) {
                    panic!(
                        "the window the overlay asks the OS for with {rows} choice(s) is \
                         {}pt tall, and the card it then draws for the {:?} fixture does \
                         not fit in it: {why}. This window is frameless and always-on-top \
                         -- no title bar, no resize border, no scroll area -- so whatever \
                         is outside it is gone.",
                        requested.y, fixture.name
                    );
                }
                checked += 1;
            }
        }
        assert_eq!(
            checked,
            FIXTURES.len() * 4,
            "the loop must have covered every fixture at 1, 2, 3 and 4 rows"
        );
    }

    /// **A row cannot grow the card, whatever its text says.**
    ///
    /// `card_fits_in` above answers "did anything get clipped", which is the
    /// user-visible question but a *derived* one: it can only ever see as far
    /// as the window's edge. This asks the direct one — how tall is the card
    /// egui lays out — in a window far too big to cull or constrain anything,
    /// so an overflowing card is measured rather than truncated by the
    /// instrument.
    ///
    /// The expected heights are the four spelled out in
    /// `the_four_card_heights_the_chrome_was_measured_from`, and they are
    /// **not** derived from `overlay_height`, `CHROME_HEIGHT` or
    /// `MEASURED_CHROME`: an assertion built out of the constants it is
    /// checking is arithmetic, not a measurement.
    ///
    /// On the shipped code this failed at every adversarial fixture: 396pt for
    /// four realistic rows, 444 with no spaces, 348 with CJK, against a 314pt
    /// window.
    #[test]
    fn no_string_a_user_can_supply_makes_the_card_taller() {
        /// Far taller than any card, so nothing is culled and no layout is
        /// constrained -- an overflow is reported, not hidden.
        const ROOMY: f32 = 900.0;
        /// What a 1/2/3/4-row card needs, measured, spelled out.
        const NEEDED: [f32; 4] = [154.0, 204.0, 254.0, 304.0];

        let mut checked = 0;
        for fixture in FIXTURES {
            for rows in 1..=4usize {
                let choices = &four_choices()[..rows];
                let ctx = styled_ctx();
                let mut needed = f32::NAN;
                let _ = ctx.run_ui(sized(ROOMY), |ui| {
                    draw_overlay_card_rows(
                        ui,
                        fixture.app,
                        fixture.item,
                        Some(fixture.user),
                        choices,
                    );
                    needed = ui.min_rect().bottom();
                });
                assert!(
                    needed.is_finite() && needed > 0.0,
                    "the card allocated no space at all for {:?}",
                    fixture.name
                );
                assert_eq!(
                    needed, NEEDED[rows - 1],
                    "a {rows}-row card drawn with the {:?} fixture needs {needed}pt, not \
                     {}pt. The row is content-sized again: its text grew the card, and \
                     the window is a fixed {}pt with no scrollbar, no resize border and \
                     no title bar, so the difference is gone for good.",
                    fixture.name,
                    NEEDED[rows - 1],
                    overlay_height(rows)
                );
                checked += 1;
            }
        }
        assert_eq!(checked, FIXTURES.len() * 4);

        // THE CONTROL, and it is the load-bearing half: the fixtures must
        // really be strings that a wrapping row would have wrapped. Without
        // this, four short strings would pass the loop above and prove
        // nothing at all. Laid out at the width the row's text column really
        // gets, with wrapping ON -- which is what the row used to do.
        let ctx = styled_ctx();
        let width = text_column_width();
        let mut wrapped = 0;
        for fixture in FIXTURES.iter().filter(|f| f.name != SHORT.name) {
            let (_, secondary) = row_text(fixture.app, fixture.item, Some(fixture.user));
            let rows_taken = ctx.fonts_mut(|fonts| {
                fonts
                    .layout(
                        secondary.clone(),
                        egui::FontId::proportional(11.0),
                        theme::TEXT_FAINT,
                        width,
                    )
                    .rows
                    .len()
            });
            assert!(
                rows_taken > 1,
                "the {:?} fixture's secondary line ({secondary:?}) fits on one line at \
                 {width}pt even when wrapped, so it is not an adversarial fixture and the \
                 assertions above prove nothing",
                fixture.name
            );
            wrapped += 1;
        }
        assert_eq!(wrapped, FIXTURES.len() - 1);

        // ...and the short fixture is the other side of the control: it does
        // NOT wrap, which is exactly why measuring off it alone hid the bug.
        let (_, short_secondary) = row_text(SHORT.app, SHORT.item, Some(SHORT.user));
        assert_eq!(
            ctx.fonts_mut(|fonts| fonts
                .layout(
                    short_secondary,
                    egui::FontId::proportional(11.0),
                    theme::TEXT_FAINT,
                    width
                )
                .rows
                .len()),
            1
        );
    }

    /// The width the row's text column really gets, computed the way
    /// `credential_row` computes it, from the card's real geometry.
    ///
    /// Measured off the painted card rather than restated: the row tile's own
    /// width, less the tile's 10pt horizontal inner margins, less the 28pt
    /// avatar and the 2pt after it, less [`CHIP_LANE`].
    fn text_column_width() -> f32 {
        let tile = row_tiles(&painted(&four_choices()[..1], overlay_height(1)))[0];
        tile.width() - 2.0 * 10.0 - 28.0 - 2.0 - CHIP_LANE
    }

    /// [`CHIP_LANE`] against the chip that is really painted.
    ///
    /// The row's text is bounded by being given an explicit width, and that
    /// width is "everything except the chip's lane". If the lane were too
    /// narrow the `Enter` chip would be pushed past the right-hand edge of a
    /// window with no horizontal scrolling — trading a clipped row for a
    /// clipped chip is not a fix. So the chip is measured where it lands, and
    /// the text is asserted to stop before it.
    ///
    /// Both facts, and both are needed: a lane wide enough for the chip is
    /// useless if the text is allowed to run underneath it.
    #[test]
    fn the_enter_chip_has_a_lane_of_its_own_and_the_text_stops_short_of_it() {
        let mut checked = 0;
        for fixture in FIXTURES {
            let ink = painted_as(fixture, &four_choices()[..1], overlay_height(1));
            let tile = row_tiles(&ink)[0];
            let chip = ink
                .iter()
                .filter(|i| i.glyphs.as_deref() == Some("Enter"))
                .map(|i| i.rect)
                .collect::<Vec<_>>();
            assert_eq!(
                chip.len(),
                1,
                "the selected row painted {} `Enter` chips for the {:?} fixture",
                chip.len(),
                fixture.name
            );
            let chip = chip[0];
            assert!(
                fits(chip, window(overlay_height(1))),
                "the `Enter` chip paints at {chip:?} for the {:?} fixture, outside the \
                 window; the text column took the lane",
                fixture.name
            );
            // The lane really is a lane: the chip's ink starts within
            // CHIP_LANE of the tile's right-hand inner edge.
            assert!(
                chip.left() >= tile.right() - CHIP_LANE - 0.5,
                "the `Enter` chip starts at {} for the {:?} fixture, further left than the \
                 {CHIP_LANE}pt lane reserved for it (tile right edge {})",
                chip.left(),
                fixture.name,
                tile.right()
            );
            // And nothing else in the row reaches into it. The chip's own two
            // runs are excluded by rect, not by glyphs, so a *label* that
            // happened to read "Enter" is still caught.
            for run in ink.iter().filter(|i| i.glyphs.is_some()) {
                if run.rect == chip {
                    continue;
                }
                if !tile.contains(run.rect.center()) {
                    continue; // header and footer, not this row
                }
                assert!(
                    run.rect.right() <= chip.left() + 0.5,
                    "the row's text run {:?} for the {:?} fixture runs to {}, into the \
                     `Enter` chip's lane which starts at {}",
                    run.glyphs,
                    fixture.name,
                    run.rect.right(),
                    chip.left()
                );
                checked += 1;
            }
        }
        assert!(
            checked >= FIXTURES.len() * 2,
            "expected at least the two text lines of each fixture's row to have been \
             checked against the chip; only {checked} runs were"
        );
    }

    /// The header names the number of rows the card is really about to draw.
    ///
    /// It was the literal `"1 match"`, which was true only while the overlay
    /// could show exactly one row. Both halves are checked against the
    /// painted glyphs, not against the function: the string the user reads is
    /// the claim.
    #[test]
    fn the_header_counts_the_rows_the_card_actually_draws() {
        assert_eq!(match_count_label(1), "1 match");
        assert_eq!(match_count_label(2), "2 matches");
        assert_eq!(match_count_label(4), "4 matches");
        // An empty slice still paints one row, so it is still one match.
        assert_eq!(match_count_label(0.max(1)), "1 match");

        // ...and on the card itself.
        for (choices, expected) in [
            (&four_choices()[..1], "1 match"),
            (&four_choices()[..4], "4 matches"),
        ] {
            let ink = painted(choices, overlay_height(choices.len()));
            glyph_run(&ink, expected);
            let stale = if expected == "1 match" {
                "4 matches"
            } else {
                "1 match"
            };
            assert_eq!(
                ink.iter()
                    .filter(|i| i.glyphs.as_deref() == Some(stale))
                    .count(),
                0,
                "the header read {stale:?} on a {}-row card",
                choices.len()
            );
        }
        // The card production still draws with no choices at all.
        glyph_run(&painted(&[], overlay_height(1)), "1 match");
    }

    /// POSITIVE CONTROL for the test above: `card_fits_in` can say no.
    ///
    /// Without this, a `card_fits_in` that returned `Ok(())` unconditionally
    /// -- or one whose walker had gone blind -- would make the assertion above
    /// green on any window size at all, which is precisely the shape of the
    /// hole it was written to close.
    ///
    /// Both of the two ways a too-small window fails are exercised: 100pt is
    /// tall enough for a single row and still loses the footer, and a
    /// one-row-sized window loses three of four rows outright to culling.
    #[test]
    fn the_card_fit_check_can_actually_fail() {
        let mutant = 100.0;
        assert!(
            mutant < requested_inner_size(1).y,
            "the control window is not actually shorter than the real one"
        );
        let one_row_in_100 = card_fits_in(1, mutant);
        assert!(
            one_row_in_100.is_err(),
            "a one-row card was declared to fit a {mutant}pt window; its footer is at the \
             bottom of a 154pt card, so this check cannot fail and its passes mean nothing"
        );
        // ...and specifically for the reason claimed, not by accident.
        assert!(
            one_row_in_100.as_ref().unwrap_err().contains("Dismiss")
                || one_row_in_100.as_ref().unwrap_err().contains("Fill"),
            "expected the footer to be what did not fit; got {one_row_in_100:?}"
        );

        let four_rows_in_one = card_fits_in(4, overlay_height(1));
        assert!(
            four_rows_in_one.is_err(),
            "four rows were declared to fit a one-row window"
        );
        assert!(
            four_rows_in_one
                .as_ref()
                .unwrap_err()
                .contains("painted 3 row tiles"),
            "expected the fourth row to be culled and counted as missing; got \
             {four_rows_in_one:?}"
        );

        // ... and the same instrument says yes to the window production
        // really asks for, so it is discriminating on size and not simply
        // always refusing.
        assert_eq!(card_fits_in(4, requested_inner_size(4).y), Ok(()));
    }

    /// The requested window grows by exactly one row per row, measured off
    /// the builder rather than off `overlay_height`.
    ///
    /// This is the half `the_window_..._fits_the_card_it_will_draw` cannot
    /// catch on its own: a window 10pt too tall for every card still fits
    /// every card. A window that stops growing does not.
    #[test]
    fn the_requested_window_grows_by_exactly_one_row_per_choice_row() {
        let mut steps = 0;
        for rows in 2..=4usize {
            let step = requested_inner_size(rows).y - requested_inner_size(rows - 1).y;
            assert!(
                (step - ROW_HEIGHT).abs() < 0.01,
                "going from {} rows to {rows} rows changed the requested window by {step}pt, \
                 not by one {ROW_HEIGHT}pt row",
                rows - 1
            );
            steps += 1;
        }
        assert_eq!(steps, 3);
        // The floor: no choices is not a shorter window.
        assert_eq!(requested_inner_size(0).y, requested_inner_size(1).y);
        // And the belt to the measurement's braces: the requested size is
        // the shared arithmetic every other test and `app::clamp_into_work_area`
        // use, so the position clamp and the window cannot disagree.
        for rows in 0..=4usize {
            assert_eq!(
                requested_inner_size(rows),
                egui::vec2(OVERLAY_WIDTH, overlay_height(rows))
            );
        }
    }

    /// The anchor survives the extraction. `overlay_position` computes where
    /// the card goes; a builder that dropped it would open every overlay
    /// wherever Windows felt like, and no drawing test would notice.
    #[test]
    fn the_requested_window_opens_where_the_caller_anchored_it() {
        let one = &four_choices()[..1];
        assert_eq!(
            overlay_options(one, Some((640.0, 480.0))).viewport.position,
            Some(egui::pos2(640.0, 480.0))
        );
        // `None` means "let the OS pick", and must not become 0,0.
        assert_eq!(overlay_options(one, None).viewport.position, None);
        // The rest of what makes this window the overlay's window, so an
        // extraction that quietly dropped one is a failure and not a silent
        // change to a frameless always-on-top card.
        let v = overlay_options(one, None).viewport;
        assert_eq!(v.decorations, Some(false));
        assert_eq!(v.transparent, Some(true));
        assert_eq!(v.window_level, Some(egui::WindowLevel::AlwaysOnTop));
        assert!(v.icon.is_some());
    }

    /// Belt to the measurement's braces, covering the ONE line of the sizing
    /// decision a measurement cannot reach.
    ///
    /// `overlay_options` is measured directly, so everything inside it is
    /// observable. What is left over is `show_prompt_overlay`'s own body,
    /// which calls `eframe::run_native` and therefore cannot be executed by
    /// any test here: it could hand `overlay_options` an empty slice, or
    /// override the size afterwards with a second `with_inner_size`, and
    /// nothing measurable would move. That residue is exactly two source
    /// facts, and they are what this counts.
    ///
    /// Deliberately the *second* guard and not the first -- a pin proves
    /// where a string is, not what the program does. Its job here is only to
    /// keep the seam that the measurement observes attached to the seam
    /// production uses.
    #[test]
    fn nothing_but_overlay_options_sizes_the_overlay_window() {
        let source = include_str!("overlay_ui.rs");
        // Split across two literals so they cannot match their own declarations.
        let sizer = concat!("with_inner_", "size(");
        assert_eq!(
            source.matches(sizer).count(),
            1,
            "expected exactly one {sizer:?} in this module -- `overlay_options`'s, which \
             `the_window_the_overlay_actually_asks_for_fits_the_card_it_will_draw` \
             measures. A second one is a window size no test can see"
        );
        let call = concat!("overlay_options(&choices", ", anchor);");
        assert_eq!(
            source.matches(call).count(),
            1,
            "expected `show_prompt_overlay` to get its options from {call:?} exactly once, \
             passing the SAME `choices` it is about to draw; the measured size is only the \
             shipped size while it does"
        );
        // The counter's own controls: each needle finds itself exactly once,
        // and the mutations they exist for are absent.
        assert_eq!(sizer.matches(sizer).count(), 1);
        assert_eq!(call.matches(call).count(), 1);
        assert_eq!(source.matches(concat!("overlay_", "options(&[], anchor)")).count(), 0);
    }

    // ------------------------------------------------- the chrome, measured

    /// **[`CHROME_HEIGHT`] against the chrome the card actually paints.**
    ///
    /// It used to be derived from the number it was then asserted to produce:
    /// `CHROME_HEIGHT == 164.0 - ROW_HEIGHT`, checked by
    /// `assert_eq!(CHROME_HEIGHT + ROW_HEIGHT, 164.0)` -- pure arithmetic over
    /// two constants, true of any consistent pair and false of none. It could
    /// have been 40 or 240 and the suite would not have moved.
    ///
    /// So measure it, the way `ROW_HEIGHT` is measured: lay a real card out at
    /// one, two, three and four rows in a window far too big to cull anything,
    /// and ask egui how much space it took. The part that does not scale with
    /// `n` is the chrome.
    #[test]
    fn the_chrome_constant_is_the_chrome_the_card_actually_paints() {
        /// Comfortably taller than a four-row card, so nothing is culled and
        /// the layout is the unconstrained one.
        const ROOMY: f32 = 700.0;

        let mut measurements = Vec::new();
        for rows in 1..=4usize {
            let choices = &four_choices()[..rows];

            // What egui says the card needs: the bottom of the space
            // `draw_overlay_card_rows` allocated, in a Ui that starts at y = 0
            // exactly as `OverlayApp::ui`'s does.
            let ctx = styled_ctx();
            let mut needed = f32::NAN;
            let _ = ctx.run_ui(sized(ROOMY), |ui| {
                assert_eq!(ui.min_rect().top(), 0.0, "the card must start at the window's top");
                draw_overlay_card_rows(ui, APP, ITEM, Some(USER), choices);
                needed = ui.min_rect().bottom();
            });
            assert!(needed.is_finite() && needed > 0.0, "the card allocated no space");
            assert!(needed < ROOMY, "the probe window was not roomy enough to be unconstrained");

            // Where the rows landed inside it.
            let tiles = row_tiles(&painted(choices, ROOMY));
            assert_eq!(
                tiles.len(),
                rows,
                "a {rows}-row card painted {} tiles in a {ROOMY}pt window, where nothing can \
                 be culled -- the measurement below would be of the wrong card",
                tiles.len()
            );

            // The chrome, as the brief defines it: everything above the first
            // row plus everything below the last. Less one ROW_GAP, because
            // `ROW_HEIGHT` already carries the gap (see `ROW_GAP`) and the
            // region from the first row's top to the last row's bottom is
            // `n * ROW_HEIGHT - ROW_GAP`, one gap short.
            let above = tiles[0].top() - 0.0;
            let below = needed - tiles[rows - 1].bottom();
            assert!(above > 0.0, "there is no header above the first row");
            assert!(below > 0.0, "there is no footer below the last row");
            measurements.push((rows, needed, above + below - ROW_GAP));
        }
        assert_eq!(measurements.len(), 4, "the loop must have covered 1, 2, 3 and 4 rows");

        // 1. The chrome does not depend on the row count. If it did,
        //    `overlay_height` could not be `a + b*n` at all.
        for (rows, _, chrome) in &measurements {
            assert!(
                (chrome - measurements[0].2).abs() < 0.01,
                "the chrome measures {chrome}pt at {rows} rows but {}pt at 1 row",
                measurements[0].2
            );
        }
        let measured = measurements[0].2;

        // 2. It is the number the constant claims.
        assert!(
            (measured - MEASURED_CHROME).abs() < 0.5,
            "the card paints {measured}pt of chrome, but MEASURED_CHROME says \
             {MEASURED_CHROME}. Whichever moved, `overlay_height` is now describing a card \
             that is not the one being drawn"
        );

        // 3. ... and the height actually requested is that chrome plus a row
        //    per row plus the stated slack -- no more, and CRUCIALLY no less.
        //    A `>= 0.0` bound here (which is what this test used to be) cannot
        //    tell 10pt of deliberate slack from 30pt of a header that stopped
        //    being drawn.
        //
        //    Deliberately NOT `assert_eq!(CHROME_HEIGHT - MEASURED_CHROME,
        //    CHROME_SLACK)`. That is arithmetic over three constants, true of
        //    any consistent triple -- the same shape of tautology this test
        //    replaced. Every number below is measured off a real frame.
        let mut checked = 0;
        for (rows, needed, _) in &measurements {
            let requested = requested_inner_size(*rows).y;
            assert!(
                requested >= *needed,
                "a {rows}-row card needs {needed}pt and the window asks for {requested}pt"
            );
            assert!(
                (requested - needed - CHROME_SLACK).abs() < 0.5,
                "a {rows}-row card needs {needed}pt, the window asks for {requested}pt, and \
                 the difference is not the {CHROME_SLACK}pt of slack this module documents"
            );
            checked += 1;
        }
        assert_eq!(checked, 4);
    }

    /// POSITIVE CONTROL for the measurement above.
    ///
    /// A `run_ui` whose `min_rect` came back unbounded, or a `sized()` that
    /// ignored its argument, would make every "needs {n}pt" number above a
    /// constant and the whole test a tautology in a new costume. These are the
    /// four numbers it must have measured, spelled out: they are not derived
    /// from `CHROME_HEIGHT`, `MEASURED_CHROME` or `overlay_height`, and if the
    /// card's layout really does change, this is the test that says so out
    /// loud rather than the one that quietly re-derives itself.
    #[test]
    fn the_four_card_heights_the_chrome_was_measured_from() {
        let mut seen = Vec::new();
        for rows in 1..=4usize {
            let ctx = styled_ctx();
            let mut needed = f32::NAN;
            let _ = ctx.run_ui(sized(700.0), |ui| {
                draw_overlay_card_rows(ui, APP, ITEM, Some(USER), &four_choices()[..rows]);
                needed = ui.min_rect().bottom();
            });
            seen.push(needed);
        }
        assert_eq!(seen, vec![154.0, 204.0, 254.0, 304.0]);
        // ... and the window really is taller than each of them by the slack.
        assert_eq!(seen[0] + CHROME_SLACK, 164.0);
    }

    /// POSITIVE CONTROL for every "is inside the window" assertion above, and
    /// for the row COUNT assertion beside them.
    ///
    /// Without this, a `fits` that answered `true` unconditionally, or a
    /// window rect that was secretly unbounded, would make the whole module
    /// green and blind. Four rows really do overflow a window sized for one,
    /// and both instruments really do say so:
    ///
    /// * the third row is painted straddling the bottom edge, and `fits`
    ///   rejects it;
    /// * the fourth row falls entirely past the edge and egui's own culling
    ///   drops its tile from `output.shapes` altogether -- so the tile count
    ///   comes back 3 rather than 4. That is exactly the shape of the failure
    ///   `every_choice_row_is_inside_the_window_...` counts for: a row that
    ///   is off the window is not a row that is merely misplaced, it is a row
    ///   that is *not there*, and an uncounted loop would call that a pass.
    #[test]
    fn the_fit_check_can_actually_fail() {
        let short = overlay_height(1);
        let ink = painted(&four_choices(), short);
        let tiles = row_tiles(&ink);

        assert_eq!(
            tiles.len(),
            3,
            "four rows in a {short}pt window: expected the fourth to fall off the window \
             entirely and be culled, leaving 3 painted tiles; found {}",
            tiles.len()
        );
        assert!(
            tiles.len() != 4,
            "the row-count assertion cannot distinguish a four-row card from a \
             three-row one"
        );
        let straddling = tiles[2];
        assert!(
            !fits(straddling, window(short)),
            "the third row is at {straddling:?} in a {short}pt window and `fits` still \
             said yes -- the fit check cannot fail, so its passes upstairs mean nothing"
        );
        assert!(straddling.bottom() > short);
        // ... and the very same rect IS accepted by a window tall enough for
        // it, so `fits` is discriminating on geometry and not simply always
        // saying no.
        assert!(fits(straddling, window(overlay_height(4))));
    }

    /// POSITIVE CONTROL for the alpha filter. Nothing in the real card paints
    /// at alpha 0, so [`painted`]'s `retain` would be untestable dead code
    /// otherwise; this proves it discards what it claims to.
    #[test]
    fn a_shape_painted_at_alpha_zero_is_not_counted_as_ink() {
        let mut out = Vec::new();
        let transparent = egui::Shape::rect_filled(
            Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(300.0, 48.0)),
            CornerRadius::same(8),
            Color32::TRANSPARENT,
        );
        walk(&transparent, &mut out);
        assert_eq!(out.len(), 1, "the walker must see the shape at all");
        assert_eq!(out[0].alpha, 0);
        // ... and a row-shaped tile at full alpha IS counted, so the filter
        // discriminates on alpha rather than on shape.
        let mut solid_out = Vec::new();
        let solid = egui::Shape::rect_filled(
            Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(300.0, 48.0)),
            CornerRadius::same(8),
            theme::BLUE_WASH,
        );
        walk(&solid, &mut solid_out);
        assert_eq!(solid_out[0].alpha, 255);

        out.retain(|i| i.alpha > 0);
        assert!(row_tiles(&out).is_empty());
        solid_out.retain(|i| i.alpha > 0);
        assert_eq!(row_tiles(&solid_out).len(), 1);
    }

    /// **The top line is user-controlled too — on the path production
    /// actually draws today — and its `.truncate()` was covered by nothing.**
    ///
    /// `no_string_a_user_can_supply_makes_the_card_taller` drives every
    /// fixture through `four_choices()[..rows]`, and on THAT path
    /// `credential_row`'s `primary` is `choice.label()`: one of four
    /// compile-time constants. The user's own strings reached only the
    /// *secondary* line. So removing `.truncate()` from the primary label
    /// changed no measurement anywhere in this file, and the row's bound was
    /// real for a reason other than the one the code claimed.
    ///
    /// The empty-choices path is the other one, and it is the one
    /// `draw_overlay_card` takes — which is the whole of production until a
    /// choice list is wired through — and the one `OverlayApp` takes whenever
    /// `choices` is empty. There `primary` is `row_text(..).0`: the
    /// **username, or the item name when there is no username**. Both
    /// user-controlled, both on the top line, at 13pt semibold, in a
    /// frameless always-on-top window with no scrollbar.
    ///
    /// Measured the same two ways as its sibling: the card's laid-out height
    /// in a window far too big to cull or constrain anything, and the row
    /// tile really painted into the window production asks the OS for --
    /// counted BEFORE any geometry, because egui culls a shape that lands
    /// entirely off the screen rect, so a clipped row comes back as nothing
    /// at all rather than as a rect in the wrong place.
    #[test]
    fn the_no_choices_card_production_draws_today_bounds_its_user_controlled_top_line() {
        /// Far taller than any card, so nothing is culled and no layout is
        /// constrained -- an overflow is reported, not hidden.
        const ROOMY: f32 = 900.0;
        /// A no-choices card paints exactly one row, so it is the one-row
        /// height, spelled out and not derived from `overlay_height`,
        /// `CHROME_HEIGHT` or `MEASURED_CHROME`.
        const NEEDED: f32 = 154.0;

        let mut checked = 0;
        // BOTH arms of the branch, because both ship. `OverlayApp::ui` passes
        // `self.username.as_deref()`, so `None` is a real value on this path,
        // and `row_text` then puts the ITEM NAME on the top line -- just as
        // user-controlled, just as unbounded, and the arm this test's own
        // commit message names. Every fixture used to go in as `Some(..)`, so
        // the fallback was never driven at all.
        for (fixture, user) in FIXTURES
            .iter()
            .flat_map(|f| [(f, Some(f.user)), (f, None)])
        {
            // The precondition, and it is the half that makes the rest mean
            // anything: on this path the top line really is the user's own
            // string. A row that had stopped painting it would pass the
            // heights below while proving nothing.
            let expected = user.unwrap_or(fixture.item);
            let (primary, _) = row_text(fixture.app, fixture.item, user);
            assert_eq!(
                primary, expected,
                "the no-choices row's primary line is no longer the user-controlled \
                 string it is meant to be (user={user:?}), so this test is no longer \
                 about one"
            );

            // How tall the card lays out, unconstrained.
            let ctx = styled_ctx();
            let mut needed = f32::NAN;
            let _ = ctx.run_ui(sized(ROOMY), |ui| {
                draw_overlay_card(ui, fixture.app, fixture.item, user);
                needed = ui.min_rect().bottom();
            });
            assert!(
                needed.is_finite() && needed > 0.0,
                "the card allocated no space at all for {:?} (user={user:?})",
                fixture.name
            );
            assert_eq!(
                needed, NEEDED,
                "the no-choices card drawn with the {:?} fixture (user={user:?}) needs \
                 {needed}pt, not \
                 {NEEDED}pt. Its TOP line is content-sized again: the username grew the \
                 row, and the window is a fixed {}pt with no scrollbar, no resize border \
                 and no title bar, so the difference is gone for good.",
                fixture.name,
                overlay_height(1)
            );

            // ... and in the window production really asks the OS for, the row
            // and the footer are painted and inside it. Row tiles are COUNTED
            // first: a row pushed entirely below the screen rect is culled, and
            // a culled row has no rect to be outside anything.
            let height = requested_inner_size(1).y;
            let ink = painted_as_user(*fixture, user, &[], height);
            let tiles = row_tiles(&ink);
            assert_eq!(
                tiles.len(),
                1,
                "the no-choices card ({:?}, user={user:?}) in the {height}pt window \
                 production asks for painted {} row tiles, not one -- the row was culled \
                 for being off the window entirely",
                fixture.name,
                tiles.len()
            );
            assert!(
                fits(tiles[0], window(height)),
                "the no-choices row for {:?} has its clickable rect at {:?}, outside the \
                 {height}pt window",
                fixture.name,
                tiles[0]
            );
            for text in ["Enter Fill", "Esc Dismiss"] {
                let hits = ink
                    .iter()
                    .filter(|i| i.glyphs.as_deref() == Some(text))
                    .collect::<Vec<_>>();
                assert_eq!(
                    hits.len(),
                    1,
                    "expected exactly one painted run reading {text:?} for {:?} \
                     (user={user:?}); the footer is off the bottom of a window with no \
                     scrollbar",
                    fixture.name
                );
                assert!(fits(hits[0].rect, window(height)));
            }
            checked += 1;
        }
        assert_eq!(
            checked,
            FIXTURES.len() * 2,
            "control: the loop above did not run BOTH the `Some` and the `None` arm of \
             every fixture, so one half of the branch is unmeasured again"
        );

        // THE CONTROL, and it is the load-bearing half -- the one the sibling
        // test has for its secondary line and nothing had for the primary.
        // These fixtures must really be strings a wrapping 13pt semibold label
        // would have wrapped; otherwise four short names pass the loop above
        // and prove nothing. Laid out at the width the row's text column really
        // gets, in the font the label really uses, with wrapping ON -- which is
        // what an untruncated `Label` does inside a `vertical` of a set width.
        let ctx = styled_ctx();
        let width = text_column_width();
        let font = egui::FontId::new(13.0, egui::FontFamily::Name(theme::SEMIBOLD.into()));
        let mut wrapped = 0;
        // Both arms here too: a control that only ever measured the usernames
        // would say nothing about the strings the `None` arm actually paints,
        // which are the item names.
        for (fixture, user) in FIXTURES
            .iter()
            .filter(|f| f.name != SHORT.name)
            .flat_map(|f| [(f, Some(f.user)), (f, None)])
        {
            let (primary, _) = row_text(fixture.app, fixture.item, user);
            let rows_taken = ctx.fonts_mut(|fonts| {
                fonts
                    .layout(primary.clone(), font.clone(), theme::INK, width)
                    .rows
                    .len()
            });
            assert!(
                rows_taken > 1,
                "the {:?} fixture's PRIMARY line ({primary:?}, user={user:?}) fits on one \
                 line at {width}pt even when wrapped, so it is not an adversarial fixture \
                 for the top label and the assertions above prove nothing about it",
                fixture.name
            );
            wrapped += 1;
        }
        assert_eq!(wrapped, (FIXTURES.len() - 1) * 2);

        // ... and the short fixture is the other side of the control: its
        // username does NOT wrap, which is exactly why measuring off it alone
        // left the primary label's bound untested.
        for user in [Some(SHORT.user), None] {
            let (short_primary, _) = row_text(SHORT.app, SHORT.item, user);
            assert_eq!(
                ctx.fonts_mut(|fonts| fonts
                    .layout(short_primary, font.clone(), theme::INK, width)
                    .rows
                    .len()),
                1,
                "the short fixture's primary line (user={user:?}) wraps after all, so it \
                 is no longer the other side of the control"
            );
        }
    }

    /// **The key newtypes cannot be built out of a bare `bool`, by anyone.**
    ///
    /// `breach.rs`'s `Prefix`/`BaseUrl` are pinned this way and these were
    /// not, which is the whole of the second finding: `pub struct
    /// EnterPressed(pub bool)` made the swap re-expressible one level out as
    /// `EnterPressed(EscapePressed::read(&ctx).0)`, in the one function no
    /// test may execute. The type error was real and the door still opened.
    ///
    /// Three independent facts, because each alone goes blind:
    ///
    /// 1. **The declarations**, spelled out. A `pub` on either tuple field
    ///    reopens the hole exactly.
    /// 2. **No `bool` goes IN.** `: bool` anywhere in `mod keys` is a
    ///    constructor (or a setter) taking the thing the type exists to stop
    ///    callers choosing -- `pub fn new(pressed: bool)` passes fact 1 and
    ///    fact 3 and reopens the hole through the front door.
    /// 3. **The module's whole inventory**: four functions, two `Self(..)`
    ///    constructions. Anything else in here is something new that has not
    ///    been argued for.
    ///
    /// Plus a whole-file count: the tuple-struct call syntax appears exactly
    /// twice in the file, at the two declarations, so nothing anywhere --
    /// production or test -- constructs one positionally.
    #[test]
    fn the_key_newtypes_cannot_be_built_from_a_bare_bool() {
        let production = this_module_production_code();
        let keys = keys_module_source();

        for decl in [
            concat!("pub struct EnterPres", "sed(bool);"),
            concat!("pub struct EscapePres", "sed(bool);"),
        ] {
            assert!(
                keys.contains(decl),
                "the declaration is no longer {decl:?}. A `pub` on the tuple field lets any \
                 call site build an EnterPressed out of the Escape key, which is the swap \
                 this pair exists to forbid, spelled one level out"
            );
        }
        assert!(
            !keys.contains(": bool"),
            "something in `mod keys` takes a `bool` argument. A constructor that accepts the \
             flag is the private field handed back: `EnterPressed::new(EscapePressed::read(\
             &ctx).pressed())` compiles and is the original bug"
        );
        assert_eq!(
            keys.matches("fn ").count(),
            4,
            "`mod keys` no longer has exactly its four functions (two `read`, two \
             `pressed`); its source is:\n{keys}"
        );
        assert_eq!(
            keys.matches("Self(").count(),
            2,
            "`mod keys` constructs one of its newtypes somewhere other than the two \
             `read` bodies"
        );
        // Positive controls: the needles match live text rather than nothing.
        assert!(keys.contains("egui::Key::Enter"));
        assert!(keys.contains("egui::Key::Escape"));
        assert!(
            production.contains(concat!("pub use keys::{EnterPres", "sed, EscapePressed};")),
            "the two types are no longer re-exported from this module"
        );

        // And nothing, anywhere in the file, builds one positionally.
        let whole = non_comment(&this_module_source());
        for name in [
            concat!("EnterPres", "sed("),
            concat!("EscapePres", "sed("),
        ] {
            assert_eq!(
                whole.matches(name).count(),
                1,
                "{name:?} occurs {} times in this file; the only expected occurrence is the \
                 declaration. A construction anywhere else means the field is reachable",
                whole.matches(name).count()
            );
        }
    }

    /// This module's own source, read off disk.
    fn this_module_source() -> String {
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/overlay_ui.rs"))
            .expect("overlay_ui.rs is readable")
    }

    /// `source` with comment-only lines dropped.
    ///
    /// The doc comments in this file *spell out* the very constructions the
    /// guards forbid, so a guard that scanned them would fire on its own
    /// explanation.
    fn non_comment(source: &str) -> String {
        source
            .lines()
            .filter(|line| {
                let t = line.trim_start();
                !(t.starts_with("//") || t.starts_with("//!"))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// This module's **production code**: everything above the first test
    /// module, comment-only lines dropped.
    ///
    /// The cut is the `mod tests` opener rather than `#[cfg(test)]`, because
    /// `mod keys` sits above it and a `cfg(test)` attribute added inside `keys`
    /// would otherwise move the cut and hide the very thing being pinned.
    ///
    /// The literal is [`BELOW_CUT_MARKER`], `concat!`-split so that this file
    /// contains it exactly ONCE -- the occurrence the cut lands on -- which is
    /// what lets `nothing_but_gated_test_modules_lives_below_the_guards_cut`
    /// assert that the cut cannot move up.
    fn this_module_production_code() -> String {
        let source = this_module_source();
        let end = source
            .find(BELOW_CUT_MARKER)
            .expect("overlay_ui.rs has a `mod tests`");
        // Control: the cut kept the production items and dropped the tests.
        let production = non_comment(&source[..end]);
        assert!(
            production.contains("fn credential_row("),
            "the production slice lost `credential_row`, so the cut is in the wrong place"
        );
        assert!(
            !production.contains("fn row_leads_with_the_username_when_known"),
            "the production slice still contains test code"
        );
        // What is below the cut -- the half this slice throws away, and so the
        // half every source guard in this file is blind to -- is walked in full
        // by `nothing_but_gated_test_modules_lives_below_the_guards_cut`, the
        // same walk the four sibling files were given.
        //
        // It used to be checked here instead, by a two-item whitelist of `pub
        // fn` and `pub const`, each needle prefixed with a carriage return and
        // a newline. That was wrong twice over. A bare `fn`, a `pub(crate) fn`,
        // a `pub struct`, `pub enum`, `pub trait`, `impl`, `static`, `mod`,
        // `macro_rules!` or `pub use` all walked straight past it -- measured,
        // green, no warnings. And the committed blob in this repository is LF:
        // this working tree is CRLF only because this machine sets
        // `core.autocrlf=true`, and there is no `.gitattributes`, so on Linux
        // CI or any clone with `core.autocrlf=false` neither needle could ever
        // match and the check was unconditionally vacuous. The walk uses
        // `lines()`, and is asserted to give the identical answer on a
        // normalised copy of this file.
        production
    }

    /// The text of `mod keys`, comment-only lines dropped: from its `pub mod`
    /// line to the first column-zero `}` after it.
    fn keys_module_source() -> String {
        let production = this_module_production_code();
        let start = production
            .find("pub mod keys {")
            .expect("overlay_ui.rs has a `mod keys`");
        let rest = &production[start..];
        let end = rest.find("\n}").expect("`mod keys` is closed at column zero");
        rest[..end].to_string()
    }
    // -----------------------------------------------------------------
    // The region BELOW the cut -- the half no source guard here reads.
    // -----------------------------------------------------------------

    /// The `cfg` attribute that makes a module test-only, split so this
    /// constant is not itself one and cannot be found by a guard looking for
    /// the real attributes.
    const BELOW_CUT_GATE: &str = concat!("#[cfg(", "test)]");

    /// The literal every source guard in this file cuts the file at. Split for
    /// the same reason, and for one more: unsplit it would be a SECOND
    /// occurrence in this file, and the uniqueness control below could not be
    /// written at all. It WAS unsplit until this test existed, and the cut
    /// landed on the right occurrence only because that one comes first.
    const BELOW_CUT_MARKER: &str = concat!("mod te", "sts {");

    /// Column-0 lines below the cut that are the CONTENTS OF A STRING LITERAL
    /// rather than source. Each is controlled below: it must still occur in
    /// this file exactly once, so a stale entry cannot quietly widen the hole
    /// this test exists to close.
    const BELOW_CUT_STRING_LINES: &[&str] = &[];

    /// `true` for `mod NAME {`, `pub mod NAME {` and `pub(crate) mod NAME {`,
    /// and for nothing else. Deliberately exact rather than a `starts_with`:
    /// a whole module written on one line is not a module opener as far as
    /// this walk is concerned, and must fail it.
    fn below_cut_is_module_opener(line: &str) -> bool {
        let t = line.strip_prefix("pub(crate) ").unwrap_or(line);
        let t = t.strip_prefix("pub ").unwrap_or(t);
        let Some(rest) = t.strip_prefix("mod ") else {
            return false;
        };
        let Some(name) = rest.strip_suffix(" {") else {
            return false;
        };
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }

    /// The two-state walk of everything from the cut to EOF, over whatever
    /// text it is handed. Returns `(visited, modules, closes, depth)` so the
    /// caller can control it for non-vacuity.
    ///
    /// **Line-ending agnostic on purpose.** `lines()` strips a trailing
    /// carriage return, so every comparison here is against the line's real
    /// text on a CRLF working tree and on an LF one alike. What this replaced
    /// was a `contains` on needles that began with one, and the committed blob
    /// in this repository is LF -- so on any checkout without this machine's
    /// `core.autocrlf=true` it matched nothing, ever. The caller runs this
    /// over a normalised copy as well and requires the same answer.
    fn walk_below_the_cut(source: &str) -> (usize, usize, usize, usize) {
        let cut = source
            .find(BELOW_CUT_MARKER)
            .expect("the cut marker is checked by the caller");
        let mut depth = 0usize;
        // The module the cut lands ON is gated by the attribute immediately
        // above the cut, which is outside the region walked here. The caller
        // asserts that attribute is there; this `true` is that assertion's
        // other half.
        let mut gated = true;
        let mut modules = 0usize;
        let mut closes = 0usize;
        let mut visited = 0usize;
        let region = &source[cut..];
        // Byte offsets are carried alongside each line so a module opener can
        // be brace-matched and its REAL close pinned; see
        // [`crate::below_cut::match_brace`] for what that closes.
        let mut expected_close: Option<usize> = None;
        let mut at = 0usize;
        let mut numbered: Vec<(usize, &str)> = Vec::new();
        for raw in region.split_inclusive('\n') {
            numbered.push((at, raw.trim_end_matches('\n').trim_end_matches('\r')));
            at += raw.len();
        }
        for &(offset, line) in &numbered {
            visited += 1;
            if depth == 0 {
                // Between modules NOTHING is allowed but blanks, comments, the
                // gate and a module opener -- at ANY indentation, because an
                // indented `fn` at file scope is still a top-level item and a
                // column-0-only filter would miss it.
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with("//") {
                    continue;
                }
                if trimmed == BELOW_CUT_GATE {
                    gated = true;
                    continue;
                }
                assert!(
                    !line.starts_with(char::is_whitespace) && below_cut_is_module_opener(trimmed),
                    "top-level source below the cut: {line:?}. Every source guard in this file \
                     slices at {BELOW_CUT_MARKER:?} and reads only what is ABOVE it, so an item \
                     down here is read by none of them: it can duplicate a call site pinned at \
                     exactly one, reintroduce a construct banned by name, or add a second \
                     positional construction of a key newtype that the whole-file count would \
                     never see -- and the suite stays green. Move it above the test modules."
                );
                assert!(
                    gated,
                    "the module {line:?} below the cut is not {BELOW_CUT_GATE:?}-gated, so it \
                     SHIPS -- and it ships in the half of the file no source guard here reads"
                );
                gated = false;
                depth = 1;
                modules += 1;
                // Where this module REALLY ends, by brace count. Only that
                // line may be accepted as its close.
                let brace = offset
                    + line
                        .rfind('{')
                        .expect("a module opener ends in an opening brace");
                expected_close = Some(crate::below_cut::match_brace(region, brace));
            } else if !line.is_empty() && !line.starts_with(char::is_whitespace) {
                // Inside a test module every item is indented, so the only
                // column-0 line is the module's own closing brace.
                if line == "}" {
                    assert_eq!(
                        Some(offset),
                        expected_close,
                        "the column-0 `}}` at byte {offset} below the cut is not the brace \
                         that closes the module it appears to close ({expected_close:?}). \
                         The module was closed EARLIER, by an indented brace the line rule \
                         cannot see, and everything between the two was walked as if it \
                         were still module contents -- top-level items at file scope, in \
                         the half of this file no guard reads. Measured surviving the whole \
                         suite at 2202 passed / 0 failed / 0 warnings and shipping three \
                         times over in the lib's DEBUG LLVM IR."
                    );
                    expected_close = None;
                    depth = 0;
                    closes += 1;
                    continue;
                }
                assert!(
                    BELOW_CUT_STRING_LINES.contains(&line),
                    "a column-0 line inside a test module below the cut: {line:?}. Either a \
                     top-level item escaped the brace count, or this is the contents of a \
                     string literal and belongs in BELOW_CUT_STRING_LINES"
                );
            }
        }
        (visited, modules, closes, depth)
    }

    /// **Below the cut there is nothing but test-only modules, and the cut is
    /// where every guard in this file believes it is.**
    ///
    /// The walk the four sibling files were given, and the reason it is here
    /// too: this file re-introduced the weak version of it one commit later.
    /// [`this_module_production_code`] carried a two-item whitelist -- `pub
    /// fn` and `pub const`, nothing else -- in place of a walk. Measured on
    /// the commit it shipped in: `pub struct BelowTheCut(pub bool);` appended
    /// at EOF gives 1770 lib + 172 bin, 0 failed, 0 warnings. So do a bare
    /// `fn`, a `pub(crate) fn`, a `pub enum`, a `pub trait`, an `impl`, a
    /// `static`, a `mod`, a `macro_rules!` and a `pub use`. It also had no
    /// non-vacuity control of any kind: an empty tail passed it. And its two
    /// needles both began with a carriage return, so on the LF blob this
    /// repository actually stores it could not fire at all.
    ///
    /// Two things can silently empty every guard in this file, and neither
    /// changes a single guard's own text:
    ///
    /// 1. **Anything appended below the test modules is invisible to all of
    ///    them.** They read the half above the cut and nothing else.
    /// 2. **The cut can move UP.** These helpers take the FIRST occurrence of
    ///    the marker, so the marker appearing in a comment or a string above
    ///    the real test modules truncates the production half and vacates
    ///    every guard downstream of the truncation -- silently, because the
    ///    guards whose needles still fall inside go on passing.
    ///
    /// The walk closes the first; the uniqueness and anchor controls close the
    /// second.
    #[test]
    fn nothing_but_gated_test_modules_lives_below_the_guards_cut() {
        let source = this_module_source();
        let source = source.as_str();

        // 1. The cut lands where the guards think it does, and there is only
        //    one place it could land.
        assert_eq!(
            source.matches(BELOW_CUT_MARKER).count(),
            1,
            "{BELOW_CUT_MARKER:?} occurs {} times in this file. Every guard here takes the \
             FIRST one, so a second occurrence -- in a comment, in a string, in a doc \
             example -- is a cut that can move up and truncate the production half all of \
             them read",
            source.matches(BELOW_CUT_MARKER).count()
        );
        let cut = source
            .find(BELOW_CUT_MARKER)
            .expect("counted exactly one just above");
        assert!(
            cut > 0 && source.as_bytes()[cut - 1] == b'\n',
            "the cut landed in the MIDDLE of a line, so the marker was matched inside a \
             comment or a string literal rather than at a real module opener"
        );
        assert!(
            source[..cut].trim_end().ends_with(BELOW_CUT_GATE),
            "the module the cut lands on is not preceded by {BELOW_CUT_GATE:?}, so the region \
             below the cut opens with a module that SHIPS"
        );

        // 2. Positive control on WHERE the cut is: the production half must
        //    still reach the last production item in the file. Were the marker
        //    matched above the real test modules, this anchor would fall below
        //    the cut instead of just above it.
        const LAST_PRODUCTION_ITEM: &str =
            concat!("let response = row.response.", "interact(Sense::click());");
        assert_eq!(
            source.matches(LAST_PRODUCTION_ITEM).count(),
            1,
            "control: {LAST_PRODUCTION_ITEM:?} is not in this file exactly once, so it no \
             longer pins anything -- repoint it at the last production item above the test \
             modules"
        );
        let anchor = source
            .find(LAST_PRODUCTION_ITEM)
            .expect("counted just above");
        assert!(
            anchor < cut,
            "the last production item this control knows about is BELOW the cut, which means \
             the cut moved up and the production half every guard in this file reads is \
             truncated"
        );
        assert!(
            cut - anchor < 4_000,
            "the cut is more than 4000 bytes past the last production item this control knows \
             about: either production was appended below the anchor (repoint the anchor) or \
             the cut moved down"
        );

        // 3. The walk, run over an LF copy of this file and a CRLF copy of the
        //    same text, which must agree. Built BOTH ways rather than compared
        //    against the bytes on disk on purpose: this repository stores LF
        //    blobs and only `core.autocrlf=true` makes the working tree CRLF,
        //    so a control that asserted "this file is CRLF" would itself be a
        //    check that fires on one machine and fails on Linux CI -- which is
        //    the defect being closed here, wearing the other hat.
        let lf = source.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        assert_ne!(
            lf, crlf,
            "control: the two copies are the same string, so comparing the walk over them \
             compares it with itself -- this file has no line endings at all"
        );
        let as_lf = walk_below_the_cut(&lf);
        let as_crlf = walk_below_the_cut(&crlf);
        assert_eq!(
            as_lf, as_crlf,
            "the walk gives a different answer on an LF copy of this file than on a CRLF \
             one, so something in it is sensitive to line endings. That is exactly how the \
             check this replaced managed to be vacuous everywhere but on a checkout with \
             `core.autocrlf=true`: its needles began with a carriage return and the \
             committed blob is LF"
        );
        // And the file as it really is on disk, whichever of the two that is.
        let as_on_disk = walk_below_the_cut(source);
        assert!(
            as_on_disk == as_lf || as_on_disk == as_crlf,
            "this file's line endings are mixed: the walk over it agrees with neither the \
             all-LF nor the all-CRLF copy of its own text"
        );

        // 4. The walk is not vacuous, and it finished.
        let (visited, modules, closes, depth) = as_on_disk;
        assert!(
            visited > 100,
            "control: the walk visited only {visited} lines below the cut, which is not a \
             test module's worth -- the slice is empty or nearly so and this test proves \
             nothing"
        );
        assert_eq!(
            depth, 0,
            "a test module below the cut is never closed by a column-0 `}}`, so the walk ran \
             off the end of the file inside it and stopped inspecting top-level lines"
        );
        assert_eq!(
            modules,
            crate::below_cut::column_zero_module_openers(
                &source[source
                    .find(BELOW_CUT_MARKER)
                    .expect("the walk just found it")..],
            ),
            "the walk opened a different number of modules below the cut than there are \
             column-0 module openers down there. DERIVED from the source rather than pinned \
             to a digit: a bare literal plus a gated second module were two coordinated \
             edits that between them widened this control without touching a word of its \
             prose. This is a NON-VACUITY control and nothing more -- it shares the opener \
             predicate with the walk it controls, so it proves the walk really opened what \
             is there, not that the predicate is right. What catches a planted item is the \
             brace-matched close, above."
        );
        assert_eq!(
            closes, modules,
            "control: every module the walk opened must also have been closed at column 0"
        );
        for known in BELOW_CUT_STRING_LINES {
            assert_eq!(
                source.matches(known).count(),
                1,
                "control: the string-literal exception {known:?} is not in this file exactly \
                 once, so it is stale and is widening this check for nothing"
            );
        }
    }

    // -------------------------------------------------- design 3a: no match

    /// Paints the real no-match card into a window of exactly `height` points
    /// and returns its ink.
    fn painted_no_match(app_name: &str, height: f32) -> Vec<Ink> {
        let ctx = styled_ctx();
        let output = ctx.run_ui(sized(height), |ui| {
            draw_no_match_card(ui, app_name);
        });
        let mut ink = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut ink);
        }
        ink.retain(|i| i.alpha > 0);
        ink
    }

    /// The locked card, painted into a window `height` tall, and its ink.
    fn painted_locked(app_name: &str, height: f32) -> Vec<Ink> {
        let ctx = styled_ctx();
        let output = ctx.run_ui(sized(height), |ui| {
            draw_locked_card(ui, app_name);
        });
        let mut ink = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut ink);
        }
        ink.retain(|i| i.alpha > 0);
        ink
    }

    /// What egui says the **locked** card needs, laid out unconstrained.
    ///
    /// A second measurement rather than a parameter on the one below, so that
    /// each card is measured through the function it actually ships.
    fn locked_card_height(app_name: &str) -> f32 {
        const ROOMY: f32 = 700.0;
        let ctx = styled_ctx();
        let mut needed = f32::NAN;
        let _ = ctx.run_ui(sized(ROOMY), |ui| {
            assert_eq!(ui.min_rect().top(), 0.0, "the card must start at the window's top");
            draw_locked_card(ui, app_name);
            needed = ui.min_rect().bottom();
        });
        assert!(needed.is_finite() && needed > 0.0, "the card allocated no space");
        assert!(needed < ROOMY, "the probe window was not roomy enough to be unconstrained");
        needed
    }

    /// What egui says the no-match card needs, laid out unconstrained.
    fn no_match_card_height(app_name: &str) -> f32 {
        /// Comfortably taller than any card this module draws, so nothing is
        /// culled and the layout is the unconstrained one.
        const ROOMY: f32 = 700.0;
        let ctx = styled_ctx();
        let mut needed = f32::NAN;
        let _ = ctx.run_ui(sized(ROOMY), |ui| {
            assert_eq!(ui.min_rect().top(), 0.0, "the card must start at the window's top");
            draw_no_match_card(ui, app_name);
            needed = ui.min_rect().bottom();
        });
        assert!(needed.is_finite() && needed > 0.0, "the card allocated no space");
        assert!(needed < ROOMY, "the probe window was not roomy enough to be unconstrained");
        needed
    }

    /// **`NO_MATCH_ROWS` is checked against the card, in both directions.**
    ///
    /// The overlay is frameless, always-on-top, hardcoded in size, and has no
    /// `ScrollArea` anywhere -- so a control past its bottom edge is
    /// unreachable, with no title bar to drag it back and nothing to scroll.
    /// `f67bf42`'s message records three separate occasions on which a text or
    /// layout change pushed a control out of this viewport. So the constant is
    /// not chosen and left alone:
    ///
    /// * the card must **fit** `overlay_height(NO_MATCH_ROWS)` -- the
    ///   load-bearing half, and the one a growing card fails; and
    /// * it must **not fit** the next size down -- the half that stops the
    ///   constant being silently too generous, which is how a body that
    ///   stopped being drawn at all would otherwise go unnoticed.
    ///
    /// The second half is only meaningful because `overlay_height` floors at
    /// one row, so "the next size down" is a real, smaller number rather than
    /// the same one: `CHROME_HEIGHT` alone, the card with its body removed.
    #[test]
    fn the_no_match_card_fits_the_window_it_asks_for() {
        let asked = no_match_options(None)
            .viewport
            .inner_size
            .expect("the no-match viewport must request an inner size at all");
        assert_eq!(asked.x, OVERLAY_WIDTH, "the no-match card asked for a {}pt-wide window", asked.x);
        assert_eq!(
            asked.y,
            overlay_height(NO_MATCH_ROWS),
            "the window asked for is not the one NO_MATCH_ROWS describes, so every assertion \
             below is about a card the OS will never be given"
        );

        let needed = no_match_card_height(APP);
        assert!(
            needed <= asked.y,
            "the no-match card lays out {needed}pt tall and the window the overlay asks the OS \
             for is {}pt. This window is frameless and always-on-top -- no title bar, no resize \
             border, no scroll area -- so the missing {:.1}pt, and the Esc hint in it, are gone",
            asked.y,
            needed - asked.y
        );
        assert!(
            needed > asked.y - ROW_HEIGHT,
            "the no-match card lays out {needed}pt, which still fits a window one ROW_HEIGHT \
             shorter than the {}pt NO_MATCH_ROWS asks for. Either NO_MATCH_ROWS is a row too \
             generous, or the card's body has stopped being drawn -- and the `fits` bound above \
             cannot tell either of those from a card that is simply the right size",
            asked.y
        );

        // And the slack exactly, in the idiom `CHROME_SLACK` set: a one-sided
        // bound cannot tell eleven points of deliberate dead space from thirty
        // points of a body line that silently stopped being painted.
        assert_eq!(
            asked.y - needed,
            NO_MATCH_SLACK,
            "the no-match card lays out {needed}pt in a {}pt window, so the dead space at its \
             bottom is {:.1}pt rather than the recorded {NO_MATCH_SLACK}pt. A font, a margin or \
             a line changing size fails here rather than by clipping a control off a window \
             that has no scrollbar",
            asked.y,
            asked.y - needed
        );
    }

    /// How much taller than it needs to be the no-match card's window is.
    ///
    /// **Measured, not chosen**, exactly like [`CHROME_SLACK`]: the card lays
    /// out at 159pt and the window is [`overlay_height`]`(`[`NO_MATCH_ROWS`]`)`
    /// = 164pt.
    ///
    /// **It was 11.0 and is now 5.0, because the card gained a control.** 3a's
    /// footer now carries the *New login* button that opens design 3c -- the
    /// destination it did not have when 3a shipped -- and a `row_button` is
    /// taller than the hint text it sits beside. Six of the eleven points of
    /// dead space went into it, which is what dead space at the bottom of a
    /// frameless card is for. Five are left, and they are pinned exactly here
    /// rather than bounded, for the reason they always were: a one-sided bound
    /// cannot tell five points of deliberate slack from thirty points of a
    /// body line that silently stopped being painted.
    ///
    /// The number is also the reason the button is a `row_button` and not a
    /// `secondary_button`: with the taller control the card laid out at 167pt
    /// in a 164pt window, i.e. with its only dismiss hint off the bottom edge
    /// of a surface that cannot scroll.
    ///
    /// Slack in this direction is the safe direction. A window taller than its
    /// card wastes eleven points; a window shorter than its card loses the
    /// bottom of a frameless, always-on-top surface with no scrollbar and no
    /// resize border -- and the bottom is where the only dismiss hint is.
    const NO_MATCH_SLACK: f32 = 5.0;

    /// **`LOCKED_ROWS` is checked against the 3b card, in both directions**,
    /// exactly as `NO_MATCH_ROWS` is against 3a -- and it is a separate test
    /// over a separate measurement rather than a note that the two cards share
    /// a drawer. Sharing is an implementation detail this test must not
    /// assume: a `draw_locked_card` that grew a third line, or stopped drawing
    /// its body, would be invisible to 3a's test and to any assertion phrased
    /// as "they are the same".
    ///
    /// The card is 3c's height evidence in miniature and for the same reason:
    /// the overlay is frameless, always-on-top, hardcoded in size and has no
    /// `ScrollArea` anywhere, so a control past the bottom edge is
    /// unreachable, and `f67bf42` records three separate occasions on which a
    /// text change put one there.
    #[test]
    fn the_locked_card_fits_the_window_it_asks_for() {
        let asked = locked_options(None)
            .viewport
            .inner_size
            .expect("the locked viewport must request an inner size at all");
        assert_eq!(asked.x, OVERLAY_WIDTH, "the locked card asked for a {}pt-wide window", asked.x);
        assert_eq!(
            asked.y,
            overlay_height(LOCKED_ROWS),
            "the window asked for is not the one LOCKED_ROWS describes, so every assertion \
             below is about a card the OS will never be given"
        );

        let needed = locked_card_height(APP);
        assert!(
            needed <= asked.y,
            "the locked card lays out {needed}pt tall and the window the overlay asks the OS \
             for is {}pt. This window is frameless and always-on-top -- no title bar, no \
             resize border, no scroll area -- so the missing {:.1}pt, and the Esc hint in it, \
             are gone",
            asked.y,
            needed - asked.y
        );
        assert!(
            needed > asked.y - ROW_HEIGHT,
            "the locked card lays out {needed}pt, which still fits a window one ROW_HEIGHT \
             shorter than the {}pt LOCKED_ROWS asks for. Either LOCKED_ROWS is a row too \
             generous, or the card's body has stopped being drawn",
            asked.y
        );
        assert_eq!(
            asked.y - needed,
            LOCKED_SLACK,
            "the locked card lays out {needed}pt in a {}pt window, so its dead space is \
             {:.1}pt rather than the recorded {LOCKED_SLACK}pt. A font, a margin or a \
             line changing size fails here rather than by \
             clipping a control off a window that has no scrollbar",
            asked.y,
            asked.y - needed
        );
    }

    /// How much taller than it needs to be the LOCKED card's window is.
    ///
    /// **Its own constant, and no longer [`NO_MATCH_SLACK`].** The two notice
    /// cards were the same height until 3a's footer gained the *New login*
    /// button that leads to design 3c; 3b's footer deliberately does not have
    /// one (see [`NEW_LOGIN_LABEL`]), so 3a is now 159pt where 3b is still the
    /// 153pt both were, in the same 164pt window. That a separate constant was
    /// already there to take the difference is exactly the point
    /// [`LOCKED_ROWS`]'s doc makes about not aliasing [`NO_MATCH_ROWS`].
    ///
    /// **This number did not move, and that is the assertion.** 3b is drawn by
    /// the same `draw_notice_card` 3a is, so a change to the shared body would
    /// show up here as well as in [`NO_MATCH_SLACK`]; that only 3a's moved is
    /// what says the button landed in 3a's footer and nowhere else.
    const LOCKED_SLACK: f32 = 11.0;

    /// **3b offers no route to 3c, and 3a does** -- read off the two painted
    /// cards, not off the argument `draw_notice_card` is handed.
    ///
    /// This replaces `the_two_notice_cards_are_the_same_height`, which was
    /// true only while the two cards had identical footers and would now fail
    /// for a reason unrelated to what it was written to catch. What is worth
    /// asserting in its place is the thing that made them differ, and it is a
    /// safety claim rather than a layout one: design 3c ends in
    /// `VaultCache::create_item`, which needs a vault this process can open,
    /// so a *New login* button on the locked card would be an offer it cannot
    /// honour -- the same defect as the locked card's own correction, which
    /// was a card claiming something about a vault it could not read.
    #[test]
    fn the_locked_card_offers_no_new_login_button() {
        let on_3a = notice_glyphs(|ui| draw_no_match_card(ui, APP));
        let on_3b = notice_glyphs(|ui| draw_locked_card(ui, APP));

        assert_eq!(
            on_3a.iter().filter(|g| *g == NEW_LOGIN_LABEL).count(),
            1,
            "control: 3a does not paint the {NEW_LOGIN_LABEL:?} button either, so the \
             assertion below proves nothing. Painted: {on_3a:?}"
        );
        assert_eq!(
            on_3b.iter().filter(|g| *g == NEW_LOGIN_LABEL).count(),
            0,
            "the locked card paints a {NEW_LOGIN_LABEL:?} button. It leads to a form that \
             ends in a write to a vault this process cannot open, so the user is offered \
             something that cannot happen. Painted: {on_3b:?}"
        );
    }

    /// Every glyph run a notice card paints, laid out unconstrained.
    fn notice_glyphs(draw: impl Fn(&mut egui::Ui) -> OverlayAction) -> Vec<String> {
        const ROOMY: f32 = 700.0;
        let ctx = styled_ctx();
        let output = ctx.run_ui(sized(ROOMY), |ui| {
            draw(ui);
        });
        let mut ink = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut ink);
        }
        ink.retain(|i| i.alpha > 0);
        ink.into_iter().filter_map(|i| i.glyphs).collect()
    }


    /// The locked card's height is a function of the font and nothing else --
    /// and in particular **not** of `app_name`. Its second line embeds the
    /// name mid-sentence rather than at the end, which is the placement most
    /// likely to wrap, so the adversarial fixtures matter more here than on
    /// 3a.
    #[test]
    fn no_app_name_a_user_can_supply_makes_the_locked_card_taller() {
        let baseline = locked_card_height(APP);
        let mut checked = 0;
        for fixture in FIXTURES {
            let height = locked_card_height(fixture.app);
            assert_eq!(
                height, baseline,
                "the {:?} fixture's app name made the locked card {height}pt instead of \
                 {baseline}pt. The window is a fixed {}pt with no scrollbar, so the difference \
                 is clipped off the bottom -- and the bottom is where the only dismiss hint is",
                fixture.name,
                overlay_height(LOCKED_ROWS)
            );
            checked += 1;
        }
        assert_eq!(checked, FIXTURES.len(), "the loop must have covered every fixture");
    }

    // -----------------------------------------------------------------
    // Design 3c: the save-a-new-login card.
    // -----------------------------------------------------------------

    /// What egui says the 3c card needs, laid out unconstrained.
    fn save_login_card_height(app_name: &str) -> f32 {
        const ROOMY: f32 = 900.0;
        let ctx = styled_ctx();
        let mut form = SaveLoginForm::new(app_name);
        let mut needed = f32::NAN;
        let _ = ctx.run_ui(sized(ROOMY), |ui| {
            assert_eq!(ui.min_rect().top(), 0.0, "the card must start at the window's top");
            draw_save_login_card(ui, &mut form);
            needed = ui.min_rect().bottom();
        });
        assert!(needed.is_finite() && needed > 0.0, "the card allocated no space");
        assert!(needed < ROOMY, "the probe window was not roomy enough to be unconstrained");
        needed
    }

    /// How much taller than it needs to be the 3c card's window is.
    ///
    /// Measured, not chosen, exactly as [`NO_MATCH_SLACK`] and [`LOCKED_SLACK`]
    /// are, and pinned exactly rather than bounded for the same reason: a
    /// one-sided bound cannot tell deliberate dead space from a row that has
    /// stopped being drawn -- and this card has four rows to lose one of.
    const SAVE_LOGIN_SLACK: f32 = 10.0;

    /// **`SAVE_LOGIN_ROWS` is checked against the card, in both directions,
    /// and against every adversarial app name.**
    ///
    /// This is the tallest card the overlay draws -- four rows and three
    /// controls -- in a frameless, always-on-top window with a hardcoded inner
    /// size, no title bar, no resize border and no `ScrollArea` anywhere. What
    /// falls off the bottom of 3a is a dismiss hint; what falls off the bottom
    /// of this one is *Save*, or the password field the user is typing into.
    ///
    /// So, in the idiom `NO_MATCH_SLACK` set:
    ///
    /// * the card must **fit** `overlay_height(SAVE_LOGIN_ROWS)`;
    /// * it must **not fit** one [`ROW_HEIGHT`] less, which is the half that
    ///   stops the constant being silently a row too generous; and
    /// * the dead space between them is pinned **exactly**.
    ///
    /// All three, for all four fixtures: `app_name` is user-controlled and it
    /// is painted into the App row.
    #[test]
    fn the_save_login_card_fits_the_window_it_asks_for() {
        let asked = save_login_options(None)
            .viewport
            .inner_size
            .expect("the save-login viewport must request an inner size at all");
        assert_eq!(
            asked.x, OVERLAY_WIDTH,
            "the save-login card asked for a {}pt-wide window",
            asked.x
        );
        assert_eq!(
            asked.y,
            overlay_height(SAVE_LOGIN_ROWS),
            "the window asked for is not the one SAVE_LOGIN_ROWS describes, so every \
             assertion below is about a card the OS will never be given"
        );

        let mut checked = 0;
        for fixture in FIXTURES {
            let needed = save_login_card_height(fixture.app);
            assert!(
                needed <= asked.y,
                "the save-login card lays out {needed}pt tall for the {:?} fixture and the \
                 window the overlay asks the OS for is {}pt. This window is frameless and \
                 always-on-top -- no title bar, no resize border, no scroll area -- so the \
                 missing {:.1}pt are gone, and on this card that is the Save button or the \
                 password field",
                fixture.name,
                asked.y,
                needed - asked.y
            );
            assert!(
                needed > asked.y - ROW_HEIGHT,
                "the save-login card lays out {needed}pt for the {:?} fixture, which still \
                 fits a window one ROW_HEIGHT shorter than the {}pt SAVE_LOGIN_ROWS asks \
                 for. Either SAVE_LOGIN_ROWS is a row too generous, or one of the card's \
                 four rows has stopped being drawn -- and the `fits` bound above cannot \
                 tell either of those from a card that is simply the right size",
                fixture.name,
                asked.y
            );
            assert_eq!(
                asked.y - needed,
                SAVE_LOGIN_SLACK,
                "the save-login card lays out {needed}pt for the {:?} fixture in a {}pt \
                 window, so the dead space at its bottom is {:.1}pt rather than the \
                 recorded {SAVE_LOGIN_SLACK}pt. A font, a margin or a row changing size \
                 fails here rather than by clipping a control off a window that has no \
                 scrollbar",
                fixture.name,
                asked.y,
                asked.y - needed
            );
            checked += 1;
        }
        assert_eq!(checked, FIXTURES.len(), "the loop must have covered every fixture");
    }

    /// **No string a user can supply makes the 3c card taller.**
    ///
    /// The companion to the bound above, and the direct question rather than
    /// the derived one: the App row paints `app_name`, which is
    /// `app::window_label`'s answer and therefore chosen by the user or by the
    /// app they ran. The card's height must be a function of the font and the
    /// four rows, and of nothing else.
    #[test]
    fn no_app_name_a_user_can_supply_makes_the_save_login_card_taller() {
        let baseline = save_login_card_height(APP);
        let mut checked = 0;
        for fixture in FIXTURES {
            let height = save_login_card_height(fixture.app);
            assert_eq!(
                height, baseline,
                "the {:?} fixture's app name made the save-login card {height}pt instead \
                 of {baseline}pt. The window is a fixed {}pt with no scrollbar, so the \
                 difference is clipped off the bottom -- and the bottom of THIS card is \
                 the Save button",
                fixture.name,
                overlay_height(SAVE_LOGIN_ROWS)
            );
            checked += 1;
        }
        assert_eq!(checked, FIXTURES.len(), "the loop must have covered every fixture");
    }

    /// **The card does not imply a capture it did not make.**
    ///
    /// `injector::ui_automation` exposes exactly one question about a
    /// foreground window -- `window_has_password_field`, which answers a
    /// `bool`. There is no username reader, and a password field's contents
    /// are not read and must not be. So exactly one of the four rows is
    /// pre-filled, and the two that could carry a credential are empty boxes
    /// with instructions in them.
    ///
    /// This reads the painted card for both instructions, as glyphs rather
    /// than by trusting the constants, because the claim is about what the
    /// user sees.
    #[test]
    fn the_card_does_not_imply_a_capture_it_did_not_make() {
        let painted = save_login_glyphs(APP);

        for hint in [USERNAME_HINT, PASSWORD_HINT] {
            assert_eq!(
                painted.iter().filter(|g| *g == hint).count(),
                1,
                "the card does not paint {hint:?}. With the field empty and nothing saying \
                 the user has to fill it, a blank box under a `Save` button reads as a \
                 value that was captured and is being hidden. Painted: {painted:?}"
            );
        }

        // The one row that IS pre-filled is pre-filled, which is the other
        // half: a card with three empty rows and nothing pre-filled would pass
        // the loop above and be a different, equally wrong card.
        assert_eq!(
            painted.iter().filter(|g| *g == APP).count(),
            1,
            "the App row does not paint {APP:?}, so the one thing this process really can \
             know about the window is not on the card. Painted: {painted:?}"
        );
    }

    /// The Folder row **states** where the item goes, and the create really
    /// puts it there.
    ///
    /// Two halves, and both are needed: the string on the card, and the fact
    /// that `app::new_login_item` passes `None` for the folder. Either alone
    /// is a card that can drift from its own effect.
    #[test]
    fn the_folder_row_states_where_the_item_goes() {
        let painted = save_login_glyphs(APP);
        assert_eq!(
            painted.iter().filter(|g| *g == FOLDER_ROW_TEXT).count(),
            1,
            "the Folder row does not paint {FOLDER_ROW_TEXT:?}. Painted: {painted:?}"
        );
        match crate::app::new_login_item(SaveLoginForm::new(APP)) {
            crate::vault_bridge::NewItem::Login { folder_id, .. } => assert_eq!(
                folder_id, None,
                "the card says the new login is unfiled and `new_login_item` files it \
                 anyway, so the row is a statement about something that does not happen"
            ),
            other => panic!("3c created something other than a login: {other:?}"),
        }
    }

    /// **The two silences read differently on the card**, which is the half of
    /// the *Not now* / *Never* distinction the user can actually see.
    ///
    /// `app::route_save_answer` is where the two do different things; this is
    /// where they *say* different things. A card whose two silences were
    /// labelled alike would make the pure function's correctness invisible at
    /// the moment it matters.
    #[test]
    fn the_two_silences_read_differently_on_the_card() {
        assert_ne!(
            NOT_NOW_LABEL, NEVER_LABEL,
            "the two silences carry the same label, so nothing on the card tells the user \
             which of them lasts forever"
        );
        assert!(
            NEVER_LABEL.to_lowercase().contains("app"),
            "the forever answer is labelled {NEVER_LABEL:?}, which does not say it is \
             scoped to one app. A bare `Never` reads as `never ask me anything`"
        );

        // All three on the card, once each. `starts_with` rather than
        // equality for the primary, because `theme::primary_button` lays its
        // label and its `Enter` chip out as one glyph run -- and the header,
        // which is `SAVE_LOGIN_LABEL`, is excluded by name so that a card
        // which lost its Save BUTTON but kept its title cannot pass.
        let painted = save_login_glyphs(APP);
        let mut checked = 0;
        for label in [SAVE_LABEL, NOT_NOW_LABEL, NEVER_LABEL] {
            let seen = painted
                .iter()
                .filter(|g| g.starts_with(label) && *g != SAVE_LOGIN_LABEL)
                .count();
            assert_eq!(
                seen, 1,
                "the card paints a control labelled {label:?} {seen} times, not once. \
                 Painted: {painted:?}"
            );
            checked += 1;
        }
        assert_eq!(checked, 3, "control: all three answers must have been looked for");
    }

    /// **Esc is `NotNow`, Enter is `Save`, and no key at all is `Never`.**
    ///
    /// Nothing on this card binds a key to the one answer that writes to
    /// `settings.json`. A user swatting an always-on-top card away with Esc --
    /// which is what Esc does on every other overlay state -- must not thereby
    /// silence an app forever, because from that surface there is no way back.
    #[test]
    fn the_keyboard_can_reach_save_but_never_reaches_never() {
        // `keys_down` is the same reader the card itself uses: `read` is the
        // only way to make either newtype in safe Rust, in a test as in
        // production, so a swapped pair here is a type error rather than a
        // green test.
        let act = |keys: &[egui::Key]| {
            let (enter, escape) = keys_down(keys);
            save_login_keyboard_action(escape, enter)
        };

        assert_eq!(
            act(&[egui::Key::Escape]),
            SaveLoginAction::NotNow,
            "Esc did something other than `Not now`. It is the gesture a user makes at a \
             card that appeared over what they were doing, and on this card the strongest \
             answer is not undoable from the surface it was given on"
        );
        assert_eq!(
            act(&[egui::Key::Enter]),
            SaveLoginAction::Save,
            "Enter did not Save, though the card paints the `Enter` chip on the Save button"
        );
        assert_eq!(act(&[]), SaveLoginAction::None, "an idle frame answered something");
        assert_eq!(
            act(&[egui::Key::A]),
            SaveLoginAction::None,
            "an unrelated key answered the card"
        );
        // Esc outranks Enter when both arrive in one frame: the weaker answer
        // wins, which is the safe direction on a card that can write.
        assert_eq!(
            act(&[egui::Key::Escape, egui::Key::Enter]),
            SaveLoginAction::NotNow,
            "with both keys down in one frame the card took the stronger answer"
        );
        // And the claim stated directly: no combination of keys produces
        // `Never`.
        let mut probed = 0;
        for combo in [
            &[][..],
            &[egui::Key::Escape][..],
            &[egui::Key::Enter][..],
            &[egui::Key::Escape, egui::Key::Enter][..],
            &[egui::Key::A][..],
        ] {
            assert_ne!(
                act(combo),
                SaveLoginAction::Never,
                "{combo:?} silenced an app forever from the keyboard, with no way back from \
                 a frameless card that is already closing"
            );
            probed += 1;
        }
        assert_eq!(probed, 5, "control: the sweep must have run every combination");
    }

    /// Every glyph run the 3c card paints for `app_name`, laid out
    /// unconstrained.
    fn save_login_glyphs(app_name: &str) -> Vec<String> {
        const ROOMY: f32 = 900.0;
        let ctx = styled_ctx();
        let mut form = SaveLoginForm::new(app_name);
        let output = ctx.run_ui(sized(ROOMY), |ui| {
            draw_save_login_card(ui, &mut form);
        });
        let mut ink = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut ink);
        }
        ink.retain(|i| i.alpha > 0);
        ink.into_iter().filter_map(|i| i.glyphs).collect()
    }


    /// **The locked card claims nothing about whether a match exists.**
    ///
    /// This is the whole content of the correction, and it is asserted about
    /// the strings themselves rather than about the painted card, so it holds
    /// however the card is later laid out. 3a's own primary line is the
    /// forbidden phrase: if it ever appears here, the locked state has become
    /// the thing it replaced.
    #[test]
    fn the_locked_card_claims_nothing_about_a_match() {
        let (primary, secondary) = locked_text(APP);
        let (forbidden, _) = no_match_text(APP);
        for line in [&primary, &secondary] {
            assert_ne!(
                line, &forbidden,
                "the locked card is drawing 3a's own claim. A locked vault does not know \
                 whether it has a login for this app -- the match engine that would say so is \
                 exactly what the lock cleared"
            );
            for phrase in ["No saved login", "logins for", "nothing for"] {
                assert!(
                    !line.contains(phrase),
                    "the locked card says {line:?}, which contains {phrase:?} -- a claim about \
                     the contents of a vault this process cannot read"
                );
            }
        }
        assert!(
            primary.contains("locked"),
            "control: the locked card does not say it is locked, so the assertions above are \
             about a card that says nothing useful at all"
        );
        assert!(
            secondary.contains(APP),
            "control: the locked card never names the window it appeared over"
        );
    }

    /// The card's height is a function of the font and nothing else -- and in
    /// particular **not** of `app_name`, which is `window_label`'s answer and
    /// therefore a string a user or the app they ran chooses.
    ///
    /// This is the exact defect the matched card was found to have: two plain
    /// wrapping labels in a fixed-height window, measured off one short
    /// fixture. A one-row card overflowed its 164pt window at 189pt. The
    /// fixtures below are the same adversarial four.
    #[test]
    fn no_app_name_a_user_can_supply_makes_the_no_match_card_taller() {
        let baseline = no_match_card_height(APP);
        let mut checked = 0;
        for fixture in FIXTURES {
            let height = no_match_card_height(fixture.app);
            assert_eq!(
                height, baseline,
                "the {:?} fixture's app name made the no-match card {height}pt instead of \
                 {baseline}pt. The window is a fixed {}pt with no scrollbar, so the difference \
                 is clipped off the bottom -- and the bottom is where the only dismiss hint is",
                fixture.name,
                overlay_height(NO_MATCH_ROWS)
            );
            checked += 1;
        }
        assert_eq!(checked, FIXTURES.len(), "the loop must have covered every fixture");

        // Positive control on the instrument: it CAN see a taller card. A
        // measurement that returned a constant would pass the loop above
        // whatever the card did.
        assert!(
            no_match_card_height(APP) < {
                let ctx = styled_ctx();
                let mut needed = f32::NAN;
                let _ = ctx.run_ui(sized(700.0), |ui| {
                    draw_overlay_card_rows(ui, APP, ITEM, Some(USER), &four_choices());
                    needed = ui.min_rect().bottom();
                });
                needed
            },
            "control: the instrument reports the same height for a one-body no-match card and \
             a four-row matched card, so it is not measuring height at all"
        );
    }

    /// **The card says what it is for**, and it says it inside the window.
    ///
    /// Not a smoke test: the header's "No match", the body's two lines and the
    /// footer's one hint are the entire content, and the body's first line is
    /// the only place the app is named. Each is found as exactly one laid-out
    /// glyph run -- so a line elided to "No saved login for ledgerlin…" has no
    /// match and fails -- and each is asserted to be inside the window the
    /// overlay actually asks for.
    #[test]
    fn the_no_match_card_names_the_app_and_the_way_out_inside_its_window() {
        let height = overlay_height(NO_MATCH_ROWS);
        let ink = painted_no_match(APP, height);
        let win = window(height);

        let (primary, secondary) = no_match_text(APP);
        let mut checked = 0;
        for text in [NO_MATCH_LABEL, primary.as_str(), secondary.as_str(), "Esc Dismiss"] {
            let rect = glyph_run(&ink, text);
            assert!(
                fits(rect, win),
                "{text:?} is painted at {rect:?}, outside the {height}pt window the overlay \
                 asks the OS for. There is no scrollbar and no title bar to drag"
            );
            checked += 1;
        }
        assert_eq!(checked, 4, "the loop must have covered all four strings");

        // And the one string that must NOT be here: this card offers no fill,
        // so a footer promising Enter would be the card contradicting itself.
        assert!(
            !ink.iter().any(|i| i.glyphs.as_deref() == Some("Enter Fill")),
            "the no-match card's footer offers `Enter Fill`. There is no item behind this \
             window and nothing for Enter to fill"
        );
    }

    /// **The locked card says what it is for, inside its window** -- 3a's
    /// content test, run against 3b's own strings.
    ///
    /// The header is the only painted thing that distinguishes the two cards,
    /// so it is read here, and 3a's primary line is asserted absent from the
    /// paint as well as from the strings: a `draw_locked_card` that called
    /// `no_match_text` would pass every height assertion above and put the lie
    /// straight back on the screen.
    #[test]
    fn the_locked_card_names_the_app_and_the_way_out_inside_its_window() {
        let height = overlay_height(LOCKED_ROWS);
        let ink = painted_locked(APP, height);
        let win = window(height);

        let (primary, secondary) = locked_text(APP);
        let mut checked = 0;
        for text in [LOCKED_LABEL, primary.as_str(), secondary.as_str(), "Esc Dismiss"] {
            let rect = glyph_run(&ink, text);
            assert!(
                fits(rect, win),
                "{text:?} is painted at {rect:?}, outside the {height}pt window the overlay \
                 asks the OS for. There is no scrollbar and no title bar to drag"
            );
            checked += 1;
        }
        assert_eq!(checked, 4, "the loop must have covered all four strings");

        let (forbidden, _) = no_match_text(APP);
        assert!(
            !ink.iter().any(|i| i.glyphs.as_deref() == Some(forbidden.as_str())),
            "the locked card painted {forbidden:?}. That is a statement about the contents of \
             a vault this process cannot read, which is the defect 3b exists to correct"
        );
        assert!(
            !ink.iter().any(|i| i.glyphs.as_deref() == Some(NO_MATCH_LABEL)),
            "the locked card is wearing the no-match card's header, so the only painted thing \
             that tells the two apart says the wrong one"
        );
    }

    /// **Esc dismisses, as it does in every other overlay state** -- and Enter
    /// does not, because there is nothing to fill.
    ///
    /// `NoMatchApp::ui` needs an `eframe::Frame` and a real always-on-top
    /// window and can never be executed here, which is exactly why the
    /// decision is `no_match_keyboard_action` and not an `if` inside it.
    #[test]
    fn escape_dismisses_the_no_match_card_and_nothing_else_does() {
        // The same reader the card itself uses -- `EscapePressed::read` is the
        // only way to make one in safe Rust, in a test as in production.
        let pressed = |key: egui::Key| keys_down(&[key]).1;

        assert_eq!(
            no_match_keyboard_action(pressed(egui::Key::Escape)),
            OverlayAction::Dismiss,
            "Esc did not dismiss the no-match card. It is the only keyboard way out of a \
             frameless, always-on-top window that appeared over what the user was doing"
        );
        assert_eq!(
            no_match_keyboard_action(pressed(egui::Key::Enter)),
            OverlayAction::None,
            "Enter did something to a card with no item behind it"
        );
        assert_eq!(
            no_match_keyboard_action(pressed(egui::Key::A)),
            OverlayAction::None,
            "an unrelated key dismissed the card"
        );
    }

    /// The locked card's ✕ dismisses too, and it matters here for the same
    /// reason it does on 3a: this window is raised in response to ANOTHER app
    /// being foregrounded -- the case Windows' foreground lock refuses
    /// keyboard focus for -- so Esc may never arrive.
    ///
    /// Driven through the shipped `draw_locked_card` and not through the
    /// shared drawer, so that a `draw_locked_card` which discarded the
    /// drawer's answer fails here rather than shipping a card with no
    /// mouse-operable way out.
    #[test]
    fn the_locked_cards_close_control_dismisses_it() {
        let height = overlay_height(LOCKED_ROWS);
        let ctx = styled_ctx();
        let _ = ctx.run_ui(sized(height), |ui| {
            draw_locked_card(ui, APP);
        });

        let close = {
            let ink = painted_locked(APP, height);
            let segs: Vec<&Ink> = ink
                .iter()
                .filter(|i| {
                    i.glyphs.is_none()
                        && i.tile.is_none()
                        && i.rect.width() < 20.0
                        && i.rect.height() < 20.0
                        && i.rect.left() > OVERLAY_WIDTH - 60.0
                })
                .collect();
            assert!(
                !segs.is_empty(),
                "no close control was painted in the locked card's header, so this window has                  no mouse-operable way out at all"
            );
            let mut r = segs[0].rect;
            for s in &segs[1..] {
                r = r.union(s.rect);
            }
            r.center()
        };

        let press = |down: bool| egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(close),
                egui::Event::PointerButton {
                    pos: close,
                    button: egui::PointerButton::Primary,
                    pressed: down,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            ..sized(height)
        };

        let mut down = OverlayAction::None;
        let _ = ctx.run_ui(press(true), |ui| down = draw_locked_card(ui, APP));
        assert_eq!(
            down,
            OverlayAction::None,
            "the locked card dismissed on mouse-DOWN, a gesture the user can still drag away              from"
        );

        let mut up = OverlayAction::None;
        let _ = ctx.run_ui(press(false), |ui| up = draw_locked_card(ui, APP));
        assert_eq!(
            up,
            OverlayAction::Dismiss,
            "clicking the ✕ did not dismiss the locked card. With Esc not guaranteed to reach              a window raised behind the foreground lock, this leaves an always-on-top card              with no way out"
        );
    }

    /// The header's ✕ dismisses, which matters more on this card than on any
    /// other: the window is raised in response to ANOTHER app being
    /// foregrounded, which is exactly the case Windows' foreground lock
    /// refuses keyboard focus for -- so Esc may never arrive, and the ✕ is the
    /// only way out that does not depend on it.
    #[test]
    fn the_no_match_cards_close_control_dismisses_it() {
        let height = overlay_height(NO_MATCH_ROWS);
        let ctx = styled_ctx();
        // Warm-up frame: the header Frame must have been laid out once before
        // its rect can be interacted with.
        let _ = ctx.run_ui(sized(height), |ui| {
            draw_no_match_card(ui, APP);
        });

        // Where the ✕ is: the close control the matched card's header uses, at
        // the right-hand end of the header strip. Found by painting rather
        // than by arithmetic, so a moved header moves the click with it.
        let close = {
            let ink = painted_no_match(APP, height);
            let segs: Vec<&Ink> = ink
                .iter()
                .filter(|i| {
                    i.glyphs.is_none()
                        && i.tile.is_none()
                        && i.rect.width() < 20.0
                        && i.rect.height() < 20.0
                        && i.rect.left() > OVERLAY_WIDTH - 60.0
                })
                .collect();
            assert!(
                !segs.is_empty(),
                "no close control was painted in the no-match card's header, so this window \
                 has no mouse-operable way out at all"
            );
            let mut r = segs[0].rect;
            for s in &segs[1..] {
                r = r.union(s.rect);
            }
            r.center()
        };

        let press = |down: bool| egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(close),
                egui::Event::PointerButton {
                    pos: close,
                    button: egui::PointerButton::Primary,
                    pressed: down,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            ..sized(height)
        };

        let mut down = OverlayAction::None;
        let _ = ctx.run_ui(press(true), |ui| down = draw_no_match_card(ui, APP));
        assert_eq!(
            down,
            OverlayAction::None,
            "the card dismissed on mouse-DOWN. egui decides a click on the release, and a card \
             that acts on the press acts on a gesture the user can still drag away from"
        );

        let mut up = OverlayAction::None;
        let _ = ctx.run_ui(press(false), |ui| up = draw_no_match_card(ui, APP));
        assert_eq!(
            up,
            OverlayAction::Dismiss,
            "clicking the ✕ did not dismiss the no-match card. With Esc not guaranteed to \
             reach a window raised behind the foreground lock, this leaves an always-on-top \
             card with no way out"
        );
    }

    /// The card can never answer `Fill`. There is no item behind it, so the
    /// variant is unreachable **by construction** -- and this is what says so
    /// about the drawing rather than about the doc comment: no click anywhere
    /// on the card produces one.
    #[test]
    fn no_click_on_the_no_match_card_ever_answers_a_fill() {
        let height = overlay_height(NO_MATCH_ROWS);
        let ctx = styled_ctx();
        let _ = ctx.run_ui(sized(height), |ui| {
            draw_no_match_card(ui, APP);
        });

        let mut dismissals = 0;
        let mut new_logins = 0;
        let mut probed = 0;
        let mut y = 2.0;
        while y < height {
            let mut x = 2.0;
            while x < OVERLAY_WIDTH {
                let at = egui::pos2(x, y);
                let press = |down: bool| egui::RawInput {
                    events: vec![
                        egui::Event::PointerMoved(at),
                        egui::Event::PointerButton {
                            pos: at,
                            button: egui::PointerButton::Primary,
                            pressed: down,
                            modifiers: egui::Modifiers::default(),
                        },
                    ],
                    ..sized(height)
                };
                let _ = ctx.run_ui(press(true), |ui| {
                    draw_no_match_card(ui, APP);
                });
                let mut answer = OverlayAction::None;
                let _ = ctx.run_ui(press(false), |ui| answer = draw_no_match_card(ui, APP));
                match answer {
                    OverlayAction::Fill(choice) => panic!(
                        "clicking ({x}, {y}) on the no-match card answered Fill({choice:?}). \
                         There is no item behind this card, so whatever that would type comes \
                         from somewhere it must not"
                    ),
                    OverlayAction::Dismiss => dismissals += 1,
                    // The one control on this card, and it is counted rather
                    // than ignored: `new_logins > 0` below is the control that
                    // the sweep really reaches the footer button, which is
                    // where a control could be pushed out of the window.
                    OverlayAction::NewLogin => new_logins += 1,
                    OverlayAction::None => {}
                }
                probed += 1;
                x += 8.0;
            }
            y += 8.0;
        }
        assert!(probed > 100, "control: only {probed} points were clicked, which is not a sweep");
        assert!(
            dismissals > 0,
            "control: {probed} clicks across the whole card produced no Dismiss either, so this \
             sweep is not reaching the card's controls and the Fill assertion above is vacuous"
        );
        assert!(
            new_logins > 0,
            "control: {probed} clicks across the whole card never hit the `New login` button. 
             Either the sweep misses the footer, or the button is outside the window it is 
             drawn in -- which on a frameless always-on-top card with no scrollbar means the 
             one route from 3a to 3c is unreachable"
        );
    }
}

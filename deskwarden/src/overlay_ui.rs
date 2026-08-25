use crate::theme;
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke};
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
/// shadow), less one `ROW_GAP` — because the gap lives inside `ROW_HEIGHT`
/// (see `ROW_GAP`) and would otherwise be counted twice.
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

/// The window a notice card of `rows` rows asks the OS for, at `anchor`.
///
/// **Private, and it stays private.** `overlay_options`' doc explains why the
/// public entry point takes the choice list rather than a count: handing it
/// the wrong number is the entire bug that function exists to prevent, so its
/// one caller is not given the opportunity to. The no-match card has no choice
/// list to count -- it has [`crate::locked_card::LOCKED_ROWS`], a constant
/// checked against the card it really draws -- so it needs this shape, and
/// nothing outside this module may have it.
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

/// What the account picker's empty-card *New login* row says, and **the
/// only egui card that may NOT carry it**.
///
/// `crate::locked_card` does not carry it, deliberately: design 3c ends in
/// `VaultCache::create_item`, which is a write through `bw serve` against an
/// unlocked vault, so a *New login* button on the locked card would be an
/// offer the process cannot honour -- the same class of defect as the locked
/// card's own correction (a card claiming something about a vault it cannot
/// read). `locked_card::the_card_offers_only_the_unlock_it_can_honour` reads
/// that card's painted runs rather than trusting the argument.
///
/// A constant for the reason [`crate::locked_card::LOCKED_LABEL`] is one: it
/// is the string a test finds in the painted output rather than one it
/// re-spells.
pub const NEW_LOGIN_LABEL: &str = "New login";

/// What the account picker's other empty-card row says, and **the only egui
/// card that may NOT carry it**.
///
/// # Why it took three releases to draw
///
/// Design 3a as drawn in egui always had two buttons. Only *New login* was
/// drawn, because
/// *Search vault* had to reach the vault window from inside
/// `main::process_foreground_event` -- and at the time that function could
/// reach nothing that opens one. A button on a frameless, always-on-top card
/// that does nothing when clicked is worse than no button: it is the same "is
/// this thing working?" the card exists to answer, moved one click later.
///
/// **What changed is not that `main` grew an escape hatch.** It is that the
/// route back was there all along and was not being used:
/// `process_foreground_event` is called *from* `run`'s own loop, on that
/// thread, and `open_vault_window` is called three times in the same loop. So
/// the answer travels the way every other overlay answer travels -- as a
/// RETURN VALUE, up through `crate::app::handle_no_match` and
/// `process_foreground_event` -- and the loop opens the window at the one door
/// it already owns. No published environment, no background thread and no
/// second window-opening route; `crate::app::disposition` is untouched and
/// still takes only the six inputs it took before, because this is not an
/// input to the decision about which card to show. It is what the card
/// answered.
///
/// `crate::locked_card` does not carry it, for a plainer reason than the one
/// [`NEW_LOGIN_LABEL`] gives: while the vault is locked there is nothing to
/// search, and a vault window opened with a query in its box would show an
/// empty list that means "locked" and reads as "nothing found".
///
/// A constant for the reason [`crate::locked_card::LOCKED_LABEL`] is one: it
/// is the string a test finds in the painted output rather than one it
/// re-spells.
pub const SEARCH_VAULT_LABEL: &str = "Search vault";

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
/// [`locked_options`]'s sibling, public for the same reason:
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
    /// The Password row's *Generate* link: **leave this card up in spirit and
    /// open design 3d**, then come back with whatever it produced.
    ///
    /// It is an answer of this card rather than a state inside it because the
    /// overlay is one window at a time -- `OVERLAY_OPEN` refuses to stack a
    /// second -- so 3c must close for 3d to open. `crate::app::save_login_flow`
    /// is what carries the half-typed form across the gap, which is why this
    /// variant travels beside a [`SaveLoginForm`] like the other three: a
    /// user who typed a username before clicking *Generate* must not lose it.
    ///
    /// **[`crate::app::route_save_answer`] never creates an item for it.**
    /// This is not a decision about the vault; it is a decision about which
    /// card is on screen.
    Generate,
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
/// *Not now*, *Never for this app*. Reached from the account picker's empty
/// card ([`crate::picker_prompt::empty_rows`])
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
/// Every label truncates, for the reason the two notice cards that used to
/// share this file did: `app_name` is user-controlled, wrapping is what made a
/// one-row card 189pt tall in a 164pt window, and this window still cannot
/// scroll.
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
                    // A lane is reserved for the *Generate* link BEFORE the
                    // field takes what is left: `save_login_field` sizes
                    // itself from `available_width`, so a link added after it
                    // would be handed nothing and pushed off the right edge
                    // of a window that cannot scroll in that direction
                    // either.
                    ui.scope(|ui| {
                        ui.set_width((ui.available_width() - GENERATE_LINK_LANE).max(1.0));
                        // `&mut form.password` deref-coerces to the
                        // `Zeroizing`'s OWN buffer, typed into in place:
                        // there is no second `String` for the value to be
                        // copied into and left in.
                        save_login_field(ui, &mut form.password, true, PASSWORD_HINT);
                    });
                    if theme::link_label(ui, SAVE_GENERATE_LABEL, 11.0).clicked() {
                        action = SaveLoginAction::Generate;
                    }
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

/// 3c's Password-row link into design **3d**.
///
/// A link rather than a button, and inside the row rather than in the footer,
/// because it belongs to the field it fills: it is the same placement the
/// edit form's own generator has, and the footer of this card is already
/// three answers wide.
pub const SAVE_GENERATE_LABEL: &str = "Generate";

/// The horizontal lane 3c's Password row keeps clear for
/// [`SAVE_GENERATE_LABEL`].
///
/// A constant rather than a literal because it is subtracted from the
/// password field's width, and a lane too small clips the link off the right
/// edge of a window with no horizontal scroll either.
const GENERATE_LINK_LANE: f32 = 62.0;

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
    form: SaveLoginForm,
    anchor: Option<(f32, f32)>,
) -> Option<(SaveLoginAction, SaveLoginForm)> {
    if OVERLAY_OPEN.swap(true, Ordering::SeqCst) {
        log::warn!(
            "save-login overlay requested for {} while one is already open in this \
             process; ignoring rather than stacking a second window",
            form.app_name
        );
        return None;
    }

    // The card opens on the form it was HANDED, not on a fresh one: coming
    // back from design 3d, that form carries the generated password and
    // whatever username the user had already typed.
    let answered = Rc::new(RefCell::new((SaveLoginAction::NotNow, form.clone())));
    let app = SaveLoginApp { form, answered: Rc::clone(&answered) };
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
        /// The one string the two surviving cards in this file are measured
        /// against. `item` and `user` went with the matched card, whose
        /// secondary line was the only place they were concatenated.
        app: &'static str,
    }

    /// The fixture the module's numbers were originally measured from. It
    /// stays, as the control: whatever the adversarial ones do, the card the
    /// overlay has always drawn must not move.
    const SHORT: Fixture = Fixture {
        name: "short",
        app: APP,
    };

    /// Realistic, and long: a real vault item in a real organisation, and the
    /// kind of address a corporate directory hands out. Nothing exotic — this
    /// is the case the shipped card lost 82 points to.
    const LONG: Fixture = Fixture {
        name: "long realistic names",
        app: "ledgerline-production-accounting-cluster-primary-host.exe",
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
    };

    /// **Nothing to wrap at.** A word wrapper's escape hatch is a space; a
    /// single unbroken token has none, so this is the worst case for any fix
    /// that relies on wrapping rather than on a bound.
    const NO_SPACES: Fixture = Fixture {
        name: "no spaces",
        app: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.exe",
    };

    /// Every fixture the card must survive, the short one first.
    const FIXTURES: [Fixture; 4] = [SHORT, LONG, CJK, NO_SPACES];






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








    // ------------------------------------------- which row answered, and how





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


    // ------------------------------------------- the window that is asked for













    // ------------------------------------------------- the chrome, measured




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
    ///    It was six and three while `RegeneratePressed` lived here. That
    ///    reader served design 3d's Ctrl+R, and 3d is `crate::generate_prompt`
    ///    now -- a bare-Win32 card that reads `WM_KEYDOWN` and `GetKeyState`
    ///    directly and has no `egui::Context` to ask. A newtype kept alive
    ///    with no caller is a seam nothing can get wrong, which is not a
    ///    reason to keep one.
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
            production.contains(concat!(
                "pub use keys::{EnterPres",
                "sed, EscapePressed};"
            )),
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
    /// The cut is the `mod geometry_tests` opener rather than `#[cfg(test)]`,
    /// because `mod keys` sits above it and a `cfg(test)` attribute added
    /// inside `keys` would otherwise move the cut and hide the very thing being
    /// pinned. It was `mod tests` until the matched card left this file: that
    /// module's every test was about the card, so it went with it, and a cut
    /// aimed at an empty module is a cut that can be deleted by tidying.
    ///
    /// The literal is [`BELOW_CUT_MARKER`], `concat!`-split so that this file
    /// contains it exactly ONCE -- the occurrence the cut lands on -- which is
    /// what lets `nothing_but_gated_test_modules_lives_below_the_guards_cut`
    /// assert that the cut cannot move up.
    fn this_module_production_code() -> String {
        let source = this_module_source();
        let end = source
            .find(BELOW_CUT_MARKER)
            .expect("overlay_ui.rs has a `mod geometry_tests`");
        // Control: the cut kept the production items and dropped the tests.
        let production = non_comment(&source[..end]);
        assert!(
            production.contains("fn show_save_login_overlay("),
            "the production slice lost `show_save_login_overlay`, so the cut is in the wrong              place"
        );
        assert!(
            !production.contains("fn the_locked_card_fits_the_window_it_asks_for"),
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
    const BELOW_CUT_MARKER: &str = concat!("mod geometry_te", "sts {");

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
            concat!("let card = draw_save_login_", "card(ui, &mut self.form);");
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

    // ------------------------------------------------ design 3b: locked card














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
    /// Measured, not chosen, exactly as [`LOCKED_SLACK`] is, and pinned
    /// exactly rather than bounded for the same reason: a
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
    /// So, in the idiom [`LOCKED_SLACK`] set:
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

    /// **3c offers the way in to 3d**, and it is on the Password row.
    ///
    /// Read out of the painted 3c card, in the idiom
    /// `the_locked_card_offers_neither_of_the_pickers_two_offers` set: a link
    /// that is
    /// computed but not painted is a destination the user cannot reach.
    #[test]
    fn the_save_login_card_paints_the_way_into_the_generator() {
        let painted = save_login_glyphs(APP);
        assert_eq!(
            painted.iter().filter(|g| *g == SAVE_GENERATE_LABEL).count(),
            1,
            "3c does not paint exactly one {SAVE_GENERATE_LABEL:?} link, so design 3d is \
             either unreachable or offered twice. Painted: {painted:?}"
        );
        // The control: the row it belongs to is still a password field with
        // its hint, so the link did not replace what it sits beside.
        assert!(
            painted.iter().any(|g| g == PASSWORD_HINT),
            "the Generate link displaced the Password row's own hint"
        );
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




}

//! **Design 3c: the save-a-login card, in bare Win32.**
//!
//! The daemon offers to save a login for a window the vault has nothing for.
//! This is that card, and it is the sixth surface in this crate drawn with
//! `CreateWindowExW` and GDI rather than with egui -- after
//! `crate::unlock_prompt`, `crate::picker_prompt`, `crate::generate_prompt`,
//! `crate::prompt_card` and `crate::locked_card`, and for the same measured
//! reason they are.
//!
//! # Why it is not an egui window any more
//!
//! The tray daemon measures 9.9 MB with no window ever opened. The moment any
//! egui window opens it becomes ~60 MB resident and **never returns**: the
//! OpenGL driver's committed arenas survive the window's destruction and are
//! only reclaimed at process exit. The five Win32 cards already in this crate
//! measure ~1.8 MB with their window on screen. This card was the last egui
//! surface on the daemon's fill path, and `crate::overlay_ui` -- the module
//! that held it -- is gone with it.
//!
//! # The seam
//!
//! Mirrors its five siblings exactly: [`SaveLoginCalls`] is a struct of `fn`
//! pointers and [`run_with`] is the whole decision, drivable by a test with no
//! window, no vault and no desktop. `protect` runs immediately after `open` and
//! **before the first pump**, and `close` runs on every exit path including the
//! failures.
//!
//! # The password, honestly
//!
//! This card holds a plaintext password in an `EDIT` control, and the care it
//! takes is `crate::unlock_prompt`'s -- **including that module's limit, which
//! is restated here rather than quietly dropped.**
//!
//! What this module owns is `Zeroizing` end to end: the `Vec<u16>` `WM_GETTEXT`
//! copies into, the `String` built from it, and the [`SaveLoginForm::password`]
//! it travels home in. None of those reaches an allocator's free list intact.
//!
//! **There is a copy this module cannot wipe.** `WM_GETTEXT` copies *out of*
//! the `EDIT` control's own internal buffer, which comctl32 allocated and still
//! owns; `ES_PASSWORD` masks the display, not the storage. [`win32::close`]
//! overwrites the control with an equal-length run of filler before the window
//! is destroyed, and that is **best effort in the strict sense**:
//! `SetWindowTextW` is free to release the old allocation and take a new one
//! rather than overwrite in place, and nothing in the API says which it did. The
//! mitigation is real but partial, and it is described that way and not as "the
//! password is wiped".
//!
//! Nothing secret is logged, nothing secret reaches a derived `Debug` (see
//! [`SaveLoginForm`]'s hand-written one, which `debug_leak_guard` is the test
//! for), and nothing secret crosses this module's boundary except through the
//! `Zeroizing` the public entry point has always returned.
//!
//! # Screen-capture exclusion goes on the TOP-LEVEL window
//!
//! `SetWindowDisplayAffinity` is refused on a child `EDIT` with
//! `E_INVALIDARG` -- measured on `unlock_prompt`, not assumed. It goes on the
//! top-level window, which covers every child it owns, and it goes on **before
//! the first pump**, so no keystroke can reach the password field while the
//! window is still capturable.
//!
//! # Three answers, and two of them are silence
//!
//! See [`SaveLoginAction`]. Conflating *Not now* with *Never for this app* is
//! the one bug on this card a user cannot undo without finding a setting, so
//! they are three separate controls, three separate [`Event`]s and three
//! separate variants all the way out to `crate::app::route_save_answer`.

use zeroize::Zeroizing;

/// The number of `crate::app::overlay_height` choice-row pitches the caller's
/// placement is computed from.
///
/// **An approximation the window then corrects, not a size anything is drawn
/// at.** It is the argument `crate::app::save_login_arm` hands the presenter's
/// `position`, and that arithmetic is the caller's; this card's own clamp is
/// against its own height, in [`crate::prompt_card::place`], which is handed
/// `layout().window.h`.
///
/// It stays at the `3` the egui card was sized by. Writing a larger number
/// would only push the anchor further up the work area for a window whose real
/// height is already known at the moment it is placed.
pub const SAVE_LOGIN_ROWS: usize = 3;

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
/// password lives in [`SaveLoginForm`] and is moved out of it into
/// `crate::app::NewLogin`, never through here. `debug_leak_guard` is the test
/// that holds that.
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
    /// daemon shows one card at a time: 3c must close for 3d to open. So this
    /// variant travels beside a [`SaveLoginForm`] like the other three -- a
    /// user who typed a username before clicking *Generate* must not lose it.
    ///
    /// **`crate::app::route_save_answer` never creates an item for it.** This
    /// is not a decision about the vault; it is a decision about which card is
    /// on screen.
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
    pub password: Zeroizing<String>,
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
            password: Zeroizing::new(String::new()),
        }
    }
}

/// What the Folder row says, and **it is a statement rather than a picker.**
///
/// 3c as drawn offers a folder dropdown. This card does not, and the reason
/// survived the port unchanged: a folder list of any length would open into --
/// and past -- the bottom edge of a frameless, always-on-top window of a fixed
/// height with nothing to scroll, which is precisely the unreachable-control
/// failure [`layout`] exists to prevent, except that a clipped popup is
/// invisible to a height measurement of the card.
///
/// So the row states where the item will go, truthfully and without a control:
/// the new login is created unfiled (`NewItem::login(.., None)`), and the vault
/// window's edit form -- which has a scrollable pane and the whole folder list
/// -- is where it is filed.
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
/// whatever the user types into this box.
pub const PASSWORD_HINT: &str = "type the password you used";

/// The window's title.
///
/// Distinct from every other title this crate opens under, because
/// `crate::foreground::pick` is a `find` over this process's own windows and
/// this card is up alongside the tray's and the hotkey listener's helper
/// windows.
pub const SAVE_LOGIN_CARD_TITLE: &str = "Deskwarden save a login";

/// What 3c's header says.
pub const SAVE_LOGIN_LABEL: &str = "Save a login";

/// 3c's primary button.
pub const SAVE_LABEL: &str = "Save";

/// 3c's Password-row control into design **3d**.
///
/// Inside the row rather than in the footer, because it belongs to the field it
/// fills: it is the same placement the edit form's own generator has, and the
/// footer of this card is already three answers wide.
pub const SAVE_GENERATE_LABEL: &str = "Generate";

/// 3c's *silence today* answer.
pub const NOT_NOW_LABEL: &str = "Not now";

/// 3c's *silence forever* answer.
///
/// It names the app-scoped thing it does, rather than saying only "Never":
/// this is the one control on the card that writes to `settings.json`, and the
/// user has to be able to tell from the words which of the two silences they
/// are choosing.
pub const NEVER_LABEL: &str = "Never for this app";

/// The four rows' captions, top to bottom.
pub const ROW_CAPTIONS: [&str; 4] = ["App", "Username", "Password", "Folder"];

/// The `Enter` chip drawn inside *Save*.
pub const SAVE_SHORTCUT: &str = "ENTER";

/// The window handle [`run_with`] deals in.
///
/// A bare `isize` newtype, not an `HWND`, for the same reason
/// `generate_prompt::GenerateWindow` is: a decision layer a test can drive must
/// not name a type that only exists behind a Win32 feature gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveWindow(pub isize);

/// What the user did with the window.
///
/// **No secret reaches this type**, and no typed text does either: what the
/// user put in the two boxes is read by [`SaveLoginCalls::take_form`], on the
/// one line of [`run_with`] that ends the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The header ✕, *Not now*, or Escape. **The weakest answer the card
    /// offers**, which is what dismissing a card has to mean.
    Cancel,
    /// The window went away underneath us. Treated exactly as `Cancel`.
    Closed,
    /// *Save*, or Enter.
    Save,
    /// *Never for this app*. **Its own variant**, and never a flag on
    /// `Cancel`.
    Never,
    /// The Password row's *Generate*.
    Generate,
}

/// The Win32 half, as `fn` pointers so [`run_with`] can be driven without a
/// desktop. Nothing here decides anything; every decision lives in
/// [`run_with`].
pub struct SaveLoginCalls {
    /// Lays out and shows the card for `form`, anchored at `anchor`. `None` if
    /// it could not be put on screen.
    ///
    /// **It takes the whole form, not a name**: coming back from design 3d the
    /// card must open on the username the user already typed and the password
    /// 3d just produced, with the caret where a user would expect it.
    pub open: fn(&SaveLoginForm, Option<(f32, f32)>) -> Option<SaveWindow>,
    /// `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on the **top-level**
    /// window, called before the first `next` -- see the module doc. Windows
    /// refuses it on a child control with `E_INVALIDARG`, and the top-level
    /// flag covers every child it owns.
    pub protect: fn(SaveWindow) -> bool,
    /// Pumps until the user does something.
    pub next: fn(SaveWindow) -> Event,
    /// Reads the two boxes out into buffers this process owns.
    ///
    /// The password comes back in a `Zeroizing`, which is the same wrapper it
    /// then travels home in -- there is no intermediate `String` for a copy to
    /// be left in.
    pub take_form: fn(SaveWindow) -> (String, Zeroizing<String>),
    /// Destroys the window, releases its resources and **scrubs the password
    /// control** -- see the module doc for what that does and does not achieve.
    pub close: fn(SaveWindow),
}

/// **The whole decision, and the only part of this module a test can run.**
///
/// 1. `protect` runs immediately after `open` and before the first `next`. This
///    card holds a plaintext password in a text box, so that ordering is not a
///    nicety: it is the window being excluded from capture before anything can
///    be typed into it.
/// 2. **Every exit path reads the form first.** All four answers travel beside
///    what the user typed, including *Never* -- the card is one window and the
///    boxes stop existing the moment it closes, so a path that skipped the read
///    would be a path that silently discarded a typed password. What is then
///    *done* with the form is `crate::app::route_save_answer`'s decision, and
///    it creates an item for exactly one of the four.
/// 3. `close` runs on every exit path, including `open` succeeding and `protect`
///    refusing. `open` returning `None` returns before ever calling it, because
///    there is no window to close there.
/// 4. **The three answers stay three.** `Cancel` is `NotNow`, `Never` is
///    `Never`, `Save` is `Save`; there is no place in this function where two of
///    them meet.
pub fn run_with(
    calls: &SaveLoginCalls,
    form: SaveLoginForm,
    anchor: Option<(f32, f32)>,
) -> Option<(SaveLoginAction, SaveLoginForm)> {
    let Some(window) = (calls.open)(&form, anchor) else {
        log::warn!("the save-a-login card could not be put on screen");
        return None;
    };

    // Before the first pump, so nothing on the card -- least of all the
    // password box it is about to offer -- is on screen while the window is
    // still capturable.
    if !(calls.protect)(window) {
        log::warn!(
            "SetWindowDisplayAffinity was refused for the save-a-login card; the password \
             typed into it is visible to screen capture on this machine"
        );
    }

    // One pump, one answer. Unlike 3d -- whose *New*, chips and stepper all
    // leave the card up -- every control on this card ends it, so there is no
    // state for a loop to carry between two events.
    let action = match (calls.next)(window) {
        // Esc, the ✕ and *Not now* are one answer, and it is the weakest one.
        // A user swatting a card away has not asked for a persistent setting.
        Event::Cancel | Event::Closed => SaveLoginAction::NotNow,
        Event::Save => SaveLoginAction::Save,
        Event::Never => SaveLoginAction::Never,
        Event::Generate => SaveLoginAction::Generate,
    };
    let (username, password) = (calls.take_form)(window);
    (calls.close)(window);
    Some((action, SaveLoginForm { app_name: form.app_name, username, password }))
}

/// **Puts design 3c on screen and answers what the user decided together with
/// what they typed.**
///
/// The signature `overlay_ui::show_save_login_overlay` had, so
/// `crate::app::REAL_OVERLAY` changes by one path and nothing else.
///
/// `None` is not an answer -- it is "the card could not be put on screen". A
/// user who dismisses it answers [`SaveLoginAction::NotNow`], which is a
/// decision and is spelled as one.
pub fn show_save_login_card(
    form: SaveLoginForm,
    anchor: Option<(f32, f32)>,
) -> Option<(SaveLoginAction, SaveLoginForm)> {
    ask_with(&REAL, form, anchor)
}

/// [`show_save_login_card`], told which [`SaveLoginCalls`] to use.
///
/// `examples/save_login_preview.rs` is its one non-production caller, swapping
/// [`SaveLoginCalls::protect`] for a stub so the window can be screenshotted.
pub fn ask_with(
    calls: &SaveLoginCalls,
    form: SaveLoginForm,
    anchor: Option<(f32, f32)>,
) -> Option<(SaveLoginAction, SaveLoginForm)> {
    run_with(calls, form, anchor)
}

/// The production [`SaveLoginCalls`].
pub static REAL: SaveLoginCalls = SaveLoginCalls {
    open: win32::open,
    protect: win32::protect,
    next: win32::next,
    take_form: win32::take_form,
    close: win32::close,
};

// ---------------------------------------------------------------------------
// Layout
//
// Logical pixels, at 100%, every one of them read off `theme` or off the five
// Win32 cards this one sits beside. Numbers invented here would be a second
// layout that has to agree with a first, which is this codebase's standing
// defect shape.
// ---------------------------------------------------------------------------

/// The card's width, and so the window's. The same
/// [`crate::picker_prompt::WIDTH`], because it is the same kind of card in the
/// same place on screen and two frameless daemon cards of different widths read
/// as two different programs.
pub const WIDTH: i32 = 380;

/// Content inset, and the top margin.
const MARGIN_X: i32 = 14;
const MARGIN_TOP: i32 = 12;

/// The width of 3c's caption column -- the design's `width: 80px`, less the
/// margin this card's rows already carry.
///
/// A **fixed** width rather than a sized-to-content one, which is what keeps
/// the four rows' bodies aligned with each other and, more importantly, keeps
/// each row's height independent of its caption.
const LABEL_W: i32 = 74;

/// The height of one of 3c's four rows -- the design's `height: 32px` input
/// box, which is what sets the row's pitch whether or not the row holds one.
///
/// **Every row is this tall, including the two that hold no field.** That is
/// what makes the card's height a function of four rows rather than of what
/// happens to be in them, and it is why an `app_name` of any length cannot grow
/// it: the App row's text is drawn `DT_END_ELLIPSIS` on one line.
const ROW_H: i32 = 30;

/// The vertical gap between two of 3c's four rows.
const ROW_GAP: i32 = 8;

/// The *Generate* control's lane in the Password row, and the gap that keeps it
/// off the field.
///
/// A constant rather than a literal because it is subtracted from the password
/// field's width, and a lane too small clips the control off the right edge of a
/// window that cannot scroll in that direction either.
const GENERATE_W: i32 = 78;
const GENERATE_GAP: i32 = 8;

/// The height of the in-row *Generate* control. Shorter than a footer button,
/// because it sits inside a [`ROW_H`] row.
const GENERATE_H: i32 = 26;

/// Button height. `theme::BUTTON_HEIGHT`, pinned by
/// [`tests::the_cards_dimensions_are_the_themes`].
const BUTTON_H: i32 = 32;

/// The footer's three answers. *Save* carries its `ENTER` chip inside itself,
/// so it is wider than its label needs; *Never for this app* is the longest
/// label on the card.
const SAVE_W: i32 = 106;
const NOT_NOW_W: i32 = 78;
const NEVER_W: i32 = 132;
const FOOTER_GAP: i32 = 8;

/// How far inside the painted field box the `EDIT` child sits, and how tall it
/// is. `unlock_prompt`'s and `picker_prompt`'s numbers, so the caret sits off
/// the border rather than against it.
const FIELD_INSET_X: i32 = 10;
const FIELD_TEXT_H: i32 = 20;

/// One rectangle of the card, in logical pixels from the window's top left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Box2 {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Box2 {
    pub fn bottom(self) -> i32 {
        self.y + self.h
    }
    pub fn right(self) -> i32 {
        self.x + self.w
    }
}

/// Every rectangle the card paints, computed once.
///
/// Pure arithmetic with no Win32 in it, for `prompt_card::layout`'s reason: a
/// control whose bottom edge fell past the window's would simply be invisible on
/// a window that neither scrolls nor resizes, and that is a property worth
/// asserting without opening anything. This is by far the tallest card in the
/// crate and the one with the most to lose -- a control past the bottom edge
/// here is *Save*, or the password field itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub window: Box2,
    /// The brand lockup's shield, and the wordmark beside it. **Not optional.**
    /// Four cards lost the lockup in porting and it had to be restored
    /// afterwards; a frameless always-on-top window that offers to put a
    /// password into the user's vault has to say whose window it is.
    pub mark: Box2,
    pub wordmark: Box2,
    pub title: Box2,
    pub close_glyph: Box2,
    pub header_rule: Box2,
    pub footer_rule: Box2,
    /// The tinted band the footer's three answers sit on.
    pub footer: Box2,
    /// The four caption boxes, in [`ROW_CAPTIONS`] order.
    pub captions: [Box2; 4],
    /// The App row's read-only value.
    pub app_value: Box2,
    /// The two painted field boxes. The `EDIT` children sit inside them; see
    /// [`field_child`].
    pub username: Box2,
    pub password: Box2,
    pub generate: Box2,
    /// The Folder row's read-only value.
    pub folder_value: Box2,
    pub save: Box2,
    pub not_now: Box2,
    pub never: Box2,
}

/// **The card's geometry. There is exactly one shape.**
///
/// The card has no rows to count, no modes and no second step: it is four
/// fixed-height rows and three answers, and nothing about it varies at runtime.
/// The window is sized to this content and to nothing else.
pub fn layout() -> Layout {
    let content_w = WIDTH - 2 * MARGIN_X;

    let lockup = crate::win32_draw::card_lockup();
    let mark = Box2 { x: MARGIN_X, y: MARGIN_TOP, w: lockup.mark_w, h: lockup.mark_h };
    let wordmark =
        Box2 { x: mark.right() + lockup.gap, y: MARGIN_TOP, w: lockup.word_w, h: lockup.mark_h };
    // The ✕ sits on the lockup's line, which is where every card header in the
    // design carries it.
    let close_glyph = Box2 { x: WIDTH - MARGIN_X - 20, y: MARGIN_TOP - 2, w: 20, h: 20 };
    let title =
        Box2 { x: MARGIN_X, y: mark.bottom() + lockup.gap_below, w: content_w - 24, h: 21 };
    let header_rule = Box2 { x: 0, y: title.bottom() + 10, w: WIDTH, h: 1 };

    let rows_top = header_rule.bottom() + 10;
    let row_y = |i: i32| rows_top + i * (ROW_H + ROW_GAP);
    let value_x = MARGIN_X + LABEL_W;
    let value_w = WIDTH - MARGIN_X - value_x;

    let captions = [
        Box2 { x: MARGIN_X, y: row_y(0), w: LABEL_W, h: ROW_H },
        Box2 { x: MARGIN_X, y: row_y(1), w: LABEL_W, h: ROW_H },
        Box2 { x: MARGIN_X, y: row_y(2), w: LABEL_W, h: ROW_H },
        Box2 { x: MARGIN_X, y: row_y(3), w: LABEL_W, h: ROW_H },
    ];
    let app_value = Box2 { x: value_x, y: row_y(0), w: value_w, h: ROW_H };
    let username = Box2 { x: value_x, y: row_y(1), w: value_w, h: ROW_H };
    // The *Generate* lane is taken out BEFORE the field takes what is left, so
    // a control added after it cannot be pushed off the right edge of a window
    // that has no horizontal scroll either.
    let password = Box2 {
        x: value_x,
        y: row_y(2),
        w: value_w - GENERATE_GAP - GENERATE_W,
        h: ROW_H,
    };
    let generate = Box2 {
        x: password.right() + GENERATE_GAP,
        y: row_y(2) + (ROW_H - GENERATE_H) / 2,
        w: GENERATE_W,
        h: GENERATE_H,
    };
    let folder_value = Box2 { x: value_x, y: row_y(3), w: value_w, h: ROW_H };

    let footer_rule = Box2 { x: 0, y: folder_value.bottom() + 10, w: WIDTH, h: 1 };
    let save = Box2 { x: MARGIN_X, y: footer_rule.bottom() + 10, w: SAVE_W, h: BUTTON_H };
    let not_now =
        Box2 { x: save.right() + FOOTER_GAP, y: save.y, w: NOT_NOW_W, h: BUTTON_H };
    let never =
        Box2 { x: not_now.right() + FOOTER_GAP, y: save.y, w: NEVER_W, h: BUTTON_H };

    let height = save.bottom() + MARGIN_TOP;
    let window = Box2 { x: 0, y: 0, w: WIDTH, h: height };
    let footer =
        Box2 { x: 0, y: footer_rule.bottom(), w: WIDTH, h: height - footer_rule.bottom() };

    Layout {
        window,
        mark,
        wordmark,
        title,
        close_glyph,
        header_rule,
        footer_rule,
        footer,
        captions,
        app_value,
        username,
        password,
        generate,
        folder_value,
        save,
        not_now,
        never,
    }
}

/// Where the `EDIT` child sits inside the field box `at` the parent paints.
///
/// A function rather than four literals at the two call sites, because the
/// window creates the children and the paint path draws the boxes around them,
/// and the two agreeing is the whole of the field looking like this app's.
pub fn field_child(at: Box2) -> Box2 {
    Box2 {
        x: at.x + FIELD_INSET_X,
        y: at.y + (at.h - FIELD_TEXT_H) / 2,
        w: at.w - 2 * FIELD_INSET_X,
        h: FIELD_TEXT_H,
    }
}

// ---------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------

/// Whether the window has gone away underneath the pump.
static GONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What the window procedure recorded, taken by `next` rather than read, so no
/// event can be delivered twice.
static PENDING: std::sync::Mutex<Option<Event>> = std::sync::Mutex::new(None);

/// The app name the card paints in its App row. **Not a secret**, but it is the
/// name of an app this user was in front of, and nothing needs it once the card
/// is down.
static APP_NAME: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// # Why every pixel here is painted by hand
///
/// `crate::unlock_prompt`'s `win32` module carries the whole argument and it is
/// not restated: a themed control renders in the shell's grey with the shell's
/// font, and the last raw-Win32 surface in this project was deleted for looking
/// foreign rather than for being broken. Every button here is a real `BUTTON`
/// window -- which is what buys focus, the space bar and `IsDialogMessage`
/// traversal -- with its painting taken over completely and handed to
/// [`crate::win32_draw`], the module all six cards draw through so none can
/// drift from the palette. The two text boxes are real `EDIT` controls, because
/// comctl32's own procedure is what draws the caret, the selection, the
/// horizontal scroll and the IME; what makes them look like this app's is the
/// box the parent paints around them and `WM_CTLCOLOREDIT`.
///
/// # GDI only
///
/// Nothing here creates a Direct2D or Direct3D device. That is measured rather
/// than stylistic: an egui window was measured at ~102 MB and a D2D device at
/// 53.85 MB against the Win32 prompt's 1.79 MB.
///
/// # GDI object hygiene
///
/// Every brush, pen, font and DC created below is restored and deleted before
/// its function returns. This is a daemon's repaint path, and a leaked handle
/// here exhausts the table over a session rather than over a run.
mod win32 {
    use super::{
        field_child, Box2, Event, SaveLoginForm, SaveWindow, APP_NAME, FOLDER_ROW_TEXT, GONE,
        NEVER_LABEL, NOT_NOW_LABEL, PASSWORD_HINT, PENDING, ROW_CAPTIONS, SAVE_GENERATE_LABEL,
        SAVE_LABEL, SAVE_LOGIN_CARD_TITLE, SAVE_LOGIN_LABEL, SAVE_SHORTCUT, USERNAME_HINT,
    };
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicI32, AtomicIsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        AddFontMemResourceEx, BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
        CreateFontIndirectW, CreatePen, CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW,
        EndPaint, FillRect, GetDC, GetDeviceCaps, InvalidateRect, ReleaseDC, RoundRect,
        SelectObject, SetBkColor, SetBkMode, SetTextColor, CLEARTYPE_QUALITY, DT_END_ELLIPSIS,
        DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FW_BOLD, FW_NORMAL, HBRUSH, HDC, HFONT,
        LOGFONTW, LOGPIXELSX, PAINTSTRUCT, PS_SOLID, SRCCOPY, TRANSPARENT,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GetClientRect, GetDlgItem, GetWindowLongPtrW, IsDialogMessageW, LoadCursorW, PeekMessageW,
        RegisterClassW, SendMessageW, SetForegroundWindow, SetWindowDisplayAffinity,
        SetWindowLongPtrW, SetWindowTextW, ShowWindow, TranslateMessage, BN_CLICKED,
        BS_PUSHBUTTON, CS_HREDRAW, CS_VREDRAW, ES_AUTOHSCROLL, ES_PASSWORD, GWLP_WNDPROC, HMENU,
        IDC_ARROW, MSG, PM_REMOVE, SW_SHOW, WDA_EXCLUDEFROMCAPTURE, WINDOW_EX_STYLE,
        WINDOW_STYLE, WM_COMMAND, WM_CTLCOLOREDIT, WM_DESTROY, WM_ERASEBKGND, WM_GETTEXT,
        WM_GETTEXTLENGTH, WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_NCHITTEST, WM_PAINT, WM_QUIT,
        WM_SETFONT, WNDCLASSW, WS_CHILD, WS_EX_TOPMOST, WS_POPUP, WS_TABSTOP, WS_VISIBLE,
    };

    use crate::win32_draw::{draw_button_with_shortcut, draw_card_lockup, rgb, ButtonSkin};

    use zeroize::Zeroizing;

    const ID_USERNAME: usize = 101;
    const ID_PASSWORD: usize = 102;
    const ID_GENERATE: usize = 103;
    const ID_SAVE: usize = 104;
    const ID_NOT_NOW: usize = 105;
    const ID_NEVER: usize = 106;

    /// `EM_SETSEL`, and the two `EDIT` focus notifications. The `windows` crate
    /// does not project the `EDIT` control's messages or notification codes
    /// under the features this crate enables, so they are the documented
    /// constants, named here rather than left as bare hex literals at the call
    /// -- exactly as `unlock_prompt` names `EM_SETSEL` and `picker_prompt` names
    /// `EN_CHANGE`.
    const EM_SETSEL: u32 = 0x00B1;
    const EN_SETFOCUS: u32 = 0x0100;
    const EN_KILLFOCUS: u32 = 0x0200;

    const CLASS_NAME: PCWSTR = w!("DeskwardenSaveLoginCard");

    /// The window's DPI as a percentage of 96, sampled once per open.
    ///
    /// **The system DPI, not the monitor's**, and a known limitation rather than
    /// an oversight -- `unlock_prompt`'s own `DPI_PERCENT` carries the whole
    /// argument: `GetDpiForWindow` lives behind a `windows` crate feature this
    /// crate does not enable, and enabling it re-pins `job_object.rs`'s
    /// whole-file hash of `Cargo.toml`.
    static DPI_PERCENT: AtomicI32 = AtomicI32::new(100);

    fn scale(v: i32) -> i32 {
        v * DPI_PERCENT.load(Ordering::SeqCst) / 100
    }

    /// Which control the pointer is over, as a control id, or 0.
    static HOVERED: AtomicIsize = AtomicIsize::new(0);

    /// The subclassed `BUTTON`s' original procedure. One slot for all of them:
    /// every button here is the same `BUTTON` class registered by the same
    /// comctl32, so the procedure it replaces is the same pointer.
    static BUTTON_PROC: AtomicIsize = AtomicIsize::new(0);

    /// The subclassed `EDIT`s' original procedure. **A second slot, and not the
    /// one above**: an `EDIT` is a different window class with a different
    /// procedure, and calling a `BUTTON`'s procedure for it would hand every
    /// keystroke to code that has no buffer to put it in.
    static EDIT_PROC: AtomicIsize = AtomicIsize::new(0);

    // ---- fonts -------------------------------------------------------------

    /// Registers the bundled Archivo cuts privately with GDI, once.
    ///
    /// `AddFontMemResourceEx` makes a face available to **this process only** --
    /// nothing is installed and nothing touches the user's font list -- and the
    /// handles are deliberately never released, because freeing one while a
    /// window still has it selected is how a surface repaints in the fallback
    /// face.
    fn register_fonts() {
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| unsafe {
            for (_, _, _, bytes) in crate::theme::ARCHIVO_FACES {
                let installed = std::cell::Cell::new(0u32);
                let handle = AddFontMemResourceEx(
                    bytes.as_ptr() as *const c_void,
                    bytes.len() as u32,
                    None,
                    installed.as_ptr(),
                );
                if handle.0.is_null() || installed.get() == 0 {
                    log::warn!("could not register a bundled Archivo face with GDI");
                }
            }
        });
    }

    fn font(family: &str, px: i32) -> HFONT {
        let (face, weight) = crate::theme::gdi_face_for(family);
        unsafe {
            let mut lf = LOGFONTW {
                lfHeight: -scale(px),
                lfWeight: if weight >= 700 { FW_BOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
                lfQuality: CLEARTYPE_QUALITY,
                ..Default::default()
            };
            for (i, ch) in face.encode_utf16().take(31).enumerate() {
                lf.lfFaceName[i] = ch;
            }
            CreateFontIndirectW(&lf)
        }
    }

    fn mono(px: i32) -> HFONT {
        unsafe {
            let mut lf = LOGFONTW {
                lfHeight: -scale(px),
                lfWeight: FW_NORMAL.0 as i32,
                lfQuality: CLEARTYPE_QUALITY,
                ..Default::default()
            };
            for (i, ch) in crate::theme::GDI_MONO_FACE.encode_utf16().take(31).enumerate() {
                lf.lfFaceName[i] = ch;
            }
            CreateFontIndirectW(&lf)
        }
    }

    /// Every face the card paints with, created at open and destroyed at close.
    /// Kept together so `close` cannot leak one by forgetting it.
    struct Fonts {
        brand: HFONT,
        title: HFONT,
        caption: HFONT,
        /// The two `EDIT`s' face, and the App row's value: the same size, so a
        /// pre-filled row and a typed one read as the same kind of thing.
        field: HFONT,
        button: HFONT,
        hint: HFONT,
    }

    impl Fonts {
        fn build() -> Self {
            use crate::theme::{BOLD, REGULAR, SEMIBOLD};
            Fonts {
                brand: font(BOLD, crate::win32_draw::card_lockup().word_px),
                title: font(BOLD, 15),
                caption: font(REGULAR, 11),
                field: font(REGULAR, 12),
                button: font(SEMIBOLD, 12),
                hint: mono(crate::theme::CHIP_TEXT_PX as i32),
            }
        }

        fn destroy(&self) {
            unsafe {
                for f in [self.brand, self.title, self.caption, self.field, self.button, self.hint]
                {
                    let _ = DeleteObject(f);
                }
            }
        }
    }

    static FONTS: Mutex<Option<Fonts>> = Mutex::new(None);
    // `Fonts` holds raw GDI handles, which are process-wide rather than
    // thread-owned. The card is modal on one thread, so nothing shares them; the
    // `Mutex` is only what lets them live in a `static` beside a window
    // procedure that has nowhere else to keep state.
    unsafe impl std::marker::Send for Fonts {}

    // ---- the window --------------------------------------------------------

    pub(super) fn open(
        form: &SaveLoginForm,
        anchor: Option<(f32, f32)>,
    ) -> Option<SaveWindow> {
        register_fonts();
        GONE.store(false, Ordering::SeqCst);
        HOVERED.store(0, Ordering::SeqCst);
        if let Ok(mut slot) = APP_NAME.lock() {
            slot.clear();
            slot.push_str(&form.app_name);
        }
        if let Ok(mut slot) = PENDING.lock() {
            *slot = None;
        }

        unsafe {
            DPI_PERCENT.store(
                {
                    let dc = GetDC(None);
                    let dpi = GetDeviceCaps(dc, LOGPIXELSX);
                    ReleaseDC(None, dc);
                    if dpi > 0 {
                        dpi * 100 / 96
                    } else {
                        100
                    }
                },
                Ordering::SeqCst,
            );
        }

        register_class();
        // **Destroy the previous set before overwriting it.** `Fonts` has no
        // `Drop` -- it holds raw `HFONT`s -- so assigning over a `Some` would
        // leak six fonts per `open` that ran without a matching `close`.
        {
            let mut slot = FONTS.lock().ok()?;
            if let Some(previous) = slot.take() {
                previous.destroy();
            }
            *slot = Some(Fonts::build());
        }

        let l = super::layout();
        let (w, h) = (scale(l.window.w), scale(l.window.h));
        let (x, y) = placed(anchor, w, h);

        let window = unsafe {
            CreateWindowExW(
                // Topmost, because it is a question asked over whatever the user
                // was doing. It takes focus deliberately: this card is TYPED
                // into, which is the property that separates it from the
                // matched-item card it is a sibling of.
                WS_EX_TOPMOST,
                CLASS_NAME,
                &HSTRING::from(SAVE_LOGIN_CARD_TITLE),
                // Frameless. A `WS_CAPTION` frame is the loudest "system dialog"
                // signal there is, and this app's own windows are frameless with
                // drawn chrome.
                WS_POPUP | WS_VISIBLE,
                x,
                y,
                w,
                h,
                None,
                None,
                None,
                None,
            )
        }
        .ok()?;

        round_corners(window);

        // **Below this line the card is on screen.** `WS_VISIBLE` is in the
        // style, so a bare `?` here would return `None`, make `run_with` answer
        // `None`, and leave a frameless topmost card with no controls and no way
        // for the user to dismiss it -- `close` is only reached with a
        // `SaveWindow` in hand. Every failure path from here on goes through
        // `abandon`, which takes the window down and frees the fonts before
        // answering `None`.
        fn abandon(window: HWND) -> Option<SaveWindow> {
            unsafe {
                let _ = DestroyWindow(window);
            }
            if let Ok(mut slot) = FONTS.lock() {
                if let Some(fonts) = slot.take() {
                    fonts.destroy();
                }
            }
            if let Ok(mut slot) = APP_NAME.lock() {
                slot.clear();
            }
            None
        }

        // The handles are copied out and the guard dropped at the end of this
        // statement: `abandon` locks `FONTS` itself, so holding the guard across
        // the `child` calls below would deadlock the failure path.
        let Some((field_font, button_font)) =
            FONTS.lock().ok().and_then(|guard| guard.as_ref().map(|f| (f.field, f.button)))
        else {
            return abandon(window);
        };

        // The two text boxes. Borderless, because the box around each is painted
        // by the parent in the app's colours; `ES_PASSWORD` on the second masks
        // the *display* only -- see the module doc on what that does not do.
        let Some(username) = child(
            window,
            w!("EDIT"),
            WS_TABSTOP.0 | ES_AUTOHSCROLL as u32,
            field_child(l.username),
            ID_USERNAME,
            field_font,
        ) else {
            return abandon(window);
        };
        let Some(password) = child(
            window,
            w!("EDIT"),
            WS_TABSTOP.0 | ES_AUTOHSCROLL as u32 | ES_PASSWORD as u32,
            field_child(l.password),
            ID_PASSWORD,
            field_font,
        ) else {
            return abandon(window);
        };
        subclass(username, &EDIT_PROC, edit_proc);
        subclass(password, &EDIT_PROC, edit_proc);

        // The card opens on the form it was HANDED, not on a fresh one: coming
        // back from design 3d, that form carries the generated password and
        // whatever username the user had already typed. `SetWindowTextW` rather
        // than a `CreateWindowExW` caption so the password never becomes an
        // `HSTRING` living longer than this statement.
        unsafe {
            if !form.username.is_empty() {
                let _ = SetWindowTextW(username, &HSTRING::from(form.username.as_str()));
            }
            if !form.password.is_empty() {
                let text = HSTRING::from(form.password.as_str());
                let _ = SetWindowTextW(password, &text);
            }
        }

        let buttons: [(usize, Box2, HFONT); 4] = [
            (ID_GENERATE, l.generate, field_font),
            (ID_SAVE, l.save, button_font),
            (ID_NOT_NOW, l.not_now, button_font),
            (ID_NEVER, l.never, button_font),
        ];
        for (id, at, face) in buttons {
            let Some(control) = child(
                window,
                w!("BUTTON"),
                WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
                at,
                id,
                face,
            ) else {
                return abandon(window);
            };
            subclass(control, &BUTTON_PROC, control_proc);
        }

        unsafe {
            let _ = ShowWindow(window, SW_SHOW);
            // Allowed to refuse, and handled rather than asserted -- the
            // property `foreground` records. A refusal leaves a topmost card on
            // screen that the user clicks once to focus.
            let _ = SetForegroundWindow(window);
            // The caret starts in the row the user has to fill in first. Coming
            // back from 3d with a username already typed it starts in the
            // password row instead, which is the row that just changed.
            let first = if form.username.is_empty() { username } else { password };
            let _ = SetFocus(first);
            SendMessageW(first, EM_SETSEL, WPARAM(0), LPARAM(-1));
        }

        Some(SaveWindow(handle_of(window)))
    }

    /// **The protection, on the top-level window.**
    ///
    /// Applied to the card itself and never to a child: Windows refuses
    /// `SetWindowDisplayAffinity` on a child control with `E_INVALIDARG`, and
    /// the top-level flag covers every child it owns. What it protects here is a
    /// **password being typed in**, in a box that is masked on screen and would
    /// not be masked against a keylogger's screen recorder catching the caret
    /// beside a known field.
    pub(super) fn protect(window: SaveWindow) -> bool {
        unsafe { SetWindowDisplayAffinity(hwnd(window.0), WDA_EXCLUDEFROMCAPTURE).is_ok() }
    }

    /// Pumps until the user does something.
    ///
    /// **This blocks.** It does not return until the window procedure has
    /// recorded an event or the window has gone away, and the event it hands
    /// back is *taken* out of `PENDING` rather than read from it -- so no event
    /// can be delivered twice.
    ///
    /// **`IsDialogMessageW` is what makes Tab move between the two boxes at
    /// all.** Escape and Enter are handled before it: `IsDialogMessage` only
    /// cancels for a real dialog box, and its idea of a default button is a
    /// `DM_GETDEFID` this window does not answer -- so Enter is read here, where
    /// it means the one thing the footer's chip says it means.
    pub(super) fn next(window: SaveWindow) -> Event {
        use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_RETURN};
        use windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN;

        let top = hwnd(window.0);
        loop {
            if GONE.load(Ordering::SeqCst) {
                return Event::Closed;
            }
            if let Some(event) = take_pending() {
                return event;
            }

            let mut msg = MSG::default();
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        GONE.store(true, Ordering::SeqCst);
                        return Event::Closed;
                    }
                    // **Esc is `Cancel`, and there is no key on this card bound
                    // to `Never`.** A user swatting a card away has not asked
                    // for a persistent setting, and "Never" is not undoable from
                    // this surface.
                    if msg.message == WM_KEYDOWN && msg.wParam.0 as u16 == VK_ESCAPE.0 {
                        return Event::Cancel;
                    }
                    if msg.message == WM_KEYDOWN && msg.wParam.0 as u16 == VK_RETURN.0 {
                        return Event::Save;
                    }
                    if !IsDialogMessageW(top, &msg).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    if GONE.load(Ordering::SeqCst) {
                        return Event::Closed;
                    }
                    if let Some(event) = take_pending() {
                        return event;
                    }
                }
            }
            // Idle. Nothing on this card animates, so this is a plain wait for
            // the next message rather than a frame tick.
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    }

    /// Copies the two boxes out into buffers this process owns.
    ///
    /// The username is an ordinary `String`; the password's UTF-16 buffer and
    /// the `String` built from it are both `Zeroizing`, so neither is handed to
    /// an allocator's free list intact. See the module doc for the copy this
    /// **cannot** reach -- comctl32's own buffer, which `close` asks
    /// `SetWindowTextW` to overwrite and which it is free to reallocate around
    /// instead.
    pub(super) fn take_form(window: SaveWindow) -> (String, Zeroizing<String>) {
        let top = hwnd(window.0);
        let username = unsafe {
            match GetDlgItem(top, ID_USERNAME as i32) {
                Ok(field) => {
                    let len = SendMessageW(field, WM_GETTEXTLENGTH, WPARAM(0), LPARAM(0)).0;
                    if len <= 0 {
                        String::new()
                    } else {
                        let mut buf = vec![0u16; len as usize + 1];
                        let copied = SendMessageW(
                            field,
                            WM_GETTEXT,
                            WPARAM(buf.len()),
                            LPARAM(buf.as_mut_ptr() as isize),
                        )
                        .0
                        .max(0) as usize;
                        String::from_utf16_lossy(&buf[..copied.min(len as usize)])
                    }
                }
                Err(_) => String::new(),
            }
        };
        let password = unsafe {
            match GetDlgItem(top, ID_PASSWORD as i32) {
                Ok(field) => {
                    let len = SendMessageW(field, WM_GETTEXTLENGTH, WPARAM(0), LPARAM(0)).0;
                    if len <= 0 {
                        Zeroizing::new(String::new())
                    } else {
                        let mut buf: Zeroizing<Vec<u16>> =
                            Zeroizing::new(vec![0u16; len as usize + 1]);
                        let copied = SendMessageW(
                            field,
                            WM_GETTEXT,
                            WPARAM(buf.len()),
                            LPARAM(buf.as_mut_ptr() as isize),
                        )
                        .0
                        .max(0) as usize;
                        Zeroizing::new(String::from_utf16_lossy(
                            &buf[..copied.min(len as usize)],
                        ))
                    }
                }
                Err(_) => Zeroizing::new(String::new()),
            }
        };
        (username, password)
    }

    /// Overwrites the password control's own buffer with an equal-length run of
    /// filler.
    ///
    /// **Best effort in the strict sense.** `SetWindowTextW` may overwrite in
    /// place or may release the old allocation and take a new one; the API does
    /// not say, and nothing here can find out. Equal length is what makes the
    /// in-place case likely rather than certain. Called from [`close`], so it
    /// runs on every exit path including the successful one.
    fn scrub_password(top: HWND) {
        unsafe {
            let Ok(field) = GetDlgItem(top, ID_PASSWORD as i32) else { return };
            let len = SendMessageW(field, WM_GETTEXTLENGTH, WPARAM(0), LPARAM(0)).0.max(0) as usize;
            if len > 0 {
                let filler = HSTRING::from("\u{2022}".repeat(len));
                let _ = SetWindowTextW(field, &filler);
            }
            let _ = SetWindowTextW(field, w!(""));
        }
    }

    pub(super) fn close(window: SaveWindow) {
        let top = hwnd(window.0);
        scrub_password(top);
        unsafe {
            let _ = DestroyWindow(top);
        }
        if let Ok(mut slot) = FONTS.lock() {
            if let Some(fonts) = slot.take() {
                fonts.destroy();
            }
        }
        if let Ok(mut slot) = PENDING.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = APP_NAME.lock() {
            slot.clear();
        }
    }

    // ---- plumbing ----------------------------------------------------------

    fn handle_of(h: HWND) -> isize {
        h.0 as isize
    }

    fn hwnd(h: isize) -> HWND {
        HWND(h as *mut c_void)
    }

    fn repaint(window: HWND) {
        unsafe {
            let _ = InvalidateRect(window, None, false);
        }
    }

    /// The card and every button on it.
    fn repaint_all(window: HWND) {
        repaint(window);
        unsafe {
            for id in [ID_GENERATE, ID_SAVE, ID_NOT_NOW, ID_NEVER] {
                if let Ok(control) = GetDlgItem(window, id as i32) {
                    repaint(control);
                }
            }
        }
    }

    fn take_pending() -> Option<Event> {
        PENDING.lock().ok().and_then(|mut slot| slot.take())
    }

    fn set_pending(event: Event) {
        if let Ok(mut slot) = PENDING.lock() {
            *slot = Some(event);
        }
    }

    fn app_name() -> String {
        APP_NAME.lock().map(|slot| slot.clone()).unwrap_or_default()
    }

    /// Where the window goes: the anchor the caller computed, clamped onto the
    /// work area against **this card's own height** -- through
    /// [`crate::prompt_card::place`], the crate's one placement function, so the
    /// six cards cannot drift into six clamps.
    fn placed(anchor: Option<(f32, f32)>, w: i32, h: i32) -> (i32, i32) {
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{
                SystemParametersInfoW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
            };
            let mut area = RECT::default();
            let ok = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut area as *mut _ as *mut c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
            if ok.is_err() || area.right <= area.left {
                return (200, 200);
            }
            crate::prompt_card::place(
                anchor,
                (area.left, area.top, area.right, area.bottom),
                w,
                h,
            )
        }
    }

    fn register_class() {
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| unsafe {
            let class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                lpszClassName: CLASS_NAME,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                // No background brush: `WM_ERASEBKGND` is answered and the whole
                // client area is painted from one back buffer, which is what
                // keeps the card from flashing system grey on a repaint.
                hbrBackground: HBRUSH::default(),
                ..Default::default()
            };
            RegisterClassW(&class);
        });
    }

    /// One child control. **`BUTTON`s are created with no text**: every label on
    /// this card is painted by `paint_control` from the app's own palette and
    /// type, so a control's own caption would only ever be a second, stale copy.
    fn child(
        parent: HWND,
        class: PCWSTR,
        style: u32,
        at: Box2,
        id: usize,
        font: HFONT,
    ) -> Option<HWND> {
        let h = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!(""),
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style),
                scale(at.x),
                scale(at.y),
                scale(at.w),
                scale(at.h),
                parent,
                HMENU(id as *mut c_void),
                None,
                None,
            )
        }
        .ok()?;
        unsafe {
            SendMessageW(h, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
        }
        Some(h)
    }

    fn round_corners(window: HWND) {
        unsafe {
            use windows::Win32::Graphics::Dwm::{
                DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
            };
            let preference = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                window,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &preference as *const _ as *const c_void,
                std::mem::size_of_val(&preference) as u32,
            );
        }
    }

    /// The card's white as a brush, for `WM_CTLCOLOREDIT`.
    ///
    /// A `OnceLock` and never deleted, exactly as `unlock_prompt::card_brush`
    /// is: the value returned from `WM_CTLCOLOREDIT` is a handle the system
    /// keeps using after the handler returns, so a brush created per message and
    /// deleted would be a use-after-free -- and one created per message and not
    /// deleted would leak one GDI object per repaint of a control the user is
    /// typing into.
    fn card_brush() -> HBRUSH {
        static BRUSH: OnceLock<isize> = OnceLock::new();
        HBRUSH(
            *BRUSH.get_or_init(|| unsafe { CreateSolidBrush(rgb(crate::theme::CARD)).0 as isize })
                as *mut c_void,
        )
    }

    /// Takes over a control's painting without losing the focus and keyboard
    /// behaviour that makes `IsDialogMessage` work.
    fn subclass(
        control: HWND,
        slot: &AtomicIsize,
        proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
    ) {
        unsafe {
            let previous = SetWindowLongPtrW(control, GWLP_WNDPROC, proc as *const () as isize);
            if previous != 0 {
                slot.store(previous, Ordering::SeqCst);
            }
        }
    }

    /// Calls whatever procedure `slot` replaced, or `DefWindowProcW` if there is
    /// none.
    unsafe fn original(
        slot: &AtomicIsize,
        control: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let previous = slot.load(Ordering::SeqCst);
        if previous == 0 {
            DefWindowProcW(control, msg, wparam, lparam)
        } else {
            CallWindowProcW(
                Some(std::mem::transmute::<
                    isize,
                    unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
                >(previous)),
                control,
                msg,
                wparam,
                lparam,
            )
        }
    }

    // ---- the window procedures ---------------------------------------------

    unsafe extern "system" fn wnd_proc(
        window: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint(window);
                LRESULT(0)
            }
            // The two fields sit inside boxes the parent painted, so their own
            // background has to be the card's white rather than the system's.
            WM_CTLCOLOREDIT => {
                let hdc = HDC(wparam.0 as *mut c_void);
                SetBkColor(hdc, rgb(crate::theme::CARD));
                SetTextColor(hdc, rgb(crate::theme::INK));
                LRESULT(card_brush().0 as isize)
            }
            // Frameless windows are dragged by their background.
            WM_NCHITTEST => {
                // **The close glyph is the one part of the background that is
                // not a title bar.** It is painted by this window rather than
                // being a child control, so answering `HTCAPTION` for the whole
                // client area turned every press on it into a window drag and
                // `WM_LBUTTONDOWN` below never fired -- the reported "clicking
                // on X doesn't work". See `win32_draw::frameless_hit`, which is
                // the pure half of this and the half the pin decides.
                crate::win32_draw::frameless_hit_test(
                    window,
                    DefWindowProcW(window, msg, wparam, lparam),
                    lparam,
                    close_glyph_rect(),
                )
            }
            WM_LBUTTONDOWN => {
                // The ✕ is `Cancel`, not `Never`: closing a card is the weakest
                // answer a user can give it, and reading it as "forever" is the
                // bug this card's three answers exist to avoid.
                if in_close_glyph(lparam) {
                    set_pending(Event::Cancel);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                // A pointer that left a control without entering another one is
                // seen here rather than by the control it left.
                if HOVERED.swap(0, Ordering::SeqCst) != 0 {
                    repaint_all(window);
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = (wparam.0 & 0xffff) as usize;
                let notification = ((wparam.0 >> 16) & 0xffff) as u32;
                if notification == BN_CLICKED {
                    clicked(id);
                }
                // The focus halo around a field box is painted by the PARENT, so
                // the parent is what has to repaint when the caret moves in or
                // out of one.
                if notification == EN_SETFOCUS || notification == EN_KILLFOCUS {
                    repaint(window);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                // **NO `PostQuitMessage` HERE, EVER.** This window is opened on
                // the daemon thread, and that thread goes on to open more
                // windows -- design 3d's generator, and this card again when 3d
                // comes back. `close()` calls `DestroyWindow`, which dispatches
                // this message synchronously on that thread, so a
                // `PostQuitMessage` here leaves the thread's quit flag set with
                // nothing left to drain it: `next()` has already returned and no
                // pump of ours runs again. The next window's pump then takes
                // that stale `WM_QUIT` out of its queue and leaves before it
                // draws a frame.
                //
                // Quitting is not this handler's job in the first place: `GONE`
                // on the line below is what `next()` reads to report
                // `Event::Closed`, and the `WM_QUIT` branch in `next()` stays
                // for a quit posted from outside.
                GONE.store(true, Ordering::SeqCst);
                LRESULT(0)
            }
            _ => DefWindowProcW(window, msg, wparam, lparam),
        }
    }

    /// **What a click on control `id` means, and the three answers stay three.**
    ///
    /// Each of the footer's buttons posts its own event. There is no shared
    /// "dismiss" path for two of them to meet on, which is what makes "*Not now*
    /// is not *Never*" a property of the wiring rather than of a comment.
    fn clicked(id: usize) {
        match id {
            ID_GENERATE => set_pending(Event::Generate),
            ID_SAVE => set_pending(Event::Save),
            ID_NOT_NOW => set_pending(Event::Cancel),
            ID_NEVER => set_pending(Event::Never),
            _ => {}
        }
    }

    /// The subclassed `BUTTON`s: everything except painting and hover is the
    /// original procedure's, which is what keeps focus, the space bar and
    /// `IsDialogMessage`'s traversal working.
    unsafe extern "system" fn control_proc(
        control: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let id = GetWindowLongPtrW(control, windows::Win32::UI::WindowsAndMessaging::GWLP_ID);
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint_control(control, id as usize);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if HOVERED.swap(id, Ordering::SeqCst) != id {
                    repaint(control);
                }
                LRESULT(0)
            }
            _ => original(&BUTTON_PROC, control, msg, wparam, lparam),
        }
    }

    /// The subclassed `EDIT`s, and **the only thing this takes over is the
    /// placeholder.**
    ///
    /// comctl32 paints the text, the caret, the selection and the horizontal
    /// scroll, and none of that is reimplemented here: the original procedure
    /// runs first and does all of it. What is added afterwards, and only when the
    /// control is empty, is the hint the design draws in the box -- which comctl32
    /// has no notion of under the plain `EDIT` class.
    ///
    /// **Nothing secret is read here.** The branch asks the control its text
    /// *length*, never its text, and it draws one of two compile-time constants.
    unsafe extern "system" fn edit_proc(
        control: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let result = original(&EDIT_PROC, control, msg, wparam, lparam);
        if msg == WM_PAINT {
            let id = GetWindowLongPtrW(control, windows::Win32::UI::WindowsAndMessaging::GWLP_ID)
                as usize;
            let empty = SendMessageW(control, WM_GETTEXTLENGTH, WPARAM(0), LPARAM(0)).0 <= 0;
            if empty {
                paint_placeholder(control, id);
            }
        }
        result
    }

    /// The hint drawn over an empty field, after comctl32 has painted it.
    fn paint_placeholder(control: HWND, id: usize) {
        let hint = if id == ID_PASSWORD { PASSWORD_HINT } else { USERNAME_HINT };
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(control, &mut rc);
            let hdc = GetDC(control);
            if hdc.is_invalid() {
                return;
            }
            let guard = FONTS.lock();
            if let Some(fonts) = guard.as_ref().ok().and_then(|slot| slot.as_ref()) {
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, rgb(crate::theme::TEXT_FAINT));
                let old = SelectObject(hdc, fonts.field);
                let mut chars: Vec<u16> = hint.encode_utf16().collect();
                let mut at = rc;
                DrawTextW(
                    hdc,
                    &mut chars,
                    &mut at,
                    DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS,
                );
                SelectObject(hdc, old);
            }
            drop(guard);
            ReleaseDC(control, hdc);
        }
    }

    /// The close glyph's rect in DEVICE pixels.
    ///
    /// One derivation, read by both the hit test and `in_close_glyph`, so the
    /// rect `WM_NCHITTEST` excuses from the drag and the rect `WM_LBUTTONDOWN`
    /// answers on can never be two different rectangles.
    fn close_glyph_rect() -> RECT {
        let l = super::layout();
        RECT {
            left: scale(l.close_glyph.x),
            top: scale(l.close_glyph.y),
            right: scale(l.close_glyph.right()),
            bottom: scale(l.close_glyph.bottom()),
        }
    }

    fn in_close_glyph(lparam: LPARAM) -> bool {
        crate::win32_draw::on_close_glyph(
            (lparam.0 & 0xffff) as i16 as i32,
            ((lparam.0 >> 16) & 0xffff) as i16 as i32,
            close_glyph_rect(),
        )
    }

    // ---- painting ----------------------------------------------------------

    /// The card's own surface: the header, the two hairlines, the footer's tint,
    /// the four captions, the two read-only values and the two field boxes.
    /// Every button paints itself.
    fn paint(window: HWND) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(window, &mut ps);
            let mut client = RECT::default();
            let _ = GetClientRect(window, &mut client);
            let (w, h) = (client.right, client.bottom);

            // Double-buffered: a surface painted straight to the window flickers
            // on every hover.
            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            let old = SelectObject(mem, bmp);

            let guard = FONTS.lock();
            let fonts = guard.as_ref().ok().and_then(|slot| slot.as_ref());
            let l = super::layout();

            // The window IS the card, so its whole client area is `theme::CARD`.
            fill_rect(mem, client, crate::theme::CARD);
            fill_box(mem, l.footer, crate::theme::CARD_TINT);
            fill_box(mem, l.header_rule, crate::theme::HAIRLINE);
            fill_box(mem, l.footer_rule, crate::theme::HAIRLINE);
            SetBkMode(mem, TRANSPARENT);

            // The two field boxes, and their focus halos. The `EDIT`s are
            // children sitting inside these, painted by comctl32 in the colours
            // `WM_CTLCOLOREDIT` hands them -- the same division of labour
            // `unlock_prompt` and `picker_prompt` draw their fields with.
            for (id, at) in [(ID_USERNAME, l.username), (ID_PASSWORD, l.password)] {
                let focused = GetDlgItem(window, id as i32)
                    .map(|control| GetFocus() == control)
                    .unwrap_or(false);
                if focused {
                    rounded(
                        mem,
                        Box2 { x: at.x - 2, y: at.y - 2, w: at.w + 4, h: at.h + 4 },
                        9,
                        crate::theme::FOCUS_RING,
                        None,
                    );
                }
                rounded(
                    mem,
                    at,
                    8,
                    crate::theme::CARD,
                    Some((1, crate::theme::BORDER)),
                );
            }

            if let Some(fonts) = fonts {
                paint_lockup(mem, &l, fonts.brand);
                text(mem, fonts.title, l.title, SAVE_LOGIN_LABEL, crate::theme::INK);

                for (caption, at) in ROW_CAPTIONS.iter().zip(l.captions.iter()) {
                    text(mem, fonts.caption, *at, caption, crate::theme::TEXT_FAINT);
                }
                // **The App row is the one pre-filled row**, and its value is
                // user-controlled: it is drawn on one `DT_END_ELLIPSIS` line, so
                // no name a user can supply can make this card taller.
                text(
                    mem,
                    fonts.field,
                    // `+ FIELD_INSET_X`, not a literal: the read-only rows'
                    // text has to start on the same column the two `EDIT`
                    // children's does, and that column is the one
                    // `field_child` insets them by.
                    Box2 { x: l.app_value.x + super::FIELD_INSET_X, ..l.app_value },
                    &app_name(),
                    crate::theme::INK,
                );
                text(
                    mem,
                    fonts.caption,
                    Box2 { x: l.folder_value.x + super::FIELD_INSET_X, ..l.folder_value },
                    FOLDER_ROW_TEXT,
                    crate::theme::TEXT_FAINT,
                );
            }

            paint_close_glyph(mem, l.close_glyph);

            drop(guard);
            let _ = BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            let _ = EndPaint(window, &ps);
        }
    }

    /// One child button.
    fn paint_control(control: HWND, id: usize) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(control, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(control, &mut rc);

            let hovered = HOVERED.load(Ordering::SeqCst) == id as isize;
            let focused = GetFocus() == control;

            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, rc.right, rc.bottom);
            let old = SelectObject(mem, bmp);
            let whole = RECT { left: 0, top: 0, right: rc.right, bottom: rc.bottom };

            let guard = FONTS.lock();
            let fonts = guard.as_ref().ok().and_then(|slot| slot.as_ref());
            let l = super::layout();
            let dpi = DPI_PERCENT.load(Ordering::SeqCst);

            // The footer's three answers sit on the tint, the Password row's
            // *Generate* on the card -- otherwise a button's rounded corners
            // show the wrong colour through them.
            let under = if id == ID_GENERATE {
                crate::theme::CARD
            } else {
                crate::theme::CARD_TINT
            };
            fill_rect(mem, whole, under);
            SetBkMode(mem, TRANSPARENT);

            if let Some(fonts) = fonts {
                let (label, hint, skin, face, box2) = control_skin(id, &l, fonts);
                let skin = if hovered { skin.hovered() } else { skin };
                let hint = hint.map(|text| (text, fonts.hint));
                let radius = if id == ID_GENERATE { 7 } else { 8 };
                if focused {
                    // **The ring is given LOGICAL size, from `layout`.**
                    // `rounded` scales everything it is handed, and `rc` came
                    // back from `GetClientRect` in device pixels already:
                    // passing it would draw the ring at 1.5x the control at
                    // 150%, running past the client area and being clipped --
                    // losing exactly the rounded corners the ring exists to
                    // draw.
                    rounded(
                        mem,
                        Box2 { x: 0, y: 0, w: box2.w, h: box2.h },
                        radius + 1,
                        crate::theme::FOCUS_RING,
                        None,
                    );
                    let inner = RECT {
                        left: whole.left + 2,
                        top: whole.top + 2,
                        right: whole.right - 2,
                        bottom: whole.bottom - 2,
                    };
                    draw_button_with_shortcut(
                        mem, inner, &label, face, skin, scale(radius), hint, dpi,
                    );
                } else {
                    draw_button_with_shortcut(
                        mem, whole, &label, face, skin, scale(radius), hint, dpi,
                    );
                }
            }
            drop(guard);

            let _ = BitBlt(hdc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            let _ = EndPaint(control, &ps);
        }
    }

    /// What button `id` says, how it is drawn, and the logical box it occupies.
    ///
    /// **Only *Save* is primary.** *Not now* and *Never for this app* are the
    /// two silences, and the design gives the stronger of them the least weight
    /// -- it is the one control on the card that writes to `settings.json`.
    fn control_skin(
        id: usize,
        l: &super::Layout,
        fonts: &Fonts,
    ) -> (String, Option<&'static str>, ButtonSkin, HFONT, Box2) {
        match id {
            ID_SAVE => (
                SAVE_LABEL.to_string(),
                Some(SAVE_SHORTCUT),
                ButtonSkin::primary(),
                fonts.button,
                l.save,
            ),
            ID_NOT_NOW => (
                NOT_NOW_LABEL.to_string(),
                None,
                ButtonSkin::secondary(),
                fonts.button,
                l.not_now,
            ),
            ID_NEVER => (
                NEVER_LABEL.to_string(),
                None,
                ButtonSkin::secondary(),
                fonts.button,
                l.never,
            ),
            _ => (
                SAVE_GENERATE_LABEL.to_string(),
                None,
                ButtonSkin::secondary(),
                fonts.field,
                l.generate,
            ),
        }
    }

    /// The brand lockup, through [`crate::win32_draw::draw_card_lockup`] -- the
    /// crate's one mark painter, which every card draws through. What is this
    /// card's own is only the logical-to-device conversion.
    fn paint_lockup(hdc: HDC, l: &super::Layout, font: HFONT) {
        let dev = |b: Box2| RECT {
            left: scale(b.x),
            top: scale(b.y),
            right: scale(b.right()),
            bottom: scale(b.bottom()),
        };
        let tracking = scale(crate::win32_draw::card_lockup().tracking);
        draw_card_lockup(hdc, dev(l.mark), dev(l.wordmark), font, tracking);
    }

    /// The header's close glyph, drawn as two strokes because no bundled face
    /// has it at this weight.
    fn paint_close_glyph(hdc: HDC, at: Box2) {
        unsafe {
            use windows::Win32::Graphics::Gdi::{LineTo, MoveToEx};
            let pen = CreatePen(PS_SOLID, scale(1).max(1), rgb(crate::theme::TEXT_FAINT));
            let old = SelectObject(hdc, pen);
            let (x, y, w, h) = (scale(at.x), scale(at.y), scale(at.w), scale(at.h));
            let pad = w / 3;
            let _ = MoveToEx(hdc, x + pad, y + pad, None);
            let _ = LineTo(hdc, x + w - pad, y + h - pad);
            let _ = MoveToEx(hdc, x + w - pad, y + pad, None);
            let _ = LineTo(hdc, x + pad, y + h - pad);
            SelectObject(hdc, old);
            let _ = DeleteObject(pen);
        }
    }

    fn fill_rect(hdc: HDC, rc: RECT, colour: eframe::egui::Color32) {
        unsafe {
            let brush = CreateSolidBrush(rgb(colour));
            FillRect(hdc, &rc, brush);
            let _ = DeleteObject(brush);
        }
    }

    /// [`fill_rect`], for a **logical** rectangle.
    fn fill_box(hdc: HDC, at: Box2, colour: eframe::egui::Color32) {
        fill_rect(
            hdc,
            RECT {
                left: scale(at.x),
                top: scale(at.y),
                right: scale(at.right()),
                bottom: scale(at.bottom()).max(scale(at.y) + 1),
            },
            colour,
        );
    }

    /// A rounded rectangle in logical coordinates, optionally stroked.
    fn rounded(
        hdc: HDC,
        at: Box2,
        radius: i32,
        fill_colour: eframe::egui::Color32,
        border: Option<(i32, eframe::egui::Color32)>,
    ) {
        unsafe {
            let brush = CreateSolidBrush(rgb(fill_colour));
            let (width, colour) = border.unwrap_or((1, fill_colour));
            let pen = CreatePen(PS_SOLID, scale(width).max(1), rgb(colour));
            let old_brush = SelectObject(hdc, brush);
            let old_pen = SelectObject(hdc, pen);
            let r = scale(radius) * 2;
            let _ = RoundRect(
                hdc,
                scale(at.x),
                scale(at.y),
                scale(at.right()),
                scale(at.bottom()),
                r,
                r,
            );
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(brush);
            let _ = DeleteObject(pen);
        }
    }

    /// One run of text, left-aligned, vertically centred and truncated with an
    /// ellipsis rather than clipped mid-letter.
    ///
    /// Every run this card paints is bounded: the card cannot scroll and cannot
    /// resize, so a label that ran past its box would simply be unreadable --
    /// and the App row's is user-controlled.
    fn text(hdc: HDC, font: HFONT, at: Box2, run: &str, colour: eframe::egui::Color32) {
        unsafe {
            let old = SelectObject(hdc, font);
            SetTextColor(hdc, rgb(colour));
            let mut chars: Vec<u16> = run.encode_utf16().collect();
            let mut rc = RECT {
                left: scale(at.x),
                top: scale(at.y),
                right: scale(at.right()),
                bottom: scale(at.bottom()),
            };
            // `DT_NOPREFIX`: these are the app's own words, and one of them is
            // an app name in which an `&` is an ampersand and never a mnemonic
            // that would be drawn as an underscore.
            DrawTextW(
                hdc,
                &mut chars,
                &mut rc,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
            SelectObject(hdc, old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `cfg(test)` seams are banned in this crate, so the fakes below are
    // ordinary `fn`s recording into statics, which is this crate's idiom.

    #[derive(Default)]
    struct Trace {
        opened: usize,
        protected: usize,
        closed: usize,
        /// Which order the calls arrived in, by name.
        order: Vec<&'static str>,
        /// The events `next` will hand out, front first.
        script: Vec<Event>,
        /// What `open` was handed.
        opened_with: Option<(String, String, String)>,
        opened_anchor: Option<Option<(f32, f32)>>,
        /// Whether `open` should refuse.
        refuse_open: bool,
        /// Whether `protect` should refuse.
        refuse_protect: bool,
        /// What `take_form` answers.
        typed: (String, String),
    }

    static TRACE: std::sync::Mutex<Option<Trace>> = std::sync::Mutex::new(None);

    /// **One test at a time.** The fakes record into a process-wide `TRACE`,
    /// because the seam is a struct of plain `fn` pointers -- the same shape
    /// the shipped one has to fit through, and one that has nowhere to put a
    /// closure's captures. `cargo test` runs these in parallel threads, so
    /// without this every trace is two tests' calls interleaved.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn trace<R>(f: impl FnOnce(&mut Trace) -> R) -> R {
        let mut guard = TRACE.lock().unwrap_or_else(|p| p.into_inner());
        f(guard.get_or_insert_with(Trace::default))
    }

    fn reset(script: Vec<Event>) {
        let mut guard = TRACE.lock().unwrap_or_else(|p| p.into_inner());
        *guard = Some(Trace { script, ..Trace::default() });
    }

    fn fake_open(form: &SaveLoginForm, anchor: Option<(f32, f32)>) -> Option<SaveWindow> {
        trace(|t| {
            t.opened += 1;
            t.order.push("open");
            t.opened_with = Some((
                form.app_name.clone(),
                form.username.clone(),
                form.password.to_string(),
            ));
            t.opened_anchor = Some(anchor);
            if t.refuse_open {
                None
            } else {
                Some(SaveWindow(7))
            }
        })
    }

    fn fake_protect(_: SaveWindow) -> bool {
        trace(|t| {
            t.protected += 1;
            t.order.push("protect");
            !t.refuse_protect
        })
    }

    fn fake_next(_: SaveWindow) -> Event {
        trace(|t| {
            t.order.push("next");
            if t.script.is_empty() {
                Event::Closed
            } else {
                t.script.remove(0)
            }
        })
    }

    fn fake_take_form(_: SaveWindow) -> (String, Zeroizing<String>) {
        trace(|t| {
            t.order.push("take_form");
            (t.typed.0.clone(), Zeroizing::new(t.typed.1.clone()))
        })
    }

    fn fake_close(_: SaveWindow) {
        trace(|t| {
            t.closed += 1;
            t.order.push("close");
        })
    }

    static FAKE: SaveLoginCalls = SaveLoginCalls {
        open: fake_open,
        protect: fake_protect,
        next: fake_next,
        take_form: fake_take_form,
        close: fake_close,
    };

    fn form() -> SaveLoginForm {
        SaveLoginForm::new("Ledgerline")
    }

    /// **The whole point of the type, and the one bug a user cannot undo.**
    ///
    /// *Not now* is silence today; *Never for this app* is silence forever.
    /// Three controls, three events, three actions, and no place where two of
    /// them meet.
    #[test]
    fn the_three_answers_stay_three() {
        let _serial = serial();
        for (event, expected) in [
            (Event::Cancel, SaveLoginAction::NotNow),
            (Event::Closed, SaveLoginAction::NotNow),
            (Event::Save, SaveLoginAction::Save),
            (Event::Never, SaveLoginAction::Never),
            (Event::Generate, SaveLoginAction::Generate),
        ] {
            reset(vec![event]);
            let (action, _) = run_with(&FAKE, form(), None).expect("the card was shown");
            assert_eq!(
                action, expected,
                "{event:?} answered {action:?} rather than {expected:?}; conflating the two \
                 silences is the one bug on this card a user cannot undo without finding a \
                 setting"
            );
        }
        assert_ne!(SaveLoginAction::NotNow, SaveLoginAction::Never);
    }

    /// **Every exit path reads what was typed, including *Never*.**
    ///
    /// The card is one window and its boxes stop existing the moment it closes,
    /// so a path that skipped the read would silently discard a typed password.
    /// What is then *done* with the form is `app::route_save_answer`'s decision.
    #[test]
    fn every_answer_carries_what_the_user_typed() {
        let _serial = serial();
        for event in [Event::Cancel, Event::Save, Event::Never, Event::Generate] {
            reset(vec![event]);
            trace(|t| t.typed = ("ada@example.com".to_string(), "hunter2".to_string()));
            let (_, answered) = run_with(&FAKE, form(), None).expect("the card was shown");
            assert_eq!(answered.username, "ada@example.com", "{event:?} lost the username");
            assert_eq!(answered.password.as_str(), "hunter2", "{event:?} lost the password");
            assert_eq!(answered.app_name, "Ledgerline", "{event:?} lost the app name");
        }
    }

    /// **`protect` runs after `open` and before the first pump.** This card
    /// holds a plaintext password in a text box, so the ordering is the window
    /// being excluded from capture before anything can be typed into it.
    #[test]
    fn the_window_is_protected_before_it_is_pumped() {
        let _serial = serial();
        reset(vec![Event::Cancel]);
        run_with(&FAKE, form(), None).expect("the card was shown");
        let order = trace(|t| t.order.clone());
        assert_eq!(
            order,
            vec!["open", "protect", "next", "take_form", "close"],
            "the card was pumped before it was excluded from screen capture, or closed without \
             reading what was typed"
        );
    }

    /// A refused exclusion is a warning and not a crash -- but the card still
    /// closes, and it still answers.
    #[test]
    fn a_refused_exclusion_still_leaves_a_card_that_closes() {
        let _serial = serial();
        reset(vec![Event::Save]);
        trace(|t| t.refuse_protect = true);
        let (action, _) = run_with(&FAKE, form(), None).expect("the card was shown");
        assert_eq!(action, SaveLoginAction::Save);
        assert_eq!(trace(|t| t.closed), 1, "a card whose protection was refused was not closed");
    }

    /// **A window that could not be opened is not closed.** There is nothing to
    /// close, and `close` is only reachable with a `SaveWindow` in hand.
    #[test]
    fn a_window_that_never_opened_is_never_closed() {
        let _serial = serial();
        reset(vec![Event::Save]);
        trace(|t| t.refuse_open = true);
        assert!(run_with(&FAKE, form(), None).is_none());
        assert_eq!(trace(|t| t.protected), 0, "a window that does not exist was protected");
        assert_eq!(trace(|t| t.closed), 0, "a window that does not exist was closed");
    }

    /// **The card opens on the form it was handed.** Coming back from design 3d
    /// that form carries a generated password and whatever username was already
    /// typed; an `open` that took only a name would throw both away.
    #[test]
    fn the_card_opens_on_the_form_it_was_handed() {
        let _serial = serial();
        reset(vec![Event::Cancel]);
        let carried = SaveLoginForm {
            app_name: "Ledgerline".to_string(),
            username: "ada@example.com".to_string(),
            password: Zeroizing::new("generated-value".to_string()),
        };
        run_with(&FAKE, carried, Some((12.0, 34.0))).expect("the card was shown");
        assert_eq!(
            trace(|t| t.opened_with.clone()),
            Some((
                "Ledgerline".to_string(),
                "ada@example.com".to_string(),
                "generated-value".to_string()
            )),
            "the card did not open on the form it was handed"
        );
        assert_eq!(
            trace(|t| t.opened_anchor),
            Some(Some((12.0, 34.0))),
            "the anchor the caller computed did not reach `open`"
        );
    }

    // ---- geometry ----------------------------------------------------------

    /// **Nothing escapes the window, and the window is not padded out around
    /// nothing.**
    ///
    /// This is the tallest card in the crate, frameless, always-on-top, with no
    /// scrollbar and no resize border. A control past the bottom edge here is
    /// *Save*, or the password field itself, and the user cannot reach it by any
    /// means.
    #[test]
    fn every_control_is_inside_the_window() {
        let l = layout();
        let boxes: Vec<(&str, Box2)> = vec![
            ("mark", l.mark),
            ("wordmark", l.wordmark),
            ("title", l.title),
            ("close_glyph", l.close_glyph),
            ("header_rule", l.header_rule),
            ("app_value", l.app_value),
            ("username", l.username),
            ("password", l.password),
            ("generate", l.generate),
            ("folder_value", l.folder_value),
            ("footer_rule", l.footer_rule),
            ("save", l.save),
            ("not_now", l.not_now),
            ("never", l.never),
        ];
        for (name, at) in boxes.iter().chain(
            l.captions.iter().enumerate().map(|(i, b)| (ROW_CAPTIONS[i], *b)).collect::<Vec<_>>().iter(),
        ) {
            assert!(at.x >= 0, "`{name}` starts left of the window");
            assert!(at.y >= 0, "`{name}` starts above the window");
            assert!(
                at.right() <= l.window.w,
                "`{name}` ends {}px past the window's right edge, on a card that cannot scroll \
                 sideways",
                at.right() - l.window.w
            );
            assert!(
                at.bottom() <= l.window.h,
                "`{name}` ends {}px past the window's bottom edge, on a card that has no \
                 scrollbar, no title bar and no resize border -- the user cannot reach it by \
                 any means",
                at.bottom() - l.window.h
            );
        }
        // And the window is not taller than the content: the last control plus
        // one margin IS the height, so a control that stopped being drawn
        // shortens the window rather than leaving a hole nothing notices.
        assert_eq!(
            l.window.h,
            l.never.bottom() + 12,
            "the window's height is no longer the footer's bottom plus one margin, so it has \
             slack in it that no test can tell from a control that vanished"
        );
    }

    /// The footer's three answers do not overlap each other, and the Password
    /// row's field does not run into its *Generate*.
    #[test]
    fn no_two_controls_overlap() {
        let l = layout();
        assert!(l.save.right() < l.not_now.x, "*Save* runs into *Not now*");
        assert!(l.not_now.right() < l.never.x, "*Not now* runs into *Never for this app*");
        assert!(
            l.password.right() < l.generate.x,
            "the password field runs into the *Generate* control, which is the lane the layout \
             reserves before the field takes what is left"
        );
        assert!(
            l.captions[0].right() <= l.app_value.x,
            "the caption column runs into the value column"
        );
        for (a, b) in [
            (l.captions[0], l.captions[1]),
            (l.captions[1], l.captions[2]),
            (l.captions[2], l.captions[3]),
        ] {
            assert!(a.bottom() < b.y, "two of the four rows overlap vertically");
        }
    }

    /// The `EDIT` children sit strictly inside the boxes the parent paints, or
    /// the caret is drawn over the border.
    #[test]
    fn the_text_boxes_sit_inside_the_boxes_that_are_painted_for_them() {
        let l = layout();
        for at in [l.username, l.password] {
            let inner = field_child(at);
            assert!(inner.x > at.x, "the field starts on its own border");
            assert!(inner.right() < at.right(), "the field ends past its own border");
            assert!(inner.y > at.y);
            assert!(inner.bottom() < at.bottom());
            assert!(inner.w > 0 && inner.h > 0);
        }
    }

    /// The card's dimensions come from `theme`, not from numbers invented here.
    #[test]
    fn the_cards_dimensions_are_the_themes() {
        assert_eq!(
            BUTTON_H,
            crate::theme::BUTTON_HEIGHT as i32,
            "the footer's buttons are no longer `theme::BUTTON_HEIGHT` tall"
        );
        assert_eq!(
            WIDTH,
            crate::picker_prompt::WIDTH,
            "two frameless daemon cards of different widths read as two different programs"
        );
        let lockup = crate::win32_draw::card_lockup();
        let l = layout();
        assert_eq!(l.mark.h, lockup.mark_h);
        assert_eq!(l.wordmark.w, lockup.word_w);
        assert!(
            l.wordmark.right() < l.close_glyph.x,
            "the wordmark runs into the header's ✕"
        );
    }

    /// **The brand lockup is drawn, and it is drawn through the crate's one
    /// mark painter.** Four cards lost it in porting and it had to be restored
    /// afterwards.
    #[test]
    fn the_card_carries_the_brand_lockup() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        assert!(
            code.contains("draw_card_lockup("),
            "the card does not draw the brand lockup. A frameless always-on-top window that \
             offers to put a password into the user's vault has to say whose window it is"
        );
    }

    // ---- labels ------------------------------------------------------------

    /// The two silences read differently on the card, and the card does not
    /// imply a capture it did not make.
    #[test]
    fn the_words_on_the_card_say_what_it_can_honestly_say() {
        assert_ne!(NOT_NOW_LABEL, NEVER_LABEL);
        assert!(
            NEVER_LABEL.to_lowercase().contains("app"),
            "the strongest answer on the card does not say what it is scoped to"
        );
        for hint in [USERNAME_HINT, PASSWORD_HINT] {
            assert!(
                hint.starts_with("type "),
                "`{hint}` reads as a description of a value this process captured. It did not: \
                 there is no username reader in this app and a password field's contents are \
                 not read"
            );
        }
        assert_eq!(ROW_CAPTIONS, ["App", "Username", "Password", "Folder"]);
        assert!(
            FOLDER_ROW_TEXT.contains("No folder"),
            "the Folder row no longer states where the item goes"
        );
    }

    /// The window's title is this card's own, so `foreground::pick`'s `find`
    /// cannot bring one of the other five forward instead.
    #[test]
    fn the_card_opens_under_a_title_of_its_own() {
        assert!(!SAVE_LOGIN_CARD_TITLE.is_empty());
        assert_ne!(SAVE_LOGIN_CARD_TITLE, "Deskwarden");
        assert_ne!(SAVE_LOGIN_CARD_TITLE, crate::prompt_card::PROMPT_CARD_TITLE);
        assert_ne!(SAVE_LOGIN_CARD_TITLE, crate::locked_card::LOCKED_CARD_TITLE);
        assert_ne!(SAVE_LOGIN_CARD_TITLE, crate::picker_prompt::PICKER_PROMPT_TITLE);
        assert_ne!(SAVE_LOGIN_CARD_TITLE, crate::unlock_prompt::UNLOCK_PROMPT_TITLE);
        assert_ne!(SAVE_LOGIN_CARD_TITLE, crate::generate_prompt::GENERATE_PROMPT_TITLE);
        assert_ne!(SAVE_LOGIN_CARD_TITLE, crate::vault_window::WINDOW_TITLE);
    }

    // ---- source pins -------------------------------------------------------

    /// The production half of this file: everything before the first column-0
    /// `#[cfg(test)]`, with line endings normalised first because this
    /// repository checks out CRLF.
    fn production() -> (String, usize) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("save_login_card.rs");
        let raw = std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
        let cut = raw.split(concat!("\n#[cfg(", "test)]\n")).next().unwrap().to_string();
        let discarded = raw.len() - cut.len();
        (cut, discarded)
    }

    /// The production half with comments stripped, so a rule that forbids a
    /// call does not also forbid explaining why the call is not there.
    fn code(source: &str) -> String {
        source
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_save_login_window_never_posts_a_thread_quit() {
        let (production, discarded) = production();
        let code = code(&production);

        // CONTROLS, so a pin that scanned nothing cannot pass.
        assert!(
            discarded > 0,
            "control: the `#[cfg(test)]` cut marker was not found, so this scan is reading the \
             test module as production and the rule below is meaningless"
        );
        assert!(
            code.contains("WM_DESTROY =>"),
            "control: the production cut does not contain the window procedure's WM_DESTROY \
             arm, so the cut is in the wrong place"
        );
        assert!(
            code.contains("GONE.store(true, Ordering::SeqCst);"),
            "control: the comment stripper has eaten code -- the WM_DESTROY arm's one \
             surviving statement is not in the text this rule scans"
        );

        assert!(
            !code.contains(concat!("PostQuit", "Message")),
            "save_login_card.rs's production half posts a thread quit. This window is opened on \
             the daemon thread, and that thread goes on to open more windows -- design 3d's \
             generator, and this card again when 3d comes back. `close()` calls \
             `DestroyWindow`, which dispatches WM_DESTROY synchronously on that thread, and \
             nothing drains the queue afterwards. `GONE` is what `next()` reads; quitting the \
             thread is not this window's job."
        );
    }

    /// **The capture exclusion goes on the top-level window, and once.**
    ///
    /// Windows refuses `SetWindowDisplayAffinity` on a child `EDIT` with
    /// `E_INVALIDARG`, so a call aimed at this card's password box would fail
    /// silently and leave the whole card capturable.
    #[test]
    fn the_capture_exclusion_goes_on_the_top_level_window() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        assert_eq!(
            code.matches("SetWindowDisplayAffinity(").count(),
            1,
            "this card holds a plaintext password in a text box and excludes itself from screen \
             capture other than exactly once"
        );
        assert!(
            code.contains("SetWindowDisplayAffinity(hwnd(window.0), WDA_EXCLUDEFROMCAPTURE)"),
            "the exclusion is not applied to the top-level window this module was handed. \
             Windows refuses it on a child control with E_INVALIDARG, and the top-level flag \
             covers every child it owns"
        );
        assert_eq!(
            code.matches("SetForegroundWindow(").count(),
            1,
            "this card is typed into, so it asks for the foreground -- once, and handled rather \
             than asserted"
        );
    }

    /// **Nothing in the production half creates a GPU device**, which is the
    /// whole reason this card stopped being an egui window.
    #[test]
    fn the_card_is_gdi_only() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        for banned in ["D2D1CreateFactory", "D3D11CreateDevice", "ID2D1", "run_native"] {
            assert!(
                !code.contains(banned),
                "save_login_card.rs names `{banned}`. The whole reason this card is bare Win32 \
                 is that the first GPU device this process creates costs ~50 MB of driver \
                 arenas that are never released"
            );
        }
    }

    /// **`IsDialogMessageW` is in the pump**, which is what makes Tab move
    /// between the two boxes at all.
    #[test]
    fn the_pump_traverses_between_the_two_boxes() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        assert!(
            code.contains("IsDialogMessageW(top, &msg)"),
            "the pump no longer runs `IsDialogMessageW`, so Tab types a tab character into the \
             username box instead of moving to the password box"
        );
        assert!(
            code.contains("VK_ESCAPE.0"),
            "Escape is no longer read in the pump, so the card cannot be dismissed from the \
             keyboard"
        );
        assert!(
            code.contains("VK_RETURN.0"),
            "Enter is no longer read in the pump, and the footer's ENTER chip promises it"
        );
    }

    /// **The password buffers are `Zeroizing`, and the control's own copy is
    /// disturbed on the way out.**
    ///
    /// See the module doc for what that does and does not achieve -- it is best
    /// effort in the strict sense, and it is not claimed as more.
    #[test]
    fn the_password_is_read_and_scrubbed_with_the_care_the_module_doc_describes() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        assert!(
            code.contains("let mut buf: Zeroizing<Vec<u16>> ="),
            "the password's UTF-16 buffer is no longer a `Zeroizing`, so the bytes \
             `WM_GETTEXT` copied are handed back to the allocator intact"
        );
        assert!(
            code.contains("fn scrub_password("),
            "the password control's own buffer is no longer overwritten before the window goes \
             down"
        );
        assert!(
            code.contains("scrub_password(top);"),
            "`close` no longer scrubs the password control, so the one exit path every answer \
             goes through leaves comctl32's buffer as it was"
        );
        // And the scrub is on the close path, which every answer reaches.
        let close_body = code
            .split("pub(super) fn close(window: SaveWindow) {")
            .nth(1)
            .expect("control: `close` is not in the production half");
        assert!(
            close_body.starts_with("\n        let top = hwnd(window.0);\n        scrub_password(top);"),
            "the scrub is no longer the first thing `close` does, so a `DestroyWindow` before it \
             would leave nothing to scrub: {}",
            &close_body[..close_body.len().min(160)]
        );
    }

    /// The placeholder path asks the control its text **length** and never its
    /// text: a password's contents are not read by this app for any reason,
    /// including deciding whether to draw a hint over them.
    #[test]
    fn the_placeholder_never_reads_the_field() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        let body = code
            .split("fn paint_placeholder(")
            .nth(1)
            .expect("control: `paint_placeholder` is not in the production half");
        let body = body.split("\n    fn ").next().unwrap_or(body);
        assert!(
            !body.contains("WM_GETTEXT,"),
            "the placeholder painter reads the field's text. It needs to know only whether the \
             field is empty, and this one is the password box"
        );
    }
}

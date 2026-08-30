//! **Design 3d: the password generator card, in bare Win32.**
//!
//! The daemon offers a freshly generated password when the user asks for one
//! from design 3c's *Generate* link. This is that card, and it is the third
//! surface in this crate drawn with `CreateWindowExW` and GDI rather than
//! with egui -- after `crate::unlock_prompt` and `crate::picker_prompt`, and
//! for the same measured reason they are.
//!
//! # Why it is not an egui window any more
//!
//! The tray daemon measures 9.9 MB with no window ever opened. The moment any
//! egui window opens it becomes ~60 MB resident and **never returns**: the
//! OpenGL driver's committed arenas survive the window's destruction and are
//! only reclaimed at process exit. The two Win32 cards already in this crate
//! measure ~2 MB with their window on screen. This card is on the daemon's own
//! fill path, so it paid that ~50 MB on the commonest reason a user has to open
//! it -- and paid it permanently.
//!
//! # The seam
//!
//! Mirrors `crate::picker_prompt` exactly: [`GenerateCalls`] is a struct of
//! `fn` pointers, and [`run_with`] is the whole decision -- drivable by a test
//! with no window, no vault and no clipboard. `protect` runs immediately after
//! `open` and **before the first pump**, and `close` runs on every exit path
//! including the failures.
//!
//! # The secret never crosses the seam
//!
//! This is the most exposed card in the app: it puts a password **on screen in
//! the clear**, deliberately -- a generated value the user has to be able to
//! read, that has not been used for anything yet, and that a masked generator
//! would give them no way to check.
//!
//! So the password is not part of the decision. [`run_with`] deals in
//! [`GenerateForm`], whose state is [`ValueState`] -- `InFlight`, `Ready`, or
//! `Failed(sentence)`, with **no payload on `Ready`**. The value itself is
//! produced by [`GenerateCalls::fill`], which runs the caller's generator and
//! parks the answer inside the window module, and it leaves that module by
//! exactly two routes: onto the clipboard ([`GenerateCalls::copy`], which is
//! this crate's one clipboard path and its clearing behaviour), and into the
//! slot [`GenerateCalls::keep`] moves it to, which is what
//! [`show_generate_prompt`] reads after the window is down. Nothing secret is
//! in [`Event`], in [`Outcome`], in [`GenerateForm`] or in any `Debug` on this
//! module's types, and the whole of what this module hands back is the
//! `Option<Zeroizing<String>>` its one public entry point already returned when
//! it was an egui card.
//!
//! # The generator is the caller's
//!
//! `crate::app::handle_no_match` chooses between `bw serve`'s generator and
//! this crate's own, and that choice stays there: this module holds no vault
//! handle and reaches for none. See [`Generator`].

use zeroize::Zeroizing;

use crate::vault_bridge::GenerateRequest;

/// The window handle [`run_with`] deals in.
///
/// A bare `isize` newtype, not an `HWND`, for the same reason
/// `picker_prompt::PickerWindow` is: a decision layer a test can drive must not
/// name a type that only exists behind a Win32 feature gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenerateWindow(pub isize);

/// The round trip that produces a password, as the caller supplies it.
///
/// A `&dyn Fn` rather than a `fn` pointer, because the caller's is a closure
/// over a vault handle -- and it crosses [`GenerateCalls::fill`] as an
/// **argument** rather than living on the seam struct, so what comes back out
/// of it is parked by the window module and never returned through the seam.
pub type Generator<'a> =
    dyn Fn(&GenerateRequest) -> Result<Zeroizing<String>, String> + 'a;

/// Which kind of secret design 3d asks for.
///
/// # This is Words / Characters / PIN, with the middle one renamed
///
/// The design draws a three-way *Words / Letters / PIN* against a backend that
/// has two request types. It resolves once the two axes are separated: the
/// **request type** is a two-way ([`crate::vault_bridge::PassphraseRecipe`]
/// against [`crate::vault_bridge::PasswordRecipe`]) and the **alphabet** is
/// what makes three of them. [`Self::Words`] is the passphrase;
/// [`Self::Characters`] and [`Self::Pin`] are both `PasswordRecipe`, differing
/// only in which character classes they turn on.
///
/// **The middle one is called *Characters*, not *Letters*, because that is what
/// it is.** This card has no character-class switches (see [`layout`]) and so
/// the general-purpose choice is the crate's own
/// [`crate::vault_bridge::PasswordRecipe::default`] -- all four classes, digits
/// and symbols included. A chip reading "Letters" over a password containing
/// `7` and `!` would be the card lying about its own output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratedKind {
    /// A word passphrase: [`crate::vault_bridge::PassphraseRecipe`].
    Words,
    /// The default character password: every class on, which is what
    /// "inherits the defaults" means here.
    Characters,
    /// Digits only -- a `PasswordRecipe` with one class on.
    ///
    /// **Representable, and it survives the round trip.** The route substitutes
    /// `uppercase + lowercase + number` only when *all four* classes arrive
    /// false; one class on is honoured.
    Pin,
}

impl GeneratedKind {
    /// Every kind, in the order the chips are drawn.
    pub const ALL: [Self; 3] = [Self::Words, Self::Characters, Self::Pin];

    /// The chip's label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Words => "Words",
            Self::Characters => "Characters",
            Self::Pin => "PIN",
        }
    }

    /// What the size readout counts, and **it is read off the kind rather than
    /// fixed**.
    ///
    /// The design draws a static "20 chars" while *Words* is selected, which
    /// does not cohere: a four-word passphrase is not twenty of anything the
    /// user chose. The size control sets `words` for a passphrase and `length`
    /// for a password, so the readout has to say which.
    pub fn unit(self) -> &'static str {
        match self {
            Self::Words => "words",
            Self::Characters | Self::Pin => "characters",
        }
    }

    /// The inclusive size range the stepper may reach.
    ///
    /// **Every lower bound is at or above the one the route would silently
    /// raise.** `bw serve` clamps a password `length` below 5 up to 5 and a
    /// passphrase `words` below 3 up to 3, with no error and a 200 -- so a
    /// stepper that could reach 4 digits would be a control that visibly says
    /// one thing and silently produces another. **A four-digit PIN is therefore
    /// not offered at all**, rather than offered and quietly turned into five.
    pub fn bounds(self) -> (u32, u32) {
        match self {
            Self::Words => (3, 10),
            Self::Characters => (8, 64),
            // 5, not 4: see above. This is the one bound in this table set by
            // the server rather than by taste.
            Self::Pin => (5, 12),
        }
    }

    /// The size a freshly chosen kind starts at.
    ///
    /// `Characters` is 20 and `Words` is 4 because
    /// [`crate::vault_bridge::PasswordRecipe`]'s and `PassphraseRecipe`'s own
    /// defaults are -- this card inherits the crate's defaults rather than
    /// inventing weaker ones.
    pub fn default_size(self) -> u32 {
        match self {
            Self::Words => 4,
            Self::Characters => 20,
            Self::Pin => 6,
        }
    }

    /// The request this kind makes at `size`, **clamped into [`Self::bounds`]
    /// first** so no caller can build a recipe the route would silently
    /// rewrite.
    pub fn recipe(self, size: u32) -> GenerateRequest {
        use crate::vault_bridge::{PassphraseRecipe, PasswordRecipe};
        let (low, high) = self.bounds();
        let size = size.clamp(low, high);
        match self {
            Self::Words => GenerateRequest::Passphrase(PassphraseRecipe {
                words: size,
                ..PassphraseRecipe::default()
            }),
            Self::Characters => GenerateRequest::Password(PasswordRecipe {
                length: size,
                ..PasswordRecipe::default()
            }),
            Self::Pin => GenerateRequest::Password(PasswordRecipe {
                length: size,
                uppercase: false,
                lowercase: false,
                number: true,
                special: false,
                // Both minima go to zero WITH the classes they belong to. A
                // `minSpecial: 1` beside `special: false` is a request that
                // asks for one of something it has just excluded, and what the
                // route does with that is not a thing this card should be
                // betting on.
                min_number: 0,
                min_special: 0,
                // **Off, and the only kind for which it is.** "Avoid ambiguous"
                // exists so a human can tell `O` from `0` and `l` from `1`.
                // With no letters in the alphabet there is nothing to confuse
                // them with, so all it would do is delete two of the ten digits
                // from a six-character secret.
                avoid_ambiguous: false,
            }),
        }
    }
}

/// Where the card's one round-trip has got to.
///
/// **There is no `Idle`.** The card opens generating -- 3d's whole premise is
/// that it *leads* with a fresh password -- so an empty state would be one the
/// user never sees and nothing ever leaves.
///
/// **And there is no password in here.** `Ready` is a bare marker: the value
/// itself is parked inside the window module by [`GenerateCalls::fill`] and
/// never travels the seam. That is what lets this type -- and
/// [`GenerateForm`], which holds it -- derive `Debug` at all; the egui card's
/// equivalent carried a `Zeroizing<String>` and had to hand-write one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueState {
    /// A request has been started and has not answered yet.
    InFlight,
    /// The generator answered, and the window is holding the password.
    Ready,
    /// The generator failed, and this is the sentence the card shows.
    Failed(String),
}

/// The card's whole state: what to ask for, how much of it, and where the
/// asking has got to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerateForm {
    kind: GeneratedKind,
    size: u32,
    state: ValueState,
}

impl GenerateForm {
    /// A card that has just opened: `kind` at its default size, already
    /// generating.
    pub fn new(kind: GeneratedKind) -> Self {
        Self { kind, size: kind.default_size(), state: ValueState::InFlight }
    }

    /// What the card is asking for.
    pub fn kind(&self) -> GeneratedKind {
        self.kind
    }

    /// How many words or characters.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Where the round-trip has got to.
    pub fn state(&self) -> &ValueState {
        &self.state
    }

    /// Whether a request is outstanding.
    pub fn in_flight(&self) -> bool {
        matches!(self.state, ValueState::InFlight)
    }

    /// Whether there is a password to save or copy.
    ///
    /// **This is what makes "Save is unreachable without a password" a property
    /// of the type rather than of the button.** The buttons are drawn dead too
    /// -- a live control that does nothing is worse than a grey one -- but a UI
    /// state is not where an invariant lives.
    pub fn ready(&self) -> bool {
        matches!(self.state, ValueState::Ready)
    }

    /// The size readout, live and labelled by kind: "4 words", "20
    /// characters".
    pub fn readout(&self) -> String {
        format!("{} {}", self.size, self.kind.unit())
    }

    /// The request the card would send right now.
    pub fn request(&self) -> GenerateRequest {
        self.kind.recipe(self.size)
    }

    /// Starts a request, and **answers `false` and changes nothing if one is
    /// already outstanding.**
    ///
    /// This is the whole of "no second generate runs concurrently", and it is a
    /// refusal in the one function that can enter [`ValueState::InFlight`]
    /// rather than a disabled button. Every path that regenerates (the *New*
    /// button, Ctrl+R, changing kind, changing size) goes through here.
    pub fn begin(&mut self) -> bool {
        if self.in_flight() {
            return false;
        }
        self.state = ValueState::InFlight;
        true
    }

    /// Records what the generator answered: `None` succeeded, `Some(sentence)`
    /// is the line the card shows.
    ///
    /// **It always leaves a state that is not [`ValueState::InFlight`]**,
    /// including on an error, and that is the point. A card whose failure left
    /// it in flight could never be regenerated, on a frameless window whose
    /// only other way out is Esc.
    pub fn finish(&mut self, failure: Option<String>) {
        self.state = match failure {
            None => ValueState::Ready,
            Some(message) => ValueState::Failed(message),
        };
    }

    /// Switches to `kind` at its default size and starts a request. Answers
    /// whether anything moved.
    ///
    /// **Refused while in flight**, by the same [`Self::begin`] the other three
    /// paths use: the answer to an outstanding request is the answer to the
    /// recipe that was sent, and pinning it onto whichever chip the user
    /// clicked in the meantime would be the card mislabelling its own output.
    pub fn choose(&mut self, kind: GeneratedKind) -> bool {
        if self.in_flight() || kind == self.kind {
            return false;
        }
        self.kind = kind;
        self.size = kind.default_size();
        self.begin()
    }

    /// Moves the size by `delta`, within [`GeneratedKind::bounds`], and starts
    /// a request. Answers whether anything moved.
    pub fn resize(&mut self, delta: i32) -> bool {
        if self.in_flight() {
            return false;
        }
        let (low, high) = self.kind.bounds();
        let next = (i64::from(self.size) + i64::from(delta)).clamp(i64::from(low), i64::from(high));
        let next = next as u32;
        if next == self.size {
            return false;
        }
        self.size = next;
        self.begin()
    }

    /// Whether the stepper's `delta` button should be live.
    pub fn can_resize(&self, delta: i32) -> bool {
        let (low, high) = self.kind.bounds();
        !self.in_flight()
            && match delta.signum() {
                1 => self.size < high,
                -1 => self.size > low,
                _ => false,
            }
    }
}

/// What the user did with the window.
///
/// **No secret reaches this type.** The password lives in the window module;
/// `Copy` and `Save` are instructions about it, not carriers of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The header ✕, the *Dismiss* route, or Escape.
    Cancel,
    /// The window went away underneath us. Treated exactly as `Cancel`.
    Closed,
    /// *New*, or Ctrl+R: ask again with the settings that are showing.
    Regenerate,
    /// *Copy*: put the password on the clipboard, and keep the card up.
    Copy,
    /// *Save to vault*, or Enter: hand the password back to design 3c.
    Save,
    /// One of the three kind chips.
    Choose(GeneratedKind),
    /// The size stepper, `+1` or `-1`.
    Resize(i32),
}

/// How [`run_with`] finished.
///
/// **`Kept` carries nothing.** The password it refers to was moved by
/// [`GenerateCalls::keep`] into the window module's own slot, and
/// [`show_generate_prompt`] is what takes it out of there -- so no secret is on
/// this type, and a caller driving [`run_with`] with stub calls never has one
/// to leak.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The user kept the password. It is in the window module's kept slot.
    Kept,
    /// The user dismissed the card. Nothing was kept.
    Cancelled,
    /// The window could not be put on screen at all.
    Unavailable,
}

/// The Win32 half, as `fn` pointers so [`run_with`] can be driven without a
/// desktop. Nothing here decides anything; every decision lives in
/// [`run_with`].
pub struct GenerateCalls {
    /// Lays out and shows the card, for the app named by the argument. `None`
    /// if it could not be put on screen.
    pub open: fn(&str) -> Option<GenerateWindow>,
    /// `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on the **top-level**
    /// window, called before the first `next` -- see the module doc. Windows
    /// refuses it on a child control with `E_INVALIDARG`, and the top-level
    /// flag covers every child it owns.
    pub protect: fn(GenerateWindow) -> bool,
    /// Pumps until the user does something.
    pub next: fn(GenerateWindow) -> Event,
    /// Redraws the card for this form state.
    pub show: fn(GenerateWindow, &GenerateForm),
    /// **Runs the generator and parks the answer where only the window can see
    /// it.** Answers the failure sentence, or `None` on success.
    ///
    /// The generator crosses as an argument rather than living on this struct
    /// because it is a closure over the caller's vault handle; the *password*
    /// does not cross at all.
    pub fill: fn(GenerateWindow, &GenerateRequest, &Generator<'_>) -> Option<String>,
    /// Puts the parked password on the clipboard, through
    /// `crate::clipboard::copy_secret` and its clearing behaviour. A no-op when
    /// there is nothing parked.
    pub copy: fn(GenerateWindow),
    /// Moves the parked password into the slot [`show_generate_prompt`] reads
    /// after the window is down. `close` wipes the parked one; it does not
    /// touch the kept one.
    pub keep: fn(GenerateWindow),
    /// Destroys the window, releases its resources and **wipes the parked
    /// password**.
    pub close: fn(GenerateWindow),
}

/// **The whole decision, and the only part of this module a test can run.**
///
/// 1. `protect` runs immediately after `open` and before the first `next`.
/// 2. The card opens generating: the in-flight state is shown, then `fill` is
///    made, then the answer is shown. That order is deliberate -- `fill` blocks
///    on a round trip, and a card that painted nothing until it came back would
///    appear frozen the moment it opened.
/// 3. `Save` is refused unless [`GenerateForm::ready`]. Enter on an in-flight
///    or failed card would otherwise hand design 3c an empty password and close
///    the generator: a credential the user did not choose being written to
///    their vault by a key they pressed to accept the one they could see.
/// 4. `close` runs on every exit path, including `Unavailable`'s predecessor --
///    there is no window to close there, which is exactly why `open` returning
///    `None` returns before ever calling it.
pub fn run_with(calls: &GenerateCalls, app_name: &str, generate: &Generator<'_>) -> Outcome {
    let Some(window) = (calls.open)(app_name) else {
        log::warn!("the password generator could not be put on screen");
        return Outcome::Unavailable;
    };

    // Before the first pump, so nothing on the card -- least of all the
    // password it is about to paint in the clear -- is on screen while the
    // window is still capturable.
    if !(calls.protect)(window) {
        log::warn!(
            "SetWindowDisplayAffinity was refused for the password generator; the password it \
             shows is visible to screen capture on this machine"
        );
    }

    let mut form = GenerateForm::new(GeneratedKind::Characters);
    refresh(calls, window, &mut form, generate);

    loop {
        match (calls.next)(window) {
            Event::Cancel | Event::Closed => {
                (calls.close)(window);
                return Outcome::Cancelled;
            }
            Event::Regenerate => {
                if form.begin() {
                    refresh(calls, window, &mut form, generate);
                }
            }
            Event::Choose(kind) => {
                if form.choose(kind) {
                    refresh(calls, window, &mut form, generate);
                }
            }
            Event::Resize(delta) => {
                if form.resize(delta) {
                    refresh(calls, window, &mut form, generate);
                }
            }
            Event::Copy => {
                // Refused without a password, for the reason `Save` is: an
                // empty copy would clear whatever the user already had on the
                // clipboard and give them nothing back for it.
                if form.ready() {
                    (calls.copy)(window);
                }
            }
            Event::Save => {
                if form.ready() {
                    (calls.keep)(window);
                    (calls.close)(window);
                    return Outcome::Kept;
                }
                log::warn!(
                    "the password generator was asked to save with no password generated; \
                     ignoring it"
                );
            }
        }
    }
}

/// One round trip, painted on both sides of itself.
///
/// The in-flight card is shown **before** `fill` blocks, so
/// [`ValueState::InFlight`] is a state the user actually sees rather than one
/// that only exists between two statements.
fn refresh(
    calls: &GenerateCalls,
    window: GenerateWindow,
    form: &mut GenerateForm,
    generate: &Generator<'_>,
) {
    (calls.show)(window, form);
    let failure = (calls.fill)(window, &form.request(), generate);
    form.finish(failure);
    (calls.show)(window, form);
}

/// The window's title.
///
/// Distinct from every other title this crate opens under, because
/// `crate::foreground::pick` is a `find` over this process's own windows and
/// this card is up alongside the tray's and the hotkey listener's helper
/// windows.
pub const GENERATE_PROMPT_TITLE: &str = "Deskwarden password generator";

/// The card's header.
pub const GENERATE_LABEL: &str = "New password";

/// 3d's primary button.
///
/// **It says *Save to vault*, and the design says *Fill & save to vault*.** The
/// missing word is the honest one: `crate::app::handle_no_match` holds no
/// injector and no `FillStats`, deliberately and by signature, so nothing on
/// this path can type into the window behind the card. What it can do is put
/// the password into design 3c, which saves it, and onto the clipboard, which
/// is how it reaches the field.
pub const GENERATE_SAVE_LABEL: &str = "Save to vault";

/// 3d's clipboard button -- the one control here that gets the password into
/// the app the user is actually looking at.
pub const GENERATE_COPY_LABEL: &str = "Copy";

/// 3d's regenerate control, which carries its `Ctrl+R` chip inside itself.
pub const GENERATE_NEW_LABEL: &str = "New";

/// What the value box says while the round-trip is outstanding.
pub const GENERATE_WORKING_TEXT: &str = "Generating…";

/// What the value box says when the generator could not be reached.
///
/// **The sentence, and not the error.** `VaultError`'s `Debug` is a URL, a
/// status code and a response body, none of which fits one truncated line and
/// any of which could carry more than it should. The detail goes to the log,
/// where `handle_no_match`'s closure writes it; the card gets the sentence, and
/// its failure state can be left by pressing *New*.
pub const GENERATE_FAILED_TEXT: &str = "Could not generate a password. Try again.";

/// The `Ctrl+R` chip drawn inside the *New* button.
pub const REGENERATE_SHORTCUT: &str = "CTRL+R";

/// The `Enter` chip drawn inside *Save to vault*.
pub const SAVE_SHORTCUT: &str = "ENTER";

/// The `Esc` chip in the footer, and the word beside it.
pub const ESC_SHORTCUT: &str = "ESC";
/// The footer hint's word.
pub const DISMISS_LABEL: &str = "Dismiss";

/// **Puts the card on screen and answers the password the user chose to keep**
/// -- `None` if they dismissed it.
///
/// The production [`REAL`] calls, [`run_with`]'s decision, and the one place in
/// this module a password is handed out.
///
/// `generate` is the round trip, passed in rather than reached for: this module
/// has no vault handle, and `crate::app::handle_no_match` is where the one that
/// exists lives.
pub fn show_generate_prompt(
    app_name: &str,
    generate: &Generator<'_>,
) -> Option<Zeroizing<String>> {
    ask_with(&REAL, app_name, generate)
}

/// [`show_generate_prompt`], told which [`GenerateCalls`] to use.
///
/// **This is the only way to get the password out**, which is why the preview
/// example goes through it rather than through a public "take what was kept":
/// a second route to the kept slot would be a second place a live credential
/// can be read from, on the one module in this crate that parks one.
///
/// `examples/generate_preview.rs` is its one non-production caller, swapping
/// [`GenerateCalls::protect`] for a stub so the window can be screenshotted.
pub fn ask_with(
    calls: &GenerateCalls,
    app_name: &str,
    generate: &Generator<'_>,
) -> Option<Zeroizing<String>> {
    let outcome = run_with(calls, app_name, generate);
    // **Taken on every path, kept on one.** `take_kept` is a take rather than a
    // read, so the slot is empty (and the value dropped, and so zeroed) after
    // this line whatever the user did -- a `Cancelled` that left a password
    // parked in a process static would be the one leak this card cannot afford.
    let kept = win32::take_kept();
    match outcome {
        Outcome::Kept => kept,
        Outcome::Cancelled | Outcome::Unavailable => None,
    }
}

/// The production [`GenerateCalls`].
pub static REAL: GenerateCalls = GenerateCalls {
    open: win32::open,
    protect: win32::protect,
    next: win32::next,
    show: win32::show,
    fill: win32::fill,
    copy: win32::copy,
    keep: win32::keep,
    close: win32::close,
};

// ---------------------------------------------------------------------------
// Layout
//
// Logical pixels, at 100%, every one of them read off `theme` or off the two
// Win32 cards this one sits beside. Numbers invented here would be a second
// layout that has to agree with a first, which is this codebase's standing
// defect shape.
// ---------------------------------------------------------------------------

/// The card's width, and so the window's. The same
/// `crate::picker_prompt::WIDTH`, because it is the same kind of card in the
/// same place on screen and two frameless daemon cards of different widths read
/// as two different programs.
pub const WIDTH: i32 = 380;

/// Content inset, and the top margin.
const MARGIN_X: i32 = 14;
const MARGIN_TOP: i32 = 12;

/// The value box's height, and **it is fixed across all three states**.
///
/// That is what makes one window serve a password, a "Generating…" and an error
/// sentence: the box is this tall whichever of them is in it, and each of them
/// is a single truncated line.
const VALUE_H: i32 = 44;

/// The height of the kind chips and the stepper buttons.
const CHIP_H: i32 = 26;

/// Button height. `theme::BUTTON_HEIGHT`, pinned by
/// [`the_cards_dimensions_are_the_themes`].
const BUTTON_H: i32 = 32;

/// The *New* button's width: its label and the [`REGENERATE_SHORTCUT`] chip
/// beside it, in one pill, the way `win32_draw::draw_button_with_shortcut`
/// draws every shortcut-bearing button in this crate.
const NEW_W: i32 = 92;

/// The three kind chips' widths, in [`GeneratedKind::ALL`] order. Not uniform,
/// because "Characters" is more than twice as wide as "PIN" and a chip padded
/// out to the longest label is a chip with a lie's worth of empty space in it.
const KIND_W: [i32; 3] = [58, 82, 44];

/// The gap between two adjacent chips.
const CHIP_GAP: i32 = 4;

/// The stepper's two buttons, and the readout between them.
const STEP_W: i32 = 26;
const READOUT_W: i32 = 86;

/// *Save to vault* carries its `ENTER` chip inside itself, so it is wider than
/// its label needs.
const SAVE_W: i32 = 140;
const COPY_W: i32 = 76;

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
/// Pure arithmetic with no Win32 in it, for `picker_prompt::layout`'s reason: a
/// control whose bottom edge fell past the window's would simply be invisible
/// on a window that neither scrolls nor resizes, and that is a property worth
/// asserting without opening anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub window: Box2,
    /// The brand lockup's shield, and the wordmark beside it. **The card had
    /// no brand at all after the port**: the egui card it replaced carried
    /// 's shield and letterspaced DESKWARDEN, and a
    /// frameless always-on-top window that offers to put a password into the
    /// user's vault has to say whose window it is. The compact lockup, not the
    /// login window's -- see [].
    pub mark: Box2,
    pub wordmark: Box2,
    pub title: Box2,
    pub close_glyph: Box2,
    /// The hairline under the header, and the one over the footer.
    pub header_rule: Box2,
    pub footer_rule: Box2,
    /// The tinted band the footer's controls sit on.
    pub footer: Box2,
    /// The box the password (or the working line, or the failure sentence) is
    /// drawn in.
    pub value: Box2,
    pub new: Box2,
    /// The three kind chips, in [`GeneratedKind::ALL`] order.
    pub kinds: [Box2; 3],
    pub minus: Box2,
    pub readout: Box2,
    pub plus: Box2,
    pub save: Box2,
    pub copy: Box2,
    /// The `Esc Dismiss` hint: the chip's box, then the word's.
    pub esc_chip: Box2,
    pub dismiss: Box2,
}

/// **The card's geometry. There is exactly one shape.**
///
/// The card has no rows, no modes and no second step, so nothing about it
/// varies at runtime: the value box is [`VALUE_H`] tall whether it is showing a
/// password, a "Generating…" or a failure sentence, and the readout's text
/// changes inside a box whose width does not. The window is sized to this
/// content and to nothing else -- the way `picker_prompt`'s empty mode is sized
/// to its own two offers -- rather than to a row count it does not have.
///
/// **No character-class switches.** The edit form has them
/// (`vault_window::CharClasses`, where all-off is made unrepresentable); this
/// surface is frameless, always-on-top, unscrollable and appears over whatever
/// the user is doing, and six toggles on it would be six more controls to push
/// off a bottom edge that cannot be scrolled back. It inherits
/// [`crate::vault_bridge::PasswordRecipe::default`] instead -- which, per
/// [`GeneratedKind`], is also why the middle chip is not called "Letters".
pub fn layout() -> Layout {
    let content_w = WIDTH - 2 * MARGIN_X;

    let lockup = crate::win32_draw::card_lockup();
    let mark = Box2 { x: MARGIN_X, y: MARGIN_TOP, w: lockup.mark_w, h: lockup.mark_h };
    let wordmark =
        Box2 { x: mark.right() + lockup.gap, y: MARGIN_TOP, w: lockup.word_w, h: lockup.mark_h };
    // The ✕ moves up onto the lockup's line, which is where every card header
    // in the design carries it -- and where it has to be now that the title is
    // no longer the top line.
    let close_glyph = Box2 { x: WIDTH - MARGIN_X - 20, y: MARGIN_TOP - 2, w: 20, h: 20 };
    let title =
        Box2 { x: MARGIN_X, y: mark.bottom() + lockup.gap_below, w: content_w - 24, h: 21 };
    let header_rule = Box2 { x: 0, y: title.bottom() + 10, w: WIDTH, h: 1 };

    let value = Box2 {
        x: MARGIN_X,
        y: header_rule.bottom() + 11,
        w: content_w - 8 - NEW_W,
        h: VALUE_H,
    };
    let new = Box2 {
        x: value.right() + 8,
        y: value.y + (VALUE_H - CHIP_H) / 2,
        w: NEW_W,
        h: CHIP_H,
    };

    let chips_y = value.bottom() + 10;
    let kinds = [
        Box2 { x: MARGIN_X, y: chips_y, w: KIND_W[0], h: CHIP_H },
        Box2 { x: MARGIN_X + KIND_W[0] + CHIP_GAP, y: chips_y, w: KIND_W[1], h: CHIP_H },
        Box2 {
            x: MARGIN_X + KIND_W[0] + CHIP_GAP + KIND_W[1] + CHIP_GAP,
            y: chips_y,
            w: KIND_W[2],
            h: CHIP_H,
        },
    ];
    // Right-aligned, and laid out from the right edge inwards so the readout's
    // box never moves when its text changes width -- a "20 characters" that
    // shifted the `+` button under the pointer between two clicks is exactly
    // the kind of thing this card cannot afford.
    let plus =
        Box2 { x: MARGIN_X + content_w - STEP_W, y: chips_y, w: STEP_W, h: CHIP_H };
    let readout =
        Box2 { x: plus.x - CHIP_GAP - READOUT_W, y: chips_y, w: READOUT_W, h: CHIP_H };
    let minus = Box2 { x: readout.x - CHIP_GAP - STEP_W, y: chips_y, w: STEP_W, h: CHIP_H };

    let footer_rule = Box2 { x: 0, y: chips_y + CHIP_H + 11, w: WIDTH, h: 1 };
    let save =
        Box2 { x: MARGIN_X, y: footer_rule.bottom() + 10, w: SAVE_W, h: BUTTON_H };
    let copy = Box2 { x: save.right() + 8, y: save.y, w: COPY_W, h: BUTTON_H };
    let esc_chip = Box2 { x: copy.right() + 10, y: save.y, w: 34, h: BUTTON_H };
    let dismiss = Box2 {
        x: esc_chip.right() + 5,
        y: save.y,
        w: MARGIN_X + content_w - (esc_chip.right() + 5),
        h: BUTTON_H,
    };

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
        value,
        new,
        kinds,
        minus,
        readout,
        plus,
        save,
        copy,
        esc_chip,
        dismiss,
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

/// The form the paint path draws. Written by `show`, read by every painter.
///
/// **No password in it** -- see [`ValueState`]. The password is `SECRET`,
/// below, and the two are separate on purpose: this one is read on every
/// repaint, and the other is read on exactly three paths.
static VIEW: std::sync::Mutex<Option<GenerateForm>> = std::sync::Mutex::new(None);

/// The name of the app the card was opened in front of.
static APP_NAME: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// **The password, and the only place in this crate it lives while the card is
/// up.**
///
/// Written by `fill`, painted by `paint`, copied by `copy`, moved out by
/// `keep`, and wiped by `close` and by `open`. Never logged, never in a
/// `Debug`, never returned through [`GenerateCalls`].
static SECRET: std::sync::Mutex<Option<Zeroizing<String>>> = std::sync::Mutex::new(None);

/// **What the user chose to keep.** `close` wipes `SECRET` and does not touch
/// this; [`show_generate_prompt`] takes it, on every path.
static KEPT: std::sync::Mutex<Option<Zeroizing<String>>> = std::sync::Mutex::new(None);

/// # Why every pixel here is painted by hand
///
/// `crate::unlock_prompt`'s `win32` module carries the whole argument and it is
/// not restated: a themed control renders in the shell's grey with the shell's
/// font, and the last raw-Win32 surface in this project was deleted for looking
/// foreign rather than for being broken. Every control here is a real `BUTTON`
/// window -- which is what buys focus, the space bar and `IsDialogMessage`
/// traversal -- with its painting taken over completely and handed to
/// [`crate::win32_draw`], the module all three cards draw through so none can
/// drift from the palette.
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
        Box2, Event, GenerateForm, GenerateWindow, GeneratedKind, Generator, ValueState, APP_NAME,
        DISMISS_LABEL, ESC_SHORTCUT, GENERATE_COPY_LABEL, GENERATE_FAILED_TEXT, GENERATE_LABEL,
        GENERATE_NEW_LABEL, GENERATE_PROMPT_TITLE, GENERATE_SAVE_LABEL, GENERATE_WORKING_TEXT,
        GONE, KEPT, PENDING, REGENERATE_SHORTCUT, SAVE_SHORTCUT, SECRET, VIEW,
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
        SelectObject, SetBkMode, SetTextColor, CLEARTYPE_QUALITY, DT_END_ELLIPSIS, DT_LEFT,
        DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FW_BOLD, FW_NORMAL, HBRUSH, HDC, HFONT, LOGFONTW,
        LOGPIXELSX, PAINTSTRUCT, PS_SOLID, SRCCOPY, TRANSPARENT,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GetClientRect, GetDlgItem, GetWindowLongPtrW, IsDialogMessageW, LoadCursorW, PeekMessageW,
        RegisterClassW, SendMessageW, SetForegroundWindow, SetWindowDisplayAffinity,
        SetWindowLongPtrW, ShowWindow, TranslateMessage, BN_CLICKED, BS_PUSHBUTTON, CS_HREDRAW,
        CS_VREDRAW, GWLP_WNDPROC, HMENU, IDC_ARROW, MSG, PM_REMOVE, SW_SHOW, WDA_EXCLUDEFROMCAPTURE,
        WINDOW_EX_STYLE, WINDOW_STYLE, WM_COMMAND, WM_DESTROY, WM_ERASEBKGND, WM_LBUTTONDOWN,
        WM_MOUSEMOVE, WM_NCHITTEST, WM_PAINT, WM_QUIT, WM_SETFONT, WNDCLASSW, WS_CHILD,
        WS_EX_TOPMOST, WS_POPUP, WS_TABSTOP, WS_VISIBLE,
    };

    use crate::win32_draw::{
        draw_button_with_shortcut, draw_card_lockup, draw_hint_chip, rgb, ButtonSkin,
    };

    const ID_NEW: usize = 101;
    const ID_MINUS: usize = 102;
    const ID_PLUS: usize = 103;
    const ID_SAVE: usize = 104;
    const ID_COPY: usize = 105;
    /// Kind chip `i` is control `ID_KIND + i`. Above every other id, so a chip
    /// id can never collide with one of the five above however many kinds
    /// [`GeneratedKind::ALL`] grows to.
    const ID_KIND: usize = 200;

    const CLASS_NAME: PCWSTR = w!("DeskwardenPasswordGenerator");

    /// The window's DPI as a percentage of 96, sampled once per open.
    ///
    /// **The system DPI, not the monitor's**, and a known limitation rather
    /// than an oversight -- `unlock_prompt`'s own `DPI_PERCENT` carries the
    /// whole argument: `GetDpiForWindow` lives behind a `windows` crate feature
    /// this crate does not enable, and enabling it re-pins `job_object.rs`'s
    /// whole-file hash of `Cargo.toml`.
    static DPI_PERCENT: AtomicI32 = AtomicI32::new(100);

    fn scale(v: i32) -> i32 {
        v * DPI_PERCENT.load(Ordering::SeqCst) / 100
    }

    /// Which control the pointer is over, as a control id, or 0.
    static HOVERED: AtomicIsize = AtomicIsize::new(0);

    /// The subclassed controls' original procedure. One slot for all of them:
    /// every control here is the same `BUTTON` class registered by the same
    /// comctl32, so the procedure it replaces is the same pointer.
    static ORIGINAL_PROC: AtomicIsize = AtomicIsize::new(0);

    // ---- fonts -------------------------------------------------------------

    /// Registers the bundled Archivo cuts privately with GDI, once.
    ///
    /// `AddFontMemResourceEx` makes a face available to **this process only**
    /// -- nothing is installed and nothing touches the user's font list -- and
    /// the handles are deliberately never released, because freeing one while a
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

    /// The monospace face, asked of the OS by the name
    /// `crate::theme::GDI_MONO_FACE` gives it -- the same file
    /// `theme::system_monospace` hands egui.
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
        /// The lockup's wordmark: `theme::CARD_HEADER_WORD_PX` in the bold
        /// cut, which is what `theme::card_header` letterspaces "DESKWARDEN"
        /// in.
        brand: HFONT,
        title: HFONT,
        /// The password's face: monospace, so `l` and `1` are distinguishable
        /// in a value the user has to be able to read off the screen.
        value: HFONT,
        /// The working line and the failure sentence, which are prose and not a
        /// secret.
        prose: HFONT,
        chip: HFONT,
        button: HFONT,
        hint: HFONT,
    }

    impl Fonts {
        fn build() -> Self {
            use crate::theme::{BOLD, REGULAR, SEMIBOLD};
            Fonts {
                brand: font(BOLD, crate::win32_draw::card_lockup().word_px),
                title: font(BOLD, 15),
                value: mono(13),
                prose: font(REGULAR, 12),
                chip: font(SEMIBOLD, 11),
                button: font(SEMIBOLD, 12),
                hint: mono(crate::theme::CHIP_TEXT_PX as i32),
            }
        }

        fn destroy(&self) {
            unsafe {
                for f in [
                    self.brand,
                    self.title,
                    self.value,
                    self.prose,
                    self.chip,
                    self.button,
                    self.hint,
                ] {
                    let _ = DeleteObject(f);
                }
            }
        }
    }

    static FONTS: Mutex<Option<Fonts>> = Mutex::new(None);
    // `Fonts` holds raw GDI handles, which are process-wide rather than
    // thread-owned. The card is modal on one thread, so nothing shares them;
    // the `Mutex` is only what lets them live in a `static` beside a window
    // procedure that has nowhere else to keep state.
    unsafe impl std::marker::Send for Fonts {}

    // ---- the window --------------------------------------------------------

    pub(super) fn open(app_name: &str) -> Option<GenerateWindow> {
        register_fonts();
        GONE.store(false, Ordering::SeqCst);
        HOVERED.store(0, Ordering::SeqCst);
        if let Ok(mut slot) = APP_NAME.lock() {
            *slot = app_name.to_string();
        }
        if let Ok(mut slot) = PENDING.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = VIEW.lock() {
            *slot = Some(GenerateForm::new(GeneratedKind::Characters));
        }
        // A password left over from a card that was not closed cleanly is wiped
        // here rather than being painted into the new one.
        wipe_secret();

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
        let (x, y) = centred(w, h);

        let window = unsafe {
            CreateWindowExW(
                // Topmost, because it is a question asked over whatever the
                // user was doing. It takes focus deliberately: the controls are
                // answered with Tab, Enter and Ctrl+R as well as with the
                // pointer.
                WS_EX_TOPMOST,
                CLASS_NAME,
                &HSTRING::from(GENERATE_PROMPT_TITLE),
                // Frameless. A `WS_CAPTION` frame is the loudest "system
                // dialog" signal there is, and this app's own windows are
                // frameless with drawn chrome.
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
        // `Unavailable`, and leave a frameless topmost card with no controls and
        // no way for the user to dismiss it -- `close` is only reached with a
        // `GenerateWindow` in hand. Every failure path from here on goes through
        // `abandon`, which takes the window down and frees the fonts before
        // answering `None`.
        fn abandon(window: HWND) -> Option<GenerateWindow> {
            unsafe {
                let _ = DestroyWindow(window);
            }
            if let Ok(mut slot) = FONTS.lock() {
                if let Some(fonts) = slot.take() {
                    fonts.destroy();
                }
            }
            wipe_secret();
            if let Ok(mut slot) = APP_NAME.lock() {
                slot.clear();
            }
            if let Ok(mut slot) = VIEW.lock() {
                *slot = None;
            }
            None
        }

        // The handles are copied out and the guard dropped at the end of this
        // statement: `abandon` locks `FONTS` itself, so holding the guard across
        // the `child` calls below would deadlock the failure path.
        let Some((chip_font, button_font)) =
            FONTS.lock().ok().and_then(|guard| guard.as_ref().map(|f| (f.chip, f.button)))
        else {
            return abandon(window);
        };

        let controls: [(usize, Box2, HFONT); 5] = [
            (ID_NEW, l.new, chip_font),
            (ID_MINUS, l.minus, chip_font),
            (ID_PLUS, l.plus, chip_font),
            (ID_SAVE, l.save, button_font),
            (ID_COPY, l.copy, button_font),
        ];
        for (id, at, face) in controls {
            let Some(control) = child(window, at, id, face) else {
                return abandon(window);
            };
            subclass(control);
        }
        for (index, at) in l.kinds.iter().enumerate() {
            let Some(control) = child(window, *at, ID_KIND + index, chip_font) else {
                return abandon(window);
            };
            subclass(control);
        }

        unsafe {
            let _ = ShowWindow(window, SW_SHOW);
            // Allowed to refuse, and handled rather than asserted -- the
            // property `foreground` records. A refusal leaves a topmost card on
            // screen that the user clicks once to focus.
            let _ = SetForegroundWindow(window);
            // The keyboard starts on *Save to vault*, which is what Enter does
            // anyway -- so the focus ring and the default action agree.
            if let Ok(control) = GetDlgItem(window, ID_SAVE as i32) {
                let _ = SetFocus(control);
            }
        }

        Some(GenerateWindow(handle_of(window)))
    }

    /// **The protection, on the top-level window.**
    ///
    /// Applied to the card itself and never to a child: Windows refuses
    /// `SetWindowDisplayAffinity` on a child control with `E_INVALIDARG`, and
    /// the top-level flag covers every child it owns. What it protects here is
    /// a **password in plain view** -- this is the one surface in the app that
    /// paints a live secret unmasked, so of the three Win32 cards this is the
    /// one the exclusion matters most on.
    pub(super) fn protect(window: GenerateWindow) -> bool {
        unsafe { SetWindowDisplayAffinity(hwnd(window.0), WDA_EXCLUDEFROMCAPTURE).is_ok() }
    }

    /// Pumps until the user does something.
    ///
    /// **This blocks.** It does not return until the window procedure has
    /// recorded an event or the window has gone away, and the event it hands
    /// back is *taken* out of `PENDING` rather than read from it -- so no event
    /// can be delivered twice.
    ///
    /// **`IsDialogMessageW` is what makes Tab, Shift+Tab, Space and Enter work
    /// at all.** Escape and Ctrl+R are handled before it: `IsDialogMessage`
    /// only cancels for a real dialog box, and Ctrl+R is this card's own chord.
    pub(super) fn next(window: GenerateWindow) -> Event {
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_ESCAPE};
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
                    if msg.message == WM_KEYDOWN && msg.wParam.0 as u16 == VK_ESCAPE.0 {
                        return Event::Cancel;
                    }
                    // **Ctrl+R, and only while Ctrl is really down.** Read at
                    // the moment the key arrives rather than tracked across
                    // messages: a modifier released while this window was not
                    // focused would otherwise leave a flag set that nothing
                    // clears. A bare `R` must not regenerate -- the user has
                    // just come from a text field on design 3c.
                    if msg.message == WM_KEYDOWN
                        && msg.wParam.0 as u16 == b'R' as u16
                        && GetKeyState(VK_CONTROL.0 as i32) < 0
                    {
                        return Event::Regenerate;
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

    /// Parks the form the paint path draws, and asks for a repaint.
    pub(super) fn show(window: GenerateWindow, form: &GenerateForm) {
        if let Ok(mut slot) = VIEW.lock() {
            *slot = Some(form.clone());
        }
        let top = hwnd(window.0);
        repaint_all(top);
    }

    /// **Runs the generator and parks the answer.**
    ///
    /// The password goes straight into `SECRET` and is never returned: what
    /// comes back is `None`, or the sentence the card shows.
    ///
    /// **This blocks the pump**, deliberately and visibly: `show` has already
    /// painted the in-flight state, and `bw serve`'s generate is one local round
    /// trip. A worker thread here would buy a card that repaints while it waits
    /// and cost a second place the password lives.
    pub(super) fn fill(
        _window: GenerateWindow,
        request: &crate::vault_bridge::GenerateRequest,
        generate: &Generator<'_>,
    ) -> Option<String> {
        match generate(request) {
            Ok(password) => {
                if let Ok(mut slot) = SECRET.lock() {
                    // The previous value is dropped by this assignment, and
                    // `Zeroizing`'s `Drop` is what wipes it.
                    *slot = Some(password);
                }
                None
            }
            Err(detail) => {
                // **The detail to the log, the sentence to the card.** The
                // caller's error can carry a URL, a status and a response body.
                log::warn!("the password generator could not be reached: {detail}");
                wipe_secret();
                Some(GENERATE_FAILED_TEXT.to_string())
            }
        }
    }

    /// Puts the parked password on the clipboard, through this crate's one
    /// clipboard path and its clearing behaviour.
    pub(super) fn copy(_window: GenerateWindow) {
        let Ok(slot) = SECRET.lock() else { return };
        if let Some(password) = slot.as_ref() {
            crate::clipboard::copy_secret(password.as_str());
        }
    }

    /// Moves the parked password into the kept slot. `close` wipes the parked
    /// one and does not touch this.
    pub(super) fn keep(_window: GenerateWindow) {
        let (Ok(mut secret), Ok(mut kept)) = (SECRET.lock(), KEPT.lock()) else {
            return;
        };
        *kept = secret.take();
    }

    /// Takes what was kept, leaving nothing behind.
    pub(super) fn take_kept() -> Option<Zeroizing<String>> {
        KEPT.lock().ok().and_then(|mut slot| slot.take())
    }

    /// Wipes the parked password. Assignment drops the old `Zeroizing`, whose
    /// `Drop` is the wipe.
    fn wipe_secret() {
        if let Ok(mut slot) = SECRET.lock() {
            *slot = None;
        }
    }

    pub(super) fn close(window: GenerateWindow) {
        unsafe {
            let _ = DestroyWindow(hwnd(window.0));
        }
        if let Ok(mut slot) = FONTS.lock() {
            if let Some(fonts) = slot.take() {
                fonts.destroy();
            }
        }
        wipe_secret();
        if let Ok(mut slot) = VIEW.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = PENDING.lock() {
            *slot = None;
        }
        // Not a secret, but it is the name of an app this user was in front of,
        // and nothing needs it once the card is down.
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

    /// The card and every control on it. `show` changes what most of them draw
    /// -- the chips' selection, the stepper's enabled state, both footer buttons
    /// -- so invalidating the parent alone would leave the old ones on screen.
    fn repaint_all(window: HWND) {
        repaint(window);
        unsafe {
            for id in [ID_NEW, ID_MINUS, ID_PLUS, ID_SAVE, ID_COPY] {
                if let Ok(control) = GetDlgItem(window, id as i32) {
                    repaint(control);
                }
            }
            for index in 0..GeneratedKind::ALL.len() {
                if let Ok(control) = GetDlgItem(window, (ID_KIND + index) as i32) {
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

    /// The form the paint path draws. An absent one is an in-flight card, which
    /// is what the window opens as.
    fn view() -> GenerateForm {
        VIEW.lock()
            .ok()
            .and_then(|slot| slot.clone())
            .unwrap_or_else(|| GenerateForm::new(GeneratedKind::Characters))
    }

    fn centred(w: i32, h: i32) -> (i32, i32) {
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
            (
                area.left + (area.right - area.left - w) / 2,
                // Slightly above centre, where every OS credential prompt puts
                // itself. **The egui card this replaced anchored itself beside
                // the field the user was in**; that anchoring is deliberately
                // dropped, exactly as `picker_prompt` dropped the no-match
                // card's, so the daemon's three cards all appear in one place
                // rather than wherever the app that raised them happens to be.
                area.top + (area.bottom - area.top - h) * 2 / 5,
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

    /// One child control. It is created with **no text**: every label on this
    /// card is painted by `paint_control` from the app's own palette and type,
    /// so a control's own caption would only ever be a second, stale copy.
    fn child(parent: HWND, at: Box2, id: usize, font: HFONT) -> Option<HWND> {
        let h = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("BUTTON"),
                w!(""),
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_PUSHBUTTON as u32),
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

    /// Takes over a control's painting without losing the focus and keyboard
    /// behaviour that makes `IsDialogMessage` work.
    fn subclass(control: HWND) {
        unsafe {
            let previous =
                SetWindowLongPtrW(control, GWLP_WNDPROC, control_proc as *const () as isize);
            if previous != 0 {
                ORIGINAL_PROC.store(previous, Ordering::SeqCst);
            }
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
                LRESULT(0)
            }
            WM_DESTROY => {
                // **NO `PostQuitMessage` HERE, EVER.** This window is opened on
                // the daemon thread, and that thread goes on to run egui windows
                // -- design 3c's save-a-login form, which is where this card's
                // password is handed back to. `close()` calls `DestroyWindow`,
                // which dispatches this message synchronously on that thread, so
                // a `PostQuitMessage` here leaves the thread's quit flag set with
                // nothing left to drain it: `next()` has already returned and no
                // pump of ours runs again. The next `eframe::run_native` then
                // takes that stale `WM_QUIT` out of `GetMessageW`, leaves its
                // loop before it draws a frame, and returns its default answer --
                // so 3c never reappears and the password the user just chose to
                // keep is silently dropped.
                //
                // Quitting is not this handler's job in the first place: `GONE`
                // on the line below is what `next()` reads to report
                // `Event::Closed`, and the `WM_QUIT` branch in `next()` stays for
                // a quit posted from outside.
                GONE.store(true, Ordering::SeqCst);
                LRESULT(0)
            }
            _ => DefWindowProcW(window, msg, wparam, lparam),
        }
    }

    /// **What a click on control `id` means.**
    ///
    /// A control whose form state has it dead posts nothing rather than posting
    /// an event `run_with` would then have to refuse: the refusal lives in
    /// `GenerateForm` too, so this is the drawn state agreeing with the
    /// invariant rather than a second copy of it.
    fn clicked(id: usize) {
        let form = view();
        if id == ID_NEW {
            if !form.in_flight() {
                set_pending(Event::Regenerate);
            }
            return;
        }
        if id == ID_MINUS || id == ID_PLUS {
            let delta = if id == ID_PLUS { 1 } else { -1 };
            if form.can_resize(delta) {
                set_pending(Event::Resize(delta));
            }
            return;
        }
        if id == ID_SAVE {
            if form.ready() {
                set_pending(Event::Save);
            }
            return;
        }
        if id == ID_COPY {
            if form.ready() {
                set_pending(Event::Copy);
            }
            return;
        }
        if id >= ID_KIND {
            if let Some(kind) = GeneratedKind::ALL.get(id - ID_KIND).copied() {
                if !form.in_flight() {
                    set_pending(Event::Choose(kind));
                }
            }
        }
    }

    /// The subclassed controls: everything except painting and hover is the
    /// original `BUTTON` procedure's, which is what keeps focus, the space bar
    /// and `IsDialogMessage`'s traversal working.
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
            _ => {
                let original = ORIGINAL_PROC.load(Ordering::SeqCst);
                if original == 0 {
                    DefWindowProcW(control, msg, wparam, lparam)
                } else {
                    CallWindowProcW(
                        Some(std::mem::transmute::<
                            isize,
                            unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
                        >(original)),
                        control,
                        msg,
                        wparam,
                        lparam,
                    )
                }
            }
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
    /// the value box and the footer hint. Every control paints itself.
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
            let form = view();
            let dpi = DPI_PERCENT.load(Ordering::SeqCst);

            // The window IS the card, so its whole client area is `theme::CARD`
            // rather than the window background the picker's list sits on.
            fill_rect(mem, client, crate::theme::CARD);
            fill_box(mem, l.footer, crate::theme::CARD_TINT);
            fill_box(mem, l.header_rule, crate::theme::HAIRLINE);
            fill_box(mem, l.footer_rule, crate::theme::HAIRLINE);
            SetBkMode(mem, TRANSPARENT);

            // The value box. `theme::CANVAS`, the same inset well the picker's
            // rows sit in, so the one thing on this card the user has to read
            // reads as the content and not as chrome.
            rounded(mem, l.value, 8, crate::theme::CANVAS, None);

            if let Some(fonts) = fonts {
                paint_lockup(mem, &l, fonts.brand);
                text(mem, fonts.title, l.title, GENERATE_LABEL, crate::theme::INK);

                // **The one place a secret is painted in this crate.** The
                // UTF-16 copy `DrawTextW` needs is a `Zeroizing<Vec<u16>>`, so
                // the buffer the glyphs are rasterised from is wiped when this
                // block ends rather than left on the stack.
                let inner = Box2 {
                    x: l.value.x + 10,
                    y: l.value.y,
                    w: l.value.w - 20,
                    h: l.value.h,
                };
                match form.state() {
                    ValueState::InFlight => text_clipped(
                        mem,
                        fonts.prose,
                        inner,
                        GENERATE_WORKING_TEXT,
                        crate::theme::TEXT_FAINT,
                    ),
                    ValueState::Failed(message) => text_clipped(
                        mem,
                        fonts.prose,
                        inner,
                        message.as_str(),
                        crate::theme::ERROR,
                    ),
                    ValueState::Ready => {
                        let secret = SECRET.lock().ok().and_then(|slot| {
                            slot.as_ref().map(|p| {
                                Zeroizing::new(p.encode_utf16().collect::<Vec<u16>>())
                            })
                        });
                        if let Some(mut chars) = secret {
                            text_utf16(mem, fonts.value, inner, &mut chars, crate::theme::INK);
                        }
                    }
                }

                // The readout, between the stepper's two buttons and painted by
                // the parent rather than being a control of its own: it is text,
                // and a `BUTTON` under it would be a tab stop that does nothing.
                text(
                    mem,
                    fonts.chip,
                    Box2 { x: l.readout.x + 4, ..l.readout },
                    &form.readout(),
                    crate::theme::TEXT_SECONDARY,
                );

                // `Esc Dismiss`. The chip is `win32_draw`'s, so it is the same
                // chip the two other cards draw, and the word beside it is the
                // hint's own.
                let chip = RECT {
                    left: scale(l.esc_chip.x),
                    top: scale(l.esc_chip.y),
                    right: scale(l.esc_chip.right()),
                    bottom: scale(l.esc_chip.bottom()),
                };
                draw_hint_chip(mem, chip, ESC_SHORTCUT, fonts.hint, dpi);
                text(mem, fonts.prose, l.dismiss, DISMISS_LABEL, crate::theme::TEXT_FAINT);
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

    /// One child control.
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
            let form = view();
            let l = super::layout();
            let dpi = DPI_PERCENT.load(Ordering::SeqCst);

            // The footer's two buttons sit on the tint, everything else on the
            // card -- otherwise a button's rounded corners show the wrong
            // colour through them.
            let under = if id == ID_SAVE || id == ID_COPY {
                crate::theme::CARD_TINT
            } else {
                crate::theme::CARD
            };
            fill_rect(mem, whole, under);
            SetBkMode(mem, TRANSPARENT);

            if let Some(fonts) = fonts {
                let (label, hint, skin, face, box2) = control_skin(id, &form, &l, fonts);
                let skin = if hovered && enabled(id, &form) { skin.hovered() } else { skin };
                let hint = hint.map(|text| (text, fonts.hint));
                let radius = if id == ID_SAVE || id == ID_COPY { 8 } else { 7 };
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
                        mem,
                        inner,
                        &label,
                        face,
                        skin,
                        scale(radius),
                        hint,
                        dpi,
                    );
                } else {
                    draw_button_with_shortcut(
                        mem,
                        whole,
                        &label,
                        face,
                        skin,
                        scale(radius),
                        hint,
                        dpi,
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

    /// Whether control `id` is live for this form state. **The same predicate
    /// [`clicked`] refuses on**, so a control that is drawn dead is a control
    /// that does nothing, and neither is a copy of the other.
    fn enabled(id: usize, form: &GenerateForm) -> bool {
        if id == ID_SAVE || id == ID_COPY {
            form.ready()
        } else if id == ID_MINUS {
            form.can_resize(-1)
        } else if id == ID_PLUS {
            form.can_resize(1)
        } else {
            !form.in_flight()
        }
    }

    /// What control `id` says, how it is drawn, and the logical box it occupies.
    fn control_skin(
        id: usize,
        form: &GenerateForm,
        l: &super::Layout,
        fonts: &Fonts,
    ) -> (String, Option<&'static str>, ButtonSkin, HFONT, Box2) {
        let live = enabled(id, form);
        let dim = |skin: ButtonSkin| if live { skin } else { skin.disabled() };
        if id == ID_NEW {
            return (
                GENERATE_NEW_LABEL.to_string(),
                Some(REGENERATE_SHORTCUT),
                dim(ButtonSkin::secondary()),
                fonts.chip,
                l.new,
            );
        }
        if id == ID_MINUS {
            return ("−".to_string(), None, dim(ButtonSkin::secondary()), fonts.chip, l.minus);
        }
        if id == ID_PLUS {
            return ("+".to_string(), None, dim(ButtonSkin::secondary()), fonts.chip, l.plus);
        }
        if id == ID_SAVE {
            return (
                GENERATE_SAVE_LABEL.to_string(),
                Some(SAVE_SHORTCUT),
                dim(ButtonSkin::primary()),
                fonts.button,
                l.save,
            );
        }
        if id == ID_COPY {
            return (
                GENERATE_COPY_LABEL.to_string(),
                None,
                dim(ButtonSkin::secondary()),
                fonts.button,
                l.copy,
            );
        }
        let index = id.saturating_sub(ID_KIND).min(GeneratedKind::ALL.len() - 1);
        let kind = GeneratedKind::ALL[index];
        // The selected chip is the blue one, which is the same `primary` skin
        // the footer's *Save* uses -- one selected treatment in the crate.
        let skin =
            if kind == form.kind() { ButtonSkin::primary() } else { ButtonSkin::secondary() };
        (kind.label().to_string(), None, dim(skin), fonts.chip, l.kinds[index])
    }

    /// The brand lockup, through [`crate::win32_draw::draw_card_lockup`] --
    /// the crate's one mark painter, which `unlock_prompt` also draws through.
    /// What is this card's own is only the logical-to-device conversion, which
    /// no other card's `Box2` type can share.
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

    /// One run of text, left-aligned and vertically centred in `at`.
    fn text(hdc: HDC, font: HFONT, at: Box2, run: &str, colour: eframe::egui::Color32) {
        // **Nothing to draw, and drawing nothing would crash.**
        //
        // An empty `Vec<u16>` has no allocation, so `as_mut_ptr` gives
        // Rust's dangling sentinel -- the type's alignment, which for
        // `u16` is the literal address 2. `DrawTextW` reads through that
        // pointer even when it is told the length is zero, so an empty
        // string here is an access violation at address 0x2 inside
        // `DrawTextExWorker`.
        //
        // It kills the whole app rather than the card: the fault happens
        // inside a window procedure, so Windows raises
        // STATUS_FATAL_USER_CALLBACK_EXCEPTION and terminates the process
        // without unwinding -- the panic hook never runs and nothing
        // reaches the log. The owner met it as the tray, the vault window
        // and an unlocked session vanishing on one CTRL+ALT+B.
        if run.is_empty() {
        return;
        }
        let mut chars: Vec<u16> = run.encode_utf16().collect();
        text_utf16(hdc, font, at, &mut chars, colour);
    }

    /// [`text`], truncated with an ellipsis rather than clipped mid-letter.
    ///
    /// Every run this card paints is bounded: the card cannot scroll and cannot
    /// resize, so a sentence that ran past the value box would simply be
    /// unreadable.
    fn text_clipped(hdc: HDC, font: HFONT, at: Box2, run: &str, colour: eframe::egui::Color32) {
        // **Nothing to draw, and drawing nothing would crash.**
        //
        // An empty `Vec<u16>` has no allocation, so `as_mut_ptr` gives
        // Rust's dangling sentinel -- the type's alignment, which for
        // `u16` is the literal address 2. `DrawTextW` reads through that
        // pointer even when it is told the length is zero, so an empty
        // string here is an access violation at address 0x2 inside
        // `DrawTextExWorker`.
        //
        // It kills the whole app rather than the card: the fault happens
        // inside a window procedure, so Windows raises
        // STATUS_FATAL_USER_CALLBACK_EXCEPTION and terminates the process
        // without unwinding -- the panic hook never runs and nothing
        // reaches the log. The owner met it as the tray, the vault window
        // and an unlocked session vanishing on one CTRL+ALT+B.
        if run.is_empty() {
        return;
        }
        let mut chars: Vec<u16> = run.encode_utf16().collect();
        text_utf16(hdc, font, at, &mut chars, colour);
    }

    /// The one text painter. Takes the UTF-16 buffer by `&mut` because
    /// `DrawTextW` writes into it -- and because the password's buffer is a
    /// `Zeroizing<Vec<u16>>` its caller owns and wipes.
    fn text_utf16(
        hdc: HDC,
        font: HFONT,
        at: Box2,
        chars: &mut [u16],
        colour: eframe::egui::Color32,
    ) {
        unsafe {
            let old = SelectObject(hdc, font);
            SetTextColor(hdc, rgb(colour));
            let mut rc = RECT {
                left: scale(at.x),
                top: scale(at.y),
                right: scale(at.right()),
                bottom: scale(at.bottom()),
            };
            // `DT_NOPREFIX`: these are the app's own words -- and one of them is
            // a generated password, in which an `&` is an ampersand and never a
            // mnemonic that would be drawn as an underscore.
            DrawTextW(
                hdc,
                chars,
                &mut rc,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
            SelectObject(hdc, old);
        }
    }

    use zeroize::Zeroizing;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A generator that answers the same fixture every time. A `fn` pointer's
    /// worth of behaviour, so nothing here reaches a vault, a network or `bw`.
    fn ok_generator(_: &GenerateRequest) -> Result<Zeroizing<String>, String> {
        Ok(Zeroizing::new("fixture-password".to_string()))
    }

    fn failing_generator(_: &GenerateRequest) -> Result<Zeroizing<String>, String> {
        Err("no route".to_string())
    }

    /// A [`GenerateCalls`] whose every pointer does nothing, for a test to
    /// override the one it is about.
    fn inert() -> GenerateCalls {
        GenerateCalls {
            open: |_| Some(GenerateWindow(1)),
            protect: |_| true,
            next: |_| Event::Cancel,
            show: |_, _| {},
            fill: |_, request, generate| match generate(request) {
                Ok(_) => None,
                Err(_) => Some(GENERATE_FAILED_TEXT.to_string()),
            },
            copy: |_| {},
            keep: |_| {},
            close: |_| {},
        }
    }

    // ---- the decision ------------------------------------------------------

    #[test]
    fn the_window_is_protected_before_it_is_ever_pumped() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static ORDER: AtomicUsize = AtomicUsize::new(0);
        static PROTECTED_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
        static PUMPED_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
        let calls = GenerateCalls {
            protect: |_| {
                PROTECTED_AT.store(ORDER.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
                true
            },
            next: |_| {
                // Record only the FIRST pump. If every pump overwrote this, the
                // last write would win and the assertion below would only mean
                // "protect happened before the final pump" -- which passes even
                // if an earlier pump ran before protect.
                let _ = PUMPED_AT.compare_exchange(
                    usize::MAX,
                    ORDER.fetch_add(1, Ordering::SeqCst),
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                Event::Cancel
            },
            ..inert()
        };
        let _ = run_with(&calls, "Ledgerline", &ok_generator);
        assert!(
            PROTECTED_AT.load(Ordering::SeqCst) < PUMPED_AT.load(Ordering::SeqCst),
            "this card paints a password in the clear; a window that can be read before it is \
             excluded from capture is a window a recorder catches the password in"
        );
    }

    #[test]
    fn a_window_that_cannot_be_opened_is_unavailable_and_not_a_silent_nothing() {
        let calls = GenerateCalls { open: |_| None, ..inert() };
        assert_eq!(run_with(&calls, "Ledgerline", &ok_generator), Outcome::Unavailable);
    }

    #[test]
    fn every_exit_path_closes_the_window() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CLOSED: AtomicUsize = AtomicUsize::new(0);
        static STEP: AtomicUsize = AtomicUsize::new(0);

        CLOSED.store(0, Ordering::SeqCst);
        let cancel = GenerateCalls {
            close: |_| {
                CLOSED.fetch_add(1, Ordering::SeqCst);
            },
            ..inert()
        };
        assert_eq!(run_with(&cancel, "Ledgerline", &ok_generator), Outcome::Cancelled);
        assert_eq!(CLOSED.load(Ordering::SeqCst), 1, "Cancel did not close the window");

        CLOSED.store(0, Ordering::SeqCst);
        let closed = GenerateCalls {
            next: |_| Event::Closed,
            close: |_| {
                CLOSED.fetch_add(1, Ordering::SeqCst);
            },
            ..inert()
        };
        assert_eq!(run_with(&closed, "Ledgerline", &ok_generator), Outcome::Cancelled);
        assert_eq!(CLOSED.load(Ordering::SeqCst), 1, "a vanished window was not closed");

        CLOSED.store(0, Ordering::SeqCst);
        STEP.store(0, Ordering::SeqCst);
        let save = GenerateCalls {
            next: |_| match STEP.fetch_add(1, Ordering::SeqCst) {
                0 => Event::Save,
                _ => Event::Cancel,
            },
            close: |_| {
                CLOSED.fetch_add(1, Ordering::SeqCst);
            },
            ..inert()
        };
        assert_eq!(run_with(&save, "Ledgerline", &ok_generator), Outcome::Kept);
        assert_eq!(
            CLOSED.load(Ordering::SeqCst),
            1,
            "the Save path is the one that most needs close -- the window's lifetime bounds the \
             process's only copy of a live password"
        );
    }

    /// **Enter on a card with nothing on it saves nothing.**
    ///
    /// A `Save` that got through here would hand design 3c an empty password and
    /// close the generator: a credential the user did not choose being written
    /// to their vault by the key they pressed to accept the one they could see.
    #[test]
    fn a_card_with_no_password_cannot_be_saved_or_copied() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static STEP: AtomicUsize = AtomicUsize::new(0);
        static KEPT_CALLS: AtomicUsize = AtomicUsize::new(0);
        static COPIED: AtomicUsize = AtomicUsize::new(0);
        STEP.store(0, Ordering::SeqCst);
        KEPT_CALLS.store(0, Ordering::SeqCst);
        COPIED.store(0, Ordering::SeqCst);

        let calls = GenerateCalls {
            next: |_| match STEP.fetch_add(1, Ordering::SeqCst) {
                0 => Event::Save,
                1 => Event::Copy,
                _ => Event::Cancel,
            },
            keep: |_| {
                KEPT_CALLS.fetch_add(1, Ordering::SeqCst);
            },
            copy: |_| {
                COPIED.fetch_add(1, Ordering::SeqCst);
            },
            ..inert()
        };
        // The generator fails, so the card is in `Failed` when Save arrives.
        assert_eq!(run_with(&calls, "Ledgerline", &failing_generator), Outcome::Cancelled);
        assert_eq!(KEPT_CALLS.load(Ordering::SeqCst), 0, "a failed card saved a password");
        assert_eq!(
            COPIED.load(Ordering::SeqCst),
            0,
            "a failed card put an empty string on the clipboard, clearing whatever the user had \
             there and giving them nothing for it"
        );
    }

    /// **The in-flight state is painted before the round trip blocks.**
    ///
    /// Otherwise it is a state that exists only between two statements, and the
    /// user of a frameless always-on-top card sees it freeze between the click
    /// and the answer.
    #[test]
    fn the_card_is_shown_in_flight_before_the_generator_is_asked() {
        static SEEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        if let Ok(mut seen) = SEEN.lock() {
            seen.clear();
        }
        let calls = GenerateCalls {
            show: |_, form| {
                if let Ok(mut seen) = SEEN.lock() {
                    seen.push(format!("show:{:?}", form.state()));
                }
            },
            fill: |_, _, _| {
                if let Ok(mut seen) = SEEN.lock() {
                    seen.push("fill".to_string());
                }
                None
            },
            ..inert()
        };
        let _ = run_with(&calls, "Ledgerline", &ok_generator);
        let seen = SEEN.lock().unwrap().clone();
        assert_eq!(
            seen,
            vec![
                "show:InFlight".to_string(),
                "fill".to_string(),
                "show:Ready".to_string(),
            ],
            "the card did not paint its in-flight state before blocking on the generator"
        );
    }

    /// **A failure is a state the user can leave.**
    ///
    /// The tray's update item shipped the opposite shape -- created disabled and
    /// only ever enabled on success -- and a user who hit its failure path was
    /// left with a control that never came back.
    #[test]
    fn a_failed_card_can_still_be_regenerated_into_a_saveable_one() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static STEP: AtomicUsize = AtomicUsize::new(0);
        static FILLS: AtomicUsize = AtomicUsize::new(0);
        STEP.store(0, Ordering::SeqCst);
        FILLS.store(0, Ordering::SeqCst);
        let calls = GenerateCalls {
            next: |_| match STEP.fetch_add(1, Ordering::SeqCst) {
                0 => Event::Regenerate,
                _ => Event::Save,
            },
            // The first round trip fails and the second succeeds.
            fill: |_, _, _| {
                if FILLS.fetch_add(1, Ordering::SeqCst) == 0 {
                    Some(GENERATE_FAILED_TEXT.to_string())
                } else {
                    None
                }
            },
            ..inert()
        };
        assert_eq!(run_with(&calls, "Ledgerline", &ok_generator), Outcome::Kept);
        assert_eq!(FILLS.load(Ordering::SeqCst), 2, "the failed card refused to regenerate");
    }

    /// **Every regenerating path asks again, and each asks for what it changed.**
    #[test]
    fn changing_the_kind_and_the_size_each_ask_for_a_new_password() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static STEP: AtomicUsize = AtomicUsize::new(0);
        static ASKED: std::sync::Mutex<Vec<GenerateRequest>> = std::sync::Mutex::new(Vec::new());
        STEP.store(0, Ordering::SeqCst);
        if let Ok(mut asked) = ASKED.lock() {
            asked.clear();
        }
        let calls = GenerateCalls {
            next: |_| match STEP.fetch_add(1, Ordering::SeqCst) {
                0 => Event::Choose(GeneratedKind::Words),
                1 => Event::Resize(1),
                // Refused: the same chip again is not a change, so nothing is
                // asked for.
                2 => Event::Choose(GeneratedKind::Words),
                _ => Event::Cancel,
            },
            fill: |_, request, _| {
                if let Ok(mut asked) = ASKED.lock() {
                    asked.push(request.clone());
                }
                None
            },
            ..inert()
        };
        let _ = run_with(&calls, "Ledgerline", &ok_generator);
        let asked = ASKED.lock().unwrap().clone();
        assert_eq!(
            asked,
            vec![
                GeneratedKind::Characters.recipe(GeneratedKind::Characters.default_size()),
                GeneratedKind::Words.recipe(GeneratedKind::Words.default_size()),
                GeneratedKind::Words.recipe(GeneratedKind::Words.default_size() + 1),
            ],
            "the card asked for the wrong recipes, or asked again for a chip that did not change"
        );
    }

    // ---- the form ----------------------------------------------------------

    #[test]
    fn a_second_generate_cannot_start_while_one_is_outstanding() {
        let mut form = GenerateForm::new(GeneratedKind::Characters);
        assert!(form.in_flight(), "the card does not open generating");
        assert!(!form.begin(), "a second request started while one was outstanding");
        assert!(!form.choose(GeneratedKind::Words), "changing kind started a second request");
        assert!(!form.resize(1), "changing size started a second request");
        assert_eq!(form.kind(), GeneratedKind::Characters);
        assert_eq!(form.size(), GeneratedKind::Characters.default_size());

        form.finish(None);
        assert!(form.ready());
        assert!(form.choose(GeneratedKind::Words), "a settled card refused to change kind");
        assert_eq!(form.size(), GeneratedKind::Words.default_size());
    }

    #[test]
    fn the_size_stepper_stops_at_the_bounds() {
        let mut checked = 0;
        for kind in GeneratedKind::ALL {
            let (low, high) = kind.bounds();
            let mut form = GenerateForm::new(kind);
            form.finish(None);
            for _ in 0..200 {
                if !form.resize(1) {
                    break;
                }
                form.finish(None);
            }
            assert_eq!(form.size(), high, "{kind:?} stepped past its own upper bound");
            assert!(!form.can_resize(1), "{kind:?} still offers `+` at its upper bound");
            for _ in 0..200 {
                if !form.resize(-1) {
                    break;
                }
                form.finish(None);
            }
            assert_eq!(form.size(), low, "{kind:?} stepped past its own lower bound");
            assert!(!form.can_resize(-1), "{kind:?} still offers `−` at its lower bound");
            checked += 1;
        }
        assert_eq!(checked, GeneratedKind::ALL.len());
    }

    #[test]
    fn the_size_readout_is_labelled_by_the_kind() {
        let mut form = GenerateForm::new(GeneratedKind::Characters);
        assert_eq!(form.readout(), "20 characters");
        form.finish(None);
        form.choose(GeneratedKind::Words);
        assert_eq!(
            form.readout(),
            "4 words",
            "the readout counts characters while a passphrase is selected"
        );
        form.finish(None);
        form.choose(GeneratedKind::Pin);
        assert_eq!(form.readout(), "6 characters");
    }

    #[test]
    fn each_kind_asks_for_the_recipe_it_names() {
        use crate::vault_bridge::GenerateRequest;
        match GeneratedKind::Words.recipe(6) {
            GenerateRequest::Passphrase(recipe) => assert_eq!(recipe.words, 6),
            other => panic!("Words asked for {other:?}"),
        }
        let characters = match GeneratedKind::Characters.recipe(24) {
            GenerateRequest::Password(recipe) => recipe,
            other => panic!("Characters asked for {other:?}"),
        };
        assert_eq!(characters.length, 24);
        assert!(
            characters.uppercase && characters.lowercase && characters.number
                && characters.special,
            "the general-purpose chip is called *Characters* precisely because every class is \
             on; a chip reading `Letters` over a password containing `7` and `!` would be the \
             card lying about its own output"
        );
        let pin = match GeneratedKind::Pin.recipe(6) {
            GenerateRequest::Password(recipe) => recipe,
            other => panic!("PIN asked for {other:?}"),
        };
        assert_eq!(pin.length, 6);
        assert!(pin.number && !pin.uppercase && !pin.lowercase && !pin.special);
        assert_eq!(pin.min_special, 0, "a PIN asked for a special character it had excluded");
        assert!(!pin.avoid_ambiguous, "a digits-only alphabet has nothing to disambiguate");
    }

    /// **No size the card offers is one the route would silently raise.**
    ///
    /// `bw serve` clamps a password `length` below 5 up to 5 and a passphrase
    /// `words` below 3 up to 3, with no error and a 200 -- so a stepper that
    /// could reach 4 digits would be a control that visibly says one thing and
    /// silently produces another.
    #[test]
    fn no_size_the_card_offers_is_one_the_route_would_raise() {
        use crate::vault_bridge::GenerateRequest;
        let mut checked = 0;
        for kind in GeneratedKind::ALL {
            let (low, high) = kind.bounds();
            assert!(low <= high, "{kind:?} has an empty range");
            assert!(
                low <= kind.default_size() && kind.default_size() <= high,
                "{kind:?} opens at a size its own stepper cannot reach"
            );
            for size in [low, kind.default_size(), high] {
                match kind.recipe(size) {
                    GenerateRequest::Passphrase(recipe) => {
                        assert!(recipe.words >= 3, "{kind:?} at {size} would be raised to 3 words");
                    }
                    GenerateRequest::Password(recipe) => {
                        assert!(
                            recipe.length >= 5,
                            "{kind:?} at {size} would be raised to 5 characters"
                        );
                    }
                }
                checked += 1;
            }
        }
        assert_eq!(checked, GeneratedKind::ALL.len() * 3);
    }

    /// **A recipe out of bounds is clamped before it is sent**, so no caller can
    /// build one the route would rewrite.
    #[test]
    fn a_recipe_outside_the_bounds_is_clamped_rather_than_sent() {
        use crate::vault_bridge::GenerateRequest;
        match GeneratedKind::Pin.recipe(1) {
            GenerateRequest::Password(recipe) => {
                assert_eq!(recipe.length, GeneratedKind::Pin.bounds().0)
            }
            other => panic!("{other:?}"),
        }
        match GeneratedKind::Words.recipe(9999) {
            GenerateRequest::Passphrase(recipe) => {
                assert_eq!(recipe.words, GeneratedKind::Words.bounds().1)
            }
            other => panic!("{other:?}"),
        }
    }

    // ---- geometry ----------------------------------------------------------

    /// **Nothing the card lays out falls off it.**
    ///
    /// This window is frameless, always-on-top, unresizable and has **no scroll
    /// area anywhere**, so a control past an edge is not merely awkward -- it is
    /// unreachable, on the surface whose *Save to vault* button is the only way
    /// the password the user is looking at reaches their vault.
    #[test]
    fn nothing_the_card_lays_out_falls_off_it() {
        let l = layout();


        // **The brand lockup**, which the port had dropped entirely and which
        // this card now carries again. Pinned to the new truth rather than
        // loosened: the card grew by the lockup's height plus its gap, and the
        // window's own height assertions below are what hold that honest.
        let lockup = crate::win32_draw::card_lockup();
        assert_eq!(
            (l.mark.x, l.mark.y),
            (MARGIN_X, MARGIN_TOP),
            "the lockup does not start at the card's own top-left inset"
        );
        assert_eq!(l.mark.h, lockup.mark_h);
        assert_eq!(
            l.mark.w,
            crate::win32_draw::mark_width(l.mark.h),
            "the mark's box is not the design artboard's ratio, so the shield would be              letterboxed inside it and drift away from the word beside it"
        );
        assert!(l.mark.right() < l.wordmark.x, "the wordmark is drawn over the shield");
        assert_eq!(l.wordmark.h, l.mark.h, "the lockup's two halves are different heights");
        assert!(
            l.wordmark.right() <= l.close_glyph.x,
            "the wordmark runs under the ✕"
        );
        assert!(
            l.wordmark.bottom() <= l.title.y,
            "the card's title runs into the brand lockup above it"
        );

        assert!(l.title.right() <= l.close_glyph.x, "the header text runs under the ✕");
        assert!(
            l.close_glyph.right() <= l.window.right() - MARGIN_X,
            "the close glyph has crossed the card's right margin"
        );
        assert!(l.title.bottom() <= l.header_rule.y);
        assert!(l.header_rule.bottom() <= l.value.y);
        assert!(l.value.right() < l.new.x, "the *New* button overlaps the value box");
        assert!(
            l.new.right() <= l.window.right() - MARGIN_X,
            "the *New* button has crossed the card's right margin"
        );
        assert!(
            l.new.y >= l.value.y && l.new.bottom() <= l.value.bottom(),
            "the *New* button is not inside the value row it belongs to"
        );

        assert!(l.value.bottom() <= l.kinds[0].y);
        for pair in l.kinds.windows(2) {
            assert!(pair[0].right() < pair[1].x, "two kind chips overlap");
        }
        assert!(
            l.kinds[2].right() < l.minus.x,
            "the kind chips have grown into the size stepper: the chips end at {} and the \
             stepper starts at {}",
            l.kinds[2].right(),
            l.minus.x
        );
        assert!(l.minus.right() < l.readout.x && l.readout.right() < l.plus.x);
        assert!(
            l.plus.right() <= l.window.right() - MARGIN_X,
            "the stepper has crossed the card's right margin"
        );
        for chip in l.kinds {
            assert_eq!(chip.y, l.minus.y, "the chips and the stepper are on different rows");
            assert_eq!(chip.h, l.minus.h);
        }
        assert!(l.kinds[0].x >= MARGIN_X, "the kind chips start inside the card's left margin");

        assert!(l.kinds[0].bottom() <= l.footer_rule.y);
        assert_eq!(l.footer.y, l.footer_rule.bottom(), "the footer's tint does not start at its rule");
        assert_eq!(
            l.footer.bottom(),
            l.window.bottom(),
            "the footer's tint stops short of the card's bottom edge, leaving a band of the \
             card's own colour under it"
        );
        assert!(l.footer_rule.bottom() <= l.save.y);
        assert!(l.save.right() < l.copy.x, "the two footer buttons overlap");
        assert!(l.copy.right() < l.esc_chip.x, "the *Esc* chip sits on the *Copy* button");
        assert!(l.esc_chip.right() < l.dismiss.x);
        assert!(
            l.dismiss.right() <= l.window.right() - MARGIN_X,
            "the footer hint has crossed the card's right margin"
        );
        assert!(l.dismiss.w > 0, "the footer hint has no room for its word");
        for button in [l.save, l.copy] {
            assert_eq!(button.h, BUTTON_H);
            assert_eq!(button.y, l.save.y, "the footer's buttons are on different rows");
        }
        // **Against the MARGIN, not against the window's edge.** A pin that only
        // forbade a control leaving the window is `MARGIN_TOP` slacker than the
        // layout it guards.
        assert_eq!(
            l.save.bottom() + MARGIN_TOP,
            l.window.bottom(),
            "the card is not sized to its own footer: it asks the OS for a {} px window whose \
             last control ends at {} px. This card has one shape and no rows, so a window taller \
             than its content is a band of bare `theme::CARD_TINT` under the buttons -- and one \
             shorter is a *Save to vault* the user cannot reach",
            l.window.h,
            l.save.bottom()
        );
        assert_eq!(l.window.w, WIDTH);
        assert_eq!(l.window.x, 0);
        assert_eq!(l.window.y, 0);
    }

    /// **The card's dimensions are the theme's**, so a redesign there cannot
    /// leave this card drawing controls of its own invented height.
    #[test]
    fn the_cards_dimensions_are_the_themes() {
        assert_eq!(
            BUTTON_H, crate::theme::BUTTON_HEIGHT as i32,
            "the footer's buttons are not the app's button height"
        );
        assert_eq!(
            WIDTH,
            crate::picker_prompt::WIDTH,
            "the daemon's two Win32 cards are different widths, which reads as two different \
             programs answering the same hotkey"
        );
    }

    /// **The card says every one of its own words**, and each of them is a
    /// constant rather than a literal at the paint site -- which is the only
    /// reason a test can read them at all on a surface no test may open.
    #[test]
    fn the_cards_words_are_the_ones_it_promises() {
        assert_eq!(GENERATE_LABEL, "New password");
        assert_eq!(
            GENERATE_SAVE_LABEL, "Save to vault",
            "the primary button says *Fill*, on a path that holds no injector and cannot type \
             into the window behind the card"
        );
        assert_eq!(GENERATE_COPY_LABEL, "Copy");
        assert_eq!(GENERATE_NEW_LABEL, "New");
        assert_eq!(ESC_SHORTCUT, "ESC");
        assert_eq!(REGENERATE_SHORTCUT, "CTRL+R");
        assert_eq!(SAVE_SHORTCUT, "ENTER");
        assert_eq!(DISMISS_LABEL, "Dismiss");
        for kind in GeneratedKind::ALL {
            assert!(!kind.label().is_empty());
        }
        assert_eq!(
            GeneratedKind::ALL.map(|k| k.label()),
            ["Words", "Characters", "PIN"],
            "the chips are drawn in `ALL` order and the card's three offers are these three"
        );
    }

    /// **The title is this window's own**, because
    /// `crate::foreground::pick` is a `find` over this process's own windows and
    /// this card is up alongside the tray's and the hotkey listener's.
    #[test]
    fn the_window_opens_under_a_title_nothing_else_uses() {
        assert!(!GENERATE_PROMPT_TITLE.is_empty());
        assert_ne!(GENERATE_PROMPT_TITLE, crate::picker_prompt::PICKER_PROMPT_TITLE);
        assert_ne!(GENERATE_PROMPT_TITLE, crate::unlock_prompt::UNLOCK_PROMPT_TITLE);
        assert_ne!(GENERATE_PROMPT_TITLE, crate::vault_window::WINDOW_TITLE);
    }

    // ---- the secret --------------------------------------------------------

    /// **Nothing on this module's public types can print a password.**
    ///
    /// The egui card this replaced carried the password inside its state enum
    /// and had to hand-write a `Debug` to keep `Zeroizing`'s derived one from
    /// printing it. Here the password is not on the type at all, which is the
    /// stronger version of the same guarantee -- and this is what says so.
    #[test]
    fn no_type_that_crosses_the_seam_can_carry_a_password() {
        const SECRET_TEXT: &str = "correct-horse-battery-staple";
        let mut form = GenerateForm::new(GeneratedKind::Characters);
        form.finish(None);
        let printed = format!("{form:?} {:?} {:?}", Event::Save, Outcome::Kept);
        assert!(
            !printed.contains(SECRET_TEXT),
            "a password reached a `Debug` on this module's types"
        );
        assert!(
            printed.contains("Ready"),
            "control: the `Debug` under test printed nothing recognisable at all: {printed}"
        );

        // And the source says the same thing structurally: `ValueState::Ready`
        // has no payload, so there is nowhere for a password to be put.
        assert_eq!(
            form.state(),
            &ValueState::Ready,
            "the settled state carries something, so it is no longer the payload-free marker \
             this module's whole secret story rests on"
        );
    }

    /// **A failure sentence is the card's own words and never the error's.**
    ///
    /// The caller's error can carry a URL, a status code and a response body.
    #[test]
    fn a_failure_shows_a_sentence_and_not_the_error() {
        const DETAIL: &str = "http://127.0.0.1:8087/generate returned 500: {\"secret\":\"x\"}";
        static SEEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        if let Ok(mut seen) = SEEN.lock() {
            seen.clear();
        }
        fn detailed(_: &GenerateRequest) -> Result<Zeroizing<String>, String> {
            Err(DETAIL.to_string())
        }
        let calls = GenerateCalls {
            show: |_, form| {
                if let Ok(mut seen) = SEEN.lock() {
                    seen.push(format!("{:?}", form.state()));
                }
            },
            ..inert()
        };
        let _ = run_with(&calls, "Ledgerline", &detailed);
        let seen = SEEN.lock().unwrap().join(" ");
        assert!(
            seen.contains(GENERATE_FAILED_TEXT),
            "the card did not show its own failure sentence: {seen}"
        );
        assert!(
            !seen.contains("127.0.0.1") && !seen.contains("secret"),
            "the generator's error detail reached the card: {seen}"
        );
    }

    // ---- source pins -------------------------------------------------------

    /// The production half of this file: everything before the first column-0
    /// `#[cfg(test)]`, with line endings normalised first because this
    /// repository checks out CRLF.
    fn production() -> (String, usize) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("generate_prompt.rs");
        let raw = std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
        let cut = raw.split(concat!("\n#[cfg(", "test)]\n")).next().unwrap().to_string();
        let discarded = raw.len() - cut.len();
        (cut, discarded)
    }

    /// The production half with comments stripped, so a rule that forbids a call
    /// does not also forbid explaining why the call is not there.
    fn code(source: &str) -> String {
        source
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_generator_window_never_posts_a_thread_quit() {
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
            "control: the comment stripper has eaten code -- the WM_DESTROY arm's one surviving \
             statement is not in the text this rule scans"
        );

        assert!(
            !code.contains(concat!("PostQuit", "Message")),
            "generate_prompt.rs's production half posts a thread quit. This window is opened on \
             the daemon thread, and that thread goes on to run egui windows -- design 3c's \
             save-a-login form, which is exactly where the password this card just produced is \
             handed back to. `close()` calls `DestroyWindow`, which dispatches WM_DESTROY \
             synchronously on that thread, and nothing drains the queue afterwards: `next()` has \
             already returned. The next `eframe::run_native` takes the stale WM_QUIT out of \
             `GetMessageW`, leaves its loop before it draws, and returns its DEFAULT answer -- \
             so 3c never reappears and the password the user chose to keep is silently dropped. \
             `GONE` is what `next()` reads; quitting the thread is not this window's job."
        );
    }

    /// **The capture exclusion goes on the top-level window, and once.**
    ///
    /// Windows refuses `SetWindowDisplayAffinity` on a child control with
    /// `E_INVALIDARG`, so a call aimed at one of this card's eight `BUTTON`s
    /// would fail silently and leave a live password capturable.
    #[test]
    fn the_capture_exclusion_goes_on_the_top_level_window() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        assert_eq!(
            code.matches("SetWindowDisplayAffinity(").count(),
            1,
            "this card paints a password in the clear and excludes itself from screen capture \
             other than exactly once"
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
            "this card is answered by typing -- Enter saves and Ctrl+R regenerates -- and asks \
             for the foreground other than exactly once"
        );
        assert_eq!(
            code.matches("run_ui_native(").count(),
            0,
            "this card has become an `eframe` window, which is the ~50 MB of unreleasable \
             OpenGL driver arenas it exists to not spend"
        );
    }

    /// **The password leaves the window module by exactly the routes the module
    /// doc names**, and by no other.
    ///
    /// A `SECRET.lock()` that answered a clone to anything but the painter, the
    /// clipboard and `keep` would be an extra copy of a live password in a
    /// process that is meant to hold exactly one.
    #[test]
    fn the_password_leaves_the_window_by_the_three_routes_and_no_other() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        assert_eq!(
            code.matches("SECRET.lock()").count(),
            5,
            "the parked password is reached from other than its five sites: `fill` writes it, \
             `paint` draws it, `copy` sends it to the clipboard, `keep` moves it out, and \
             `wipe_secret` clears it. Each new site is a new copy of a live secret"
        );
        assert_eq!(
            code.matches("copy_secret(").count(),
            1,
            "the card writes to the clipboard other than through `crate::clipboard::copy_secret`, \
             which is the crate's one clipboard path and the only one that clears itself"
        );
        assert!(
            code.contains("*slot = None;"),
            "control: `wipe_secret`'s body is not in the text this rule scans"
        );
        // Nothing formats the secret. `{password}` or `{:?}` anywhere near it
        // would be a live credential in a log file on disk.
        for forbidden in ["{password}", "{secret}", "log::info!", "log::debug!"] {
            assert!(
                !code.contains(forbidden),
                "`{forbidden}` appears in a module that holds a password in a process static"
            );
        }
    }
}

//! The auto-type sequence **runner**: turning a parsed keystroke sequence into
//! a list of things to do, and doing them without ever typing a secret into a
//! window nobody verified.
//!
//! [`crate::key_sequence`] ends at the notation. It parses, renders and
//! previews, and its own module doc says in as many words that it does not
//! type anything because "sending synthetic input is a separate problem
//! (focus, elevation, per-key timing, cancellation)". This module is that
//! separate problem.
//!
//! # The shape: a pure plan, then a dumb runner
//!
//! Everything that can be got wrong is in [`plan`], which is a pure function
//! from `(tokens, resolved values)` to either a [`Plan`] or a [`Refusal`] and
//! is unit-tested directly. [`run`] is then deliberately boring: walk the
//! steps, and before each one ask whether the target still has foreground.
//! Both halves are testable without sending a single real keystroke -- [`run`]
//! because it takes a [`Keyboard`], which the tests at the bottom implement
//! with a recorder whose "foreground" answer they choose.
//!
//! # The hazard this module exists to contain
//!
//! `SendInput` types into whatever holds keyboard focus *right now*. The
//! existing username-Tab-password fill contains that with a single
//! [`crate::injector::send_input::ensure_foreground`] check, and that is
//! sufficient *because the whole burst is over in under a second*.
//!
//! **A sequence is not.** `{USERNAME}{ENTER}{DELAY 2000}{PASSWORD}{ENTER}` --
//! the Microsoft 365 shape this whole feature exists for -- spends two
//! seconds doing nothing between the address screen and the password screen.
//! In those two seconds the user can alt-tab, a notification can take
//! foreground, or the browser can open a window. A runner that checked
//! foreground once at the start and then typed for five seconds would type
//! the *password* into whatever won that race. That is the same family of
//! defect as the one fixed in `aae9429`, and it is the single thing this
//! module is designed around.
//!
//! So: [`run`] re-asks before **every** step, and the moment the answer is no
//! it returns, having emitted nothing further. There is no "finish the token
//! we started" path, because the token we started is the one holding the
//! password.
//!
//! ## Why the granularity is "per step", and why a text run is chopped
//!
//! Per-token would still be too coarse: `{PASSWORD}` is one token and could
//! be a 60-character passphrase. Per-*character* would be too fine -- a
//! `GetForegroundWindow` between every keystroke triples the cost of the one
//! operation that is already deliberately slowed to 3ms/char, and it buys
//! nothing, because the gap it would close is microseconds wide.
//!
//! The gap that actually matters is measured in *time*, not tokens. So
//! [`plan`] chops every run of typed text into chunks whose projected
//! duration is at most [`MAX_BURST`], using the sequence's own current
//! `{DELAY=n}` rate to project it. Whatever the rate, the window between two
//! foreground checks is bounded by a quarter of a second, and a `{DELAY}` --
//! the long gap, the dangerous one -- is *always* followed by a check before
//! anything is typed.
//!
//! **Both the chop and the projection count UTF-16 code units, not
//! characters**, because that is the unit the keyboard actually pauses on:
//! `RealKeyboard::type_text` sleeps once per `encode_utf16` unit. Counting
//! characters made every astral-plane character (emoji, CJK extension B)
//! cost two sleeps against a budget of one, so a burst projected at 249ms
//! really slept up to 498ms and the gap between two foreground checks was
//! twice its stated bound. Text in the BMP -- accents, ordinary CJK,
//! Cyrillic -- was never affected, which is why it survived so long.
//!
//! # Refuse the whole sequence, or type what we can?
//!
//! Refuse. Every time, and before the first keystroke.
//!
//! A `{S:PIN}` that does not resolve, a `{TOTP}` with no code, a
//! `{PICKCHARS}` this build does not implement: each of these means the
//! sequence the user wrote is not the sequence we would perform. Half-typing
//! a login leaves a form in a state the user then has to diagnose, and in the
//! `{PICKCHARS}` case the alternative -- typing the token's characters as
//! literal text -- would put the string `{PICKCHARS}` into a login form, which
//! is never what anyone meant. A refusal names the construct and types
//! nothing, so the user's next move is obvious.
//!
//! # The total time bound
//!
//! `{DELAY 3600000}` parses (the editor caps what *it* writes at an hour, but
//! a sequence pasted from KeePass is not the editor's output). Honouring it
//! would leave a fill armed for an hour: an hour in which every step is a
//! foreground check that will eventually pass on some unrelated window that
//! happens to be the one we matched, and an hour in which the plaintext
//! password sits in memory waiting. [`MAX_SEQUENCE`] bounds the *projected*
//! total, and a sequence over it is refused at plan time -- before the
//! password is even asked for a second time.
//!
//! That bound covers typing and delays. It does **not** cover key presses,
//! which are charged nothing: an arbitrarily long run of `{TAB}` passes it.
//! See [`MAX_SEQUENCE`]'s own doc for why that is a correction to the claim
//! rather than a gap in the defence.
//!
//! # Secrets
//!
//! A [`Plan`] contains the password in plaintext by construction: that is what
//! it is for. So [`Plan`] wipes every one of its text steps on `Drop`, the
//! same discipline [`crate::login_ui::LoginForm`] applies, and the same reason
//! [`crate::vault_window::detail::DetailAction`]'s secret variants carry no
//! values. The allocator-probe harness in `login_ui.rs` watches for exactly
//! this, and `plan_tests::a_dropped_plan_does_not_release_the_password_in_the_clear`
//! points it at a `Plan`.

use crate::key_sequence::{FieldRef, KeyDef, Modifier, Token, KEYS};
use std::time::Duration;
use zeroize::Zeroize;

/// The pause between simulated keystrokes when the sequence does not say.
///
/// Controls that do per-character work on their own UI thread -- the game
/// launchers the SendInput fallback exists for, in particular -- drop
/// characters delivered faster than this. Three milliseconds is imperceptible
/// to the user and substantially reduces the risk; it is a mitigation and not
/// a guarantee, since detecting a partially delivered batch is out of scope.
///
/// **The one such constant in the crate.** `send_input` used to hold a second
/// 3ms literal of its own for the default fill's straight-line typing. The
/// default fill is now a [`Plan`] like any other (see
/// [`crate::injector::default_plan`]), so a fill with no `{DELAY=n}` types at
/// this rate because it is stamped with this constant, rather than because two
/// literals happen to agree.
pub const DEFAULT_RATE: Duration = Duration::from_millis(3);

/// The longest a single uninterrupted burst of typing may be projected to
/// take before [`plan`] splits it, so [`run`] can re-verify foreground.
///
/// A quarter of a second is chosen against the thing it is defending: a human
/// alt-tabbing. Nobody switches windows and starts reading in under 250ms, so
/// a burst this short cannot straddle a switch the user could notice, while
/// still being ~80 characters at the default rate -- long enough that the
/// common case (one password, one burst) costs exactly one extra check.
pub const MAX_BURST: Duration = Duration::from_millis(250);

/// What one `SendInput` call costs, charged by [`Step::projected`] on top of
/// the sleep, once per UTF-16 unit typed and once per key press.
///
/// **Why it exists.** `RealKeyboard::type_text` makes a `SendInput` call per
/// `encode_utf16` unit and `press_key` makes one per chord. While this was
/// charged nothing, the projection was not an estimate of real time but a sum
/// of the *sleeps* -- so a run with a small `rate` was projected at a fraction
/// of what it takes, and the [`MAX_BURST`] gap between two foreground checks
/// really was longer than the constant says. A run of key presses, which
/// sleeps nowhere at all, was projected at exactly zero: free, at any length,
/// against [`MAX_SEQUENCE`].
///
/// # Where the number comes from
///
/// It is measured, at a floor, and then rounded up -- and the measurement is
/// deliberately of the one thing that can be measured without injecting
/// anything into the user's desktop. `SendInput(0, NULL, sizeof(INPUT))`
/// performs the full user32 stub and kernel transition and injects **no**
/// input; 200,000 such calls averaged 7.8us each, against 5.0us each for an
/// equally-marshalled call to `GetTickCount` in the same harness. The
/// difference, ~2.8us, is `SendInput`'s own cost with the injection work
/// removed.
///
/// A real call does strictly more than that: it builds an input packet, walks
/// whatever low-level keyboard hook chain is installed, and posts to the raw
/// input thread. So 2.8us is a **lower bound**, and a lower bound is the
/// unsafe direction to charge. Ten microseconds is that floor rounded up by
/// ~3.5x, which is the direction that is safe to be wrong in: over-charging
/// makes [`MAX_BURST`] chunks *smaller* (more foreground checks, never fewer)
/// and makes [`MAX_SEQUENCE`] refuse *sooner*.
///
/// It is still an estimate and is not claimed to be otherwise. What changed is
/// that the estimate is no longer of zero, and no longer describes a call the
/// keyboard provably makes as costing nothing.
///
/// At [`DEFAULT_RATE`] this is 0.3% of a keystroke and at [`MIN_RATE`] 1%, so
/// it does not meaningfully shrink an ordinary fill; its whole effect is on
/// the two cases where the sleep was small or absent.
pub const SEND_INPUT_COST: Duration = Duration::from_micros(10);

/// The floor a `{DELAY=n}` typing rate is clamped to.
///
/// **Without it, `{DELAY=0}` switched both bounds off.** It parses cleanly --
/// `key_sequence::whole_number` rejects *leading* zeros, not a bare zero -- and
/// at `rate == 0` [`TextRun::chunk_units`] computes `MAX_BURST / 1ns`, i.e.
/// 250,000,000 units, so a 5000-character password became **one** chunk with no
/// foreground check anywhere inside it; and [`Step::projected`] returned
/// `ZERO`, so [`MAX_SEQUENCE`] saw nothing at all. Both defences evaporated on
/// a sequence a user can write by hand.
///
/// # Why a floor here at all, now that the syscall is charged
///
/// A `Step::Text` is the one place [`run`] cannot look inside, so its
/// projection is the only thing standing between the user and an unbounded
/// gap between two foreground checks. Charging it zero costs exactly that
/// guarantee.
///
/// One millisecond is a floor, not a measurement, and it is wrong in the safe
/// direction for both consumers: over-charging a keystroke makes [`MAX_BURST`]
/// chunks *smaller* (more foreground checks, never fewer) and makes
/// [`MAX_SEQUENCE`] refuse *sooner*.
///
/// **This used to be the only thing under a zero rate, and it was not enough.**
/// [`Step::projected`] charged a text unit its `rate` and nothing for the
/// `SendInput` call that accompanies every one of them, so at **any** rate the
/// real elapsed time of a burst exceeded its projection by roughly
/// `units x syscall cost` -- at the tightest chunk, a few percent over the
/// [`MAX_BURST`] gap rather than under it. That is now charged:
/// [`SEND_INPUT_COST`] is added per unit, so the projection is an estimate of
/// real elapsed time rather than a sum of sleeps, and this floor is a floor
/// under the *sleep* alone.
///
/// The two constants are not redundant. Drop this floor and `{DELAY=0}` still
/// projects at `SEND_INPUT_COST` per unit -- a hundredfold looser bound, and
/// [`chunk_units`](TextRun::chunk_units) would hand back 25,000-unit chunks.
/// Drop [`SEND_INPUT_COST`] and a key press is free again.
///
/// Clamping rather than refusing, because `{DELAY=0}` is a legible request
/// ("as fast as you can") and refusing a sequence that parses, for a reason
/// about our own internal arithmetic, would be a worse answer than honouring
/// it at the fastest rate we can still bound.
pub const MIN_RATE: Duration = Duration::from_millis(1);

/// The longest a whole sequence may be projected to spend **typing and
/// waiting**.
///
/// Sixty seconds is far more than any real sign-in flow -- the motivating
/// Microsoft 365 case is about three -- and far less than the hour a
/// `{DELAY 3600000}` would ask for. See the module doc.
///
/// # How loosely it bounds a key-only sequence
///
/// A key press used to be charged nothing at all, so a sequence of nothing but
/// `{TAB}` and `{ENTER}` passed this check at any length: the bound was on
/// typing and delays, not on step count, and the doc claimed more than that.
///
/// It is now charged [`SEND_INPUT_COST`], which is what such a press really
/// costs, so this bound does apply to key presses -- but only at
/// 60s / 10us = six million of them. That is a real bound and a very loose
/// one, and the looseness is the honest answer rather than a shortfall:
/// charging a key some larger invented constant would make the sentence
/// "bounded at 60s" bite sooner at the cost of putting a fiction into
/// [`Step::projected`], which [`MAX_BURST`] chunking also depends on being an
/// honest projection of real time.
///
/// The looseness costs little, because a key-only run is not what this
/// constant defends against. The thing [`MAX_SEQUENCE`] exists to stop is a
/// fill that stays *armed* -- a plaintext password sitting in memory while a
/// `{DELAY 3600000}` runs down, and a foreground check that will eventually
/// pass on some unrelated window. A run of bare key presses arms nothing: it
/// carries no password, it sleeps nowhere, and [`run`] re-verifies foreground
/// before every single one of them, so the user switching away stops it at the
/// next key.
pub const MAX_SEQUENCE: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Modifiers
// ---------------------------------------------------------------------------

/// The modifiers held down for one key press.
///
/// A set rather than a single modifier because `^+{TAB}` is a real chord.
/// `Copy` and three `bool`s rather than bitflags because there are three of
/// them and a wrong bit here is a wrong chord sent to the user's browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModSet {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl ModSet {
    pub fn is_empty(self) -> bool {
        !self.shift && !self.ctrl && !self.alt
    }

    fn with(mut self, m: Modifier) -> Self {
        match m {
            Modifier::Shift => self.shift = true,
            Modifier::Ctrl => self.ctrl = true,
            Modifier::Alt => self.alt = true,
            // `~` is not a modifier at all -- see `plan`.
            Modifier::Enter => {}
        }
        self
    }

    /// How the modifiers read in a refusal message.
    pub fn label(self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        parts.join("+")
    }
}

// ---------------------------------------------------------------------------
// Steps
// ---------------------------------------------------------------------------

/// One thing the runner does, between two foreground checks.
///
/// The variants are deliberately the three *physically different* acts --
/// type characters, press a key, wait -- and not one variant per [`Token`].
/// A `{USERNAME}` and the literal text around it are the same act and become
/// one [`Self::Text`]; a `{DELAY=50}` is not an act at all and becomes no step,
/// only a change to the rate the following [`Self::Text`] carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Type these characters, `rate` apart.
    ///
    /// **May contain a password.** [`Plan`]'s `Drop` wipes it.
    Text { text: String, rate: Duration },
    /// Press this key, with these modifiers held.
    Key { key: &'static KeyDef, mods: ModSet },
    /// Do nothing for this long.
    Wait(Duration),
}

impl Step {
    /// How long this step is expected to take. Used both for [`MAX_BURST`]
    /// chunking and for the [`MAX_SEQUENCE`] bound, so the two cannot disagree
    /// about what a step costs.
    ///
    /// **Text is counted in UTF-16 code units, because that is what the
    /// keyboard sleeps on**: `RealKeyboard::type_text` pauses once per
    /// `encode_utf16` unit, so an astral-plane character (emoji, CJK
    /// extension B) costs two pauses and not one. Counting `chars` here made
    /// every projection of such text exactly half its real duration.
    ///
    /// **Every `SendInput` call is charged [`SEND_INPUT_COST`] on top of the
    /// sleep.** It used to be charged nothing, which made this a sum of sleeps
    /// rather than a projection of real time: a burst at a small rate ran
    /// past the [`MAX_BURST`] gap it was chunked to fit, and a run of key
    /// presses -- which sleeps nowhere -- was projected at exactly zero and
    /// so was free at any length against [`MAX_SEQUENCE`]. The keyboard makes
    /// one such call per UTF-16 unit typed and one per chord pressed, so this
    /// is a cost that provably exists; see the constant for how it is derived
    /// and why it is deliberately rounded up.
    pub fn projected(&self) -> Duration {
        match self {
            // Per unit, not per step: `RealKeyboard::type_text` calls
            // `SendInput` once for each `encode_utf16` unit, the same thing it
            // sleeps `rate` for.
            Self::Text { text, rate } => {
                (*rate + SEND_INPUT_COST) * text.encode_utf16().count() as u32
            }
            // One call for the whole chord: `RealKeyboard::press_key` sends
            // the down-and-up batch in a single `SendInput` and returns, with
            // no sleep anywhere in it.
            Self::Key { .. } => SEND_INPUT_COST,
            Self::Wait(d) => *d,
        }
    }
}

/// A whole sequence, ready to run.
///
/// Owns its steps and **wipes every text step on `Drop`**. Not a bare
/// `Vec<Step>` for exactly that reason: a `Vec<Step>` would hand the
/// plaintext password back to the allocator on every early return in this
/// module, and there is no place in the type system to hang the wipe on.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    steps: Vec<Step>,
}

impl Plan {
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// What the whole plan is expected to take. The value [`plan`] checks
    /// against [`MAX_SEQUENCE`].
    pub fn projected(&self) -> Duration {
        self.steps.iter().map(Step::projected).sum()
    }
}

/// **The backstop: no `Plan` is released to the allocator holding a password
/// in the clear.**
///
/// Every exit from [`run`] -- success, foreground abort, a `SendInput`
/// failure -- drops the plan, and one of those exits is the one that fires
/// when a password has just been half-typed into a window that stopped being
/// ours. Wiping here rather than at each call site means the guarantee is a
/// property of the type and not of six callers remembering.
impl Drop for Plan {
    fn drop(&mut self) {
        for step in &mut self.steps {
            if let Step::Text { text, .. } = step {
                text.zeroize();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// Why a sequence will not be typed. Every variant names the thing the user
/// has to go and change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A field placeholder whose value is not there: `{S:PIN}` on an item with
    /// no `PIN`, `{TOTP}` with no code.
    Unresolved(String),
    /// A construct this build does not implement -- `{PICKCHARS}`,
    /// `{APPACTIVATE Foo}`, a `(` group. Carried faithfully by the parser
    /// (nothing is ever dropped) and refused faithfully here.
    Unsupported(String),
    /// A `+`, `^` or `%` that is not followed by a key.
    DanglingModifier(String),
    /// A key spelling with no virtual-key code behind it.
    UntypableKey(String),
    /// Projected to take longer than [`MAX_SEQUENCE`].
    TooLong(Duration),
    /// A sequence that would type nothing at all.
    Nothing,
}

impl Refusal {
    /// The sentence shown to the user and written to the log. Phrased as the
    /// fact plus the reference, so the next move is to go and fix the named
    /// thing -- the same phrasing rule
    /// [`crate::key_sequence::resolve_preview`]'s `missing` follows.
    pub fn message(&self) -> String {
        match self {
            Self::Unresolved(what) => {
                format!("the auto-type sequence needs {what}, and this item has none")
            }
            Self::Unsupported(what) => {
                format!("the auto-type sequence uses {what}, which this build cannot type")
            }
            Self::DanglingModifier(what) => {
                format!("the auto-type sequence has a {what} modifier with no key after it")
            }
            Self::UntypableKey(key) => {
                format!("the auto-type sequence presses {{{key}}}, which has no key code")
            }
            Self::TooLong(projected) => format!(
                "the auto-type sequence would take {}s, over the {}s limit",
                projected.as_secs(),
                MAX_SEQUENCE.as_secs()
            ),
            Self::Nothing => "the auto-type sequence types nothing".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// The values a sequence resolves against
// ---------------------------------------------------------------------------

/// Everything [`plan`] reads, borrowed.
///
/// Borrowed rather than owned for the same reason
/// [`crate::key_sequence::ResolveSource`] is: building it must not make a
/// second copy of the password. The one copy [`plan`] does make lands in a
/// [`Plan`], which wipes it.
pub struct Resolved<'a> {
    pub username: &'a str,
    pub password: &'a str,
    /// The current one-time code, if one was fetched. `None` is not "no TOTP
    /// configured" and not "fetch failed" -- from here both are the same
    /// thing, a `{TOTP}` that cannot be typed, and both refuse.
    pub totp: Option<&'a str>,
    pub custom: Vec<(&'a str, &'a str)>,
}

impl<'a> Resolved<'a> {
    fn custom_value(&self, name: &str) -> Option<&'a str> {
        self.custom.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
    }
}

// ---------------------------------------------------------------------------
// Virtual keys
// ---------------------------------------------------------------------------

/// The Win32 virtual-key code for a [`KeyDef`] spelling, and whether it is an
/// *extended* key.
///
/// Extended matters: the arrows, `Home`/`End`, `PgUp`/`PgDn`, `Insert` and
/// `Delete` on the navigation cluster share virtual-key codes with the numeric
/// keypad, and a `VK_DELETE` sent without `KEYEVENTF_EXTENDEDKEY` is the
/// keypad's `.` to some applications. Getting this wrong types a full stop
/// into a login form.
///
/// `None` is a spelling with no code, which [`plan`] refuses rather than
/// silently performing as nothing. Every entry in [`KEYS`] has one today --
/// `every_known_key_has_a_virtual_key` is the test that keeps it that way when
/// someone adds a spelling.
pub fn virtual_key(token: &str) -> Option<(u16, bool)> {
    use windows::Win32::UI::Input::KeyboardAndMouse as k;
    let (vk, extended) = match token {
        "ENTER" => (k::VK_RETURN, false),
        "TAB" => (k::VK_TAB, false),
        "ESC" => (k::VK_ESCAPE, false),
        "SPACE" => (k::VK_SPACE, false),
        "BACKSPACE" | "BKSP" | "BS" => (k::VK_BACK, false),
        "DELETE" | "DEL" => (k::VK_DELETE, true),
        "INSERT" | "INS" => (k::VK_INSERT, true),
        "HOME" => (k::VK_HOME, true),
        "END" => (k::VK_END, true),
        "PGUP" => (k::VK_PRIOR, true),
        "PGDN" => (k::VK_NEXT, true),
        "UP" => (k::VK_UP, true),
        "DOWN" => (k::VK_DOWN, true),
        "LEFT" => (k::VK_LEFT, true),
        "RIGHT" => (k::VK_RIGHT, true),
        "F1" => (k::VK_F1, false),
        "F2" => (k::VK_F2, false),
        "F3" => (k::VK_F3, false),
        "F4" => (k::VK_F4, false),
        "F5" => (k::VK_F5, false),
        "F6" => (k::VK_F6, false),
        "F7" => (k::VK_F7, false),
        "F8" => (k::VK_F8, false),
        "F9" => (k::VK_F9, false),
        "F10" => (k::VK_F10, false),
        "F11" => (k::VK_F11, false),
        "F12" => (k::VK_F12, false),
        _ => return None,
    };
    Some((vk.0, extended))
}

/// The `{ENTER}` definition, for `~`.
fn enter_key() -> Option<&'static KeyDef> {
    KEYS.iter().find(|k| k.token == "ENTER")
}

// ---------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------

/// Accumulates a run of typed text at one rate, flushing it into
/// [`MAX_BURST`]-bounded [`Step::Text`] chunks.
struct TextRun {
    pending: String,
    rate: Duration,
}

impl TextRun {
    fn new(rate: Duration) -> Self {
        Self { pending: String::new(), rate }
    }

    /// Appends to the accumulator **without ever letting `String` grow it**.
    ///
    /// This is the half of the wipe that was missing. `push_str` past capacity
    /// goes through `GlobalAlloc::realloc`, and when the block moves, the old
    /// buffer -- password and all -- is handed back to the allocator unwiped.
    /// Neither [`Self::flush`]'s `zeroize` nor `Drop for TextRun` can reach
    /// it: both act on the buffer that exists *now*, and by then the leaked
    /// one is long gone. It happened on the **ordinary success path**, for any
    /// sequence with content after `{PASSWORD}`.
    ///
    /// So growth is done by hand here: allocate a buffer that is big enough,
    /// copy across, **wipe the old one, and only then release it**. After this
    /// returns, `push_str` below is guaranteed to fit, so `String` never
    /// reallocates this buffer at all.
    ///
    /// Doubling rather than reserving exactly keeps a long sequence from being
    /// quadratic; the floor gives a first allocation that an ordinary username
    /// does not immediately outgrow.
    fn push(&mut self, text: &str) {
        if self.pending.capacity() - self.pending.len() < text.len() {
            let needed = self.pending.len() + text.len();
            let mut grown =
                String::with_capacity(needed.max(self.pending.capacity() * 2).max(64));
            grown.push_str(&self.pending);
            // Before the handover, not after: the assignment is what frees the
            // old buffer, and it must already be zeroed when it goes.
            self.pending.zeroize();
            self.pending = grown;
        }
        debug_assert!(
            self.pending.capacity() - self.pending.len() >= text.len(),
            "the hand-grown buffer must leave String nothing to reallocate"
        );
        self.pending.push_str(text);
    }

    /// The most **UTF-16 code units** that fit in one burst at this rate. At
    /// least one, so a `{DELAY=100000}` cannot produce a zero-length chunk and
    /// loop forever.
    ///
    /// Units, not characters, because a unit is what the keyboard sleeps on:
    /// [`crate::injector::send_input::RealKeyboard::type_text`] pauses once
    /// per `encode_utf16` unit. Budgeting sleeps but spending *characters* is
    /// how a burst projected at 249ms came to sleep 498ms of astral-plane
    /// text -- a half-second hole between two foreground checks, from a
    /// counter that was right for every character in the BMP.
    ///
    /// **The per-unit cost budgeted here is the one [`Step::projected`]
    /// charges**, sleep plus [`SEND_INPUT_COST`], and not the sleep alone. The
    /// two must agree or the chunks this produces do not satisfy the bound the
    /// projection is then checked against: with the syscall uncharged here, a
    /// chunk sized to 250ms of *sleep* took 250ms plus one syscall per unit,
    /// so the gap between two foreground checks ran past [`MAX_BURST`] by a
    /// few percent at the tightest rate.
    fn chunk_units(&self) -> usize {
        let per_unit = self.rate.max(Duration::from_nanos(1)) + SEND_INPUT_COST;
        (MAX_BURST.as_nanos() / per_unit.as_nanos()).max(1) as usize
    }

    fn flush(&mut self, out: &mut Vec<Step>) {
        if self.pending.is_empty() {
            return;
        }
        let budget = self.chunk_units();
        let mut start = 0usize;
        let mut used = 0usize;
        for (idx, ch) in self.pending.char_indices() {
            let cost = ch.len_utf16();
            // `used > 0` keeps a single character that is on its own wider
            // than the budget from producing an empty chunk and looping: an
            // astral character costs 2 units and a one-unit budget is
            // reachable via `{DELAY=250}`. A surrogate pair is never split,
            // so that one case overshoots by exactly one unit's rate -- the
            // smallest overshoot that still emits a character at all.
            if used > 0 && used + cost > budget {
                // Slicing the accumulated `String` allocates each chunk once,
                // at exactly its size. The previous `Vec<char>` + `collect()`
                // grew an intermediate buffer per chunk, handing reallocated
                // plaintext back to the allocator on the way.
                out.push(Step::Text {
                    text: self.pending[start..idx].to_string(),
                    rate: self.rate,
                });
                start = idx;
                used = 0;
            }
            used += cost;
        }
        // `pending` is non-empty and `start` always sits on a character
        // boundary strictly before its end, so this tail is never empty.
        out.push(Step::Text { text: self.pending[start..].to_string(), rate: self.rate });
        // The accumulated plaintext must not be handed back to the allocator
        // when this `String` is reallocated or dropped: `clear` alone would
        // leave the bytes in the buffer.
        self.pending.zeroize();
        self.pending.clear();
    }
}

impl Drop for TextRun {
    fn drop(&mut self) {
        self.pending.zeroize();
    }
}

/// **The whole of the typing decision, as a pure function.**
///
/// `(tokens, values) -> Plan | Refusal`, with no clock, no Win32 and no I/O,
/// so every branch below is reachable from a unit test. This is where the
/// crate's signature defect -- correct code that nothing calls -- is defended
/// against by construction: there is nothing here that needs a window to
/// exercise.
pub fn plan(tokens: &[Token], values: &Resolved<'_>) -> Result<Plan, Refusal> {
    // **A `Plan` from the first push, not a bare `Vec<Step>` wrapped up at the
    // end.** `Plan`'s own doc says why -- "a `Vec<Step>` would hand the
    // plaintext password back to the allocator on every early return in this
    // module" -- and this function was the one place that did exactly that.
    // `run.flush` empties the accumulator into these steps at every `{KEY}`,
    // `{DELAY}` and `{DELAY=n}`, so any sequence with a key or a delay after
    // `{PASSWORD}` had the password sitting in a `Step::Text` here; and five
    // of the six `return Err` paths below sit *after* that point, so each of
    // them dropped a `Vec` full of plaintext with no wipe anywhere on it.
    // Wrapping at the end put the guarantee where the failures were not.
    //
    // Named `acc` rather than shadowing `steps`, so that a future `push` to a
    // bare local is a name that does not exist rather than one that silently
    // works. See `sequence_refusal_tests`.
    let mut acc = Plan { steps: Vec::new() };
    let mut run = TextRun::new(DEFAULT_RATE);
    let mut pending_mods = ModSet::default();
    let mut pending_mod_label: Option<String> = None;

    for token in tokens {
        // A modifier binds to the *next* token, and the only next token it can
        // mean anything on is a key. Anything else and we would be guessing:
        // KeePass applies a modifier to the first character of following text,
        // which for `{PASSWORD}` would send Ctrl+<first letter of the
        // password> -- a chord, into the user's browser, derived from a
        // secret. Refusing is the only honest reading.
        // `^+{TAB}` stacks two modifiers onto one key, and `+~` is
        // Shift+Enter, so another modifier is a legitimate next token; the
        // refusal is for a modifier followed by text, a delay, or nothing.
        if let Some(label) = &pending_mod_label {
            if !matches!(token, Token::Key(_) | Token::Modifier(_)) {
                return Err(Refusal::DanglingModifier(label.clone()));
            }
        }

        match token {
            Token::Literal(text) => run.push(text),
            Token::Field(FieldRef::Username) => {
                if values.username.is_empty() {
                    return Err(Refusal::Unresolved("a username".into()));
                }
                run.push(values.username);
            }
            Token::Field(FieldRef::Password) => {
                if values.password.is_empty() {
                    return Err(Refusal::Unresolved("a password".into()));
                }
                run.push(values.password);
            }
            Token::Field(FieldRef::Totp) => match values.totp {
                Some(code) if !code.is_empty() => run.push(code),
                _ => return Err(Refusal::Unresolved("a one-time code".into())),
            },
            Token::Field(FieldRef::Custom(name)) => match values.custom_value(name) {
                Some(value) if !value.is_empty() => run.push(value),
                _ => return Err(Refusal::Unresolved(format!("a field called {name}"))),
            },
            Token::Key(key) => {
                if virtual_key(key.token).is_none() {
                    return Err(Refusal::UntypableKey(key.token.to_string()));
                }
                run.flush(&mut acc.steps);
                acc.steps.push(Step::Key { key, mods: pending_mods });
                pending_mods = ModSet::default();
                pending_mod_label = None;
            }
            Token::Delay(ms) => {
                run.flush(&mut acc.steps);
                acc.steps.push(Step::Wait(Duration::from_millis(u64::from(*ms))));
            }
            Token::DelayRate(ms) => {
                // The rate change applies from here on, so the text typed
                // *before* it must be flushed at the old rate first.
                run.flush(&mut acc.steps);
                // Clamped: a rate of zero switches off both MAX_BURST chunking
                // and the MAX_SEQUENCE bound. See MIN_RATE.
                run.rate = Duration::from_millis(u64::from(*ms)).max(MIN_RATE);
            }
            Token::Modifier(Modifier::Enter) => {
                // `~` is KeePass's shorthand for the Enter *key*, not a
                // modifier, whatever the parser's variant is called.
                let Some(key) = enter_key() else {
                    return Err(Refusal::UntypableKey("ENTER".into()));
                };
                run.flush(&mut acc.steps);
                acc.steps.push(Step::Key { key, mods: pending_mods });
                pending_mods = ModSet::default();
                pending_mod_label = None;
            }
            Token::Modifier(m) => {
                pending_mods = pending_mods.with(*m);
                // `Modifier::label` is a chip caption and ends in "+";
                // a sentence wants the bare name.
                pending_mod_label = Some(m.label().trim_end_matches('+').to_string());
            }
            Token::Grouping(c) => {
                return Err(Refusal::Unsupported(format!("a {c} group")));
            }
            Token::Unknown(raw) => {
                return Err(Refusal::Unsupported(format!("{{{raw}}}")));
            }
        }
    }

    if let Some(label) = pending_mod_label {
        return Err(Refusal::DanglingModifier(label));
    }
    run.flush(&mut acc.steps);

    let plan = acc;
    if plan.is_empty() {
        return Err(Refusal::Nothing);
    }
    let projected = plan.projected();
    if projected > MAX_SEQUENCE {
        return Err(Refusal::TooLong(projected));
    }
    Ok(plan)
}

// ---------------------------------------------------------------------------
// The runner
// ---------------------------------------------------------------------------

/// Everything [`run`] needs from the outside world.
///
/// A trait, and not four free functions, purely so the tests below can answer
/// `holds_foreground` however they like and record what was typed. There is no
/// second production implementation and there is not meant to be: the point is
/// that the abort logic is exercised, not that the keyboard is swappable.
pub trait Keyboard {
    /// Whether `hwnd` is still the window we are allowed to type into. Asked
    /// again before every single step -- see the module doc.
    fn holds_foreground(&self, hwnd: isize) -> bool;
    fn type_text(&self, text: &str, rate: Duration) -> Result<(), String>;
    fn press_key(&self, key: &'static KeyDef, mods: ModSet) -> Result<(), String>;
    fn wait(&self, how_long: Duration);
}

/// Performs `plan` against `hwnd`, abandoning it the instant `hwnd` stops
/// being the foreground window.
///
/// **The check is before the step, never after**, and there is no path that
/// performs a step it has not just re-verified. The returned `Err` names how
/// far it got, because "your password went somewhere" and "nothing happened"
/// are different things to a user and the message is what tells them which.
pub fn run<K: Keyboard>(kb: &K, hwnd: isize, plan: &Plan) -> Result<(), String> {
    let total = plan.len();
    for (index, step) in plan.steps().iter().enumerate() {
        if !kb.holds_foreground(hwnd) {
            return Err(format!(
                "auto-type stopped: window {hwnd} is no longer in front (after {index} of \
                 {total} steps). Nothing further was typed."
            ));
        }
        match step {
            Step::Text { text, rate } => kb.type_text(text, *rate)?,
            Step::Key { key, mods } => kb.press_key(key, *mods)?,
            Step::Wait(how_long) => kb.wait(*how_long),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Telling the user
// ---------------------------------------------------------------------------

/// How a refused or abandoned sequence reaches the **user**, not just the log.
///
/// A fill that silently does nothing is indistinguishable from a fill that was
/// never triggered, and the two want completely different next moves from the
/// user ("fix the `{S:PIN}` in your sequence" versus "press the hotkey
/// again"). A refusal that only reaches `log::error!` is a refusal nobody
/// reads.
///
/// A trait so the decision to notify is pinned by a test with a recorder,
/// rather than by a `MessageBoxW` nothing can call.
///
/// **Passed in, never reached for.** There used to be a `RealNotifier` whose
/// `refused` recorded under `#[cfg(test)]` and opened a window otherwise, and
/// production named it through a `const REAL_NOTIFIER` inside `fill_from_vault`.
/// A `cfg(test)` gate is a property of a *compilation unit*, and `main.rs`
/// links this library compiled **without** it -- so every binary test that
/// reached that const got the real task-modal dialog, on the desktop of
/// whoever ran `cargo test`. The gate is gone; the only way to a dialog is to
/// hold a [`FnNotifier`] whose function is `show_refusal`, which is
/// `pub(crate)` and so cannot be named from the binary at all; the only such
/// value is [`REAL_NOTIFIER`], named exclusively on lines no test executes.
pub trait Notifier {
    fn refused(&self, detail: &str);
}

/// Shows the user a refusal: a plain task-modal message box, on its own thread.
///
/// On its own thread because `MessageBoxW` blocks until dismissed, and the
/// caller is either the app's message loop (a plan-time refusal) or the typing
/// thread (a mid-sequence abort). Neither may sit in a modal loop -- the first
/// would freeze the tray and every window, and the second would hold the
/// plan's memory alive for as long as the box was on screen. Fire and forget:
/// there is no answer to read back, because there is nothing to ask.
///
/// A free `fn` and not a method body, for exactly the reason [`crate::app::FnPresenter`]
/// gives: it lets the production notifier be a struct literal naming this
/// function, with no argument and no expression in it for a mutation to hide
/// in. It compiles in every configuration, including under `cfg(test)` -- what
/// keeps it out of the suite is that nothing in a test can name it, not a gate
/// that only one of the two crates agrees with.
pub(crate) fn show_refusal(detail: &str) {
    log::warn!("autofill refused: {detail}");
    let detail = detail.to_string();
    std::thread::spawn(move || {
        use windows::core::HSTRING;
        use windows::Win32::UI::WindowsAndMessaging::{
            MessageBoxW, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MB_TOPMOST,
        };
        let text = HSTRING::from(detail);
        let caption = HSTRING::from("Deskwarden autofill");
        unsafe {
            MessageBoxW(None, &text, &caption, MB_OK | MB_ICONWARNING | MB_SETFOREGROUND | MB_TOPMOST);
        }
    });
}

/// A [`Notifier`] that is nothing but the function it forwards to.
///
/// The same shape, and for the same reason, as [`crate::app::FnPresenter`]:
/// a function *reference* rather than a hand-written `impl` per notifier, so
/// the production value can be a struct literal with nothing computed in it.
pub struct FnNotifier {
    /// Asked to tell the user that a fill was refused, and why.
    pub refused: fn(&str),
}

impl Notifier for FnNotifier {
    fn refused(&self, detail: &str) {
        (self.refused)(detail);
    }
}

/// The production notifier: the real dialog, named and not called.
///
/// This is the whole of the refusal path no test can execute, and it is data.
/// It is named on three lines in `main.rs` and one in [`crate::injector`], all
/// of them production-only; `notifier_wiring_tests` below holds this literal
/// by source position, and `main`'s own guard holds the four call sites.
pub const REAL_NOTIFIER: FnNotifier = FnNotifier { refused: show_refusal };

/// A [`Notifier`] that remembers what it was asked to show and opens nothing.
///
/// **Deliberately not `#[cfg(test)]`.** The binary's tests link this library
/// compiled without `cfg(test)`, so a recorder that only exists under the gate
/// is a recorder the binary's tests cannot have -- and "cannot have a recorder"
/// was precisely how they ended up with the dialog. Existing in every
/// configuration is what lets `main.rs`'s dispatch tests pass one, and so what
/// makes the hazard unrepresentable rather than merely avoided by discipline.
///
/// A `Mutex` and not a `RefCell`, because [`crate::injector::perform`] runs on
/// the typing thread and a notifier handed to it must be `Sync`.
#[derive(Default)]
pub struct RecordingNotifier(std::sync::Mutex<Vec<String>>);

impl RecordingNotifier {
    /// Everything this has been asked to show, cleared.
    pub fn take(&self) -> Vec<String> {
        let mut held = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut held)
    }
}

impl Notifier for RecordingNotifier {
    fn refused(&self, detail: &str) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(detail.to_string());
    }
}

#[cfg(test)]
mod notifier_tests {
    use super::*;

    #[test]
    fn a_recorder_keeps_every_refusal_verbatim_and_taking_clears() {
        let rec = RecordingNotifier::default();
        rec.refused("the auto-type sequence uses {PICKCHARS}");
        assert_eq!(rec.take(), vec!["the auto-type sequence uses {PICKCHARS}".to_string()]);
        // ...and taking clears, so one assertion cannot see another's notice.
        assert!(rec.take().is_empty());
    }

    /// An [`FnNotifier`] forwards to the function it was built from -- the one
    /// line of code between [`REAL_NOTIFIER`]'s field and the real dialog.
    #[test]
    fn an_fn_notifier_forwards_to_the_function_it_was_built_from() {
        use std::sync::Mutex;
        static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
        fn record(detail: &str) {
            SEEN.lock().unwrap().push(detail.to_string());
        }
        let n = FnNotifier { refused: record };
        n.refused("something is already typing");
        assert_eq!(&*SEEN.lock().unwrap(), &["something is already typing".to_string()]);
    }

    /// **The one line that reaches a real window, held by source position.**
    ///
    /// Nothing can execute [`REAL_NOTIFIER`]'s initialiser under test, so
    /// nothing else can tell that it still names `show_refusal` rather than
    /// something quieter -- and a production build that silently stopped
    /// telling the user anything would leave the whole suite green. This is
    /// the same device, for the same reason, as `app::prompt_wiring_tests`.
    #[test]
    fn the_production_notifier_is_the_real_dialog_and_nothing_computed() {
        // `concat!`-split so this test cannot match its own text.
        let needle =
            concat!("const REAL_NOTIFIER: FnNotifier = FnNotifier { ", "refused: show_refusal };");
        let source = include_str!("sequence.rs");
        assert_eq!(
            source.matches(needle).count(),
            1,
            "expected {needle:?} exactly once in sequence.rs. The production notifier is the \
             one value in this crate that opens a real window, and it is a struct literal of \
             one function reference precisely so that there is no expression in it for a \
             mutation to hide in -- wrapping `show_refusal` in a closure here would re-open \
             the hole that shape exists to close"
        );
    }
}

#[cfg(test)]
mod refusal_lifetime_tests {
    use super::*;
    use crate::key_sequence::parse;
    use crate::login_ui::password_lifetime_tests::{plaintext_reached_the_allocator, PROBE};

    /// **A refusal that happens after a flush must not release the password.**
    ///
    /// `plan` accumulated into a bare `Vec<Step>` and only wrapped it in a
    /// [`Plan`] at the very end, so the wipe existed only for sequences that
    /// *succeeded*. `run.flush` moves the accumulated text into a
    /// [`Step::Text`] at every `{KEY}`, `{DELAY}` and `{DELAY=n}`, so a
    /// sequence with any of those after `{PASSWORD}` had the plaintext sitting
    /// in that `Vec` -- and five separate `return Err` paths dropped it there,
    /// unwiped, straight to the allocator.
    ///
    /// The existing lifetime tests all assert on plans that **succeed**, which
    /// is why nothing saw it: the one route they never took was the failing
    /// one. Each sequence below refuses at a *different* `return Err`, because
    /// they are five sites and not one.
    #[test]
    fn a_refusal_after_a_flush_does_not_release_the_password() {
        // The probe is awake on this thread and in this direction. Without
        // this, a probe that had gone deaf would make every assertion below
        // pass by saying nothing.
        let bare = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "the probe cannot see an unwiped password, so this test proves nothing"
        );

        // Leaked, so that building `Resolved` inside the watch allocates
        // nothing that carries the probe on its own.
        let password: &'static str = Box::leak(PROBE.to_string().into_boxed_str());
        let values = || Resolved { username: "u", password, totp: None, custom: vec![] };

        // The fixture check: the prefix these all share really does flush the
        // password **mid-parse**, before the refusing token is reached.
        //
        // Asserting merely that *some* step holds the password is not enough
        // and this test said so wrongly at first: `plan` flushes once more at
        // the end, so `{PASSWORD}` alone also puts the password in a step, and
        // weakening the prefix to that left this passing. What distinguishes a
        // mid-parse flush is that the password is in a step which is **not the
        // last** -- there is a `{TAB}` after it, which is exactly what forced
        // the flush that the refusing token then abandons.
        let prefix = plan(&parse("{PASSWORD}{TAB}"), &values()).expect("the prefix plans");
        let before_the_last = &prefix.steps()[..prefix.len() - 1];
        assert!(
            before_the_last
                .iter()
                .any(|s| matches!(s, Step::Text { text, .. } if text.contains(PROBE))),
            "the fixture never flushed the password mid-parse, so the refusals below \
             would prove nothing"
        );
        drop(prefix);

        for sequence in [
            // Unresolved -- a custom field the item has not got.
            "{PASSWORD}{TAB}{S:NOPE}",
            // Unsupported -- a construct this build cannot type.
            "{PASSWORD}{TAB}{PICKCHARS}",
            // DanglingModifier, from the end-of-token-list check.
            "{PASSWORD}{TAB}+",
            // Unsupported -- a grouping.
            "{PASSWORD}{TAB}(x)",
            // DanglingModifier, from the in-loop check: a different `return`.
            "{PASSWORD}{TAB}+hello",
        ] {
            let tokens = parse(sequence);
            let mut refused = false;
            let leaked = plaintext_reached_the_allocator(|| {
                refused = plan(&tokens, &values()).is_err();
            });
            assert!(refused, "{sequence} was expected to refuse, and planned");
            assert!(
                !leaked,
                "{sequence} handed the password to the allocator in the clear"
            );
        }
    }

    /// The control the reviewer used to isolate the leak to the flushed step:
    /// a refusal reached **before** any flush has nothing in the accumulator's
    /// `Vec` to release, and was already clean. It is here so that a future
    /// change which makes the flushing case leak again cannot be mistaken for
    /// the probe having gone quiet across the board.
    ///
    /// **Which is precisely what it could not tell you, for as long as it had
    /// no control of its own.** Its whole claim is a `!leaked`, so a probe that
    /// had gone deaf passed it vacuously -- the one failure mode this test was
    /// written to rule out, in the test written to rule it out.
    ///
    /// The positive control is first for two reasons. It is what makes the
    /// `!leaked` below mean something; and arming it takes `PROBE_LOCK` for the
    /// rest of this thread, which the probe plaintext allocated afterwards then
    /// sits inside. Building probe-bearing fixtures before the first arm is the
    /// crate's house rule and it is the one shape that hold does not cover: it
    /// is safe here only because `Box::leak` never frees, and that is a fact
    /// about this fixture rather than a licence.
    #[test]
    fn a_refusal_before_any_flush_was_never_the_leaking_case() {
        use crate::login_ui::password_lifetime_tests::plaintext_reached_the_allocator;

        let bare = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "control: the probe cannot see an ordinary String's plaintext go past the \
             allocator, so the `!leaked` below is satisfied by a deaf instrument and this \
             test -- whose entire job is to tell a real fix from a deaf probe -- proves nothing"
        );

        let password: &'static str = Box::leak(PROBE.to_string().into_boxed_str());
        let tokens = parse("{PASSWORD}{S:NOPE}");
        let mut refused = false;
        let leaked = plaintext_reached_the_allocator(|| {
            refused = plan(
                &tokens,
                &Resolved { username: "u", password, totp: None, custom: vec![] },
            )
            .is_err();
        });
        assert!(refused, "the fixture was expected to refuse");
        assert!(!leaked, "the refusal reached before any flush released the password");
    }
}

#[cfg(test)]
mod plan_tests {
    use super::*;
    use crate::key_sequence::parse;

    /// **The username and the password are different strings, and neither is
    /// a substring of the other.** A fixture whose two values agree proves
    /// nothing about which one `{PASSWORD}` typed -- the exact hazard that
    /// produced six defects in this crate.
    const USER: &str = "work.account@contoso.com";
    const PASS: &str = "Zq7-tremulous-BADGER";

    fn values() -> Resolved<'static> {
        Resolved { username: USER, password: PASS, totp: Some("123456"), custom: vec![("PIN", "4821")] }
    }

    /// **Never call this inside a `plaintext_reached_the_allocator` closure.**
    ///
    /// It concatenates every text step into an ordinary `String` that nothing
    /// wipes, so the plaintext it builds goes back to the allocator in the
    /// clear when the caller drops it. That is harmless for the assertions in
    /// this module, which run outside any watch -- and it is a live trap for a
    /// future `!leaked` assertion, which would fail on this helper's own
    /// temporary and be read as a defect in `plan`. See
    /// `refusal_lifetime_tests`, which deliberately asserts with `contains`
    /// rather than collecting anything.
    fn typed(plan: &Plan) -> String {
        plan.steps()
            .iter()
            .filter_map(|s| match s {
                Step::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn keys(plan: &Plan) -> Vec<(&'static str, ModSet)> {
        plan.steps()
            .iter()
            .filter_map(|s| match s {
                Step::Key { key, mods } => Some((key.token, *mods)),
                _ => None,
            })
            .collect()
    }

    fn waits(plan: &Plan) -> Vec<Duration> {
        plan.steps()
            .iter()
            .filter_map(|s| match s {
                Step::Wait(d) => Some(*d),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_default_sequence_plans_to_username_tab_password() {
        let p = plan(&parse(crate::key_sequence::DEFAULT_SEQUENCE), &values()).unwrap();
        assert_eq!(typed(&p), format!("{USER}{PASS}"));
        assert_eq!(keys(&p), vec![("TAB", ModSet::default())]);
    }

    /// The motivating case, end to end through the planner: two screens, an
    /// Enter between them, and a wait for the second one to load.
    #[test]
    fn the_microsoft_365_shape_plans_to_two_screens_with_a_wait_between() {
        let p = plan(&parse("{USERNAME}{ENTER}{DELAY 2000}{PASSWORD}{ENTER}"), &values()).unwrap();
        let shape: Vec<&str> = p
            .steps()
            .iter()
            .map(|s| match s {
                Step::Text { .. } => "text",
                Step::Key { .. } => "key",
                Step::Wait(_) => "wait",
            })
            .collect();
        assert_eq!(shape, vec!["text", "key", "wait", "text", "key"]);
        assert_eq!(waits(&p), vec![Duration::from_millis(2000)]);
        // The username is typed on the FIRST screen and the password on the
        // second: a planner that emitted them in one burst, or swapped them,
        // would log into nothing.
        let Step::Text { text, .. } = &p.steps()[0] else { panic!("first step is text") };
        assert_eq!(text, USER);
        let Step::Text { text, .. } = &p.steps()[3] else { panic!("fourth step is text") };
        assert_eq!(text, PASS);
    }

    #[test]
    fn a_custom_field_and_a_one_time_code_resolve() {
        let p = plan(&parse("{S:PIN}{TAB}{TOTP}"), &values()).unwrap();
        assert_eq!(typed(&p), "4821123456");
    }

    #[test]
    fn literal_text_around_a_field_joins_it_into_one_burst() {
        let p = plan(&parse("a{USERNAME}b"), &values()).unwrap();
        assert_eq!(typed(&p), format!("a{USER}b"));
        assert_eq!(p.len(), 1, "one contiguous run of typing is one step: {:?}", p.steps());
    }

    // -- refusals -----------------------------------------------------------

    #[test]
    fn a_missing_custom_field_refuses_the_whole_sequence() {
        let mut v = values();
        v.custom.clear();
        assert_eq!(plan(&parse("{USERNAME}{TAB}{S:PIN}"), &v), Err(Refusal::Unresolved("a field called PIN".into())));
    }

    #[test]
    fn an_empty_custom_field_refuses_rather_than_typing_nothing_there() {
        let v = Resolved { custom: vec![("PIN", "")], ..values() };
        assert!(matches!(plan(&parse("{S:PIN}"), &v), Err(Refusal::Unresolved(_))));
    }

    #[test]
    fn a_one_time_code_that_did_not_arrive_refuses() {
        let v = Resolved { totp: None, ..values() };
        assert_eq!(plan(&parse("{TOTP}"), &v), Err(Refusal::Unresolved("a one-time code".into())));
    }

    #[test]
    fn a_missing_password_refuses_before_the_username_is_typed() {
        let v = Resolved { password: "", ..values() };
        // The refusal must come from `plan`, not from a runner that has
        // already typed the username: that is the whole point of planning
        // first.
        assert_eq!(plan(&parse("{USERNAME}{TAB}{PASSWORD}"), &v), Err(Refusal::Unresolved("a password".into())));
    }

    /// A `{PICKCHARS}` typed as literal text would put the eight characters
    /// `{PICKCHARS}` into a login form. The parser carries it faithfully; the
    /// planner refuses it by name.
    #[test]
    fn an_unknown_keepass_token_refuses_and_names_itself() {
        let err = plan(&parse("{USERNAME}{PICKCHARS}{PASSWORD}"), &values()).unwrap_err();
        assert_eq!(err, Refusal::Unsupported("{PICKCHARS}".into()));
        assert!(err.message().contains("{PICKCHARS}"), "message: {}", err.message());
    }

    #[test]
    fn a_grouping_character_refuses_rather_than_being_typed() {
        assert!(matches!(plan(&parse("({USERNAME})"), &values()), Err(Refusal::Unsupported(_))));
    }

    #[test]
    fn a_modifier_with_no_key_after_it_refuses() {
        assert_eq!(plan(&parse("^{USERNAME}"), &values()), Err(Refusal::DanglingModifier("Ctrl".into())));
        assert_eq!(plan(&parse("{USERNAME}+"), &values()), Err(Refusal::DanglingModifier("Shift".into())));
    }

    #[test]
    fn an_hour_long_delay_is_refused_at_plan_time() {
        let err = plan(&parse("{USERNAME}{DELAY 3600000}{PASSWORD}"), &values()).unwrap_err();
        assert!(matches!(err, Refusal::TooLong(_)), "got {err:?}");
        // Positive control: just under the bound plans fine, so the test
        // above is pinning the bound and not merely that delays refuse.
        let ok = format!("{{USERNAME}}{{DELAY {}}}{{PASSWORD}}", MAX_SEQUENCE.as_millis() - 5000);
        assert!(plan(&parse(&ok), &values()).is_ok());
    }

    #[test]
    fn a_sequence_that_types_nothing_refuses() {
        assert_eq!(plan(&parse(""), &values()), Err(Refusal::Nothing));
    }

    // -- modifiers ----------------------------------------------------------

    #[test]
    fn modifiers_accumulate_onto_the_key_that_follows_and_then_reset() {
        let p = plan(&parse("^+{TAB}{TAB}"), &values()).unwrap();
        assert_eq!(
            keys(&p),
            vec![
                ("TAB", ModSet { shift: true, ctrl: true, alt: false }),
                // A modifier that leaked onto the next key would be a stuck
                // Ctrl+Shift+Tab, which cycles the user's browser tabs.
                ("TAB", ModSet::default()),
            ]
        );
    }

    #[test]
    fn a_modifier_followed_by_another_modifier_stacks_rather_than_refusing() {
        // `^+{TAB}` is one chord, not a dangling `^`. The guard that catches
        // `^{USERNAME}` must not catch this.
        assert!(plan(&parse("^+{TAB}"), &values()).is_ok());
        // ...and `+~` is Shift+Enter, because `~` is a key.
        let p = plan(&parse("+~"), &values()).unwrap();
        assert_eq!(keys(&p), vec![("ENTER", ModSet { shift: true, ..Default::default() })]);
    }

    #[test]
    fn a_modifier_before_a_delay_refuses() {
        // Not text and not a key: there is nothing for the Ctrl to land on.
        assert_eq!(
            plan(&parse("{TAB}^{DELAY 100}{TAB}"), &values()),
            Err(Refusal::DanglingModifier("Ctrl".into()))
        );
    }

    #[test]
    fn a_tilde_is_the_enter_key_and_not_a_modifier() {
        let p = plan(&parse("{USERNAME}~"), &values()).unwrap();
        assert_eq!(keys(&p), vec![("ENTER", ModSet::default())]);
    }

    #[test]
    fn every_known_key_has_a_virtual_key() {
        for key in KEYS {
            assert!(virtual_key(key.token).is_some(), "{} has no virtual-key code", key.token);
        }
        // Positive control: the lookup can say no.
        assert!(virtual_key("PICKCHARS").is_none());
    }

    #[test]
    fn the_navigation_keys_are_marked_extended() {
        // A `VK_DELETE` sent without the extended flag is the keypad's `.` to
        // some applications -- a full stop typed into a login form.
        for token in ["DELETE", "INSERT", "HOME", "END", "PGUP", "PGDN", "UP", "DOWN", "LEFT", "RIGHT"] {
            assert_eq!(virtual_key(token).map(|(_, ext)| ext), Some(true), "{token}");
        }
        for token in ["ENTER", "TAB", "SPACE", "BACKSPACE", "F1"] {
            assert_eq!(virtual_key(token).map(|(_, ext)| ext), Some(false), "{token}");
        }
    }

    // -- bursts -------------------------------------------------------------

    /// **The burst bound is measured in the units the keyboard really sleeps
    /// on.**
    ///
    /// `RealKeyboard::type_text` sleeps once per `encode_utf16` unit, so an
    /// astral-plane character costs *two* sleeps. Projecting and chopping by
    /// `char` made a burst of emoji take exactly twice its projection --
    /// 249ms projected, 498ms really slept -- and that doubling is the gap
    /// between two foreground checks.
    ///
    /// The fixture is deliberately **mixed and non-uniform**: one astral
    /// character (2 units) alternating with one BMP character (1 unit), so a
    /// `chars()` count and an `encode_utf16()` count disagree by a ratio the
    /// assertions below can actually see. A fixture of pure emoji would have
    /// let an off-by-exactly-2 error look like a units/chars mix-up either
    /// way round.
    #[test]
    fn an_astral_burst_is_projected_and_chopped_at_its_real_cost() {
        // 160 chars, 240 UTF-16 code units: the two counts differ by 1.5x.
        let astral = "\u{1F600}a".repeat(80);
        assert_eq!(astral.chars().count(), 160);
        assert_eq!(astral.encode_utf16().count(), 240);

        let v = Resolved { password: &astral, ..values() };
        let p = plan(&parse("{PASSWORD}"), &v).unwrap();
        assert!(p.len() > 1, "720ms of real typing must not be one burst");

        for step in p.steps() {
            let Step::Text { text, rate } = step else { continue };
            // What `plan` believes a step costs is what `type_text` will
            // really spend -- one sleep AND one `SendInput` per UTF-16 unit,
            // not a `chars()` projection of either.
            assert_eq!(
                step.projected(),
                (*rate + SEND_INPUT_COST) * text.encode_utf16().count() as u32,
                "a step's projection is not the time the keyboard will really take"
            );
            assert!(
                step.projected() <= MAX_BURST,
                "a burst of {:?} exceeds {MAX_BURST:?}",
                step.projected()
            );
        }

        // Round-tripping proves no surrogate pair was chopped in half: a
        // split pair could not have reassembled into the original `String`.
        assert_eq!(typed(&p), astral, "chopping lost, reordered or split a character");
    }

    /// The BMP case the fix must not disturb: Cyrillic is 1 unit per char, so
    /// units and chars agree and the chunking is exactly what it always was.
    #[test]
    fn bmp_text_is_chopped_by_the_same_count_as_before() {
        let cyrillic = "\u{43F}\u{430}\u{440}\u{43E}\u{43B}\u{44C}".repeat(40);
        assert_eq!(cyrillic.chars().count(), cyrillic.encode_utf16().count());
        let v = Resolved { password: &cyrillic, ..values() };
        let p = plan(&parse("{PASSWORD}"), &v).unwrap();
        for step in p.steps() {
            assert!(step.projected() <= MAX_BURST, "{:?}", step.projected());
        }
        assert_eq!(typed(&p), cyrillic);
    }

    #[test]
    fn a_long_run_of_text_is_split_so_foreground_can_be_rechecked() {
        let long = "x".repeat(500);
        let v = Resolved { password: &long, ..values() };
        let p = plan(&parse("{PASSWORD}"), &v).unwrap();
        assert!(p.len() > 1, "500 chars at 3ms is 1.5s and must not be one burst");
        for step in p.steps() {
            assert!(
                step.projected() <= MAX_BURST,
                "a burst of {:?} exceeds {MAX_BURST:?}",
                step.projected()
            );
        }
        assert_eq!(typed(&p), long, "splitting must not lose or reorder a character");
    }

    #[test]
    fn a_short_password_is_a_single_burst() {
        // Positive control for the test above: the split is driven by
        // duration, not applied unconditionally.
        let p = plan(&parse("{PASSWORD}"), &values()).unwrap();
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn a_slow_typing_rate_shortens_the_burst_instead_of_lengthening_it() {
        // `{DELAY=50}` at 20 characters is a full second of typing. Bounding
        // by character count instead of duration would have made this one
        // burst -- a second-wide hole in the foreground check.
        let v = Resolved { password: "abcdefghijklmnopqrst", ..values() };
        let p = plan(&parse("{DELAY=50}{PASSWORD}"), &v).unwrap();
        for step in p.steps() {
            assert!(step.projected() <= MAX_BURST, "{:?}", step.projected());
        }
        assert_eq!(typed(&p), "abcdefghijklmnopqrst");
    }

    #[test]
    fn a_rate_change_applies_only_to_the_text_after_it() {
        let p = plan(&parse("ab{DELAY=40}cd"), &values()).unwrap();
        let rates: Vec<(String, Duration)> = p
            .steps()
            .iter()
            .filter_map(|s| match s {
                Step::Text { text, rate } => Some((text.clone(), *rate)),
                _ => None,
            })
            .collect();
        assert_eq!(
            rates,
            vec![("ab".into(), DEFAULT_RATE), ("cd".into(), Duration::from_millis(40))]
        );
    }

    #[test]
    fn a_pathological_rate_still_makes_progress() {
        // `{DELAY=100000}` makes even one character exceed MAX_BURST; the
        // chunk size must floor at one rather than looping on a zero.
        let v = Resolved { password: "ab", ..values() };
        let p = plan(&parse("{DELAY=100000}{PASSWORD}"), &v);
        // Two characters at 100s each is well over the total bound, so this
        // refuses -- but it refuses, rather than hanging, which is the point.
        assert!(matches!(p, Err(Refusal::TooLong(_))), "{p:?}");
    }

    /// **How loosely [`MAX_SEQUENCE`] bounds a key-only sequence, pinned so it
    /// stays a decision.**
    ///
    /// A key press used to be charged `Duration::ZERO`, so a run of `{TAB}`
    /// passed the total-time check at any length -- the bound simply did not
    /// apply to it. It is now charged [`SEND_INPUT_COST`], the real cost of the
    /// one `SendInput` it makes, so the bound does apply; at 10us a press it
    /// takes six million of them to reach 60s, which is loose but is what such
    /// a run really costs.
    ///
    /// Both halves are asserted here so neither can drift: keys are charged
    /// something rather than nothing, and delays are still bounded.
    #[test]
    fn keys_are_charged_their_syscall_against_the_total_bound_and_delays_too() {
        // Twenty thousand tab presses: charged, and far from the bound.
        let many_keys = "{TAB}".repeat(20_000);
        let p = plan(&parse(&many_keys), &values()).expect("a key-only sequence is not refused");
        assert_eq!(p.len(), 20_000, "the keys did not all become steps");
        assert_eq!(
            p.projected(),
            SEND_INPUT_COST * 20_000,
            "a key press must be charged the one SendInput it makes"
        );
        assert_ne!(
            p.projected(),
            Duration::ZERO,
            "a key-only run is projected as free again, so MAX_SEQUENCE does not apply to it"
        );
        assert!(
            p.projected() < MAX_SEQUENCE,
            "twenty thousand presses is meant to be well inside the bound; the cost is wrong"
        );

        // The bounded half, in the same test so the two claims cannot drift:
        // one delay past the limit is still refused.
        let too_long = format!("{{DELAY {}}}", MAX_SEQUENCE.as_millis() + 1);
        assert!(
            matches!(plan(&parse(&too_long), &values()), Err(Refusal::TooLong(_))),
            "the delay half of the bound stopped working"
        );
    }

    /// **Nothing the keyboard actually calls is projected as free.**
    ///
    /// The bound [`run`] types under is computed from [`Step::projected`], and
    /// while that charged the sleep and nothing else, a step with no sleep in
    /// it -- a key press, or a `Step::Text` at `rate == 0` -- was projected at
    /// `Duration::ZERO` however much of it there was. A fill could therefore
    /// spend unbounded real time between two foreground checks while believing
    /// it was inside its budget, which is the window changing under a fill that
    /// thinks it still owns it.
    ///
    /// `Step::Text { rate: ZERO }` is constructed directly rather than parsed,
    /// because [`MIN_RATE`] clamps `{DELAY=0}` on the way through [`plan`] and
    /// would hide exactly the case this is about. The clamp is a second,
    /// independent defence and has its own test; this one is about the
    /// projection standing on its own.
    #[test]
    fn a_zero_delay_run_is_not_projected_as_free() {
        // Positive control on the instrument: the sleep half is still counted,
        // so a zero below is a fact about the syscall charge and not about a
        // projection that returns zero for everything.
        assert_eq!(
            Step::Wait(Duration::from_secs(7)).projected(),
            Duration::from_secs(7),
            "control: projected() no longer counts a delay either"
        );

        let free_text = Step::Text { text: "a".repeat(100_000), rate: Duration::ZERO };
        assert_eq!(
            free_text.projected(),
            SEND_INPUT_COST * 100_000,
            "a hundred thousand zero-delay units must be charged a hundred thousand syscalls"
        );
        assert!(
            free_text.projected() > MAX_BURST,
            "a zero-delay run of 100,000 characters is projected at {:?}, inside the burst \
             bound -- so `run` would type all of it between two foreground checks",
            free_text.projected()
        );

        let free_key = Step::Key { key: enter_key().expect("ENTER is typable"), mods: ModSet::default() };
        assert_eq!(free_key.projected(), SEND_INPUT_COST, "a key press is free again");

        // And the whole-plan sum, which is what `MAX_SEQUENCE` reads, carries
        // it: a `Plan` of nothing but zero-delay steps is not projected at
        // zero. Built through `plan` so the sum really is the one production
        // checks; `{DELAY=1}` is the fastest rate that survives the clamp.
        let p = plan(&parse("{DELAY=1}{PASSWORD}{TAB}"), &values())
            .expect("a one-millisecond rate plans");
        let units = values().password.encode_utf16().count() as u32;
        assert!(units > 0, "control: the fixture password is empty, so this counts nothing");
        assert_eq!(
            p.projected(),
            (MIN_RATE + SEND_INPUT_COST) * units + SEND_INPUT_COST,
            "the plan's sum must carry the syscall charge for both the text and the key"
        );
    }

    /// **`{DELAY=0}` used to switch off both bounds at once.**
    ///
    /// It parses cleanly -- `key_sequence::whole_number` rejects *leading*
    /// zeros, not a bare zero -- and at `rate == 0`, `chunk_units` computed
    /// `MAX_BURST / 1ns`, i.e. 250,000,000 units. A 5000-character password was
    /// therefore **one** chunk, typed with no foreground check anywhere inside
    /// it, while `projected()` returned `ZERO` so `MAX_SEQUENCE` saw nothing to
    /// bound. The chunking tests covered `{DELAY=50}` and `{DELAY=100000}` and
    /// never the one value that turned the machinery off.
    ///
    /// Both halves are asserted here, because clamping the rate without
    /// charging the time (or the reverse) would fix one and leave the other.
    #[test]
    fn a_zero_typing_rate_is_clamped_rather_than_disabling_both_bounds() {
        let password = "p".repeat(5_000);
        assert_ne!(
            password.as_str(),
            USER,
            "the fixture cannot tell a swap from a fill"
        );

        let p = plan(&parse("{DELAY=0}{PASSWORD}"), &Resolved { password: &password, ..values() })
            .expect("{DELAY=0} parses, so it must plan rather than refuse");

        // Half one: it is chopped, and every chunk is inside the burst bound,
        // so `run` gets a foreground check between each of them.
        assert!(p.len() > 1, "a 5000-unit run was not chopped at all");
        for step in p.steps() {
            assert!(
                step.projected() <= MAX_BURST,
                "a chunk projected {:?}, past MAX_BURST",
                step.projected()
            );
        }

        // Half two: the projection `MAX_SEQUENCE` reads is no longer zero --
        // the floor rate for the sleep, plus the syscall that happens whether
        // or not there is a sleep.
        assert_eq!(
            p.projected(),
            (MIN_RATE + SEND_INPUT_COST) * 5_000,
            "the run must be charged at the floor rate plus its syscalls, not at nothing"
        );

        // The clamp is about time, not about text: nothing was dropped,
        // duplicated or reordered on the way.
        assert_eq!(typed(&p), password, "the clamp changed what gets typed");

        // And the bound it restored really bites. Before, this projected at
        // zero and was waved through however long it was.
        let huge = "p".repeat(MAX_SEQUENCE.as_millis() as usize + 1);
        assert!(
            matches!(
                plan(&parse("{DELAY=0}{PASSWORD}"), &Resolved { password: &huge, ..values() }),
                Err(Refusal::TooLong(_))
            ),
            "a zero-rate run past MAX_SEQUENCE was still waved through"
        );
    }

    /// **Secrets: a dropped `Plan` does not hand the password to the
    /// allocator in the clear.**
    ///
    /// Uses `login_ui`'s allocator-watch harness -- the same instrument that
    /// pins `LoginForm`'s `Drop`. Deleting the `Drop for Plan` impl fails
    /// this.
    #[test]
    fn a_dropped_plan_does_not_release_the_password_in_the_clear() {
        use crate::login_ui::password_lifetime_tests::{plaintext_reached_the_allocator, PROBE};

        // Positive control: a bare `String` of the probe, dropped without a
        // wipe, IS seen -- so a negative result below means the wipe worked
        // and not that the instrument is deaf.
        let bare = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "the allocator watch is not seeing an unwiped drop"
        );

        let password = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        let built = plan(&parse("{USERNAME}{TAB}{PASSWORD}"), &Resolved { password: &password, ..values() })
            .expect("plans");
        assert!(
            !plaintext_reached_the_allocator(move || drop(built)),
            "a dropped Plan released the password in the clear"
        );
    }

    /// **Secrets: a `Plan` dropped while the stack UNWINDS does not release
    /// the password either.**
    ///
    /// `login_ui` has `an_unwind_does_not_release_the_master_password_in_the_
    /// clear` for `LoginForm`, and this is its `Plan` equivalent, which was
    /// absent. Success (the test above), refusal-before-flush and
    /// refusal-after-flush (`refusal_lifetime_tests`) were all covered; the
    /// unwind was not.
    ///
    /// It is not a hypothetical exit. A `Plan` is moved onto the typing
    /// thread (`RealSendInput::fill_sequence`), and a panic anywhere in
    /// `perform` -- an arithmetic overflow in the burst chop, an `expect` on
    /// a poisoned lock -- unwinds that thread with the plan live. What
    /// carries the password out is `Drop for Plan`, and `Drop` running during
    /// an unwind is a different fact from `Drop` running at end of scope only
    /// in that nothing here had ever asserted it.
    ///
    /// Both halves are needed. The control shows the watch really does see an
    /// ordinary `String` go past *on an unwinding stack* -- panic machinery
    /// allocates, and an instrument that lost the thread there would make the
    /// assertion below pass by seeing nothing at all.
    #[test]
    fn an_unwind_does_not_release_the_password_in_the_clear() {
        use crate::login_ui::password_lifetime_tests::{plaintext_reached_the_allocator, PROBE};
        use std::panic::AssertUnwindSafe;

        let bare = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        assert!(
            plaintext_reached_the_allocator(move || {
                let _ = std::panic::catch_unwind(AssertUnwindSafe(move || {
                    let _held = bare;
                    panic!("deliberate: unwinding past a live plan");
                }));
            }),
            "control: an ordinary String unwound past the allocator unnoticed, so the \
             assertion below is about an instrument that sees nothing"
        );

        // Built outside the watch, for the reason the test above gives: the
        // temporaries of building a plan are not what this is measuring.
        let password = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        let built = plan(
            &parse("{USERNAME}{TAB}{PASSWORD}"),
            &Resolved { password: &password, ..values() },
        )
        .expect("plans");
        assert!(
            !plaintext_reached_the_allocator(move || {
                let _ = std::panic::catch_unwind(AssertUnwindSafe(move || {
                    let _held = built;
                    panic!("deliberate: unwinding past a live plan");
                }));
            }),
            "an unwind past a live Plan released the password in the clear"
        );
    }

    /// **Secrets: a `plan` that refuses *after* accumulating the password does
    /// not hand it to the allocator either.**
    ///
    /// The test above covers the success path only: it drops a built [`Plan`],
    /// so what it pins is `Drop for Plan`. The refusal path never builds one.
    /// `{PASSWORD}{PICKCHARS}` pushes the whole password into
    /// `TextRun::pending`, then returns `Err` before any `flush`, so no `Plan`
    /// exists and `Drop for Plan` never runs. **The only thing between that
    /// password and the allocator is `impl Drop for TextRun`** -- and
    /// replacing its `zeroize` with a no-op left the whole suite green.
    ///
    /// The five shapes below reach that state by genuinely different routes --
    /// an unsupported token, a dangling modifier, a grouping, an over-long
    /// delay, and a `{S:Field}` accumulated before an unrelated refusal -- so
    /// a fix that only rescued one of them is visible here.
    #[test]
    fn a_refused_plan_does_not_release_the_accumulated_password_in_the_clear() {
        use crate::login_ui::password_lifetime_tests::{plaintext_reached_the_allocator, PROBE};

        // Positive control, in this test rather than borrowed from the one
        // above, so this test fails loudly if the instrument goes deaf.
        let bare = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "the allocator watch is not seeing an unwiped drop"
        );

        // **Leaked on purpose.** An owned `String` moved into the closure is
        // freed by the closure's *own* drop, inside the watched region, which
        // trips the probe even when the wipe under test works perfectly. The
        // leak makes the only free that can happen inside the region the one
        // this test is actually about.
        let password: &'static str = Box::leak(PROBE.to_string().into_boxed_str());

        for sequence in [
            "{PASSWORD}{PICKCHARS}",      // unsupported token, after the field
            "{S:PIN}{PASSWORD}{PICKCHARS}", // a custom field accumulated first
            "{PASSWORD}(x)",              // a grouping
            "{PASSWORD}^",                // a modifier left dangling at the end
            "{PASSWORD}{DELAY 3600000}",  // refused by the total-time bound
        ] {
            let tokens = parse(sequence);
            assert!(
                !plaintext_reached_the_allocator(move || {
                    let v = Resolved { password, ..values() };
                    assert!(
                        plan(&tokens, &v).is_err(),
                        "{sequence} was expected to refuse, so this test is \
                         not exercising the refusal path at all"
                    );
                }),
                "the plaintext accumulated by {sequence} reached the allocator \
                 in the clear when plan refused"
            );
        }
    }

    /// **Secrets: the accumulator does not hand the password to the allocator
    /// when it grows.**
    ///
    /// The two tests above watch `dealloc`, and `dealloc` is not how a
    /// growing `String` releases its old buffer -- `realloc` is. While
    /// `Watcher::realloc` forwarded blindly, both of them were silent on this
    /// axis, and `TextRun::pending` was a plain `String` grown with
    /// `push_str`: the password went in first, the literal after it forced the
    /// grow, and the old block -- password and all -- went back to the
    /// allocator unwiped on the **ordinary success path**. `Drop for TextRun`
    /// and `flush`'s `zeroize` both act on the *current* buffer and never saw
    /// it.
    ///
    /// Any sequence with content after `{PASSWORD}` reaches it:
    /// `{PASSWORD}@contoso.com`, `{PASSWORD}{S:PIN}`, or a `{USERNAME}` long
    /// enough that the accumulator grows part-way through the password.
    ///
    /// **Two positive controls, not one.** The bare drop proves the watch is
    /// armed at all; the grown `String` proves it is armed *on the growth
    /// axis specifically*, which is the half that was missing. Without the
    /// second, deleting the `realloc` scan again would leave this test green
    /// and vacuous -- the exact shape of failure this module's doc warns
    /// about.
    #[test]
    fn growing_the_accumulator_does_not_release_the_password_in_the_clear() {
        use crate::login_ui::password_lifetime_tests::{plaintext_reached_the_allocator, PROBE};

        // The fixture must not be able to mistake one string for another.
        assert_ne!(PROBE, USER, "the fixture cannot tell the password from the username");
        assert!(!FILLER.contains(PROBE), "the filler would trip the probe on its own");

        // Control one: the instrument is armed.
        let bare = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "the allocator watch is not seeing an unwiped drop"
        );

        // Control two: the instrument is armed *on growth*. Built exactly
        // like `TextRun::pending` -- empty, then the probe, then a literal
        // that forces the grow -- so a heap that answered the subject's grow
        // in place would answer this one in place too, and this control would
        // fail rather than let the subject pass vacuously.
        let mut grown = String::new();
        grown.push_str(PROBE);
        assert!(
            plaintext_reached_the_allocator(|| grown.push_str(FILLER)),
            "the allocator watch is not seeing a reallocated buffer"
        );
        drop(grown);

        // **Leaked on purpose**, for the reason the refusal test above gives:
        // the only free inside the watched region must be the one under test.
        let password: &'static str = Box::leak(PROBE.to_string().into_boxed_str());
        // The literal comes *after* the field, so the password is already in
        // the accumulator when the grow happens.
        let tokens = parse(&format!("{{PASSWORD}}{FILLER}"));
        let mut built: Option<Plan> = None;
        assert!(
            !plaintext_reached_the_allocator(|| {
                built = Some(
                    plan(&tokens, &Resolved { password, ..values() }).expect("plans"),
                );
            }),
            "TextRun::pending's reallocated buffer released the password in the clear"
        );

        // The plan really did carry the password, so a refusal cannot be what
        // made the assertion above pass.
        let built = built.expect("the watched region built a plan");
        assert!(typed(&built).contains(PROBE), "the plan never held the password");
    }

    /// Long enough that growing a probe-sized accumulation to hold it moves
    /// the block rather than extending it in place. Only `z`, so it can never
    /// contain the probe.
    const FILLER: &str = concat!(
        "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
    );
}

#[cfg(test)]
pub(crate) mod run_tests {
    use super::*;
    use crate::key_sequence::parse;
    use std::cell::{Cell, RefCell};

    const USER: &str = "work.account@contoso.com";
    const PASS: &str = "Zq7-tremulous-BADGER";

    fn values() -> Resolved<'static> {
        Resolved { username: USER, password: PASS, totp: Some("123456"), custom: vec![] }
    }

    #[derive(Debug, PartialEq, Eq)]
    enum Emitted {
        Text(String),
        Key(&'static str, ModSet),
        Waited(Duration),
    }

    /// A keyboard that types into a `Vec` and answers `holds_foreground`
    /// however the test says. **It sends no real input**, which is the same
    /// discipline `main.rs`'s `NeverTypes` applies -- there is no code path
    /// from this test module to `SendInput`.
    pub(crate) struct FakeKeyboard {
        /// Foreground answers, consumed one per check. Exhausting the list
        /// means "still in front" -- so a test only has to say when it stops.
        answers: RefCell<std::collections::VecDeque<bool>>,
        checks: Cell<u32>,
        emitted: RefCell<Vec<Emitted>>,
        /// Made to fail on the Nth emission, to pin that a `SendInput`
        /// failure stops the sequence too.
        fail_at: Cell<Option<u32>>,
        emissions: Cell<u32>,
    }

    impl FakeKeyboard {
        pub(crate) fn new() -> Self {
            Self {
                answers: RefCell::new(Default::default()),
                checks: Cell::new(0),
                emitted: RefCell::new(Vec::new()),
                fail_at: Cell::new(None),
                emissions: Cell::new(0),
            }
        }

        /// Foreground for the first `n` checks, then gone for good.
        pub(crate) fn loses_foreground_after(n: usize) -> Self {
            let me = Self::new();
            let mut answers: std::collections::VecDeque<bool> = std::iter::repeat(true).take(n).collect();
            answers.push_back(false);
            *me.answers.borrow_mut() = answers;
            me
        }

        /// Everything actually emitted, as a flat list a sibling module can
        /// read. `Emitted` is private to this module and stays that way; what
        /// leaves is a rendering, which is all `injector::mod`'s tests need to
        /// say "it typed the username, pressed Tab, then typed the password"
        /// -- and, crucially, enough to catch a run that stopped early or that
        /// swapped the two.
        pub(crate) fn transcript(&self) -> Vec<String> {
            self.emitted
                .borrow()
                .iter()
                .map(|e| match e {
                    Emitted::Text(t) => format!("type {t}"),
                    Emitted::Key(k, _) => format!("press {k}"),
                    Emitted::Waited(d) => format!("wait {}ms", d.as_millis()),
                })
                .collect()
        }

        /// How many times [`Keyboard::holds_foreground`] has been asked.
        pub(crate) fn foreground_checks(&self) -> u32 {
            self.checks.get()
        }

        fn note(&self, e: Emitted) -> Result<(), String> {
            let n = self.emissions.get();
            self.emissions.set(n + 1);
            if self.fail_at.get() == Some(n) {
                return Err("SendInput dropped a keystroke".into());
            }
            self.emitted.borrow_mut().push(e);
            Ok(())
        }
    }

    impl Keyboard for FakeKeyboard {
        fn holds_foreground(&self, _hwnd: isize) -> bool {
            self.checks.set(self.checks.get() + 1);
            let mut answers = self.answers.borrow_mut();
            // Once the deque has produced a `false` it stays empty; the
            // default for an exhausted deque is the *last* answer given, so a
            // window that went away does not come back.
            match answers.pop_front() {
                Some(a) => {
                    if !a {
                        answers.clear();
                        answers.push_back(false);
                    }
                    a
                }
                None => true,
            }
        }

        fn type_text(&self, text: &str, _rate: Duration) -> Result<(), String> {
            self.note(Emitted::Text(text.to_string()))
        }

        fn press_key(&self, key: &'static KeyDef, mods: ModSet) -> Result<(), String> {
            self.note(Emitted::Key(key.token, mods))
        }

        fn wait(&self, how_long: Duration) {
            self.emitted.borrow_mut().push(Emitted::Waited(how_long));
        }
    }

    fn ms365() -> Plan {
        plan(&parse("{USERNAME}{ENTER}{DELAY 2000}{PASSWORD}{ENTER}"), &values()).unwrap()
    }

    #[test]
    fn a_sequence_that_keeps_foreground_runs_to_completion() {
        let kb = FakeKeyboard::new();
        run(&kb, 42, &ms365()).unwrap();
        assert_eq!(
            *kb.emitted.borrow(),
            vec![
                Emitted::Text(USER.into()),
                Emitted::Key("ENTER", ModSet::default()),
                Emitted::Waited(Duration::from_millis(2000)),
                Emitted::Text(PASS.into()),
                Emitted::Key("ENTER", ModSet::default()),
            ]
        );
    }

    /// **The check happens before EVERY step, not once at the start.**
    ///
    /// Hoisting the check out of the loop leaves the test above green and
    /// fails only here.
    #[test]
    fn foreground_is_rechecked_once_per_step() {
        let kb = FakeKeyboard::new();
        let p = ms365();
        run(&kb, 42, &p).unwrap();
        assert_eq!(kb.checks.get() as usize, p.len());
        assert!(p.len() > 1, "the plan must have more than one step for this to mean anything");
    }

    /// **The alt-tab case: the user switches windows during the `{DELAY}`.**
    ///
    /// Three steps have run (username, Enter, the wait); the fourth is the
    /// password. Nothing after the switch may be emitted.
    #[test]
    fn losing_foreground_during_the_delay_types_no_password() {
        let kb = FakeKeyboard::loses_foreground_after(3);
        let err = run(&kb, 42, &ms365()).unwrap_err();

        let emitted = kb.emitted.borrow();
        assert_eq!(
            *emitted,
            vec![
                Emitted::Text(USER.into()),
                Emitted::Key("ENTER", ModSet::default()),
                Emitted::Waited(Duration::from_millis(2000)),
            ]
        );
        assert!(
            !emitted.contains(&Emitted::Text(PASS.into())),
            "the password was typed after the window went away"
        );
        assert!(err.contains("no longer in front"), "got: {err}");
        assert!(err.contains("after 3 of 5"), "the error must say how far it got: {err}");
    }

    #[test]
    fn losing_foreground_before_the_first_step_types_nothing_at_all() {
        let kb = FakeKeyboard::loses_foreground_after(0);
        let err = run(&kb, 42, &ms365()).unwrap_err();
        assert!(kb.emitted.borrow().is_empty(), "{:?}", kb.emitted.borrow());
        assert!(err.contains("after 0 of 5"), "got: {err}");
    }

    /// A long password is split into bursts, and losing foreground between
    /// two bursts stops the rest of the *password* -- not just the rest of
    /// the sequence. This is what the [`MAX_BURST`] chunking buys.
    #[test]
    fn losing_foreground_mid_password_stops_the_remaining_bursts() {
        let long = "p".repeat(500);
        let p = plan(&parse("{PASSWORD}"), &Resolved { password: &long, ..values() }).unwrap();
        assert!(p.len() >= 3, "needs several bursts to be a meaningful test: {}", p.len());

        let kb = FakeKeyboard::loses_foreground_after(1);
        run(&kb, 42, &p).unwrap_err();

        let typed: usize = kb
            .emitted
            .borrow()
            .iter()
            .map(|e| match e {
                Emitted::Text(t) => t.chars().count(),
                _ => 0,
            })
            .sum();
        assert!(typed < long.len(), "the whole password was typed anyway ({typed} chars)");
        assert!(typed > 0, "positive control: the first burst should have been typed");
    }

    #[test]
    fn a_keystroke_failure_stops_the_sequence() {
        let kb = FakeKeyboard::new();
        kb.fail_at.set(Some(1)); // the {ENTER} after the username
        let err = run(&kb, 42, &ms365()).unwrap_err();
        assert!(err.contains("SendInput"), "got: {err}");
        assert_eq!(*kb.emitted.borrow(), vec![Emitted::Text(USER.into())]);
    }

    #[test]
    fn modifiers_reach_the_keyboard_with_the_key_they_belong_to() {
        let kb = FakeKeyboard::new();
        let p = plan(&parse("^+{TAB}{TAB}"), &values()).unwrap();
        run(&kb, 42, &p).unwrap();
        assert_eq!(
            *kb.emitted.borrow(),
            vec![
                Emitted::Key("TAB", ModSet { shift: true, ctrl: true, alt: false }),
                Emitted::Key("TAB", ModSet::default()),
            ]
        );
    }
}

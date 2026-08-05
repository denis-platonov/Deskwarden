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
/// The same 3ms [`crate::injector::send_input`] already uses, and for the same
/// reason: controls that do per-character work on their own UI thread drop
/// characters delivered faster than that. Shared as a constant here rather
/// than a second literal so a sequence with no `{DELAY=n}` types at exactly
/// the rate the default fill does.
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

/// The longest a whole sequence may be *projected* to take.
///
/// Sixty seconds is far more than any real sign-in flow -- the motivating
/// Microsoft 365 case is about three -- and far less than the hour a
/// `{DELAY 3600000}` would ask for. See the module doc.
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
    pub fn projected(&self) -> Duration {
        match self {
            Self::Text { text, rate } => *rate * text.chars().count() as u32,
            // A key press is one `SendInput` call; it is not free, but it is
            // not measurable against a 250ms burst either.
            Self::Key { .. } => Duration::ZERO,
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

    /// The most characters that fit in one burst at this rate. At least one,
    /// so a `{DELAY=100000}` cannot produce a zero-length chunk and loop
    /// forever.
    fn chunk_chars(&self) -> usize {
        let rate = self.rate.max(Duration::from_nanos(1));
        (MAX_BURST.as_nanos() / rate.as_nanos()).max(1) as usize
    }

    fn flush(&mut self, out: &mut Vec<Step>) {
        if self.pending.is_empty() {
            return;
        }
        let chunk = self.chunk_chars();
        let chars: Vec<char> = self.pending.chars().collect();
        for window in chars.chunks(chunk) {
            out.push(Step::Text { text: window.iter().collect(), rate: self.rate });
        }
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
    let mut steps: Vec<Step> = Vec::new();
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
            Token::Literal(text) => run.pending.push_str(text),
            Token::Field(FieldRef::Username) => {
                if values.username.is_empty() {
                    return Err(Refusal::Unresolved("a username".into()));
                }
                run.pending.push_str(values.username);
            }
            Token::Field(FieldRef::Password) => {
                if values.password.is_empty() {
                    return Err(Refusal::Unresolved("a password".into()));
                }
                run.pending.push_str(values.password);
            }
            Token::Field(FieldRef::Totp) => match values.totp {
                Some(code) if !code.is_empty() => run.pending.push_str(code),
                _ => return Err(Refusal::Unresolved("a one-time code".into())),
            },
            Token::Field(FieldRef::Custom(name)) => match values.custom_value(name) {
                Some(value) if !value.is_empty() => run.pending.push_str(value),
                _ => return Err(Refusal::Unresolved(format!("a field called {name}"))),
            },
            Token::Key(key) => {
                if virtual_key(key.token).is_none() {
                    return Err(Refusal::UntypableKey(key.token.to_string()));
                }
                run.flush(&mut steps);
                steps.push(Step::Key { key, mods: pending_mods });
                pending_mods = ModSet::default();
                pending_mod_label = None;
            }
            Token::Delay(ms) => {
                run.flush(&mut steps);
                steps.push(Step::Wait(Duration::from_millis(u64::from(*ms))));
            }
            Token::DelayRate(ms) => {
                // The rate change applies from here on, so the text typed
                // *before* it must be flushed at the old rate first.
                run.flush(&mut steps);
                run.rate = Duration::from_millis(u64::from(*ms));
            }
            Token::Modifier(Modifier::Enter) => {
                // `~` is KeePass's shorthand for the Enter *key*, not a
                // modifier, whatever the parser's variant is called.
                let Some(key) = enter_key() else {
                    return Err(Refusal::UntypableKey("ENTER".into()));
                };
                run.flush(&mut steps);
                steps.push(Step::Key { key, mods: pending_mods });
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
    run.flush(&mut steps);

    let plan = Plan { steps };
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
}

#[cfg(test)]
mod run_tests {
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
    struct FakeKeyboard {
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
        fn new() -> Self {
            Self {
                answers: RefCell::new(Default::default()),
                checks: Cell::new(0),
                emitted: RefCell::new(Vec::new()),
                fail_at: Cell::new(None),
                emissions: Cell::new(0),
            }
        }

        /// Foreground for the first `n` checks, then gone for good.
        fn loses_foreground_after(n: usize) -> Self {
            let me = Self::new();
            let mut answers: std::collections::VecDeque<bool> = std::iter::repeat(true).take(n).collect();
            answers.push_back(false);
            *me.answers.borrow_mut() = answers;
            me
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
pub trait Notifier {
    fn refused(&self, detail: &str);
}

/// The production notifier: a plain task-modal message box, on its own thread.
///
/// On its own thread because `MessageBoxW` blocks until dismissed, and the
/// caller is either the app's message loop (a plan-time refusal) or the typing
/// thread (a mid-sequence abort). Neither may sit in a modal loop -- the first
/// would freeze the tray and every window, and the second would hold the
/// plan's memory alive for as long as the box was on screen. Fire and forget:
/// there is no answer to read back, because there is nothing to ask.
pub struct RealNotifier;

impl Notifier for RealNotifier {
    fn refused(&self, detail: &str) {
        log::warn!("autofill refused: {detail}");
        let detail = detail.to_string();
        // **Under test this records instead of opening a window.** A message
        // box in a test suite is a real window on a real desktop that a real
        // person has to dismiss, and this crate's rule is that no test opens
        // one. Recording is also strictly better than skipping: it lets
        // `app::fill_dispatch_tests` assert that the user was *actually*
        // told, which is the half of "not just the log" that a `MessageBoxW`
        // nothing can call could never pin.
        #[cfg(test)]
        {
            NOTICES.with(|n| n.borrow_mut().push(detail));
            return;
        }
        #[cfg(not(test))]
        std::thread::spawn(move || {
            use windows::core::HSTRING;
            use windows::Win32::UI::WindowsAndMessaging::{
                MessageBoxW, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MB_TOPMOST,
            };
            let text = HSTRING::from(detail);
            let caption = HSTRING::from("Deskwarden autofill");
            unsafe {
                MessageBoxW(
                    None,
                    &text,
                    &caption,
                    MB_OK | MB_ICONWARNING | MB_SETFOREGROUND | MB_TOPMOST,
                );
            }
        });
    }
}

#[cfg(test)]
thread_local! {
    /// What [`RealNotifier`] would have shown the user, on this thread.
    static NOTICES: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Everything [`RealNotifier`] has been asked to show on this thread, cleared.
#[cfg(test)]
pub(crate) fn take_notices() -> Vec<String> {
    NOTICES.with(|n| std::mem::take(&mut *n.borrow_mut()))
}

#[cfg(test)]
mod notifier_tests {
    use super::*;

    #[test]
    fn a_refusal_is_recorded_verbatim() {
        let _ = take_notices();
        RealNotifier.refused("the auto-type sequence uses {PICKCHARS}");
        assert_eq!(take_notices(), vec!["the auto-type sequence uses {PICKCHARS}".to_string()]);
        // ...and taking clears, so one test cannot see another's notice.
        assert!(take_notices().is_empty());
    }
}

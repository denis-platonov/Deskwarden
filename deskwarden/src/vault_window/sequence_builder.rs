//! **Design 4a -- the sequence builder, as its own screen.**
//!
//! The builder itself is not new. Every pure function it stands on --
//! [`detail_edit::step_rows`], [`detail_edit::sequence_tally`],
//! [`detail_edit::sequence_with`] and the rest -- has been in this crate since
//! the keystroke work landed, and so has the step list that draws them. What
//! was missing is the thing the owner went looking for and could not find:
//!
//! > Don't see sequebce builder as per design anywhere on UI
//!
//! And they were right. The whole of 4a was folded into ONE block inside the
//! edit form's `Fill rule` card -- **shut by default**, inside a card that is
//! itself only drawn when the item is bound to an app, inside a form only
//! reached by pressing Edit. Four doors, all of them shut, in a pane 298pt
//! wide. The design is a workspace: a rail naming what the sequence belongs
//! to, the steps in the middle at a width they can be read at, and a rail of
//! measurements -- timing, checks, budget -- beside them. None of that fits in
//! a column narrower than one of its own cards, which is why what shipped was
//! the parts of 4a that survive being squeezed and none of the parts that do
//! not.
//!
//! So this module hosts the same functions at the width they were designed
//! for, and the read pane's `FILL RULE` card is the one door.
//!
//! # What is NOT reproduced, and why
//!
//! **4a's "Send for real -- needs SAP Logon focused".** A fill is a thing the
//! overlay does, against a window the user has focused, through
//! [`preflight::verdict`](super::preflight::verdict). A button here could only
//! either send a real password from a screen that is itself the foreground --
//! which is the one condition the gate refuses -- or pretend. 4d's rehearsal
//! is the honest version of the same want, and it is on this screen.
//!
//! **The template's `{WAIT=250}` and `{CTRL+A}` spellings.** They are the
//! design's, not KeePass's, and this app carries foreign sequences byte for
//! byte. See [`detail_edit::TEMPLATE_CHIPS`] for the spellings that are real.
//!
//! **The hatch on the password step.** [`super::detail_edit`] argues that one
//! at length: the colour carries the meaning and egui has no tiling brush.

use crate::app_identity::AppIdentityCache;
use crate::key_sequence::{self, FieldRef, ResolveSource, Token};
use crate::theme;
use crate::vault_window::detail_edit::{
    self, sequence_tally, step_rows, StepKind, StepRow,
};
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke};
use std::time::Duration;

// ---------------------------------------------------------------------------
// The acts, and what each one is called
// ---------------------------------------------------------------------------

/// One ACT of a sequence: the unit 4a's timing strip draws a bar for.
///
/// **Not a row and not a [`crate::injector::sequence::Step`]**, which is the
/// same distinction [`detail_edit::SequenceTally::steps`] argues: a
/// `{DELAY=n}` is a row and no act, a run of text either side of a
/// `{USERNAME}` is three rows and one act, and the runner chops a long text
/// run into several `Step`s so it can re-check the foreground between them.
/// The strip draws what the user did, so it draws acts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Act {
    pub kind: StepKind,
    /// The act in the step list's own words, so the strip and the list cannot
    /// drift -- a text run is its tokens' [`Token::chip_label`]s joined.
    pub label: String,
    /// Whether this act types a secret. Drives the bar's colour, and nothing
    /// else: the label of a secret act is the field's NAME, never its value.
    pub secret: bool,
    /// **How long this act WAITS**, for the one kind that is a wait and
    /// `None` for every other.
    ///
    /// Carried beside [`Self::label`] rather than parsed back out of it,
    /// because the two are worded differently on purpose. `label` is
    /// `key_sequence::wait_label` -- "Wait 0.3s", in the SECONDS the owner
    /// asked to set waits in -- and 8a's compact chip is `250 ms`, which is
    /// shorter and exact. A caller that needs the design's spelling needs the
    /// number, not a re-reading of the sentence.
    pub wait: Option<Duration>,
}

/// How two tokens of one typing run are joined in an act's label.
const RUN_JOIN: &str = " + ";

/// A key's name as a keycap or an act's label spells it: a held modifier's
/// chip label is `Shift+`, with the plus that the chip row reads as "and
/// then", and a cap that said `Shift+` beside a `+` would say it twice.
fn key_name(label: &str) -> &str {
    label.trim_end_matches('+')
}

/// The acts `sequence` performs, in order.
///
/// **This grouping must agree with the one behind
/// [`detail_edit::SequenceTally::steps`]**, because the timing strip pairs one
/// of these with one duration off the runner's own plan. They are two
/// functions rather than one because only this one needs labels and only that
/// one is on the hot path of every frame of the edit form -- so the agreement
/// is asserted rather than assumed, over a corpus, by
/// `the_acts_here_are_the_acts_the_tally_counts`.
pub fn acts(sequence: &str) -> Vec<Act> {
    let tokens = detail_edit::sequence_view(sequence).tokens;
    let mut acts: Vec<Act> = Vec::new();
    let mut typing = false;
    // Held for the key that follows, exactly as the runner holds it, so
    // `+{TAB}` is one act called "Shift + Tab" rather than two.
    let mut held: Vec<String> = Vec::new();
    for token in &tokens {
        match token {
            Token::Literal(_) | Token::Field(_) => {
                let secret =
                    matches!(token, Token::Field(FieldRef::Password | FieldRef::Totp));
                if typing {
                    if let Some(last) = acts.last_mut() {
                        last.label.push_str(RUN_JOIN);
                        last.label.push_str(&token.chip_label());
                        last.secret |= secret;
                    }
                } else {
                    acts.push(Act {
                        kind: StepKind::Text,
                        label: token.chip_label(),
                        secret,
                        wait: None,
                    });
                    typing = true;
                }
            }
            Token::Key(_) => {
                let mut label = String::new();
                for m in held.drain(..) {
                    // `Shift+`, as the chip row spells a held modifier, is
                    // `Shift` here: the join supplies the plus.
                    label.push_str(key_name(&m));
                    label.push_str(RUN_JOIN);
                }
                label.push_str(&token.chip_label());
                acts.push(Act { kind: StepKind::Key, label, secret: false, wait: None });
                typing = false;
            }
            Token::Delay(ms) => {
                acts.push(Act {
                    kind: StepKind::Wait,
                    label: token.chip_label(),
                    secret: false,
                    wait: Some(Duration::from_millis(u64::from(*ms))),
                });
                typing = false;
            }
            Token::Modifier(_) => {
                held.push(token.chip_label());
                typing = false;
            }
            // Not an act: it changes how the acts below type.
            Token::DelayRate(_) => {}
            Token::Grouping(_) | Token::Unknown(_) => typing = false,
        }
    }
    acts
}

// ---------------------------------------------------------------------------
// 4a's timing strip
// ---------------------------------------------------------------------------

/// One bar of the timing strip: an act, where it starts, and how long it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingSpan {
    pub act: Act,
    /// From the start of the sequence.
    pub at: Duration,
    pub len: Duration,
}

/// 4a's strip, or `None` when the runner refuses the sequence outright -- in
/// which case [`detail_edit::sequence_warning`] is already saying why and a
/// strip drawn from nothing would be a picture of a fill that cannot happen.
///
/// **Every duration comes off the runner's own plan** and none is computed
/// here. The plan's `Step`s are merged back into acts -- adjacent
/// `Step::Text`s are one act, which is the inverse of the burst chunking
/// [`crate::injector::sequence::plan`] applies -- and then paired, in order,
/// with [`acts`]. If the two ever disagree in length this answers `None`
/// rather than drawing a strip whose bars are labelled with the wrong acts.
///
/// The [`Plan`](crate::injector::sequence::Plan) is read and dropped inside
/// this function; its `Drop` wipes the plaintext it copied. Nothing it held
/// reaches the return value: a span carries a label built from the TOKENS.
pub fn timing_spans(sequence: &str, source: &ResolveSource<'_>) -> Option<Vec<TimingSpan>> {
    let tokens = detail_edit::sequence_view(sequence).tokens;
    let values = crate::injector::sequence::Resolved {
        username: source.username,
        password: source.password,
        totp: detail_edit::edit_time_totp(source.totp),
        custom: source.custom.clone(),
    };
    let plan = crate::injector::sequence::plan(&tokens, &values).ok()?;
    let mut lengths: Vec<Duration> = Vec::new();
    let mut last_was_text = false;
    for step in plan.steps() {
        let is_text = matches!(step, crate::injector::sequence::Step::Text { .. });
        if is_text && last_was_text {
            if let Some(last) = lengths.last_mut() {
                *last += step.projected();
            }
        } else {
            lengths.push(step.projected());
        }
        last_was_text = is_text;
    }
    drop(plan);

    let acts = acts(sequence);
    if acts.len() != lengths.len() {
        return None;
    }
    let mut at = Duration::ZERO;
    let mut spans = Vec::with_capacity(acts.len());
    for (act, len) in acts.into_iter().zip(lengths) {
        spans.push(TimingSpan { act, at, len });
        at += len;
    }
    Some(spans)
}

/// The strip's scale: the marks under the bars, in order, ending at `total`.
///
/// 4a draws `0 / 0.5 / 1.0 / 1.5 / 2.1 s` -- an even step, and then the real
/// total as the last mark however close the step left it. The step is chosen
/// so there are between three and six marks whatever the sequence measures,
/// because a fixed half-second step gives a 40-second sequence eighty of them.
pub fn timing_ticks(total: Duration) -> Vec<Duration> {
    if total.is_zero() {
        return vec![Duration::ZERO];
    }
    let ms = total.as_millis().max(1) as u64;
    // The first step that leaves four marks or fewer before the total.
    let step = [50u64, 100, 250, 500, 1_000, 2_000, 5_000, 10_000, 30_000]
        .into_iter()
        .find(|s| ms / s <= 4)
        .unwrap_or(60_000);
    let mut marks = vec![Duration::ZERO];
    let mut at = step;
    while at < ms {
        marks.push(Duration::from_millis(at));
        at += step;
    }
    marks.push(total);
    marks
}

// ---------------------------------------------------------------------------
// 4a's checks
// ---------------------------------------------------------------------------

/// What a check is saying about the sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckTone {
    /// This sequence does the safe thing, and here is the thing it does.
    Pass,
    /// This sequence can go wrong in a named way, and here is the fix.
    Caution,
    /// **Not about this sequence at all** -- a guarantee the runner makes
    /// whatever is typed here. 4a draws two of them, and they are worth
    /// drawing for the reason 4e gives: the editor can only warn, and the
    /// user's confidence has to come from what happens at send time.
    Fact,
}

/// A fix a check offers, applied by the caller to the stored string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckFix {
    /// Put a pause after the `{ENTER}` at this TOKEN index.
    PauseAfterEnter(usize),
}

/// The pause [`CheckFix::PauseAfterEnter`] adds, in milliseconds. 4a's own
/// number, and the one its caption prints.
pub const PAUSE_AFTER_ENTER_MS: u32 = 200;

/// The caption on that fix's button.
pub const ADD_PAUSE_LABEL: &str = "Add 200 ms";

/// One line of 4a's `Checks` card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub tone: CheckTone,
    /// The statement, in one line.
    pub line: String,
    /// What follows from it. Empty when the line says everything.
    pub note: String,
    pub fix: Option<CheckFix>,
}

/// 4a's checks card, for `sequence`.
///
/// **Every one of these is a statement this crate can stand behind**, which is
/// why there are four and not the six a plausible-looking card would have. The
/// two `Fact`s are enforcements that really exist --
/// [`preflight::verdict`](super::preflight::verdict) refuses a secret step
/// against an unmasked control, and
/// [`crate::injector::sequence::plan`] emits no step for an empty text run --
/// and the two others are properties of the token list that can be read off it
/// without guessing.
pub fn checks(sequence: &str) -> Vec<Check> {
    let tokens = detail_edit::sequence_view(sequence).tokens;
    let secret_at = tokens
        .iter()
        .position(|t| matches!(t, Token::Field(FieldRef::Password | FieldRef::Totp)));
    let mut out = Vec::new();

    if let Some(secret_at) = secret_at {
        // **A field change before the secret.** The one mistake that puts a
        // password somewhere it can be read: with no Tab (or any other key)
        // between the user name and the password, both are typed into the box
        // that had focus, and the second half of a user name box is not a
        // masked control.
        let changed = tokens[..secret_at].iter().any(|t| matches!(t, Token::Key(_)));
        // Nothing typed before it is not a missing field change -- the
        // sequence starts at the password, wherever focus already is.
        let types_before = tokens[..secret_at]
            .iter()
            .any(|t| matches!(t, Token::Literal(_) | Token::Field(_)));
        out.push(if changed || !types_before {
            Check {
                tone: CheckTone::Pass,
                line: FIELD_CHANGE_PASS.to_string(),
                note: String::new(),
                fix: None,
            }
        } else {
            Check {
                tone: CheckTone::Caution,
                line: FIELD_CHANGE_CAUTION.to_string(),
                note: FIELD_CHANGE_NOTE.to_string(),
                fix: None,
            }
        });

        out.push(Check {
            tone: CheckTone::Fact,
            line: MASKED_GATE.to_string(),
            note: String::new(),
            fix: None,
        });
    }

    // **An Enter with more to come and no pause behind it.** The dialog Enter
    // submitted is still on screen for as long as it takes to close, and the
    // keystroke after it lands in whatever is behind it.
    if let Some(index) = enter_needing_a_pause(&tokens) {
        out.push(Check {
            tone: CheckTone::Caution,
            line: ENTER_CAUTION.to_string(),
            note: ENTER_NOTE.to_string(),
            fix: Some(CheckFix::PauseAfterEnter(index)),
        });
    }

    out.push(Check {
        tone: CheckTone::Fact,
        line: NO_EMPTY_TEXT.to_string(),
        note: String::new(),
        fix: None,
    });
    out
}

/// The token index of an `{ENTER}` that has an act after it and no pause
/// directly behind it, or `None`.
///
/// Only the FIRST one: the card offers a fix per line, and a second caution
/// saying the same thing about a later Enter would be one the user fixes by
/// pressing the first button anyway.
fn enter_needing_a_pause(tokens: &[Token]) -> Option<usize> {
    for (index, token) in tokens.iter().enumerate() {
        let Token::Key(key) = token else { continue };
        if key.token != "ENTER" {
            continue;
        }
        let rest = &tokens[index + 1..];
        // A rate change is not an act and cannot be what happens next.
        let next = rest.iter().find(|t| !matches!(t, Token::DelayRate(_)));
        match next {
            None => continue,
            Some(Token::Delay(_)) => continue,
            Some(_) => return Some(index),
        }
    }
    None
}

/// `sequence` with a [`PAUSE_AFTER_ENTER_MS`] pause inserted after the token at
/// `index`.
///
/// Out of range changes nothing, and it returns the input **unchanged** there
/// rather than a re-rendered copy -- the care
/// [`detail_edit::sequence_moved`] takes, for its reason: a click that does
/// nothing must not be able to re-spell a sequence this build merely carries.
pub fn with_pause_after(sequence: &str, index: usize) -> String {
    let mut tokens = key_sequence::effective_tokens(sequence);
    if index >= tokens.len() {
        return sequence.to_string();
    }
    tokens.insert(index + 1, Token::Delay(PAUSE_AFTER_ENTER_MS));
    key_sequence::render(&tokens)
}

// ---------------------------------------------------------------------------
// The steps: what one row of 4a's list is, and how the list is edited
// ---------------------------------------------------------------------------

/// One row of the builder's list: a span of adjacent tokens the runner
/// performs as one thing.
///
/// **A row is an ACT wherever the tokens make one**, which is what 4a draws:
/// its first row is `Ctrl` + `A` -- a held modifier and the key it is held
/// for -- in ONE row with a `+` between the caps. [`acts`] is that grouping:
/// a run of adjacent text tokens is one act, a key with the modifiers before
/// it is one, a wait is one. [`detail_edit::step_rows`] is one row per TOKEN,
/// the grain the edit form's list is drawn at and edits at; this list edits
/// at the act's grain, so dragging the password step moves
/// `hello{PASSWORD}world` whole, because that is the one thing the user did
/// and the one bar the timing strip would draw for it.
///
/// **Plus the tokens that are not acts**, each as its own row, because they
/// are still in the string and still have to be seen, moved and removed: a
/// `{DELAY=n}` rate change, a grouping character, a construct this build does
/// not understand, and a modifier with no key directly after it. [`acts`]
/// leaves those out because the timing strip has no bar for them; a list
/// that left them out would be editing a string the user cannot see all of.
/// `the_steps_that_are_acts_are_the_acts` holds the two groupings together
/// where they overlap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// 1-based, as drawn.
    pub number: usize,
    pub kind: StepKind,
    /// The token indices this row spans, contiguous. What a move or a
    /// removal acts on.
    pub tokens: std::ops::Range<usize>,
    /// The token rows in it, in order: what each cap, pill and value is drawn
    /// from. Never empty.
    pub rows: Vec<StepRow>,
    /// Whether any token in it types a secret.
    pub secret: bool,
}

impl Step {
    /// The far cell: the first row's, which on a text run is the rate every
    /// token in the run types at, and the dash on everything else.
    pub fn note(&self) -> &str {
        &self.rows[0].note
    }

    /// What the row says beside its step: its rows' explanations, the
    /// distinct ones, joined. A rate's, a secret's, a lone modifier's `held
    /// for the next key` -- and NOT that one on `Shift + Tab`, where the key
    /// it is held for is drawn beside it and the sentence would explain what
    /// the row already shows.
    pub fn aside(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        for row in &self.rows {
            if row.aside.is_empty() || parts.contains(&row.aside.as_str()) {
                continue;
            }
            if self.rows.len() > 1 && row.aside == detail_edit::MODIFIER_NOTE {
                continue;
            }
            parts.push(&row.aside);
        }
        parts.join(" \u{b7} ")
    }

    /// The step in words, for the chip that follows the pointer while it is
    /// dragged: its rows' labels joined the way [`acts`] joins them.
    pub fn label(&self) -> String {
        self.rows.iter().map(|row| key_name(&row.label)).collect::<Vec<_>>().join(RUN_JOIN)
    }

    /// Whether this build knows every token in it.
    pub fn understood(&self) -> bool {
        self.rows.iter().all(|row| row.understood)
    }
}

/// Where each of [`steps`]' rows begins and ends in the token list, by kind.
///
/// Pure over the tokens, so a move and a removal can find their spans without
/// the values a drawn row needs.
pub fn step_spans(tokens: &[Token]) -> Vec<(StepKind, std::ops::Range<usize>)> {
    let mut spans = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let start = i;
        let kind = match &tokens[i] {
            Token::Literal(_) | Token::Field(_) => {
                while matches!(tokens.get(i), Some(Token::Literal(_) | Token::Field(_))) {
                    i += 1;
                }
                StepKind::Text
            }
            Token::Modifier(_) => {
                // Held for the key that follows, as `acts` holds it: the
                // modifiers and that key are one row. With no key directly
                // after them each modifier stands alone, and its row says so.
                let mut j = i;
                while matches!(tokens.get(j), Some(Token::Modifier(_))) {
                    j += 1;
                }
                i = if matches!(tokens.get(j), Some(Token::Key(_))) { j + 1 } else { i + 1 };
                StepKind::Key
            }
            Token::Key(_) => {
                i += 1;
                StepKind::Key
            }
            Token::Delay(_) => {
                i += 1;
                StepKind::Wait
            }
            Token::DelayRate(_) => {
                i += 1;
                StepKind::Rate
            }
            Token::Grouping(_) | Token::Unknown(_) => {
                i += 1;
                StepKind::Raw
            }
        };
        spans.push((kind, start..i));
    }
    spans
}

/// The list for `sequence`, as drawn. `reveal` is the eye, as in
/// [`detail_edit::step_rows`], which decides what every row in it says.
pub fn steps(sequence: &str, source: &ResolveSource<'_>, reveal: bool) -> Vec<Step> {
    let rows = step_rows(sequence, source, reveal);
    let tokens = detail_edit::sequence_view(sequence).tokens;
    step_spans(&tokens)
        .into_iter()
        .enumerate()
        .map(|(index, (kind, span))| Step {
            number: index + 1,
            kind,
            rows: rows[span.clone()].to_vec(),
            secret: rows[span.clone()].iter().any(|row| row.secret),
            tokens: span,
        })
        .collect()
}

/// `sequence` with step `from` moved so that it becomes step `to`, the other
/// steps keeping their order. Every token in the step's span travels, which
/// is what makes moving the password step move `hello{PASSWORD}world` whole
/// and `Shift + Tab` move with its modifier.
///
/// **Answers the string AND where the step is in it as re-read**, because
/// the two are not the same question. Two text steps put side by side become
/// one -- the runner types adjacent text tokens as one run, so the list
/// draws one row for them -- and a lone modifier put before a key joins it;
/// so the list re-read from the answer can be shorter than `to` was counted
/// against, and a selection that took `to` on trust pointed at the wrong
/// row (measured: Delete after Alt+Down took the wait away instead of the
/// key it had just moved). The index is found by rendered character offset
/// rather than by token index, because re-parsing merges two adjacent
/// literals into one token and shifts every index after them.
///
/// Out of range, or `from == to`, hands the input back UNCHANGED rather
/// than re-rendered, with `from` -- `detail_edit::sequence_moved`'s rule,
/// for its reason: a gesture that changes nothing must not re-spell a
/// sequence this build merely carries.
pub fn sequence_with_step_moved(sequence: &str, from: usize, to: usize) -> (String, usize) {
    let tokens = key_sequence::effective_tokens(sequence);
    let spans = step_spans(&tokens);
    if from == to || from >= spans.len() || to >= spans.len() {
        return (sequence.to_string(), from);
    }
    let mut order: Vec<usize> = (0..spans.len()).collect();
    let moved = order.remove(from);
    order.insert(to, moved);
    let placed = |steps: &[usize]| -> Vec<Token> {
        steps.iter().flat_map(|&step| tokens[spans[step].1.clone()].iter().cloned()).collect()
    };
    let offset = key_sequence::render(&placed(&order[..to])).len();
    let stored = detail_edit::store(&placed(&order));
    // Where that offset falls in the string as it will be read back: the
    // step whose rendered start is the last at or before it.
    let read_back = key_sequence::effective_tokens(&stored);
    let mut at = 0;
    let mut index = 0;
    for (i, (_, span)) in step_spans(&read_back).iter().enumerate() {
        if at > offset {
            break;
        }
        index = i;
        at += key_sequence::render(&read_back[span.clone()]).len();
    }
    (stored, index)
}

/// `sequence` with step `index` gone, every token of it. Out of range changes
/// nothing. Taking the last step away stores the empty string, which is the
/// default again -- `detail_edit::store`'s rule, for its reason.
pub fn sequence_without_step(sequence: &str, index: usize) -> String {
    let mut tokens = key_sequence::effective_tokens(sequence);
    let spans = step_spans(&tokens);
    let Some((_, span)) = spans.get(index) else { return sequence.to_string() };
    tokens.drain(span.clone());
    detail_edit::store(&tokens)
}

/// The checks' words. 4a's, where 4a has them.
pub const FIELD_CHANGE_PASS: &str = "A field change happens before the secret is typed.";
pub const FIELD_CHANGE_CAUTION: &str = "Nothing moves the cursor before the secret is typed.";
pub const FIELD_CHANGE_NOTE: &str =
    "Whatever box had focus gets the user name and the password one after the other. Add a Tab.";
pub const MASKED_GATE: &str = "The secret step is gated on a masked control.";
pub const ENTER_CAUTION: &str = "No wait after Enter.";
pub const ENTER_NOTE: &str =
    "If the dialog is slow to close, the next keystroke lands in the window behind it.";
pub const NO_EMPTY_TEXT: &str = "Deskwarden never sends a text step whose content is empty.";

// ---------------------------------------------------------------------------
// The screen's own state
// ---------------------------------------------------------------------------

/// Everything the builder screen edits, and the one thing it is editing it
/// against.
///
/// **The sequence is the only field that changes the item.** `app` is the
/// binding as stored, carried so the screen can name what the rule sends to
/// and so a save writes the binding back whole -- see [`SequenceDraft::saved`],
/// which replaces one field of it and touches nothing else. That matters more
/// than it looks: [`crate::app_match::AppMatch::sequence`] lives beside fields
/// this screen does not understand and must not re-spell.
pub struct SequenceDraft {
    pub item_id: String,
    pub item_name: String,
    /// What the item is filed under, for the rail. Empty when nothing.
    pub folder: String,
    /// The binding as stored.
    pub app: crate::app_match::AppMatch,
    /// The app's product name, resolved by the caller's identity cache.
    pub app_name: String,
    /// The sequence as stored, so Discard has something to go back to and the
    /// header can say whether anything changed.
    pub original: String,
    /// The sequence being edited. **Stored verbatim** -- see
    /// [`crate::app_match::AppMatch::sequence`].
    pub sequence: String,
    pub template_view: bool,
    pub template_draft: String,
    pub template_touched: bool,
    /// The eye. Shut on arrival, every time.
    pub revealing: bool,
    pub literal_draft: String,
    pub wait_draft: String,
    /// The step the keyboard acts on, by index into [`steps`]; `None` when
    /// none is. Set by a click on a row and moved by the arrows -- see
    /// `step_keys`. Not saved and not compared: a selection is not an edit,
    /// and [`SequenceDraft::changed`] does not look at it.
    pub selected: Option<usize>,
    /// **Whether the "discard your changes?" card is up over this one.**
    ///
    /// The owner: "if dirty rule and Esc - should ask about being reset".
    /// The edit form has asked that question since it had a Cancel, and a
    /// builder that threw an edited rule away on one press of Escape was
    /// the one screen in this app where a way out was not also a question.
    ///
    /// Not part of the rule and not stored: it is a state of closing the
    /// card, and a card that opened with it set would open on a dialogue.
    pub discard_prompt: bool,
}

impl SequenceDraft {
    /// The draft for `item`'s binding, or `None` when the item has no binding
    /// -- in which case there is nowhere for a sequence to live at all. See
    /// [`crate::app_match::AppMatch::sequence`], and [`fill_rule_visible`].
    pub fn for_item(
        item: &crate::vault_bridge::VaultItem,
        folder: &str,
        app_name: &str,
    ) -> Option<Self> {
        let app = crate::vault_bridge::extract_app_match(item)?;
        let sequence = app.sequence.clone();
        Some(Self {
            item_id: item.id.clone(),
            item_name: item.name.clone(),
            folder: folder.to_string(),
            app_name: app_name.to_string(),
            app,
            original: sequence.clone(),
            template_draft: sequence.clone(),
            sequence,
            template_view: false,
            template_touched: false,
            revealing: false,
            literal_draft: String::new(),
            wait_draft: DEFAULT_WAIT_SECONDS.to_string(),
            selected: None,
            discard_prompt: false,
        })
    }

    /// **Starts this draft from the sequence the EDIT FORM is holding**,
    /// rather than from the one the item stores.
    ///
    /// The two differ whenever the form has been open long enough to change
    /// the rule and has not been saved -- and the builder opened from that
    /// form has to continue the user's work, not reach behind it for the
    /// vault's copy. `original` moves with it, so the screen opens reporting
    /// no unsaved change of its own: what is unsaved belongs to the form the
    /// user came from and will be saved by it.
    ///
    /// Consuming, so a caller cannot build one and forget to seed it.
    #[must_use]
    pub fn continuing(mut self, sequence: &str) -> Self {
        self.sequence = sequence.to_string();
        self.original = self.sequence.clone();
        self.template_draft = self.sequence.clone();
        self
    }

    /// Whether the sequence has moved off what the item stores.
    pub fn changed(&self) -> bool {
        self.sequence != self.original
    }

    /// The fault that stops a save, or `None`. An inherited string is never
    /// judged by it -- [`detail_edit::AppMatchDraft::template_fault`]'s rule,
    /// for its reason: a sequence written by another password manager must not
    /// become unsaveable merely by being looked at.
    pub fn fault(&self) -> Option<&'static str> {
        if !self.template_touched {
            return None;
        }
        detail_edit::template_fault(&self.sequence)
    }

    pub fn saveable(&self) -> bool {
        self.changed() && self.fault().is_none()
    }

    /// The binding this draft would write: the stored one with its sequence
    /// replaced, and nothing else touched.
    pub fn saved(&self) -> crate::app_match::AppMatch {
        crate::app_match::AppMatch { sequence: self.sequence.clone(), ..self.app.clone() }
    }
}

/// The wait box's starting value, in seconds. The edit form's own default, for
/// its reason.
const DEFAULT_WAIT_SECONDS: &str = "1";

/// What the screen reports out to the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuilderAction {
    None,
    Save,
    /// Leave without saving. The caller drops the draft.
    Discard,
    /// 4d. Carries nothing, exactly as [`detail_edit::EditAction::Rehearse`]
    /// does and for the same reason: the plan is built from the SEQUENCE with
    /// every field resolved to a fixed sample, so there is no argument here
    /// that could carry a secret.
    Rehearse,
}

/// Whether the read pane should offer a way in here at all.
///
/// A sequence is stored on the binding, so an item bound to nothing has
/// nowhere to put one. That is not a gap in this screen -- it is what
/// [`crate::app_match::AppMatch::sequence`] decided, and the read pane's
/// `MATCHED APP` card is already where a binding is made.
pub fn fill_rule_visible(app_match: Option<&crate::app_match::AppMatch>) -> bool {
    app_match.is_some()
}

// ---------------------------------------------------------------------------
// The screen
// ---------------------------------------------------------------------------

// **There is no second column, so there are no column widths.**
//
// 4a is `grid-template-columns: 1fr 340px` -- the steps and a rail of
// measurements -- and this screen has drawn both a rail and, before that, a
// third column for what the rule belongs to. The band took the third away and
// the owner has now taken the rail: "4a only keep builder without timing and
// right panel as well". `RAIL_WIDTH`, `COLUMN_GAP` and `STEPS_FLOOR` went with
// it, and so did `column`, which existed to lay one.

/// The band under it, and the steps column: 4a's `padding: 16px 20px`.
const BAND_PAD_X: i8 = 20;
const BAND_PAD_Y: i8 = 16;

/// 4a's tiles in that band: `width: 34px; height: 34px`.
const BAND_TILE: f32 = 34.0;

/// `gap: 18px` between the band's three groups, `gap: 11px` between a tile and
/// the words beside it.
const BAND_GAP: f32 = 18.0;
const BAND_TILE_GAP: f32 = 11.0;

/// The rules either side of 4a's arrow: `width: 34px; height: 1px`, six points
/// clear of the mark between them.
const BAND_ARROW_RULE: f32 = 34.0;
const BAND_ARROW_GAP: f32 = 6.0;

/// 4a's caption over each subject: `font-size: 11px; font-weight: 700;
/// letter-spacing: 0.08em`.
const BAND_CAPTION_PX: f32 = 11.0;
const BAND_CAPTION_TRACKING: f32 = 0.08;

/// The name under it, `font-size: 14px; font-weight: 700`, and `gap: 8px`
/// between the name and what sits beside it; the folder beside the item's
/// name at the small size.
const BAND_NAME_PX: f32 = 14.0;
const BAND_NAME_GAP: f32 = 8.0;
const BAND_FOLDER_PX: f32 = 11.0;

/// `gap: 2px` between a subject's caption and its name.
const BAND_COLUMN_GAP: f32 = 2.0;

/// The chip beside a name: `font-size: 11px; border-radius: 5px; padding: 2px
/// 7px`.
const BAND_CHIP_PX: f32 = 11.0;
const BAND_CHIP_PAD_X: f32 = 7.0;
const BAND_CHIP_PAD_Y: f32 = 2.0;
const BAND_CHIP_RADIUS: u8 = 5;

/// When the chips drop UNDER the app's name (see [`destination_band`]), the
/// space between the name and them. Not a declaration of 4a's, which never
/// drops them: the column's `2px` is between two text runs whose line boxes
/// already carry slack, and a filled chip has none, so it gets two more.
const BAND_CHIPS_UNDER_GAP: f32 = 4.0;

/// Between the band's two lines when it takes two. Not 4a's either, for the
/// same reason: the band's own `16px` would read as a third subject's worth
/// of air, and the `8px` inside a name line as none.
const BAND_LINE_GAP: f32 = 12.0;

/// 4a's SEQUENCE band: `padding: 12px 20px; gap: 10px`, its runs at
/// `font-size: 12px`, the caption tracked `0.08em`.
const SEQUENCE_BAND_PAD_X: i8 = 20;
const SEQUENCE_BAND_PAD_Y: i8 = 12;
const SEQUENCE_BAND_GAP: f32 = 10.0;
const SEQUENCE_BAND_PX: f32 = 12.0;
const SEQUENCE_BAND_TRACKING: f32 = 0.08;

/// 4a's step row: `gap: 12px; border-radius: 10px; padding: 11px 14px`, and
/// the column's `gap: 8px` between rows.
const STEP_ROW_GAP_X: f32 = 12.0;
const STEP_ROW_RADIUS: u8 = 10;
const STEP_ROW_PAD_X: i8 = 14;
const STEP_ROW_PAD_Y: i8 = 11;
const STEP_ROW_GAP: f32 = 8.0;

/// The handle: a `width: 20px` column of three `12px` by `2px` bars, `gap:
/// 3px`.
const STEP_GRIP_WIDTH: f32 = 20.0;
const STEP_GRIP_BAR: egui::Vec2 = egui::vec2(12.0, 2.0);
const STEP_GRIP_GAP: f32 = 3.0;

/// The index: mono `font-size: 12px` in a `width: 14px` column.
const STEP_INDEX_WIDTH: f32 = 14.0;
const STEP_INDEX_PX: f32 = 12.0;

/// The kind chip: `width: 52px; padding: 3px 0; border-radius: 5px;
/// font-size: 11px; letter-spacing: 0.06em`.
const STEP_KIND_WIDTH: f32 = 52.0;
const STEP_KIND_PAD_Y: f32 = 3.0;
const STEP_KIND_RADIUS: u8 = 5;
const STEP_KIND_PX: f32 = 11.0;
const STEP_KIND_TRACKING: f32 = 0.06;

/// A keycap: mono `font-size: 12px; border: 1px solid; border-bottom-width:
/// 2px; border-radius: 5px; padding: 2px 7px`.
const KEYCAP_PX: f32 = 12.0;
const KEYCAP_PAD_X: f32 = 7.0;
const KEYCAP_PAD_Y: f32 = 2.0;
const KEYCAP_RADIUS: u8 = 5;
const KEYCAP_EDGE: f32 = 1.0;
const KEYCAP_FOOT: f32 = 2.0;

/// A field pill: `gap: 6px; border-radius: 6px; padding: 3px 8px`, a
/// 12-point mark and the name at `font-size: 12px`.
const FIELD_PILL_GAP: f32 = 6.0;
const FIELD_PILL_RADIUS: u8 = 6;
const FIELD_PILL_PAD_X: f32 = 8.0;
const FIELD_PILL_PAD_Y: f32 = 3.0;
const FIELD_PILL_MARK: f32 = 12.0;
const FIELD_PILL_PX: f32 = 12.0;

/// The middle cell's gap by kind -- `7px` on a key row, `8px` on a text row,
/// `10px` on a wait row -- and the space between the middle's lines when it
/// wraps (not 4a's; it never wraps).
const STEP_KEY_GAP: f32 = 7.0;
const STEP_TEXT_GAP: f32 = 8.0;
const STEP_WAIT_GAP: f32 = 10.0;
const STEP_WRAP_GAP: f32 = 3.0;

/// The runs: `font-size: 12px` on the explanation, the value and the far
/// cell's dash, mono `13px` on a wait, mono `11px` on a rate.
const STEP_TEXT_PX: f32 = 12.0;
const STEP_WAIT_PX: f32 = 13.0;
const STEP_RATE_PX: f32 = 11.0;

pub const SCREEN_TITLE: &str = "Fill rule";
pub const SAVE_LABEL: &str = "Save rule";
pub const SAVE_BLOCKED_LABEL: &str = "Save (fix the template)";
pub const DISCARD_LABEL: &str = "Discard";
pub const VAULT_ITEM_HEADING: &str = "Vault item";
pub const SENDS_TO_HEADING: &str = "Sends only to";
pub const SEQUENCE_HEADING: &str = "Sequence";
pub const TIMING_HEADING: &str = "Timing";
pub const CHECKS_HEADING: &str = "Checks";
pub const BUDGET_HEADING: &str = "Budget";

/// The note under the rehearse button -- 4d's own sentence, which the edit
/// form had no room for and this screen does.
pub const REHEARSE_NOTE: &str =
    "Types sample-user and not-a-real-password into a scratch window so you can watch the \
     timing. No secret leaves the vault.";

/// The eye's two captions, the edit form's words.
const REVEAL: &str = "Show what it types";
const HIDE: &str = "Hide what it types";

/// **4a's builder, in this app's standard modal.**
///
/// The screen took the window once -- the item list undrawn, the detail pane
/// zeroed -- because 4a is drawn at 1240 points and this was three columns.
/// The rail went, then the third column, and what is left is one column of
/// steps, which belongs in the shape every other card that asks a question
/// already has. The owner: "make standard modal layout with X and button at
/// the bottom, make it taller and more narrow".
///
/// So it is `theme::modal_card`: a coloured header band carrying the title and
/// the dismiss mark, a body, and a footer whose two answers sit at the bottom.
/// The two controls were in the header and are in the footer, which is where
/// this app puts an answer; `Discard` is the outlined left-hand one because
/// the way out is always the quieter of the two.
///
/// `item_icon` is the caller's `icons.textures.get(item.id)`, the texture the
/// read pane's header draws; `apps` is the window's identity cache, asked for
/// the bound program's icon. Both are for the tiles in [`destination_band`].
pub fn draw_sequence_builder(
    ui: &mut egui::Ui,
    draft: &mut SequenceDraft,
    palette: &[FieldRef],
    source: &ResolveSource<'_>,
    item_icon: Option<&egui::TextureHandle>,
    apps: &mut AppIdentityCache,
) -> BuilderAction {
    let ctx = ui.ctx().clone();

    // **The bound app's icon, asked of the cache once per frame and copied
    // out**, so the borrow of `apps` is over before the card's two closures
    // take the draft. `known_label` and not `label`: this path is a stored
    // binding's, proved when the binding was made, not one being typed --
    // which is the case the debounce behind `label` exists for. The name is
    // not read from it: `draft.app_name` was resolved by the caller off the
    // same cache when the screen opened.
    let app_icon = {
        let label = apps.known_label(&ctx, &draft.app.path, &draft.app.process);
        if label.pending {
            // A channel is not input, and egui does not repaint for one.
            ctx.request_repaint_after(AppIdentityCache::POLL_INTERVAL);
        }
        label.icon.cloned()
    };
    // **The scrim's id is a LITERAL**, and `item_list::MODAL_SCRIM_AREAS` is
    // kept honest by a walk over this crate's sources looking for exactly this
    // declaration. That list answers "is a modal up" for the item list's arrow
    // keys and the read pane's toast, so a scrim it cannot see is a modal
    // those two do not know about.
    theme::modal_scrim(&ctx, egui::Area::new(egui::Id::new("sequence-builder-scrim")));

    // **Read before the body draws**, so a press cannot be swallowed by a text
    // box that happens to have focus: the box takes a bare `s`, never the
    // chord. CTRL+S is 4a's own caption on the Save button.
    //
    // The one other `egui::Key::S` in this app's production is the read pane's
    // Send-a-record chord, and the two do not compete: they differ by SHIFT
    // and `consume_shortcut` compares the whole modifier set.
    let save_chord = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::S);
    let chorded = draft.saveable() && ctx.input_mut(|i| i.consume_shortcut(&save_chord));

    // **Escape is the way out**, like every other card in this app -- the
    // window picker's own Esc was wired for the same report. The owner: "Esc
    // doesn't close modal".
    //
    // **Unless one of the add menus is open, in which case the press is
    // that menu's.** egui shuts an open `Popup` on Escape itself; taking the
    // key here first meant a user who opened `+ Text`, thought better of it
    // and pressed Escape lost the whole card. The owner: "Esc should close A
    // value from this item popup first and second Esc exits Fill rule (or
    // calls discard modal)". Asked BEFORE the body draws, because by the end
    // of the frame the menu has answered the press and shut itself, and a
    // question asked then would say "nothing was open" about a frame in
    // which something was.
    //
    // `consume_key` otherwise, so the press is taken off the queue and
    // nothing behind the scrim reads it as its own Escape, and read here for
    // the reason the save chord is: a text box with focus must not be able
    // to swallow it.
    //
    // **Nor while the discard question is up**: that card has an Escape of
    // its own, and it means "keep editing" -- see
    // `detail_edit::draw_discard_confirm`.
    let menu_open = egui::Popup::is_any_open(&ctx);
    let escaped = !menu_open
        && !draft.discard_prompt
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));

    // Read before the card, because `modal_card` takes the body and the
    // confirm as two closures that must not both borrow the draft.
    let blocked = draft.fault().is_some();
    let saveable = draft.saveable();
    let label = if blocked { SAVE_BLOCKED_LABEL } else { SAVE_LABEL };
    let height = body_height(&ctx);

    let press = theme::modal_card(
        &ctx,
        egui::Area::new(egui::Id::new("sequence-builder")),
        theme::ModalCard {
            accent: theme::BLUE,
            // No mark: this is a form, not a question about a consequence.
            glyph: theme::ModalGlyph::None,
            title: SCREEN_TITLE,
            width: card_width(&ctx),
            dismiss: DISCARD_LABEL,
            // **The bands are the body.** Each carries 4a's own
            // `padding: 16px 20px`, so the card's margin would inset them
            // twice over -- see [`theme::ModalBody`].
            body: theme::ModalBody::Flush,
        },
        |ui| {
            // **The card is as tall as its content, and no taller than the
            // window.** `auto_shrink` on the y means "be as tall as your
            // content" -- which is what the owner asked for ("make height
            // dynamic, so it adjusts based on the content of the modal") --
            // and `max_height` is what keeps a twenty-step sequence from
            // making a card taller than the window it floats over: past
            // that the body scrolls. Both are needed. On the x it stays
            // off, because the card's width is the card's, not its widest
            // row's.
            egui::ScrollArea::vertical()
                .max_height(height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    // **The ground under the rows is the window's grey.** 4a
                    // draws its card on `#f7f6f5` and lays white step rows
                    // on it; the two bands above paint their own fills over
                    // that ground (white, then the tint). Painted as the
                    // BODY's ground rather than as the step block's fill, so
                    // the grey reaches the footer on a sequence of two steps
                    // as well as one of twenty -- the owner: "background
                    // under sequence should be gray as per design".
                    egui::Frame::new().fill(theme::WINDOW_BG).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        // **The bands butt, and their rules are the joins.**
                        // 4a stacks them with no gap at all: the destination
                        // band's `border-bottom` IS the top of the SEQUENCE
                        // band, and the SEQUENCE band's is the top of the
                        // rows. egui puts `item_spacing` between two stacked
                        // children, so four of those joins came out as eight
                        // points of white -- the owner, of the two above and
                        // below the tint: "white spaces on top and below
                        // Sequnce". Zeroed for the STACK and handed back
                        // inside each band's own frame, which is
                        // `theme::modal_card`'s trick one level up.
                        ui.spacing_mut().item_spacing.y = 0.0;
                        destination_band(ui, draft, item_icon, app_icon.as_ref());
                        steps_column(ui, draft, palette, source);
                    });
                });
        },
        |ui| theme::primary_button_enabled(ui, label, Some("CTRL+S"), saveable),
    );

    // **The question, over the card it is about**, and drawn last so its
    // scrim covers the rule it is asking about. Only the two ways out can
    // raise it, and only on a rule that has been edited.
    if draft.discard_prompt {
        match detail_edit::draw_discard_confirm(&ctx) {
            Some(detail_edit::DiscardAnswer::Discard) => {
                draft.discard_prompt = false;
                return BuilderAction::Discard;
            }
            Some(detail_edit::DiscardAnswer::KeepEditing) => draft.discard_prompt = false,
            None => {}
        }
        return BuilderAction::None;
    }

    if chorded || press.confirmed {
        BuilderAction::Save
    } else if escaped || press.dismissed {
        // **An edited rule asks first.** Untouched, the card just closes:
        // there is nothing to lose and a question about nothing is a
        // question a reader learns to click through. `changed` is the same
        // test the Save button is lit by, so the card asks exactly when
        // Save would have had something to write.
        if draft.changed() {
            draft.discard_prompt = true;
            BuilderAction::None
        } else {
            BuilderAction::Discard
        }
    } else {
        BuilderAction::None
    }
}

/// How wide the card is against this window.
///
/// **Narrower than 4a**, at the owner's word. The design is drawn at 1240
/// points across two columns; this is one column in a modal, and a modal has
/// to leave the window it floats over visible at the edges. The `min` is
/// what keeps it inside the 900-point window this app can open at.
///
/// **600 and not the 640 it was**: the body went flush (see
/// [`theme::ModalBody`]) and handed the bands back the 40 points the card's
/// margin had been spending on top of their own, so the column inside is
/// the width it always was and the card round it is 40 narrower. "I think
/// modal should be thinner so no white spaces on left and right".
const CARD_WIDTH: f32 = 600.0;
const CARD_GUTTER: f32 = 48.0;

fn card_width(ctx: &egui::Context) -> f32 {
    CARD_WIDTH.min((ctx.content_rect().width() - 2.0 * CARD_GUTTER).max(320.0))
}

/// The tallest the body may be before it scrolls.
///
/// **Taller than it was**, also at the owner's word: the card is narrower, so
/// the steps need the room back vertically. What is subtracted is the chrome
/// the body sits between -- `theme::modal_card`'s header band and its footer.
/// Not the body's own margin any more: this card's body is flush (see
/// [`theme::ModalBody`]), so those points top and bottom are the body's own
/// to spend.
const CARD_CHROME: f32 = 96.0;

fn body_height(ctx: &egui::Context) -> f32 {
    (ctx.content_rect().height() - 2.0 * CARD_GUTTER - CARD_CHROME).max(240.0)
}

/// **4a's band under the header: what the rule belongs to, and what it may
/// type into.**
///
/// A BAND and not a rail. It was drawn as a column of two cards beside the
/// steps, which made a three-column picture out of a two-column design and cost
/// the steps 252 points of the width they are the point of. 4a runs it across
/// the full width -- `padding: 16px 20px`, the two tiles 34 square with an
/// arrow between them -- and gives the grid under it `1fr 340px`.
///
/// The arrow between the two halves is 4a's own: a 34-point rule, the mark, and
/// another rule. It says the rule has a direction, which two cards stacked in a
/// rail could not.
///
/// **4a's amber notice at the band's far end is not drawn.** The owner pointed
/// at it and said "remove". It was also what grew the card: a `Frame` inside a
/// wrapping row is placed at the cursor before its size is known and never
/// wraps, so the pill was laid at x = 822 against a lane that ended at 728,
/// and the card followed it out to 1025 points -- measured in a 1240-point
/// window, with the header band correctly at 642 across the top of it and the
/// owner looking at the difference. The band is the two subjects and the
/// arrow, and nothing else.
///
/// # How it wraps, and that it is decided rather than left to egui
///
/// 4a is drawn at 1240 points and this band has 560 to work with: the card's
/// 640 less the modal's `20` a side and its own. With the design's content in
/// it -- `SAP Production` filed under `Work`, `SAP Logon 760` with two chips --
/// the vault item's half measures 183, the arrow 96 and the app's half 342,
/// which with the two `gap: 18px` is 657: too wide by a hundred, so the
/// COMMON case has to wrap. Left to a wrapping row it would not have: a
/// `horizontal` inside one is placed at whatever width is left of the line
/// and then overflows it, which is the 983-point band the probe found.
///
/// So the band measures its three groups first and takes the first of three
/// forms that fits the lane:
///
/// 1. **4a exactly**: both subjects and the arrow on one line, the chips
///    beside the app's name.
/// 2. **The chips give first.** They drop under the app's name, inside the
///    subject's own column, and the line holds. They are the widest thing in
///    the band and the least important -- they qualify the name beside them
///    -- so they are the first thing asked to move, and they move within
///    their subject rather than off the line.
/// 3. **Two lines**: the vault item on the first, the arrow and the app on
///    the second. The arrow travels with the app because it points at it; a
///    line ending in a bare arrow would read as a continuation mark. The
///    chips take whichever of their two places fits that line.
///
/// [`band_form`] is the decision, pure, so it is tested on the numbers rather
/// than inferred from a paint. Measured after the change, in the 640-point
/// card with the content above: form 2, one line of 547 in the 560 lane.
fn destination_band(
    ui: &mut egui::Ui,
    draft: &SequenceDraft,
    item_icon: Option<&egui::TextureHandle>,
    app_icon: Option<&egui::TextureHandle>,
) {
    egui::Frame::new()
        .fill(theme::CARD)
        .inner_margin(Margin::symmetric(BAND_PAD_X, BAND_PAD_Y))
        .show(ui, |ui| {
            // The stack outside this band is butted; its own contents are
            // ordinary stacked widgets and want the app's spacing back.
            ui.spacing_mut().item_spacing = theme::ITEM_SPACING;
            ui.set_width(ui.available_width());
            // The band's own lane, read before a row is begun -- NOT
            // `available_width` inside a wrapping row, which answers the
            // wrap width rather than what is left of the line (`3168f5a`).
            let lane = ui.available_width();
            let chips = band_chips(draft);
            let item = BandSubject {
                caption: VAULT_ITEM_HEADING,
                name: &draft.item_name,
                icon: item_icon,
                accent: true,
                beside: BandBeside::Folder(&draft.folder),
            };
            let app = BandSubject {
                caption: SENDS_TO_HEADING,
                name: &draft.app_name,
                icon: app_icon,
                accent: false,
                beside: BandBeside::Chips(&chips),
            };
            let form = band_form(
                lane,
                item.size(ui, ChipsGo::Beside).x,
                band_arrow_width(),
                app.size(ui, ChipsGo::Beside).x,
                app.size(ui, ChipsGo::Under).x,
            );
            // Each line at its height before anything is placed on it -- see
            // [`centred_row`] for the measurement -- and hung from the top,
            // not centred: see [`hanging_row`] and [`BandSubject::draw`] for
            // where 4a's `align-items: center` is kept and where it is not.
            match form {
                BandForm::OneLine(chips_go) => {
                    let height = item.size(ui, ChipsGo::Beside).y.max(app.size(ui, chips_go).y);
                    hanging_row(ui, height, |ui| {
                        ui.spacing_mut().item_spacing.x = BAND_GAP;
                        item.draw(ui, ChipsGo::Beside);
                        band_arrow(ui);
                        app.draw(ui, chips_go);
                    });
                }
                BandForm::TwoLines(chips_go) => {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = BAND_LINE_GAP;
                        item.draw(ui, ChipsGo::Beside);
                        hanging_row(ui, app.size(ui, chips_go).y, |ui| {
                            ui.spacing_mut().item_spacing.x = BAND_GAP;
                            band_arrow(ui);
                            app.draw(ui, chips_go);
                        });
                    });
                }
            }
        });
    theme::hairline(ui);
}

/// Where the app subject's chips go. See [`destination_band`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChipsGo {
    /// 4a's own: on the name's line, after it.
    Beside,
    /// Under the name, in the subject's column.
    Under,
}

/// Which of [`destination_band`]'s three forms the lane allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BandForm {
    OneLine(ChipsGo),
    TwoLines(ChipsGo),
}

/// The decision behind [`destination_band`], on widths alone: the lane, the
/// vault item's half, the arrow, and the app's half with its chips beside the
/// name and under it. First of the three forms that fits, in the order the
/// band's doc argues.
fn band_form(lane: f32, item: f32, arrow: f32, app_beside: f32, app_under: f32) -> BandForm {
    let one_line = |app: f32| item + BAND_GAP + arrow + BAND_GAP + app <= lane;
    if one_line(app_beside) {
        BandForm::OneLine(ChipsGo::Beside)
    } else if one_line(app_under) {
        BandForm::OneLine(ChipsGo::Under)
    } else if arrow + BAND_GAP + app_beside <= lane {
        BandForm::TwoLines(ChipsGo::Beside)
    } else {
        BandForm::TwoLines(ChipsGo::Under)
    }
}

/// The chips after the app's name: its process, and its window title when
/// the title is really part of the rule.
///
/// A title needle is read for a hosted window and ignored otherwise, so
/// printing one here on an ordinary process would name a condition that does
/// not apply.
fn band_chips(draft: &SequenceDraft) -> Vec<String> {
    let mut chips = vec![draft.app.process.clone()];
    if draft.app.hosted && !draft.app.title.is_empty() {
        chips.push(draft.app.title.clone());
    }
    chips
}

/// What sits beside a subject's name on its line.
enum BandBeside<'a> {
    /// The vault item's folder, in the faint ink. Nothing when unfiled.
    Folder(&'a str),
    /// The app's [`band_chips`].
    Chips(&'a [String]),
}

/// One half of [`destination_band`]: a tile, a caption in 4a's small capitals,
/// and a name with whatever the caller puts beside it.
///
/// `accent` picks 4a's two tile treatments apart -- the vault item's is the
/// brand wash (`border: 1px solid #b8c7ea`, the monogram in `#1b3fa0`) and the
/// app's the plain grey one (`#eae7e7`, `#605d5d`) -- which is the only thing
/// that says which end of the arrow is which when both names are words.
///
/// **The tile shows the real artwork when there is any**, and the monogram
/// only as the fallback: the item's favicon, which the read pane's header
/// under the scrim was drawing a click ago, and the program's own icon, which
/// [`crate::app_identity::AppIdentityCache`] resolved for the edit form's app
/// row. Through [`theme::avatar_artwork_tile`] and [`theme::avatar_image`],
/// as that row draws it: the artwork and the monogram are the same tile, at
/// the same size, with the same edge, and only the contents differ. 4a fills
/// both tiles (`#eef2fc`, `#f3f2f2`) and this app's avatar tile does not, at
/// the owner's word -- see [`theme::avatar`] -- so what this band draws is
/// the app's tile, not the picture's.
///
/// The tile is 34 where the crate's other avatars are 32. `favicon::decode_rgba`
/// keeps a 64-pixel copy, sized for a 32-point draw at 200%; here the artwork
/// box is `theme::ARTWORK_BOX` of 34, which at 200% is 41 pixels and well
/// inside that.
struct BandSubject<'a> {
    caption: &'static str,
    name: &'a str,
    icon: Option<&'a egui::TextureHandle>,
    accent: bool,
    beside: BandBeside<'a>,
}

impl BandSubject<'_> {
    /// 4a's caption: `font-size: 11px; font-weight: 700; letter-spacing:
    /// 0.08em; text-transform: uppercase; color: #9b9797`. Uppercased here
    /// rather than stored so, for the reason `theme::section_card_header`
    /// gives: the capitals belong to this band, and the same strings are
    /// printed in sentence case elsewhere.
    fn caption_job(&self) -> egui::text::LayoutJob {
        theme::letterspaced(
            &self.caption.to_uppercase(),
            BAND_CAPTION_PX,
            theme::BOLD,
            BAND_CAPTION_PX * BAND_CAPTION_TRACKING,
            theme::TEXT_GHOST,
        )
    }

    fn name_font() -> egui::FontId {
        egui::FontId::new(BAND_NAME_PX, egui::FontFamily::Name(theme::BOLD.into()))
    }

    /// The galleys this subject is drawn from, with its chips where `go`
    /// says, so the band can choose a form and size its rows before it draws
    /// anything. **The same galleys the drawing spends**: a size measured
    /// here and a size laid out below are the same number because they are
    /// the same layout.
    fn metrics(&self, ui: &egui::Ui, go: ChipsGo) -> SubjectMetrics {
        let caption = ui.ctx().fonts_mut(|f| f.layout_job(self.caption_job())).size();
        let name = run_size(ui, self.name, Self::name_font());
        let (beside, under) = match (&self.beside, go) {
            (BandBeside::Folder(folder), _) if folder.is_empty() => (egui::Vec2::ZERO, egui::Vec2::ZERO),
            (BandBeside::Folder(folder), _) => {
                let folder = run_size(ui, folder, egui::FontId::proportional(BAND_FOLDER_PX));
                (egui::vec2(BAND_NAME_GAP + folder.x, folder.y), egui::Vec2::ZERO)
            }
            (BandBeside::Chips(chips), ChipsGo::Beside) => {
                let mut beside = egui::Vec2::ZERO;
                for chip in chips.iter().map(|chip| band_chip_size(ui, chip)) {
                    beside.x += BAND_NAME_GAP + chip.x;
                    beside.y = beside.y.max(chip.y);
                }
                (beside, egui::Vec2::ZERO)
            }
            (BandBeside::Chips(chips), ChipsGo::Under) => {
                let mut under = egui::Vec2::ZERO;
                for (i, chip) in chips.iter().map(|chip| band_chip_size(ui, chip)).enumerate() {
                    under.x += chip.x + if i > 0 { BAND_NAME_GAP } else { 0.0 };
                    under.y = under.y.max(chip.y);
                }
                (egui::Vec2::ZERO, under)
            }
        };
        SubjectMetrics { caption, name, beside, under }
    }

    /// What the whole subject lays out as: the tile row -- the tile, its gap
    /// and the column's head, as tall as the taller of the two -- and, when
    /// the chips have dropped under the name, the chips row hanging under it.
    fn size(&self, ui: &egui::Ui, go: ChipsGo) -> egui::Vec2 {
        let metrics = self.metrics(ui, go);
        let head = metrics.head();
        let row = egui::vec2(
            BAND_TILE + BAND_TILE_GAP + head.x.max(metrics.under.x),
            BAND_TILE.max(head.y),
        );
        let under = if metrics.under.y > 0.0 {
            under_drop(row.y, head.y) + metrics.under.y
        } else {
            0.0
        };
        egui::vec2(row.x, row.y + under)
    }

    /// **The tile is centred against the column's HEAD -- the caption and the
    /// name's line -- and the chips hang under the two of them.**
    ///
    /// 4a's `align-items: center` centres the whole column on the tile, and
    /// in 4a the whole column IS the head: 12 + 2 + 17 against a 34-point
    /// tile, which puts the caption's ink three and a half points under the
    /// tile's top. The chips-under form has no counterpart in 4a, and
    /// centring its 50-point column on the tile put the caption's ink SIX
    /// POINTS ABOVE the tile's top -- measured, `SENDS ONLY TO` at 186..194
    /// over a tile starting at 192 -- which is the owner's "heading feels too
    /// high". So the head is what the tile centres against, exactly as in
    /// 4a, and the chips are a third line under it; the caption lands where
    /// 4a's does whether or not anything hangs below.
    fn draw(&self, ui: &mut egui::Ui, go: ChipsGo) {
        let metrics = self.metrics(ui, go);
        let head = metrics.head();
        let size = self.size(ui, go);
        let row_height = BAND_TILE.max(head.y);
        ui.allocate_ui_with_layout(size, egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            // The tile row at its own size, and the head inside it at ITS own
            // size: a `vertical` of unknown height dropped into a centred row
            // is placed by the height egui guesses for it and grows downward
            // from there (the tile a third of the way up its column that an
            // earlier probe found).
            ui.allocate_ui_with_layout(
                egui::vec2(size.x, row_height),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = BAND_TILE_GAP;
                    match self.icon {
                        Some(texture) => {
                            let tile = theme::avatar_artwork_tile(ui, BAND_TILE, self.accent);
                            theme::avatar_image(ui, tile, texture, self.accent);
                        }
                        None => {
                            theme::avatar(ui, &theme::initials(self.name), BAND_TILE, self.accent)
                        }
                    }
                    ui.allocate_ui_with_layout(
                        head,
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.spacing_mut().item_spacing.y = BAND_COLUMN_GAP;
                            ui.add(egui::Label::new(self.caption_job()).extend());
                            let (line, _) =
                                ui.allocate_exact_size(metrics.line(), egui::Sense::hover());
                            self.paint_name_line(ui, line, go);
                        },
                    );
                },
            );
            if let (BandBeside::Chips(chips), ChipsGo::Under) = (&self.beside, go) {
                ui.add_space(under_drop(row_height, head.y));
                let (row, _) = ui.allocate_exact_size(
                    egui::vec2(size.x, metrics.under.y),
                    egui::Sense::hover(),
                );
                let mut x = row.left() + BAND_TILE + BAND_TILE_GAP;
                for chip in *chips {
                    x += paint_band_chip(ui, x, row.center().y, chip) + BAND_NAME_GAP;
                }
            }
        });
    }

    /// **The name's line, painted, with everything on it hung from ONE
    /// line: the cap-middle of the name.**
    ///
    /// The name, the folder and the chips were egui labels centred by their
    /// boxes, and a box is ascent plus descent, which an 14-point bold face,
    /// an 11-point regular one and an 11-point mono one divide three ways.
    /// Measured: the name's cap-middle at 215 and the folder's at 216 -- the
    /// two shared a BASELINE, which is right in running text and reads as a
    /// slip beside a pill -- and the chip's letters at 223.5 inside a pill
    /// centred on 226, two and a half points high, because the chip painted
    /// its galley at its box's top. The owner: "folder and *.exe not
    /// centered".
    ///
    /// Cap-middle rather than baseline, because it is the line this crate
    /// already aligns faces on ([`theme::face_ink_middle`]: two faces
    /// aligned on it read as being on one line whatever each is spelling)
    /// and because a pill has no baseline to share -- its own middle is the
    /// only line it can offer, and the name's cap-middle is where that
    /// middle is put. The line's box is the tallest of the three; all three
    /// inks centre on it.
    fn paint_name_line(&self, ui: &egui::Ui, line: egui::Rect, go: ChipsGo) {
        let middle = line.center().y;
        let painter = ui.painter();
        let name_font = Self::name_font();
        let name = painter.layout_no_wrap(self.name.to_string(), name_font.clone(), theme::INK);
        let mut x = line.left();
        painter.galley(
            egui::pos2(x, middle - theme::face_ink_middle(ui, &name_font)),
            name.clone(),
            theme::INK,
        );
        x += name.size().x + BAND_NAME_GAP;
        match (&self.beside, go) {
            (BandBeside::Folder(folder), _) if !folder.is_empty() => {
                let font = egui::FontId::proportional(BAND_FOLDER_PX);
                let galley = painter.layout_no_wrap(folder.to_string(), font.clone(), theme::TEXT_FAINT);
                painter.galley(
                    egui::pos2(x, middle - theme::face_ink_middle(ui, &font)),
                    galley,
                    theme::TEXT_FAINT,
                );
            }
            (BandBeside::Chips(chips), ChipsGo::Beside) => {
                for chip in *chips {
                    x += paint_band_chip(ui, x, middle, chip) + BAND_NAME_GAP;
                }
            }
            _ => {}
        }
    }
}

/// The space between the tile row's bottom and the chips hanging under it,
/// so that the chips sit [`BAND_CHIPS_UNDER_GAP`] under the name's line
/// wherever that line ended up inside the row: the head is centred in the
/// row, so its bottom is half the row's slack above the row's bottom.
fn under_drop(row_height: f32, head_height: f32) -> f32 {
    (BAND_CHIPS_UNDER_GAP - (row_height - head_height) / 2.0).max(0.0)
}

/// [`BandSubject::metrics`]: the caption's galley, the name's, whatever sits
/// beside the name (its own gap included) and whatever sits under it.
struct SubjectMetrics {
    caption: egui::Vec2,
    name: egui::Vec2,
    beside: egui::Vec2,
    under: egui::Vec2,
}

impl SubjectMetrics {
    /// The name's line: the name and what is beside it, as tall as the
    /// taller.
    fn line(&self) -> egui::Vec2 {
        egui::vec2(self.name.x + self.beside.x, self.name.y.max(self.beside.y))
    }

    /// The column's head: caption over line, at 4a's `gap: 2px`. What the
    /// tile is centred against -- see [`BandSubject::draw`].
    fn head(&self) -> egui::Vec2 {
        let line = self.line();
        egui::vec2(self.caption.x.max(line.x), self.caption.y + BAND_COLUMN_GAP + line.y)
    }
}

/// A row of `height`, laid left to right with every child centred on its
/// middle -- `detail::header_row`'s idiom, for the reason it gives: egui
/// centres each child on the band as it stands when that child is placed, so
/// a row that finds its height from its tallest child leaves the ones placed
/// before it sitting high. Measured here before the rows were given their
/// heights: a step row's grip and kind chip centred on y = 378 with the
/// controls beside them on 381, and the SEQUENCE caption on 312 against its
/// pill's 317.
fn centred_row<R>(ui: &mut egui::Ui, height: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), height),
        egui::Layout::left_to_right(egui::Align::Center),
        add,
    )
    .inner
}

/// A row of `height` whose children hang from its TOP. For the destination
/// band, whose subjects are each a 34-point tile row with, sometimes, a chips
/// row under it: hung from the top, the two tiles, the arrow and the two
/// captions share one line whatever hangs under either subject, which
/// centring would break the moment one subject grew a third line.
fn hanging_row<R>(ui: &mut egui::Ui, height: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), height),
        egui::Layout::left_to_right(egui::Align::Min),
        add,
    )
    .inner
}

/// The size of `text` set in `font` on one line: what an `extend`ed label of
/// it is laid out as.
fn run_size(ui: &egui::Ui, text: &str, font: egui::FontId) -> egui::Vec2 {
    ui.painter().layout_no_wrap(text.to_string(), font, egui::Color32::BLACK).size()
}

/// 4a's mono chip beside a name in the band: `font-size: 11px; background:
/// #f3f2f2; border-radius: 5px; padding: 2px 7px`. Painted with its left edge
/// at `left` and its middle -- the pill's, and its letters' cap-middle -- on
/// `middle`; answers its width. See [`BandSubject::paint_name_line`] for why
/// the letters are put on the pill's middle rather than at its box's top.
fn paint_band_chip(ui: &egui::Ui, left: f32, middle: f32, text: &str) -> f32 {
    let font = egui::FontId::new(BAND_CHIP_PX, egui::FontFamily::Monospace);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font.clone(), theme::TEXT_SECONDARY);
    let size = galley.size() + egui::vec2(BAND_CHIP_PAD_X * 2.0, BAND_CHIP_PAD_Y * 2.0);
    let rect = egui::Rect::from_center_size(egui::pos2(left + size.x / 2.0, middle), size);
    ui.painter().rect_filled(rect, CornerRadius::same(BAND_CHIP_RADIUS), theme::CANVAS);
    ui.painter().galley(
        egui::pos2(rect.left() + BAND_CHIP_PAD_X, middle - theme::face_ink_middle(ui, &font)),
        galley,
        theme::TEXT_SECONDARY,
    );
    size.x
}

/// What [`paint_band_chip`] paints for `text`, for [`BandSubject::metrics`].
fn band_chip_size(ui: &egui::Ui, text: &str) -> egui::Vec2 {
    run_size(ui, text, egui::FontId::new(BAND_CHIP_PX, egui::FontFamily::Monospace))
        + egui::vec2(BAND_CHIP_PAD_X * 2.0, BAND_CHIP_PAD_Y * 2.0)
}

/// 4a's `--->` between the two subjects: a rule, the mark, a rule.
///
/// Drawn rather than set, for this app's standing reason: the codepoints that
/// would spell it are not in the bundled face, and a glyph that falls through
/// to whatever the system has is a different arrow on every machine.
fn band_arrow(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(band_arrow_width(), BAND_TILE),
        egui::Sense::hover(),
    );
    let middle = rect.center().y;
    let painter = ui.painter();
    for left in [rect.left(), rect.right() - BAND_ARROW_RULE] {
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(left, middle - 0.5),
                egui::vec2(BAND_ARROW_RULE, 1.0),
            ),
            CornerRadius::ZERO,
            theme::BORDER,
        );
    }
    // 4a's `M5 12h14` and `M13 6l6 6-6 6` in a 24-unit box, at `ARROW_MARK`.
    let mark = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, middle),
        egui::Vec2::splat(ARROW_MARK),
    );
    let unit = ARROW_MARK / 24.0;
    let stroke = egui::Stroke::new(2.0 * unit, theme::TEXT_GHOST);
    let at = |x: f32, y: f32| egui::pos2(mark.left() + x * unit, mark.top() + y * unit);
    painter.line_segment([at(5.0, 12.0), at(19.0, 12.0)], stroke);
    painter.add(egui::Shape::line(
        vec![at(13.0, 6.0), at(19.0, 12.0), at(13.0, 18.0)],
        stroke,
    ));
}

/// What [`band_arrow`] allocates across: the two rules, their gaps, the mark.
fn band_arrow_width() -> f32 {
    BAND_ARROW_RULE * 2.0 + BAND_ARROW_GAP * 2.0 + ARROW_MARK
}

/// The arrow mark's own box: 4a's `<svg width="16" height="16">`.
const ARROW_MARK: f32 = 16.0;

/// 4a's middle column: the sequence, in whichever of its two views is up, and
/// the palette that adds to it.
///
/// **The SEQUENCE band is a subheading and not a card.** It was drawn inside
/// `theme::section_card` -- a white tile with a hairline round it and ten
/// points of radius, whose top corners the band took -- and the owner:
/// "Sequence is not a tile but a subheading with separator on gray
/// background". 4a's band has `background: #fbfaf9` and `border-bottom: 1px
/// solid #eae7e7` and nothing else, no border of its own and no radius, and
/// the rows under it sit on the column's own ground at `padding: 16px 20px`.
/// So the card is gone: the band, its rule, and the rows in a plain frame at
/// the design's padding, stacked straight under the destination band's rule
/// the way 4a stacks them.
fn steps_column(
    ui: &mut egui::Ui,
    draft: &mut SequenceDraft,
    palette: &[FieldRef],
    source: &ResolveSource<'_>,
) {
    let summary = match sequence_tally(&draft.sequence, source) {
        Some(tally) => detail_edit::tally_short(&tally),
        None => detail_edit::TALLY_REFUSED.to_string(),
    };
    if let Some(wants_template) = sequence_band(ui, &summary, draft.template_view) {
        // Seeded verbatim, every time the view is entered -- so a user who
        // opens the template view and closes it again has changed nothing at
        // all.
        if wants_template {
            draft.template_draft = draft.sequence.clone();
        }
        draft.template_view = wants_template;
    }
    egui::Frame::new()
        .inner_margin(Margin::symmetric(BAND_PAD_X, BAND_PAD_Y))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = theme::ITEM_SPACING;
            ui.set_width(ui.available_width());
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);

            // **Above everything**, for the reason the edit form's block puts
            // it above the fold: a sequence the runner refuses outright is not
            // a detail of the builder, it is the fact that this binding does
            // nothing at all.
            if let Some(warning) = detail_edit::sequence_warning(&draft.sequence, source) {
                ui.label(RichText::new(warning).size(12.0).color(theme::ERROR));
                ui.add_space(6.0);
            }
            if draft.sequence.is_empty() {
                ui.label(
                    RichText::new(detail_edit::APP_SEQUENCE_DEFAULT_NOTICE)
                        .size(11.0)
                        .color(theme::TEXT_FAINT),
                );
                ui.add_space(6.0);
            }

            if draft.template_view {
                // Read before the three disjoint field borrows below, which
                // are fine together -- a whole-draft borrow beside them is not.
                let fault = draft.fault();
                detail_edit::template_editor(
                    ui,
                    &mut draft.template_draft,
                    &mut draft.sequence,
                    &mut draft.template_touched,
                    fault,
                );
            } else {
                let list = steps(&draft.sequence, source, draft.revealing);
                let edit = step_list(ui, &list, Some(&mut draft.selected))
                    .or_else(|| step_keys(ui, list.len(), &mut draft.selected));
                match edit {
                    Some(StepEdit::Move { from, to }) => {
                        let (sequence, landed) = sequence_with_step_moved(&draft.sequence, from, to);
                        draft.sequence = sequence;
                        // The selection follows the step, so a second
                        // Alt+Down moves the same one again -- to where it
                        // IS, which after a merge is not `to`.
                        draft.selected = Some(landed);
                    }
                    Some(StepEdit::Remove(index)) => {
                        draft.sequence = sequence_without_step(&draft.sequence, index);
                        draft.selected = None;
                    }
                    None => {}
                }
                // 4a's `padding-top: 4px` between the last step and the
                // row of add buttons. In the template view the insert chips
                // ARE the palette, and a second set of buttons writing to a
                // string the user is editing by hand would fight the cursor.
                ui.add_space(ADD_ROW_LIFT);
                add_step_row(ui, draft, palette);
                ui.add_space(STEP_ROW_GAP);
                ui.label(RichText::new(REORDER_HINT).size(11.0).color(theme::TEXT_FAINT));
            }
        });
    ui.add_space(theme::SECTION_GAP);
}

/// The line under the list that says how it is edited now that its rows carry
/// no controls: the drag, and the keyboard's equivalent of it. See
/// [`step_keys`] for why the keyboard half exists.
pub const REORDER_HINT: &str = "Drag a step by its handle to reorder it. Select a step to remove it \
                                with Delete, or move it with Alt+Up and Alt+Down.";

/// **4a's SEQUENCE band**: the caption, the tally, and the view toggle at the
/// far end. Answers the view the toggle asked for, or `None`.
///
/// Not `theme::section_card_header`, which is 8a's card title -- `11px 16px`,
/// `letter-spacing: 0.06em`, in [`theme::TEXT_MUTED`] with the note after it
/// -- and was what this band went through until the owner put the two side
/// by side: "heading is diif here". 4a declares its own: `padding: 12px 20px;
/// gap: 10px; border-bottom: 1px solid #eae7e7; background: #fbfaf9`, the
/// caption `font-size: 12px; font-weight: 700; letter-spacing: 0.08em;
/// text-transform: uppercase; color: #605d5d`, the tally `font-size: 12px;
/// color: #9b9797`, a `flex: 1` spacer, and the Steps/Template pill. No
/// radius and no edge of its own: it is a strip, not a tile (see
/// [`steps_column`]).
///
/// The pill is [`detail_edit::view_toggle_compact`]: the crate's one
/// segmented control at 4a's own box (`border: 1px solid #d7d3d3;
/// border-radius: 7px`, the lit cell `#1b3fa0` behind white at `4px 11px`)
/// rather than at the 28-point field height the rest of the app draws it at.
/// The taller pill set the band's height, and the band came out four points
/// over the design -- the owner, of the two side by side: "too high". It
/// moved up here from the body, where it sat over the first row; the edit
/// form's inline builder still draws its own copy below the tally, at the
/// form's height, because that form has no band to put it in.
///
/// The line always holds all three: the lane is 560 at the card's narrowest
/// (`card_width` never gives it less than 640) and the caption, the tally and
/// the pill measure 76 + 10 + 87 + 10 + 122 with the design's content, so
/// the `flex: 1` spacer is [`egui::Layout::right_to_left`] and no wrap is
/// built for a case the width rules out.
fn sequence_band(ui: &mut egui::Ui, tally: &str, template_view: bool) -> Option<bool> {
    let mut wants = None;
    egui::Frame::new()
        .fill(theme::CARD_TINT)
        .inner_margin(Margin::symmetric(SEQUENCE_BAND_PAD_X, SEQUENCE_BAND_PAD_Y))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = theme::ITEM_SPACING;
            ui.set_width(ui.available_width());
            // At the pill's height, which is the band's: 4a's `align-items:
            // center` puts the caption on the pill's middle, and egui does
            // that only for a row that knows its height first.
            let line = theme::segment_height_compact(ui);
            centred_row(ui, line, |ui| {
                ui.spacing_mut().item_spacing.x = SEQUENCE_BAND_GAP;
                ui.add(
                    egui::Label::new(theme::letterspaced(
                        &SEQUENCE_HEADING.to_uppercase(),
                        SEQUENCE_BAND_PX,
                        theme::BOLD,
                        SEQUENCE_BAND_PX * SEQUENCE_BAND_TRACKING,
                        theme::TEXT_MUTED,
                    ))
                    .extend(),
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(tally).size(SEQUENCE_BAND_PX).color(theme::TEXT_GHOST),
                    )
                    .extend(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    wants = detail_edit::view_toggle_compact(ui, template_view);
                });
            });
        });
    theme::hairline(ui);
    wants
}

// ---------------------------------------------------------------------------
// 4a's step rows
// ---------------------------------------------------------------------------

/// What a step row puts on egui's drag-and-drop clipboard while its handle is
/// held: which step is in the air. A named type for
/// `item_list::DraggedItem`'s reason -- the payload store is keyed by type,
/// and this is what lets the list tell its own drag from an item row's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DraggedStep {
    from: usize,
}

/// An edit the list asked for, applied by [`steps_column`] after the list has
/// drawn. `to` is the step's index once moved -- see
/// [`sequence_with_step_moved`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepEdit {
    Move { from: usize, to: usize },
    Remove(usize),
}

/// What a row's own surface reported this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowPress {
    None,
    /// The row was clicked, and is the selection now.
    Select,
    /// The context menu's entries.
    Remove,
    MoveUp,
    MoveDown,
}

/// The step list as 4a draws it: one row per [`Step`], reordered by dragging a
/// row's handle, and nothing else on the row.
///
/// **The builder's own, and not [`detail_edit::sequence_steps`].** That list
/// is drawn in the edit form's `Fill rule` card at a pane 298 points wide,
/// and 4a's row does not fit there: the handle, the index, the 52-point kind
/// chip, the three `gap: 12px` between them and the `padding: 11px 14px`
/// round them come to 150 before a step is drawn, and that card's rows have
/// about 200. That list also keeps its `<` `>` `x`, because it is drawn for
/// a CREATE, where the modal cannot open, and a row a pointer cannot drag on
/// a form a pointer may not be at needs its buttons. One function drawing
/// both would be two layouts behind an `if`, which is two idioms with one
/// name. The two share everything that is not geometry or gesture:
/// [`detail_edit::step_rows`] decides what a row says, and
/// [`detail_edit::store`] what the string becomes.
///
/// **The `<` `>` `x` came off these rows at the owner's word** -- "remove
/// controls from steps and make them draggable" -- and 4a's row has none: a
/// drag handle, an index, a chip, the step, a far cell. What a row can no
/// longer do with a button it does three other ways, all of them on the same
/// [`StepEdit`]: the handle is dragged (the list's own `DragAndDrop`, see
/// [`step_drop`]); the row is right-clicked, for a menu that moves or removes
/// it; and the row is clicked to select it, after which the keyboard moves or
/// removes it ([`step_keys`]). 4a offers no removal control at all -- its
/// only way to take a step out is the template view -- and that is not enough
/// on its own: a step the user cannot take away without editing the string
/// by hand is a builder that has stopped building.
///
/// `selected` is `None` in the template view, where the list is the read-out
/// of what the string became rather than the thing being edited: no handle
/// drags, no row selects, no menu opens. Returns the one edit asked for,
/// applied by the caller after the loop.
fn step_list(
    ui: &mut egui::Ui,
    list: &[Step],
    mut selected: Option<&mut Option<usize>>,
) -> Option<StepEdit> {
    let mut edit = None;
    let editable = selected.is_some();
    let mut rects: Vec<egui::Rect> = Vec::with_capacity(list.len());
    ui.scope(|ui| {
        // 4a's `gap: 8px` between rows, in place of the body's spacing.
        ui.spacing_mut().item_spacing.y = STEP_ROW_GAP;
        for (index, step) in list.iter().enumerate() {
            let is_selected = selected.as_deref().copied().flatten() == Some(index);
            let (rect, press) = step_row(ui, step, index, editable, is_selected);
            rects.push(rect);
            match press {
                RowPress::None => {}
                RowPress::Select => {
                    if let Some(selected) = selected.as_deref_mut() {
                        *selected = Some(index);
                    }
                }
                RowPress::Remove => edit = Some(StepEdit::Remove(index)),
                RowPress::MoveUp if index > 0 => {
                    edit = Some(StepEdit::Move { from: index, to: index - 1 });
                }
                RowPress::MoveDown if index + 1 < list.len() => {
                    edit = Some(StepEdit::Move { from: index, to: index + 1 });
                }
                RowPress::MoveUp | RowPress::MoveDown => {}
            }
        }
    });
    if editable {
        if let Some(dropped) = step_drop(ui, &rects) {
            edit = Some(dropped);
        }
    }
    edit
}

/// **The drop half of the list, list-wide rather than row by row.** A
/// pointer spends half of a drag over the 8-point gaps between rows, and a
/// drop that landed in one would be lost by a per-row `dnd_release_payload`.
/// So the slot is read off the rows' rects -- the number of rows whose middle
/// the pointer is below -- the insertion line is painted in that gap, and on
/// release the payload is taken and the move made. Released outside the
/// list, the payload is left for egui to clear at the end of the pass, and
/// nothing moves: a drop off the list is a drag the user abandoned, not a
/// removal.
fn step_drop(ui: &egui::Ui, rects: &[egui::Rect]) -> Option<StepEdit> {
    let dragged = egui::DragAndDrop::payload::<DraggedStep>(ui.ctx())?;
    let (first, last) = (rects.first()?, rects.last()?);
    let list = first.union(*last).expand2(egui::vec2(0.0, STEP_ROW_GAP));
    let pointer = ui.ctx().pointer_latest_pos()?;
    if !list.contains(pointer) {
        return None;
    }
    let slot = rects.iter().filter(|rect| rect.center().y < pointer.y).count();
    let to = if slot > dragged.from { slot - 1 } else { slot };
    if to != dragged.from {
        let y = if slot == 0 {
            first.top() - STEP_ROW_GAP / 2.0
        } else if slot == rects.len() {
            last.bottom() + STEP_ROW_GAP / 2.0
        } else {
            (rects[slot - 1].bottom() + rects[slot].top()) / 2.0
        };
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(list.left(), y - STEP_DROP_LINE / 2.0),
                egui::pos2(list.right(), y + STEP_DROP_LINE / 2.0),
            ),
            CornerRadius::same(1),
            theme::BLUE,
        );
    }
    if ui.ctx().input(|i| i.pointer.any_released()) {
        egui::DragAndDrop::take_payload::<DraggedStep>(ui.ctx());
        return (to != dragged.from).then_some(StepEdit::Move { from: dragged.from, to });
    }
    None
}

/// The insertion line's weight while a step is dragged over a gap.
const STEP_DROP_LINE: f32 = 2.0;

/// **The keyboard's half of reordering and removal**, for the parity a drag
/// alone cannot give: the `<` `>` `x` were reachable without a pointer and a
/// drag is not. With a step selected and no text box holding the focus -- the
/// palette's two boxes and the template each take arrow keys and Backspace of
/// their own -- Delete and Backspace take the step away, Alt+Up and Alt+Down
/// move it, and Up and Down move the selection. Read after the rows have
/// drawn, so the edit lands on the next frame the way a drop does.
fn step_keys(ui: &egui::Ui, count: usize, selected: &mut Option<usize>) -> Option<StepEdit> {
    let Some(index) = *selected else { return None };
    if index >= count {
        *selected = None;
        return None;
    }
    if ui.memory(|m| m.focused().is_some()) {
        return None;
    }
    let pressed = |modifiers: egui::Modifiers, key: egui::Key| {
        ui.ctx().input_mut(|i| i.consume_key(modifiers, key))
    };
    if pressed(egui::Modifiers::NONE, egui::Key::Delete)
        || pressed(egui::Modifiers::NONE, egui::Key::Backspace)
    {
        return Some(StepEdit::Remove(index));
    }
    if pressed(egui::Modifiers::ALT, egui::Key::ArrowUp) {
        return (index > 0).then(|| StepEdit::Move { from: index, to: index - 1 });
    }
    if pressed(egui::Modifiers::ALT, egui::Key::ArrowDown) {
        return (index + 1 < count).then(|| StepEdit::Move { from: index, to: index + 1 });
    }
    if pressed(egui::Modifiers::NONE, egui::Key::ArrowUp) {
        *selected = Some(index.saturating_sub(1));
    } else if pressed(egui::Modifiers::NONE, egui::Key::ArrowDown) {
        *selected = Some((index + 1).min(count - 1));
    }
    None
}

/// One of 4a's rows: `display: flex; align-items: center; gap: 12px;
/// background: #ffffff; border: 1px solid #eae7e7; border-radius: 10px;
/// padding: 11px 14px`. The handle, the index and the kind chip at fixed
/// widths, the step in the `flex: 1` middle, and the far cell at the end.
///
/// **The far end is laid first, right to left, and the middle takes what is
/// left.** That is 4a's `flex: 1` without measuring the far cell by hand:
/// it claims its own width, and the nested left-to-right scope inside is
/// handed the remainder as a lane. The middle wraps inside that lane (see
/// [`step_middle`]), so a row too long for the card grows downward and its
/// other cells stay on its first line -- the prefix reads with the step, and
/// the explanation hangs under it.
///
/// A secret step wears [`theme::secret_band`], 4a's fifth row: the danger
/// wash and edge, the index in [`theme::DANGER_QUIET`] (`#a2554d`), the kind
/// chip in [`theme::ERROR`] behind white, the pill the same, and its runs in
/// [`theme::SECRET_INK`]. The hatch is still not drawn -- see
/// `detail_edit::SECRET_STEP_RADIUS` for the argument -- and the sub-band
/// under it (`Sends only if the focused control is a masked field`) is not:
/// that gate is `preflight`'s and is not a per-step setting in this build.
///
/// **A selected row wears 8a's fresh-row treatment** -- `background:
/// #eef2fc; border: 1px solid #b8c7ea`, the wash and edge `detail_edit`'s
/// app row puts on the binding just made -- because that is what this
/// design system already uses for "this row is the one in hand". The secret
/// row keeps its own wash, which is the point of it, and takes the edge in
/// [`theme::BLUE`] so the selection still reads over red.
///
/// Answers the row's rect, for [`step_drop`], and what its surface reported.
fn step_row(
    ui: &mut egui::Ui,
    step: &Step,
    index: usize,
    editable: bool,
    selected: bool,
) -> (egui::Rect, RowPress) {
    let mut press = RowPress::None;
    let ground = match (step.secret, selected) {
        (true, false) => theme::secret_band(),
        (true, true) => theme::secret_band().stroke(Stroke::new(1.0, theme::BLUE)),
        (false, false) => egui::Frame::new().fill(theme::CARD).stroke(Stroke::new(1.0, theme::HAIRLINE)),
        (false, true) => {
            egui::Frame::new().fill(theme::BLUE_WASH).stroke(Stroke::new(1.0, theme::BLUE_EDGE))
        }
    };
    let framed = ground
        .corner_radius(CornerRadius::same(STEP_ROW_RADIUS))
        .inner_margin(Margin::symmetric(STEP_ROW_PAD_X, STEP_ROW_PAD_Y))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let height = step_row_height(ui);
            centred_row(ui, height, |ui| {
                ui.spacing_mut().item_spacing.x = STEP_ROW_GAP_X;
                step_grip(ui, step, index, editable);
                step_index(ui, step);
                step_kind_chip(ui, step);
                step_middle(ui, step, height);
            });
        });
    if editable {
        // Registered with the row's own id after its contents, as
        // `item_list`'s rows are: the handle inside it senses a drag and the
        // row senses a click, and egui tells the two gestures apart by
        // travel.
        let response = framed.response.interact(egui::Sense::click());
        if response.clicked() {
            press = RowPress::Select;
        }
        response.context_menu(|ui| {
            if ui.button(MENU_MOVE_UP).clicked() {
                press = RowPress::MoveUp;
                ui.close();
            }
            if ui.button(MENU_MOVE_DOWN).clicked() {
                press = RowPress::MoveDown;
                ui.close();
            }
            if ui.button(MENU_REMOVE).clicked() {
                press = RowPress::Remove;
                ui.close();
            }
        });
    }
    (framed.response.rect, press)
}

/// The row's context menu, for the pointer that would rather not drag: the
/// two moves the arrows used to be, and the removal the `x` was.
pub const MENU_MOVE_UP: &str = "Move up";
pub const MENU_MOVE_DOWN: &str = "Move down";
pub const MENU_REMOVE: &str = "Remove step";

/// The row's band: its tallest cell, known before the row is laid so every
/// cell is centred on the same line (see [`centred_row`]). The field pill,
/// which stands a point over the keycap; the middle may still grow past this
/// by wrapping, and then it grows downward from the line the other cells are
/// on.
fn step_row_height(ui: &egui::Ui) -> f32 {
    let row = |px: f32, family: &str| {
        ui.ctx()
            .fonts_mut(|f| f.row_height(&egui::FontId::new(px, egui::FontFamily::Name(family.into()))))
    };
    let pill = row(FIELD_PILL_PX, theme::BOLD) + FIELD_PILL_PAD_Y * 2.0 + 2.0;
    let keycap = row(KEYCAP_PX, theme::MONO_BOLD) + KEYCAP_PAD_Y * 2.0 + KEYCAP_EDGE + KEYCAP_FOOT;
    pill.max(keycap)
}

/// **A bare run on a step row, inked on the row's line.**
///
/// The row's boxed cells -- the kind chip, the field pill, the keycap --
/// each centre their own ink in their own box (see [`run_top`]), and a run
/// with no box round it has to land on the same line. `ui.add` of a `Label`
/// does not: egui centres the galley's BOX, which is ascent plus descender,
/// and a face inks the upper part of that -- about a point high, which is
/// exactly what the owner saw between `WAIT` and `1s`.
fn row_run(ui: &mut egui::Ui, text: &str, font: egui::FontId, ink: egui::Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), font.clone(), ink);
    let (rect, _) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
    ui.painter().galley(
        egui::pos2(rect.left(), run_top(ui, rect.center().y, &font)),
        galley,
        ink,
    );
}

/// **A wait's duration without the word.**
///
/// The row's label is `key_sequence::wait_label`'s -- `Wait 0.3s` -- which
/// is right everywhere it is read as a sentence: the resolved preview's
/// `[Wait 0.3s]`, the edit form's chips. On a step row it is said twice,
/// because the row's kind chip two cells to the left already reads `WAIT`.
/// The owner: "wait wait 0.3 s - remove second wait". 4a's own wait row is
/// `250 ms`, the duration alone, for exactly this reason.
///
/// Trimmed here rather than in `wait_label`, because the other two readers
/// have no chip beside them and need the word.
fn wait_duration(label: &str) -> &str {
    label.strip_prefix(WAIT_WORD).unwrap_or(label)
}

/// What [`wait_duration`] takes off the front.
const WAIT_WORD: &str = "Wait ";

/// **Where a hand-painted galley's top goes so its INK sits on `middle`.**
/// [`theme::face_ink_middle`] is where the face inks its cap-height band
/// inside its own line box, and taking it off the middle is what puts that
/// band there -- a galley placed by its box instead sits about a point high,
/// because a face inks the upper part of its box.
///
/// **`middle` is the BOX's, not the row's.** It was the row's, lifted by
/// [`theme::line_lift`] to the line egui's own labels ink on, back when
/// every row ended in two egui-laid runs -- the rate and the explanation --
/// that a pill had to agree with. Those went ("remove these"), and what is
/// left on a row is the pill, the chip and the keycap, each of which is a
/// box of its own: the lift then only moved a run off the middle of the box
/// it is inside, which is what the owner saw -- "pills text not centered".
/// Measured: every one of them inked 1.03 points over its own box's middle,
/// and now on it.
fn run_top(ui: &egui::Ui, middle: f32, font: &egui::FontId) -> f32 {
    middle - theme::face_ink_middle(ui, font)
}

/// 4a's drag handle: a `width: 20px` column of three bars, `width: 12px;
/// height: 2px; border-radius: 2px; background: #d7d3d3`, `gap: 3px` -- and
/// the drag source that reorders the list.
///
/// Drawn, not a glyph, for the crate's standing reason: `⋮` resolves to
/// nothing in Archivo and a mark out of a fallback face brings its own weight
/// and baseline. On the secret row 4a tints the bars `#e0b2ac`, which is not
/// in the palette; [`theme::DANGER_EDGE`] is the band's own edge, one shade
/// over, and the band's edge is what the bars are.
///
/// **The drag is `item_list`'s idiom, not `Ui::dnd_drag_source`**: the
/// handle's rect is re-registered with `Sense::drag`, the payload set on the
/// frame the drag starts, and a chip naming the step follows the pointer
/// from a `Tooltip`-order layer -- nothing re-parented, nothing allocated
/// twice. The hit area is the handle's 20-point column at the row's height,
/// which is wider than the bars and is what a finger or a hurried pointer
/// lands on.
fn step_grip(ui: &mut egui::Ui, step: &Step, index: usize, editable: bool) {
    let height = STEP_GRIP_BAR.y * 3.0 + STEP_GRIP_GAP * 2.0;
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(STEP_GRIP_WIDTH, height), egui::Sense::hover());
    let color = if step.secret { theme::DANGER_EDGE } else { theme::BORDER_STRONG };
    for bar in 0..3 {
        let top = rect.top() + bar as f32 * (STEP_GRIP_BAR.y + STEP_GRIP_GAP);
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.center().x - STEP_GRIP_BAR.x / 2.0, top),
                STEP_GRIP_BAR,
            ),
            CornerRadius::same(2),
            color,
        );
    }
    if editable {
        let hit = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(STEP_GRIP_WIDTH, ui.max_rect().height()),
        );
        let response = ui.interact(hit, ui.id().with(("step-grip", index)), egui::Sense::drag());
        response.dnd_set_drag_payload(DraggedStep { from: index });
        if response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            step_ghost(ui, &step.label());
        }
    }
}

/// The chip that follows the pointer while a step is dragged, naming it --
/// `item_list::drag_ghost`'s idiom, painted into a `Tooltip`-order layer so it
/// allocates nothing in the row, and offset from the pointer's hot spot so it
/// does not cover the gap the step is about to be dropped in.
fn step_ghost(ui: &egui::Ui, label: &str) {
    let Some(pointer) = ui.ctx().pointer_interact_pos() else { return };
    const PAD: egui::Vec2 = egui::Vec2::new(8.0, 5.0);
    const CURSOR_OFFSET: egui::Vec2 = egui::Vec2::new(14.0, 10.0);
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("sequence-step-ghost"),
    ));
    let galley = painter.layout_no_wrap(
        label.to_string(),
        egui::FontId::new(STEP_TEXT_PX, egui::FontFamily::Name(theme::SEMIBOLD.into())),
        theme::CARD,
    );
    let rect = egui::Rect::from_min_size(pointer + CURSOR_OFFSET, galley.size() + PAD * 2.0);
    painter.rect_filled(rect, CornerRadius::same(8), theme::BLUE);
    painter.galley(rect.min + PAD, galley, theme::CARD);
}

/// The index: mono `font-size: 12px; color: #9b9797` in a `width: 14px`
/// column, so the chips after it line up down the list whatever the count.
fn step_index(ui: &mut egui::Ui, step: &Step) {
    let ink = if step.secret { theme::DANGER_QUIET } else { theme::TEXT_GHOST };
    let font = egui::FontId::new(STEP_INDEX_PX, egui::FontFamily::Monospace);
    let galley = ui.painter().layout_no_wrap(step.number.to_string(), font.clone(), ink);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(STEP_INDEX_WIDTH, galley.size().y),
        egui::Sense::hover(),
    );
    ui.painter().galley(
        egui::pos2(rect.left(), run_top(ui, rect.center().y, &font)),
        galley,
        ink,
    );
}

/// The kind chip: `width: 52px; padding: 3px 0; border-radius: 5px;
/// font-size: 11px; font-weight: 700; letter-spacing: 0.06em; text-align:
/// center`. A FIXED 52 and centred, so KEY, TEXT and WAIT line up down the
/// column.
///
/// Its two colours vary by kind, 4a's three pairs and the secret's fourth:
/// grey `#f3f2f2`/`#444141` on a key, the field wash `#eef2fc`/`#14307a` on
/// text, the wait grey `#f3f2f2`/`#7d7979` on a wait, and `#b42318` behind
/// white on a secret. A rate change wears the wait's pair -- it is about
/// time, not a key -- and a step this build does not understand wears the
/// key's, which is the plain one.
fn step_kind_chip(ui: &mut egui::Ui, step: &Step) {
    let (fill, ink) = match step.kind {
        _ if step.secret => (theme::ERROR, egui::Color32::WHITE),
        StepKind::Text => (theme::BLUE_WASH, theme::BLUE_DEEP),
        StepKind::Wait | StepKind::Rate => (theme::CANVAS, theme::TEXT_FAINT),
        StepKind::Key | StepKind::Raw => (theme::CANVAS, theme::TEXT_SECONDARY),
    };
    let font = egui::FontId::new(STEP_KIND_PX, egui::FontFamily::Name(theme::BOLD.into()));
    let job = theme::letterspaced(
        step.kind.badge(),
        STEP_KIND_PX,
        theme::BOLD,
        STEP_KIND_PX * STEP_KIND_TRACKING,
        ink,
    );
    let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(STEP_KIND_WIDTH, galley.size().y + STEP_KIND_PAD_Y * 2.0),
        egui::Sense::hover(),
    );
    ui.painter().rect_filled(rect, CornerRadius::same(STEP_KIND_RADIUS), fill);
    ui.painter().galley(
        egui::pos2(
            rect.center().x - galley.size().x / 2.0,
            run_top(ui, rect.center().y, &font),
        ),
        galley,
        ink,
    );
}

/// The `flex: 1` middle: the step itself -- every token in it, in order --
/// what it resolves to, and the explanation beside it.
///
/// A key step is its keycaps with 4a's `+` between them (`font-size: 11px;
/// color: #9b9797`): `Ctrl` + `A`, `Shift` + `Tab`. A text step is a pill
/// for each field in it and the text itself for each literal, each followed
/// by what it resolves to when the eye is open.
///
/// Wrapped, at 4a's own gap for the kind (`7px` on a key row, `8px` on a
/// text row, `10px` on a wait row), so a text step longer than the lane
/// drops the rest of itself onto a second line rather than off the card.
/// The runs are `extend`ed and never break mid-word.
fn step_middle(ui: &mut egui::Ui, step: &Step, height: f32) {
    let gap = match step.kind {
        StepKind::Key => STEP_KEY_GAP,
        StepKind::Wait => STEP_WAIT_GAP,
        StepKind::Text | StepKind::Rate | StepKind::Raw => STEP_TEXT_GAP,
    };
    // The wrapping row at the ROW's height rather than `horizontal_wrapped`,
    // which starts its first line at the style's `interact_size` and centres
    // the pill on that: measured a point and a half under the cells beside
    // it. Given the row's band, its first line IS the row's line.
    let wrapped = egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true);
    ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), height), wrapped, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(gap, STEP_WRAP_GAP);
        let plain = |text: &str, ink: egui::Color32| {
            egui::Label::new(theme::semibold(text.to_string(), STEP_TEXT_PX).color(ink)).extend()
        };
        match step.kind {
            StepKind::Key => {
                for (i, row) in step.rows.iter().enumerate() {
                    if i > 0 {
                        ui.add(
                            egui::Label::new(
                                RichText::new("+").size(STEP_RATE_PX).color(theme::TEXT_GHOST),
                            )
                            .extend(),
                        );
                    }
                    keycap(ui, key_name(&row.label));
                }
            }
            StepKind::Text => {
                for row in &step.rows {
                    match &row.field {
                        Some(field) => field_pill(ui, field, &row.label, row.secret),
                        // A literal: the text itself, in the row's own ink.
                        // 4a draws no literal, and a pill round typed text
                        // would make it look like a field.
                        None => {
                            ui.add(plain(&row.label, theme::INK));
                        }
                    }
                    if !row.payload.is_empty() {
                        // 4a's value after the pill: `font-size: 12px; color:
                        // #9b9797` for a value, and the mask in mono
                        // `#8c3c33` on the secret row.
                        let text = RichText::new(row.payload.clone()).size(STEP_TEXT_PX);
                        let text = if row.secret {
                            text.family(egui::FontFamily::Monospace).color(theme::SECRET_INK)
                        } else {
                            text.color(theme::TEXT_GHOST)
                        };
                        ui.add(egui::Label::new(text).extend());
                    }
                }
            }
            // 4a's `250 ms`: mono `font-size: 13px; font-weight: 600`, the
            // duration and nothing else -- in the seconds the owner asked to
            // set waits in rather than the design's milliseconds.
            //
            // **Painted, not `ui.add`.** egui centres a label by its galley
            // BOX, and a box is ascent plus descender where a face inks only
            // the upper part of it -- so `1s` sat a point over the middle of
            // the `WAIT` chip two cells to its left, which is hand-painted
            // and inks on the middle exactly. The owner: "1 s is not
            // centered". [`row_run`] is the same rule the chip uses.
            StepKind::Wait => {
                row_run(
                    ui,
                    wait_duration(&step.rows[0].label),
                    egui::FontId::new(
                        STEP_WAIT_PX,
                        egui::FontFamily::Name(theme::MONO_BOLD.into()),
                    ),
                    theme::INK,
                );
            }
            StepKind::Rate => {
                ui.add(plain(&step.rows[0].label, theme::INK));
            }
            StepKind::Raw => {
                let label = ui.add(plain(&step.rows[0].label, theme::TEXT_FAINT));
                if !step.understood() {
                    label.on_hover_text(detail_edit::SEQUENCE_UNKNOWN_TIP);
                }
            }
        }
    });
}

// **The row's far cell is not drawn**, and neither is the sentence beside
// the step. 4a ends each row with the typing rate (`50 ms/char`) or a dash,
// and explains the step in a few words after it (`hidden — never shown
// here`, `submit`); the owner, of both at once: "remove these". They are the
// per-step half of the timing the same owner scoped out of this screen --
// "4a only keep builder without timing" -- and what is left is the step
// itself, which is the thing being edited. [`Step::note`] and [`Step::aside`]
// stay: they are what the tokens MEAN, they are tested, and the edit form's
// own chips still say some of it.

/// 4a's keycap: mono `font-size: 12px; font-weight: 600; border: 1px solid
/// #d7d3d3; border-bottom-width: 2px; border-radius: 5px; padding: 2px 7px;
/// background: #ffffff`.
///
/// **The heavier foot is painted as a second rectangle.** A `Stroke` has one
/// width for all four sides, so the cap is the edge colour filled to the
/// outline and the face filled over it, inset one point on three sides and
/// two on the fourth -- which leaves exactly CSS's border showing, and is
/// what makes it a keycap rather than a box. A different shape from 8a's
/// pills on purpose; what it shares with them is the line its letters sit
/// on, through [`run_top`].
fn keycap(ui: &mut egui::Ui, text: &str) {
    let _ = keycap_sensed(ui, text, egui::Sense::hover());
}

/// [`keycap`] **as a control**: the same cap, pressable.
///
/// The `+ Key` menu's palette -- the owner: "for keys buttons on + Key click
/// use same pills as in sequencer". What the press adds to the sequence is a
/// key step, and a key step is drawn as this cap, so the palette IS the
/// result at the size it will be.
fn keycap_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let response = keycap_sensed(ui, text, egui::Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn keycap_sensed(ui: &mut egui::Ui, text: &str, sense: egui::Sense) -> egui::Response {
    let font = egui::FontId::new(KEYCAP_PX, egui::FontFamily::Name(theme::MONO_BOLD.into()));
    let galley = ui.painter().layout_no_wrap(text.to_string(), font.clone(), theme::INK);
    let size = egui::vec2(
        galley.size().x + KEYCAP_PAD_X * 2.0 + KEYCAP_EDGE * 2.0,
        galley.size().y + KEYCAP_PAD_Y * 2.0 + KEYCAP_EDGE + KEYCAP_FOOT,
    );
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(KEYCAP_RADIUS), theme::BORDER_STRONG);
    let face = egui::Rect::from_min_max(
        rect.min + egui::vec2(KEYCAP_EDGE, KEYCAP_EDGE),
        rect.max - egui::vec2(KEYCAP_EDGE, KEYCAP_FOOT),
    );
    painter.rect_filled(face, CornerRadius::same(KEYCAP_RADIUS - 1), theme::CARD);
    // **The FACE's middle, not the cap's.** The foot is a point heavier
    // than the other three edges, so the two are half a point apart, and
    // CSS centres a keycap's letters in the content box -- inside the
    // border, not across it. Measured at exactly that half point when this
    // read `rect.center().y`.
    painter.galley(
        egui::pos2(face.left() + KEYCAP_PAD_X, run_top(ui, face.center().y, &font)),
        galley,
        theme::INK,
    );
    response
}

/// 4a's field pill: `gap: 6px; border-radius: 6px; background: #eef2fc;
/// border: 1px solid #b8c7ea; padding: 3px 8px`, a 12-point mark, and the
/// name at `font-size: 12px; font-weight: 600; color: #14307a`. The secret
/// row's is `#b42318` behind white at `font-weight: 700`, with no edge.
///
/// The mark is the crate's own for the field -- [`theme::FieldMark`], the
/// family the picker's gutter draws -- painted by [`theme::paint_field_mark`]
/// in the pill's ink. 4a draws a bust for the username and a padlock for the
/// password; this family's password mark is a key, and one family is worth
/// more than one matched glyph.
fn field_pill(ui: &mut egui::Ui, field: &FieldRef, name: &str, secret: bool) {
    let _ = field_pill_sensed(ui, field, name, secret, egui::Sense::hover());
}

/// [`field_pill`] **as a control**: the same pill, pressable.
///
/// The `+ Text` menu's palette. It was `detail_edit::palette_button` -- an
/// ordinary outlined button with the field's name on it -- and the owner,
/// of the menu beside the rows it writes into: "make sure css matches the
/// rest". A palette whose buttons are the very pill the step will be says
/// what the press does without a word of explanation.
fn field_pill_button(
    ui: &mut egui::Ui,
    field: &FieldRef,
    name: &str,
    secret: bool,
) -> egui::Response {
    let response = field_pill_sensed(ui, field, name, secret, egui::Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn field_pill_sensed(
    ui: &mut egui::Ui,
    field: &FieldRef,
    name: &str,
    secret: bool,
    sense: egui::Sense,
) -> egui::Response {
    let (fill, edge, ink, face) = if secret {
        (theme::ERROR, theme::ERROR, egui::Color32::WHITE, theme::BOLD)
    } else {
        (theme::BLUE_WASH, theme::BLUE_EDGE, theme::BLUE_DEEP, theme::SEMIBOLD)
    };
    let font = egui::FontId::new(FIELD_PILL_PX, egui::FontFamily::Name(face.into()));
    let galley = ui.painter().layout_no_wrap(name.to_string(), font.clone(), ink);
    let size = egui::vec2(
        FIELD_PILL_PAD_X * 2.0 + FIELD_PILL_MARK + FIELD_PILL_GAP + galley.size().x,
        galley.size().y + FIELD_PILL_PAD_Y * 2.0 + 2.0,
    );
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(FIELD_PILL_RADIUS),
        fill,
        egui::Stroke::new(1.0, edge),
        egui::StrokeKind::Inside,
    );
    let mark = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 1.0 + FIELD_PILL_PAD_X + FIELD_PILL_MARK / 2.0, rect.center().y),
        egui::Vec2::splat(FIELD_PILL_MARK),
    );
    theme::paint_field_mark(painter, mark, field_mark(field), ink);
    painter.galley(
        egui::pos2(mark.right() + FIELD_PILL_GAP, run_top(ui, rect.center().y, &font)),
        galley,
        ink,
    );
    response
}

/// The mark for a field, in the picker's own family.
fn field_mark(field: &FieldRef) -> theme::FieldMark {
    match field {
        FieldRef::Username => theme::FieldMark::Person,
        FieldRef::Password => theme::FieldMark::Key,
        FieldRef::Totp => theme::FieldMark::Clock,
        FieldRef::Custom(_) => theme::FieldMark::Tag,
    }
}

/// **4a's add-a-step row**: three dashed buttons, and the two quiet controls
/// that act on the whole sequence at the far end.
///
/// The design: `display: flex; gap: 8px; padding-top: 4px`, then `+ Text`,
/// `+ Key`, `+ Wait` at [`theme::dashed_button`]'s box, a `flex: 1` spacer,
/// and a sentence at the right. What this screen drew instead was a section
/// card titled "Add a step" holding four captioned palettes stacked down the
/// page -- the owner, with the design's row in hand: "No Add a step section -
/// just buttons like this below, that's it".
///
/// **Each button opens its own menu**, because the design's row is 4b's way
/// in and 4b is scoped out ("4b and 4d we can not do for now"): the step
/// editor those buttons lead to in the design does not exist here yet. So
/// each one carries the palette that used to sit under its caption, on
/// `egui::Popup::menu` -- the crate's one floating layer, the same one the
/// detail pane's kebab and this design system's dropdown use.
///
/// **The eye and the reset are at the spacer's end**, where 4a puts its
/// sentence: they are not ways to add a step, they act on the sequence
/// already built, and a row that mixed the two kinds at the same end would
/// read as five things to press.
fn add_step_row(ui: &mut egui::Ui, draft: &mut SequenceDraft, palette: &[FieldRef]) {
    centred_row(ui, theme::BUTTON_HEIGHT, |ui| {
        ui.spacing_mut().item_spacing.x = ADD_ROW_GAP;
        add_text_menu(ui, draft, palette);
        add_key_menu(ui, draft);
        add_wait_menu(ui, draft);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = ADD_ROW_GAP;
            if !draft.sequence.is_empty() && theme::secondary_button(ui, USE_DEFAULT).clicked() {
                // The empty string, not the default's own spelling: an empty
                // stored value IS the default, and rendering it would turn an
                // item that inherits into one that pins.
                draft.sequence = String::new();
            }
            let caption = if draft.revealing { HIDE } else { REVEAL };
            if theme::secondary_button(ui, caption).clicked() {
                draft.revealing = !draft.revealing;
            }
        });
    });
}

/// `+ Text`: the item's own fields, and a box for literal text.
///
/// The two are one menu because they are one step in the sequence -- a text
/// step is fields and literals in a row -- and because 4a's `+ Text` is the
/// one button that leads to both.
fn add_text_menu(ui: &mut egui::Ui, draft: &mut SequenceDraft, palette: &[FieldRef]) {
    let button = theme::dashed_button(ui, ADD_TEXT_LABEL);
    // **Not the default close-on-click**: this menu holds a text box, and a
    // menu that shut on the first click into its own field could never be
    // typed into.
    let width = palette_menu_width(ui, palette);
    egui::Popup::menu(&button)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            menu_body(ui, Some(width), |ui| {
                menu_caption(ui, ADD_VALUE_CAPTION);
                // **What the pills really measured**, read off the lane they
                // were laid in rather than off the estimate that opened the
                // menu: the two differ by the rounding in every pill's own
                // galley, and the row under this one is laid to THIS number
                // so that the button at its end and the last pill end on one
                // edge. The owner: "make sure both last pill and button have
                // same padding, text box can be trimmed if needed".
                let laid = ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(MENU_PALETTE_GAP, MENU_PALETTE_GAP);
                    for field in palette {
                        let secret = matches!(field, FieldRef::Password | FieldRef::Totp);
                        if field_pill_button(ui, field, &field.label(), secret).clicked() {
                            draft.sequence = detail_edit::sequence_with(
                                &draft.sequence,
                                Token::Field(field.clone()),
                            );
                            ui.close();
                        }
                    }
                    if palette.is_empty() {
                        ui.label(RichText::new(NO_FIELDS).size(11.0).color(theme::TEXT_FAINT));
                    }
                    ui.min_rect().right()
                });
                let pills_end = laid.inner;
                menu_caption(ui, ADD_LITERAL_CAPTION);
                ui.horizontal(|ui| {
                    // **`row_button`, not `secondary_button`.** The box
                    // beside it is `theme::SECTION_FIELD_HEIGHT`, 28 points,
                    // and the app's field-height button is this one -- the
                    // same pairing the edit form's `Browse\u{2026}` row
                    // draws. The owner, of a 32-point button beside a
                    // 28-point box: "text button make sure same size as
                    // field and paddings same as other button".
                    // **The button is laid FIRST, from the right**, so it
                    // ends exactly where the last pill above it ends and
                    // the box takes what is left -- "make sure both last
                    // pill and button have same padding, text box can be
                    // trimmed if needed". Measured the other way round, off
                    // `row_button_width` and a subtraction, it landed two
                    // points out: a button's drawn width is egui's and not
                    // this crate's arithmetic.
                    ui.set_max_width(pills_end - ui.min_rect().left());
                    let mut box_response = None;
                    let add = ui
                        .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = theme::ROW_BUTTON_GAP;
                            let add = theme::row_button(ui, ADD_LITERAL_BUTTON).clicked();
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    let room = ui.available_width();
                                    box_response = Some(theme::section_text_field_within(
                                        ui,
                                        &mut draft.literal_draft,
                                        false,
                                        room,
                                    ));
                                },
                            );
                            add
                        })
                        .inner;
                    let typed = box_response.as_ref().is_some_and(|box_response| {
                        entered(box_response, ui)
                    });
                    if add || typed {
                        // Escaping is this app's job, not the user's.
                        if let Some(next) = detail_edit::sequence_with_literal(
                            &draft.sequence,
                            &draft.literal_draft,
                        ) {
                            draft.sequence = next;
                            draft.literal_draft.clear();
                            ui.close();
                        }
                    }
                });
            });
        });
}

/// `+ Key`: the keys this app knows how to send.
fn add_key_menu(ui: &mut egui::Ui, draft: &mut SequenceDraft) {
    let button = theme::dashed_button(ui, ADD_KEY_LABEL);
    egui::Popup::menu(&button).show(|ui| {
        menu_body(ui, Some(ADD_MENU_WIDTH), |ui| {
            menu_caption(ui, ADD_KEY_CAPTION);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(MENU_PALETTE_GAP, MENU_PALETTE_GAP);
                for key in key_sequence::KEYS.iter().filter(|k| k.palette) {
                    if keycap_button(ui, key_name(key.label)).clicked() {
                        draft.sequence =
                            detail_edit::sequence_with(&draft.sequence, Token::Key(key));
                        ui.close();
                    }
                }
            });
        });
    });
}

/// `+ Wait`: a number of seconds, which is what the owner asked waits be set
/// in -- the sequence stores the milliseconds.
fn add_wait_menu(ui: &mut egui::Ui, draft: &mut SequenceDraft) {
    let button = theme::dashed_button(ui, ADD_WAIT_LABEL);
    egui::Popup::menu(&button)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            menu_body(ui, None, |ui| {
                menu_caption(ui, ADD_WAIT_CAPTION);
                ui.horizontal(|ui| {
                    let box_response = theme::section_text_field_within(
                        ui,
                        &mut draft.wait_draft,
                        false,
                        WAIT_BOX_WIDTH,
                    );
                    // **Only a number goes in**: see
                    // `key_sequence::only_a_number`, and the owner's "only
                    // allow numbers". Applied to what the box now holds
                    // rather than to the key that arrived, so a paste is
                    // filtered too.
                    if box_response.changed() {
                        draft.wait_draft = key_sequence::only_a_number(&draft.wait_draft);
                    }
                    ui.label(RichText::new(WAIT_UNIT).size(11.0).color(theme::TEXT_FAINT));
                    let addable = key_sequence::wait_ms_from_seconds(&draft.wait_draft).is_some();
                    let typed = entered(&box_response, ui);
                    ui.add_enabled_ui(addable, |ui| {
                        if theme::row_button(ui, ADD_WAIT_BUTTON).clicked() || (typed && addable) {
                            if let Some(next) =
                                detail_edit::sequence_with_wait(&draft.sequence, &draft.wait_draft)
                            {
                                draft.sequence = next;
                                ui.close();
                            }
                        }
                    });
                });
                if key_sequence::wait_ms_from_seconds(&draft.wait_draft).is_none() {
                    ui.add_space(MENU_PALETTE_GAP);
                    ui.label(RichText::new(WAIT_REFUSAL).size(11.0).color(theme::TEXT_FAINT));
                }
            });
        });
}

/// **Whether the box that drew `response` was just submitted with Enter.**
///
/// A one-line box with a button beside it is a tiny form, and a tiny form
/// submits on Return -- the owner, of the two Add buttons: "these fields
/// should also add by Enter". `lost_focus` and the key together is egui's
/// own idiom for it: a `TextEdit` gives focus up on Return, and the key
/// press is still in this frame's input to be read.
fn entered(response: &egui::Response, ui: &egui::Ui) -> bool {
    response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

/// A caption over one of a menu's palettes: the edit form's own small faint
/// label, so a menu opened off this row reads as part of the same app.
fn menu_caption(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(11.0).color(theme::TEXT_FAINT));
}

/// **What every one of the three menus is inside.**
///
/// egui's menu layer is a frame with its own padding and its own spacing,
/// and what was drawn into it was a column of egui's default widgets --
/// outlined buttons, a bare `TextEdit` with egui's own focus border. The
/// owner: "make sure css matches the rest". So each menu is this: the card's
/// own margin, the app's `item_spacing`, and inside it nothing but controls
/// this design system already draws.
fn menu_body<R>(
    ui: &mut egui::Ui,
    width: Option<f32>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    // **`None` means "as wide as what is in it".** The wait menu -- one small
    // box, the word `seconds`, a button -- was being held open at the key
    // palette's width, which left a third of it empty: "popup size should be
    // as per size of elemetns - right now lots of space on the right". Only
    // a menu whose content WRAPS needs a width, because there the width is
    // the wrap budget rather than a result of what is inside.
    if let Some(width) = width {
        ui.set_min_width(width);
        ui.set_max_width(width);
    }
    // **One gap down the menu, and it is the menu's own padding.** A caption
    // and the row under it were `item_spacing` apart and each block another
    // `MENU_BLOCK_GAP` on top, so the air over `Text to type` was sixteen
    // points against five at the top of the card -- the owner: "padding on
    // top of Text to type seems to be big comared to popup to A value from
    // this item".
    ui.spacing_mut().item_spacing = egui::vec2(MENU_PALETTE_GAP, MENU_BLOCK_GAP);
    add(ui)
}

/// **How wide the `+ Text` menu opens: wide enough for its pills on ONE
/// line.**
///
/// The owner, of a palette that had wrapped onto two: "value pills in one
/// line". The pills are the item's own fields, so the width cannot be a
/// number -- an item with six custom fields has six pills. It is measured
/// off the very galleys the pills will draw, floored at the width the other
/// two menus use so the three do not differ for no reason, and capped so a
/// login with a dozen fields opens a menu and not a second window; past the
/// cap the row wraps as it did.
fn palette_menu_width(ui: &egui::Ui, palette: &[FieldRef]) -> f32 {
    let pills: f32 = palette
        .iter()
        .map(|field| {
            // **Measured in the face it will be DRAWN in.** A secret's pill
            // is bold and the rest are semibold, and measuring them all
            // semibold came up a few points short over four pills -- which
            // is a menu one pill too narrow, and the last of them wrapping
            // onto a line of its own: "should be one line of pills since
            // popup is not wider than Fill rule modal itlsef".
            let secret = matches!(field, FieldRef::Password | FieldRef::Totp);
            field_pill_width(ui, &field.label(), secret)
        })
        .sum();
    let gaps = MENU_PALETTE_GAP * palette.len().saturating_sub(1) as f32;
    // **The wider of the menu's two rows, and nothing over.** It was floored
    // at the key palette's width, which left the menu wider than anything in
    // it once the pills fitted -- the owner: "make that popup size of field
    // - tighther". The other row is the literal box and its Add, and the box
    // has a width of its own to keep: a text box narrower than
    // [`LITERAL_BOX_WIDTH`] is one that cannot show what was typed into it.
    let field_row = LITERAL_BOX_WIDTH
        + theme::ROW_BUTTON_GAP
        + theme::row_button_width(ui, ADD_LITERAL_BUTTON);
    // **Capped at the CARD, not at a number of my own.** "the rest of values
    // put in one line unless it becomes wider than Fill rule modal, then
    // transfer to next line and make the popup width dinamic to the width of
    // values": so the pills take one line for as long as one line fits
    // inside the card this menu hangs off, and wrap inside that width after
    // that. The menu is never wider than the thing it belongs to.
    let ceiling = card_width(ui.ctx());
    (pills + gaps).max(field_row).min(ceiling)
}

/// How wide [`field_pill`] will draw for `name`, without drawing it -- the
/// same arithmetic its own body does, over the same galley.
fn field_pill_width(ui: &egui::Ui, name: &str, secret: bool) -> f32 {
    let face = if secret { theme::BOLD } else { theme::SEMIBOLD };
    let font = egui::FontId::new(FIELD_PILL_PX, egui::FontFamily::Name(face.into()));
    let galley = ui.painter().layout_no_wrap(name.to_string(), font, theme::INK);
    FIELD_PILL_PAD_X * 2.0 + FIELD_PILL_MARK + FIELD_PILL_GAP + galley.size().x
}

/// 4a's three captions, and the row's `gap: 8px` and `padding-top: 4px`.
const ADD_TEXT_LABEL: &str = "+ Text";
const ADD_KEY_LABEL: &str = "+ Key";
const ADD_WAIT_LABEL: &str = "+ Wait";
const ADD_ROW_GAP: f32 = 8.0;
const ADD_ROW_LIFT: f32 = 4.0;

/// How wide the KEY menu opens -- its caps wrap, so its width is a budget
/// rather than a result. The value menu has no number: it is as wide as its
/// own pills, up to the card's width (see [`palette_menu_width`]).
const ADD_MENU_WIDTH: f32 = 280.0;

/// The literal box's own width, which is what the value menu is at least as
/// wide as.
const LITERAL_BOX_WIDTH: f32 = 160.0;

/// The gaps inside a menu: between two palette cells, and down the menu
/// between every line of it -- a caption and its row, and one block and the
/// next. One number, because a menu with two rhythms in it reads as two
/// menus; see [`menu_body`].
const MENU_PALETTE_GAP: f32 = 4.0;
const MENU_BLOCK_GAP: f32 = 6.0;

/// The wait box. Wide enough for `3600` and no wider: it takes a number of
/// seconds, and a box the width of the menu would promise a sentence.
const WAIT_BOX_WIDTH: f32 = 72.0;

/// The captions inside the menus.
const ADD_VALUE_CAPTION: &str = "A value from this item";
const ADD_KEY_CAPTION: &str = "A key to press";
const ADD_LITERAL_CAPTION: &str = "Text to type";
const ADD_LITERAL_BUTTON: &str = "Add text";
const ADD_WAIT_CAPTION: &str = "How long to wait";
const ADD_WAIT_BUTTON: &str = "Add wait";
const WAIT_UNIT: &str = "seconds";
const NO_FIELDS: &str = "This item has no fields to reference yet.";

/// The reset: back to the app default, which is an EMPTY stored sequence.
const USE_DEFAULT: &str = "Use the default";

/// The refusal under the wait box, said as the rule rather than as "invalid".
const WAIT_REFUSAL: &str = "Type a number of seconds, up to 3600.";

// **4a's right rail is not drawn**, and neither is its timing strip.
//
// The design puts a 340-point column of measurements beside the steps -- the
// strip, the checks, the budget bars and 4d's way in -- and this screen drew
// all four. The owner has scoped them out: "4b and 4d we can not do for now
// and 4a only keep builder without timing and right panel as well".
//
// **The measurements are not deleted, only the rail.** `checks`,
// `timing_spans`, `timing_ticks` and `with_pause_after` are above, pure, `pub`
// and covered by their own tests; `BAR_HEIGHT`, `BAR_FLOOR` and the four
// functions that PAINTED them are gone, because a drawing function with no
// caller is dead code and this crate keeps none. The day the rail comes back
// it comes back onto logic that never stopped being right.

#[cfg(test)]
mod tests {

    use super::*;
    use crate::vault_window::detail::TotpState;

    fn source() -> ResolveSource<'static> {
        ResolveSource {
            username: "a.novak@ledgerline.com",
            password: "correct-horse-battery",
            custom: Vec::new(),
            totp: &TotpState::NoSecret,
        }
    }

    /// A corpus wide enough that a grouping bug in either function shows up.
    const CORPUS: &[&str] = &[
        "",
        "{USERNAME}{TAB}{PASSWORD}",
        "{USERNAME}{TAB}{DELAY 250}{PASSWORD}{ENTER}",
        "{DELAY=40}{USERNAME}{TAB}{PASSWORD}",
        "hello{USERNAME}world{TAB}{PASSWORD}",
        "+{TAB}{USERNAME}",
        "{USERNAME}{ENTER}{PASSWORD}",
        "{PASSWORD}",
        "{DELAY 500}",
    ];


    #[test]
    fn the_acts_here_are_the_acts_the_tally_counts() {
        for sequence in CORPUS {
            let Some(tally) = sequence_tally(sequence, &source()) else { continue };
            assert_eq!(
                acts(sequence).len(),
                tally.steps,
                "the timing strip and the tally disagree about {sequence:?}"
            );
        }
    }

    #[test]
    fn every_span_is_labelled_and_none_carries_a_value() {
        let spans = timing_spans("{USERNAME}{TAB}{PASSWORD}", &source()).expect("plans");
        assert_eq!(spans.len(), 3);
        for span in &spans {
            assert!(!span.act.label.is_empty());
            assert!(
                !span.act.label.contains("correct-horse-battery"),
                "a span label carried the password: {:?}",
                span.act.label
            );
        }
        assert!(spans[2].act.secret, "the password act is the secret one");
        assert!(!spans[1].act.secret);
    }

    #[test]
    fn the_spans_run_end_to_end_and_add_up_to_the_tally() {
        let sequence = "{USERNAME}{TAB}{DELAY 250}{PASSWORD}{ENTER}";
        let spans = timing_spans(sequence, &source()).expect("plans");
        let tally = sequence_tally(sequence, &source()).expect("tallies");
        let mut at = Duration::ZERO;
        for span in &spans {
            assert_eq!(span.at, at, "spans must not overlap or leave gaps");
            at += span.len;
        }
        assert_eq!(at, tally.total);
    }

    #[test]
    fn a_sequence_the_runner_refuses_draws_no_strip() {
        // No password on the item, so `{PASSWORD}` cannot resolve.
        let empty = ResolveSource {
            username: "",
            password: "",
            custom: Vec::new(),
            totp: &TotpState::NoSecret,
        };
        assert!(timing_spans("{PASSWORD}", &empty).is_none());
        assert!(sequence_tally("{PASSWORD}", &empty).is_none());
    }

    #[test]
    fn the_scale_ends_on_the_real_total_and_stays_readable() {
        for ms in [80u64, 250, 2_100, 9_000, 59_000] {
            let total = Duration::from_millis(ms);
            let marks = timing_ticks(total);
            assert_eq!(marks.first(), Some(&Duration::ZERO));
            assert_eq!(marks.last(), Some(&total));
            assert!(marks.len() <= 7, "{ms} ms produced {} marks", marks.len());
            assert!(marks.windows(2).all(|w| w[0] <= w[1]), "marks out of order at {ms} ms");
        }
    }

    #[test]
    fn a_password_typed_with_no_field_change_before_it_is_cautioned() {
        let found = checks("{USERNAME}{PASSWORD}");
        assert!(
            found.iter().any(|c| c.tone == CheckTone::Caution && c.line == FIELD_CHANGE_CAUTION),
            "{found:?}"
        );
        let fixed = checks("{USERNAME}{TAB}{PASSWORD}");
        assert!(
            fixed.iter().any(|c| c.tone == CheckTone::Pass && c.line == FIELD_CHANGE_PASS),
            "{fixed:?}"
        );
    }

    #[test]
    fn a_sequence_that_starts_at_the_password_is_not_cautioned_for_it() {
        // Nothing was typed first, so there is no field to have moved out of.
        let found = checks("{PASSWORD}");
        assert!(found.iter().any(|c| c.tone == CheckTone::Pass && c.line == FIELD_CHANGE_PASS));
    }

    #[test]
    fn a_sequence_with_no_secret_has_neither_secret_check() {
        let found = checks("{USERNAME}{TAB}");
        assert!(!found.iter().any(|c| c.line == FIELD_CHANGE_PASS));
        assert!(!found.iter().any(|c| c.line == FIELD_CHANGE_CAUTION));
        assert!(!found.iter().any(|c| c.line == MASKED_GATE));
    }

    #[test]
    fn an_enter_with_something_after_it_offers_the_pause() {
        let found = checks("{USERNAME}{ENTER}{PASSWORD}");
        let caution = found
            .iter()
            .find(|c| c.line == ENTER_CAUTION)
            .expect("the Enter caution is drawn");
        let Some(CheckFix::PauseAfterEnter(index)) = caution.fix else {
            panic!("the caution offers no fix: {caution:?}")
        };
        let fixed = with_pause_after("{USERNAME}{ENTER}{PASSWORD}", index);
        assert_eq!(fixed, "{USERNAME}{ENTER}{DELAY 200}{PASSWORD}");
        // And the fix really closes the check it came from.
        assert!(!checks(&fixed).iter().any(|c| c.line == ENTER_CAUTION));
    }

    #[test]
    fn an_enter_that_ends_the_sequence_is_not_cautioned() {
        assert!(!checks("{USERNAME}{TAB}{PASSWORD}{ENTER}")
            .iter()
            .any(|c| c.line == ENTER_CAUTION));
    }

    #[test]
    fn a_fix_aimed_past_the_end_changes_nothing_at_all() {
        let before = "{USERNAME}{TAB}{PASSWORD}";
        assert_eq!(with_pause_after(before, 99), before);
    }

    #[test]
    fn the_two_facts_are_always_there_to_be_read() {
        let found = checks("{USERNAME}{TAB}{PASSWORD}");
        assert!(found.iter().any(|c| c.tone == CheckTone::Fact && c.line == MASKED_GATE));
        assert!(found.iter().any(|c| c.tone == CheckTone::Fact && c.line == NO_EMPTY_TEXT));
    }

    #[test]
    fn a_draft_saves_the_binding_with_only_its_sequence_changed() {
        let stored = crate::app_match::AppMatch {
            process: "saplogon.exe".into(),
            title: "Sign in".into(),
            hosted: true,
            path: "C:\\SAP\\saplogon.exe".into(),
            args: "-x".into(),
            sequence: "{USERNAME}{TAB}{PASSWORD}".into(),
            trigger: crate::app_match::TriggerMode::Prompt,
        };
        let mut draft = SequenceDraft {
            item_id: "id".into(),
            item_name: "SAP Production".into(),
            folder: String::new(),
            app_name: "SAP Logon".into(),
            app: stored.clone(),
            original: stored.sequence.clone(),
            sequence: stored.sequence.clone(),
            template_view: false,
            template_draft: stored.sequence.clone(),
            template_touched: false,
            revealing: false,
            literal_draft: String::new(),
            wait_draft: "1".into(),
            selected: None,
            discard_prompt: false,
        };
        assert!(!draft.changed());
        assert!(!draft.saveable());

        draft.sequence = "{USERNAME}{TAB}{PASSWORD}{ENTER}".into();
        assert!(draft.changed());
        assert!(draft.saveable());
        let saved = draft.saved();
        assert_eq!(saved.sequence, "{USERNAME}{TAB}{PASSWORD}{ENTER}");
        assert_eq!(saved.process, stored.process);
        assert_eq!(saved.title, stored.title);
        assert_eq!(saved.hosted, stored.hosted);
        assert_eq!(saved.path, stored.path);
        assert_eq!(saved.args, stored.args);
        assert_eq!(saved.trigger, stored.trigger);
    }

    #[test]
    fn a_template_that_will_not_parse_stops_the_save_only_once_it_is_typed_in() {
        let stored = crate::app_match::AppMatch {
            process: "x.exe".into(),
            title: String::new(),
            hosted: false,
            path: String::new(),
            args: String::new(),
            sequence: "{USERNAME".into(),
            trigger: crate::app_match::TriggerMode::Prompt,
        };
        let mut draft = SequenceDraft {
            item_id: "id".into(),
            item_name: "x".into(),
            folder: String::new(),
            app_name: "x".into(),
            app: stored.clone(),
            original: stored.sequence.clone(),
            sequence: stored.sequence.clone(),
            template_view: true,
            template_draft: stored.sequence.clone(),
            template_touched: false,
            revealing: false,
            literal_draft: String::new(),
            wait_draft: "1".into(),
            selected: None,
            discard_prompt: false,
        };
        // Inherited and merely looked at: no fault, and nothing to save.
        assert!(draft.fault().is_none());

        draft.template_touched = true;
        draft.sequence = "{USERNAME".into();
        assert!(draft.fault().is_some());
        assert!(!draft.saveable(), "an unparseable template must not be saveable");
    }

    #[test]
    fn an_item_bound_to_nothing_has_nowhere_for_a_sequence_to_live() {
        assert!(!fill_rule_visible(None));
        assert!(fill_rule_visible(Some(&crate::app_match::AppMatch {
            process: "x.exe".into(),
            title: String::new(),
            hosted: false,
            path: String::new(),
            args: String::new(),
            sequence: String::new(),
            trigger: crate::app_match::TriggerMode::Prompt,
        })));
    }

    // -----------------------------------------------------------------------
    // The steps, and the two edits on them
    // -----------------------------------------------------------------------

    /// The labels of the steps that ARE acts, in order.
    fn act_steps(sequence: &str) -> Vec<String> {
        steps(sequence, &source(), false)
            .into_iter()
            .filter(|step| match step.kind {
                StepKind::Text | StepKind::Wait => true,
                // A modifier with no key after it has no bar in the strip.
                StepKind::Key => !step
                    .rows
                    .iter()
                    .all(|row| row.aside == detail_edit::MODIFIER_NOTE),
                StepKind::Rate | StepKind::Raw => false,
            })
            .map(|step| step.label())
            .collect()
    }

    /// **Wherever [`acts`] has a bar, [`steps`] has the same row**: the same
    /// grouping, the same words, over the corpus. The rows `steps` adds --
    /// a rate change, a raw token, a lone modifier -- are the ones `acts`
    /// has no bar for, and they are filtered here rather than reconciled,
    /// because that difference is the point of `steps`.
    #[test]
    fn the_steps_that_are_acts_are_the_acts() {
        for sequence in CORPUS.iter().chain(&["hello{USERNAME}world{TAB}", "+{TAB}{USERNAME}"]) {
            let expected: Vec<String> = acts(sequence).into_iter().map(|act| act.label).collect();
            assert_eq!(act_steps(sequence), expected, "for {sequence:?}");
        }
    }

    #[test]
    fn a_step_is_a_span_and_a_move_moves_all_of_it() {
        // Three tokens, one step: the text run travels whole.
        let sequence = "hello{USERNAME}world{TAB}{PASSWORD}";
        assert_eq!(steps(sequence, &source(), false).len(), 3);
        // Sent past `Password`, a field, the run lands beside it and the two
        // are one text step: it is step 1 of two, not step 2 of three.
        assert_eq!(
            sequence_with_step_moved(sequence, 0, 2),
            ("{TAB}{PASSWORD}hello{USERNAME}world".to_string(), 1)
        );
        // A modifier travels with the key it is held for.
        assert_eq!(
            sequence_with_step_moved("+{TAB}{USERNAME}", 1, 0),
            ("{USERNAME}+{TAB}".to_string(), 0)
        );
        // Moving down past a neighbour and up past one are each other's
        // undo, with neighbours that cannot merge.
        let (moved, at) = sequence_with_step_moved("{USERNAME}{TAB}{DELAY 250}", 0, 1);
        assert_eq!((moved.as_str(), at), ("{TAB}{USERNAME}{DELAY 250}", 1));
        assert_eq!(
            sequence_with_step_moved(&moved, 1, 0),
            ("{USERNAME}{TAB}{DELAY 250}".to_string(), 0)
        );
    }

    #[test]
    fn a_move_that_changes_nothing_hands_the_string_back_untouched() {
        let sequence = "{USERNAME}{TAB}{PASSWORD}";
        assert_eq!(sequence_with_step_moved(sequence, 1, 1), (sequence.to_string(), 1));
        assert_eq!(sequence_with_step_moved(sequence, 7, 0), (sequence.to_string(), 7));
        assert_eq!(sequence_with_step_moved(sequence, 0, 7), (sequence.to_string(), 0));
        assert_eq!(sequence_without_step(sequence, 7), sequence);
    }

    #[test]
    fn two_text_steps_put_side_by_side_become_one_and_the_moved_index_says_where() {
        // [Username][Tab][Password] with Tab sent to the end: the two fields
        // are adjacent and the list re-reads as two steps, not three -- and
        // Tab, asked to be step 2, is step 1.
        let (moved, at) = sequence_with_step_moved("{USERNAME}{TAB}{PASSWORD}", 1, 2);
        assert_eq!((moved.as_str(), at), ("{USERNAME}{PASSWORD}{TAB}", 1));
        assert_eq!(steps(&moved, &source(), false).len(), 2);
        // The moved step itself merging: `hello` sent past Tab lands beside
        // `world`, and the one step they become is where it is.
        let (moved, at) = sequence_with_step_moved("hello{TAB}world", 0, 1);
        assert_eq!((moved.as_str(), at), ("{TAB}helloworld", 1));
        // A lone modifier sent before a key joins it.
        let (moved, at) = sequence_with_step_moved("+{USERNAME}{TAB}", 0, 1);
        assert_eq!((moved.as_str(), at), ("{USERNAME}+{TAB}", 1));
        assert_eq!(steps(&moved, &source(), false).len(), 2);
    }

    #[test]
    fn removing_a_step_removes_its_whole_span_and_the_last_one_restores_the_default() {
        assert_eq!(sequence_without_step("{USERNAME}+{TAB}{PASSWORD}", 1), "{USERNAME}{PASSWORD}");
        assert_eq!(sequence_without_step("hello{USERNAME}world{TAB}", 0), "{TAB}");
        // The empty string IS the default, and the item inherits again.
        assert_eq!(sequence_without_step("{TAB}", 0), "");
    }

    #[test]
    fn a_lone_modifier_is_its_own_row_and_says_so() {
        let list = steps("+hello{TAB}", &source(), false);
        assert_eq!(list.len(), 3, "{list:?}");
        assert_eq!(list[0].kind, StepKind::Key);
        assert_eq!(list[0].aside(), detail_edit::MODIFIER_NOTE);
        // And with its key it is one row, saying nothing about being held.
        let held = steps("+{TAB}", &source(), false);
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].label(), "Shift + Tab");
        assert_eq!(acts("+{TAB}")[0].label, "Shift + Tab");
        assert_eq!(held[0].aside(), "");
    }

    // -----------------------------------------------------------------------
    // The modal, drawn: what one frame of it paints
    // -----------------------------------------------------------------------

    /// A shape the frame painted that is not a run of text, with the three
    /// things the assertions below tell shapes apart by.
    struct PaintedRect {
        rect: egui::Rect,
        fill: egui::Color32,
        stroke: egui::Color32,
        corners: CornerRadius,
    }

    #[derive(Default)]
    struct Painted {
        texts: Vec<(String, egui::Rect)>,
        /// Each run's true INK -- the union of its glyphs' coverage, which
        /// is what the eye sees centred or not, as against the galley's box
        /// (ascent plus descent), which a face never fills.
        inks: Vec<(String, egui::Rect)>,
        /// Every galley painted, with where it was put: what a test that
        /// needs a run's own glyphs -- a token's, inside the template line
        /// -- reads, since `inks` is the union over a whole galley.
        galleys: Vec<(egui::Pos2, std::sync::Arc<egui::Galley>)>,
        rects: Vec<PaintedRect>,
    }

    impl Painted {
        fn strings(&self) -> Vec<&str> {
            self.texts.iter().map(|(t, _)| t.as_str()).collect()
        }

        /// The one run spelled `text`, as INK. See [`Painted::inks`].
        fn ink_of(&self, text: &str) -> egui::Rect {
            let found: Vec<egui::Rect> =
                self.inks.iter().filter(|(t, _)| t == text).map(|(_, r)| *r).collect();
            assert_eq!(found.len(), 1, "expected one {text:?} inked, found {}", found.len());
            found[0]
        }

        fn rects_of(&self, text: &str) -> Vec<egui::Rect> {
            self.texts.iter().filter(|(t, _)| t == text).map(|(_, r)| *r).collect()
        }

        fn rect_of(&self, text: &str) -> egui::Rect {
            let found = self.rects_of(text);
            assert_eq!(
                found.len(),
                1,
                "expected one {text:?}, found {}. Painted: {:?}",
                found.len(),
                self.strings()
            );
            found[0]
        }

        /// The card's own frame: the one filled rectangle stroked in the
        /// card's accent. `modal_card` strokes the same rect twice -- once
        /// through the `Frame`, once over the bands -- and the second has no
        /// fill, which is what tells them apart.
        fn card(&self) -> egui::Rect {
            let found: Vec<&PaintedRect> = self
                .rects
                .iter()
                .filter(|r| r.fill == theme::CARD && r.stroke == theme::BLUE)
                .collect();
            assert_eq!(found.len(), 1, "expected one card frame, found {}", found.len());
            found[0].rect
        }

        /// The header band: the accent fill whose top corners are rounded
        /// and whose bottom corners are not. See `theme::modal_header_band`.
        fn header_band(&self) -> egui::Rect {
            let found: Vec<&PaintedRect> = self
                .rects
                .iter()
                .filter(|r| r.fill == theme::BLUE && r.corners.nw > 0 && r.corners.sw == 0)
                .collect();
            assert_eq!(found.len(), 1, "expected one header band, found {}", found.len());
            found[0].rect
        }

        /// The row frame -- 4a's white card with the hairline round it, or
        /// the selected or secret one -- that holds `inside`.
        fn row_around(&self, inside: egui::Rect) -> egui::Rect {
            self.rects
                .iter()
                .filter(|r| {
                    r.corners.nw == STEP_ROW_RADIUS
                        && r.corners.sw == STEP_ROW_RADIUS
                        && r.rect.contains_rect(inside)
                })
                .map(|r| r.rect)
                .min_by(|a, b| a.height().total_cmp(&b.height()))
                .unwrap_or_else(|| panic!("no step row is painted round {inside:?}"))
        }

        /// The three bars of the drag handle on the row that holds `inside`,
        /// as one rect: what a drag starts on.
        fn grip_of(&self, row: egui::Rect) -> egui::Rect {
            let bars: Vec<egui::Rect> = self
                .rects
                .iter()
                .filter(|r| {
                    (r.rect.width() - STEP_GRIP_BAR.x).abs() < 0.01
                        && (r.rect.height() - STEP_GRIP_BAR.y).abs() < 0.01
                        && row.contains_rect(r.rect)
                })
                .map(|r| r.rect)
                .collect();
            assert_eq!(bars.len(), 3, "expected the handle's three bars in {row:?}");
            bars.iter().skip(1).fold(bars[0], |a, b| a.union(*b))
        }
    }

    fn walk(shape: &egui::Shape, painted: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => {
                painted.texts.push((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                ));
                let mut ink = egui::Rect::NOTHING;
                for row in &text.galley.rows {
                    for glyph in &row.glyphs {
                        if glyph.uv_rect.is_nothing() {
                            continue;
                        }
                        let at = text.pos
                            + row.pos.to_vec2()
                            + glyph.pos.to_vec2()
                            + glyph.uv_rect.offset;
                        ink = ink.union(egui::Rect::from_min_size(at, glyph.uv_rect.size));
                    }
                }
                if ink.is_finite() {
                    painted.inks.push((text.galley.text().to_string(), ink));
                }
                painted.galleys.push((text.pos, text.galley.clone()));
            }
            egui::Shape::Rect(rect) => painted.rects.push(PaintedRect {
                rect: rect.rect,
                fill: rect.fill,
                stroke: rect.stroke.color,
                corners: rect.corner_radius,
            }),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, painted)),
            _ => {}
        }
    }

    /// The design's own content: `SAP Production` filed under `Work`, bound
    /// to `SAP Logon 760` by process and window class, typing the design's
    /// five-step sequence with a wait in it.
    fn draft() -> SequenceDraft {
        draft_named("SAP Logon 760")
    }

    fn draft_named(app_name: &str) -> SequenceDraft {
        let stored = crate::app_match::AppMatch {
            process: "saplogon.exe".into(),
            title: "SAPFEWndClass".into(),
            hosted: true,
            path: String::new(),
            args: String::new(),
            sequence: "{USERNAME}{TAB}{PASSWORD}{DELAY 250}{ENTER}".into(),
            trigger: crate::app_match::TriggerMode::Prompt,
        };
        SequenceDraft {
            item_id: "id".into(),
            item_name: "SAP Production".into(),
            folder: "Work".into(),
            app_name: app_name.into(),
            app: stored.clone(),
            original: stored.sequence.clone(),
            sequence: stored.sequence.clone(),
            template_view: false,
            template_draft: stored.sequence.clone(),
            template_touched: false,
            revealing: false,
            literal_draft: String::new(),
            wait_draft: "1".into(),
            selected: None,
            discard_prompt: false,
        }
    }

    /// One frame of the builder over a `window`-sized screen.
    ///
    /// **The clock advances a tenth of a second per frame**, so the `Area`
    /// the card floats in is fully opaque by the time a frame is read:
    /// egui fades an area in by multiplying its layer's opacity, and a
    /// headless context's clock does not move on its own -- the same clock
    /// `detail_edit`'s harness keeps, for the same reason. The colours the
    /// assertions match on are the theme's, and a faded frame paints none of
    /// them.
    struct Modal {
        ctx: egui::Context,
        window: egui::Vec2,
        clock: std::cell::Cell<f64>,
        icon: Option<egui::TextureHandle>,
        /// The fields the `+ Text` menu offers. Two by default, because
        /// that is what most of these tests need on screen; the menu's own
        /// width is about how many there are, so its test asks for four.
        palette: Vec<FieldRef>,
    }

    impl Modal {
        fn over(window: egui::Vec2) -> Self {
            let ctx = egui::Context::default();
            let modal = Self {
                ctx,
                window,
                clock: std::cell::Cell::new(0.0),
                icon: None,
                palette: vec![FieldRef::Username, FieldRef::Password],
            };
            // A font set registered during a frame is usable from the next
            // one on, so two throwaway frames -- every harness in this crate
            // runs them.
            let _ = modal.ctx.run_ui(modal.input(&[]), |_ui| {});
            theme::apply(&modal.ctx);
            let _ = modal.ctx.run_ui(modal.input(&[]), |_ui| {});
            modal
        }

        /// The same harness, with these fields in the `+ Text` menu.
        fn with_palette(mut self, palette: Vec<FieldRef>) -> Self {
            self.palette = palette;
            self
        }

        /// The same harness with a favicon for the vault item's tile.
        fn with_item_icon(mut self) -> Self {
            self.icon = Some(self.ctx.load_texture(
                "item-icon",
                egui::ColorImage::from_rgba_unmultiplied([1, 1], &[255, 255, 255, 255]),
                egui::TextureOptions::default(),
            ));
            self
        }

        fn input(&self, events: &[egui::Event]) -> egui::RawInput {
            self.clock.set(self.clock.get() + 0.1);
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.window)),
                time: Some(self.clock.get()),
                events: events.to_vec(),
                ..Default::default()
            }
        }

        fn frame_with(&self, draft: &mut SequenceDraft, events: &[egui::Event]) -> Painted {
            self.frame_with_input(draft, self.input(events))
        }

        /// The builder at rest: an `egui::Area` is laid out from the size it
        /// had on the previous frame, the `ScrollArea` inside it from the
        /// frame after that, and the card re-centres as its body settles --
        /// measured, the rows sixty points higher on the fifth frame than on
        /// the third, and the hint under them painted only from the fourth.
        /// So the frame read is the fifth.
        fn frame(&self, draft: &mut SequenceDraft) -> Painted {
            for _ in 0..4 {
                let _ = self.frame_with(draft, &[]);
            }
            self.frame_with(draft, &[])
        }

        /// A press and a release at `at`, which is what egui counts as a
        /// click, then the frame after, which is where the click lands.
        fn click(&self, draft: &mut SequenceDraft, at: egui::Pos2) -> Painted {
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            let _ = self.frame_with(draft, &[egui::Event::PointerMoved(at), button(true)]);
            let _ = self.frame_with(draft, &[button(false)]);
            self.frame_with(draft, &[])
        }

        /// A press at `from`, the pointer carried to `to` in four moves --
        /// egui counts a press a drag once it has travelled, so one move is
        /// not enough -- and a release there. Answers the frame after.
        fn drag(&self, draft: &mut SequenceDraft, from: egui::Pos2, to: egui::Pos2) -> Painted {
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            let _ = self.frame_with(draft, &[egui::Event::PointerMoved(from), button(from, true)]);
            for step in 1..=4 {
                let at = from + (to - from) * (step as f32 / 4.0);
                let _ = self.frame_with(draft, &[egui::Event::PointerMoved(at)]);
            }
            let _ = self.frame_with(draft, &[button(to, false)]);
            self.frame_with(draft, &[])
        }

        /// One key, pressed and released, then the frame after.
        fn key(&self, draft: &mut SequenceDraft, key: egui::Key, modifiers: egui::Modifiers) -> Painted {
            let event = |pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            };
            let _ = self.frame_with_input(
                draft,
                egui::RawInput { modifiers, ..self.input(&[event(true)]) },
            );
            let _ = self.frame_with(draft, &[event(false)]);
            self.frame_with(draft, &[])
        }

        /// What the builder REPORTED for a key press, rather than what it
        /// painted after one -- the two ways out answer here and nowhere on
        /// screen.
        fn key_action(
            &self,
            draft: &mut SequenceDraft,
            key: egui::Key,
            modifiers: egui::Modifiers,
        ) -> BuilderAction {
            let event = egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            };
            let palette = self.palette.clone();
            let totp = crate::vault_window::detail::TotpState::NoSecret;
            let source = ResolveSource {
                username: "a.novak@ledgerline.com",
                password: "correct-horse-battery",
                custom: Vec::new(),
                totp: &totp,
            };
            let mut apps = AppIdentityCache::default();
            let mut action = BuilderAction::None;
            let input = egui::RawInput { modifiers, ..self.input(&[event]) };
            let _ = self.ctx.run_ui(input, |ui| {
                action = draw_sequence_builder(
                    ui,
                    draft,
                    &palette,
                    &source,
                    self.icon.as_ref(),
                    &mut apps,
                );
            });
            action
        }

        fn frame_with_input(&self, draft: &mut SequenceDraft, input: egui::RawInput) -> Painted {
            let palette = self.palette.clone();
            let totp = crate::vault_window::detail::TotpState::NoSecret;
            let source = ResolveSource {
                username: "a.novak@ledgerline.com",
                password: "correct-horse-battery",
                custom: Vec::new(),
                totp: &totp,
            };
            let mut apps = AppIdentityCache::default();
            let output = self.ctx.run_ui(input, |ui| {
                let _ = draw_sequence_builder(
                    ui,
                    draft,
                    &palette,
                    &source,
                    self.icon.as_ref(),
                    &mut apps,
                );
            });
            let mut painted = Painted::default();
            for clipped in &output.shapes {
                walk(&clipped.shape, &mut painted);
            }
            painted
        }
    }

    /// The narrowest window this app opens at, and the design's own width.
    /// `card_width` answers 640 for both, which is what the card is asserted
    /// to be.
    const WINDOWS: [egui::Vec2; 2] = [egui::vec2(900.0, 740.0), egui::vec2(1240.0, 740.0)];

    /// **The header band spans the card edge to edge, and nothing the body
    /// paints runs past the card.**
    ///
    /// The half with no other check, and it has regressed twice: the owner's
    /// screenshot had the blue band stopping at x = 655 with the white card
    /// running on to 890. `theme::modal_card` sets `ui.set_width(card.width)`
    /// and lays the band at that width; the card is a `Frame`, and a `Frame`
    /// grows to hold whatever its body claims, so any child of the body that
    /// ignores its lane drags the card out from under the band. Measured
    /// before this pass, in a 1240-point window: the card 1025.3 wide against
    /// a band of 642. The amber notice's `Frame` was the child -- see
    /// [`destination_band`] -- and after it went the two are 642 and 642.
    ///
    /// The card's width is asserted too, against [`CARD_WIDTH`] plus the
    /// stroke either side of it, so a card that grew AND took the band with
    /// it could not pass by agreeing with itself.
    #[test]
    fn the_header_band_spans_the_card_and_nothing_paints_past_it() {
        for window in WINDOWS {
            let modal = Modal::over(window);
            let painted = modal.frame(&mut draft());
            let card = painted.card();
            let band = painted.header_band();
            assert!(
                (card.width() - (CARD_WIDTH + 2.0 * theme::MODAL_BAND_BLEED)).abs() <= 0.5,
                "in a {}-point window the card is {} wide, not the {} it was asked for",
                window.x,
                card.width(),
                CARD_WIDTH
            );
            assert!(
                (band.left() - card.left()).abs() <= 0.5 && (band.right() - card.right()).abs() <= 0.5,
                "in a {}-point window the header band runs {}..{} over a card running {}..{}",
                window.x,
                band.left(),
                band.right(),
                card.left(),
                card.right()
            );
            for (text, rect) in &painted.texts {
                assert!(
                    rect.left() >= card.left() - 0.5 && rect.right() <= card.right() + 0.5,
                    "{text:?} is painted at {rect:?}, past the card at {card:?}"
                );
            }
            for shape in painted.rects.iter().filter(|r| r.fill.a() == 255) {
                assert!(
                    shape.rect.left() >= card.left() - 0.5 && shape.rect.right() <= card.right() + 0.5,
                    "a {:?} rectangle is painted at {:?}, past the card at {card:?}",
                    shape.fill,
                    shape.rect
                );
            }
        }
    }

    /// [`band_form`] on the numbers the band's doc quotes, and on the two
    /// lanes either side of each of its three thresholds.
    #[test]
    fn the_band_takes_the_first_of_its_three_forms_that_fits() {
        // The design's content in the 640-point card: 183, 96, 342 beside and
        // 232 under, in a 560 lane.
        let (item, arrow, beside, under) = (183.0, 96.0, 342.0, 232.0);
        assert_eq!(band_form(700.0, item, arrow, beside, under), BandForm::OneLine(ChipsGo::Beside));
        assert_eq!(band_form(560.0, item, arrow, beside, under), BandForm::OneLine(ChipsGo::Under));
        // Too narrow for one line even with the chips under: the app takes
        // the second line with its arrow, and the chips go beside the name
        // there while that line holds them...
        assert_eq!(band_form(500.0, item, arrow, beside, under), BandForm::TwoLines(ChipsGo::Beside));
        // ...and under it when it does not.
        assert_eq!(band_form(400.0, item, arrow, beside, under), BandForm::TwoLines(ChipsGo::Under));
        // The thresholds themselves are inclusive: a line that exactly fits
        // is a line that fits.
        assert_eq!(band_form(657.0, item, arrow, beside, under), BandForm::OneLine(ChipsGo::Beside));
        assert_eq!(band_form(547.0, item, arrow, beside, under), BandForm::OneLine(ChipsGo::Under));
        assert_eq!(band_form(456.0, item, arrow, beside, under), BandForm::TwoLines(ChipsGo::Beside));
    }

    /// **With the design's content in the 640-point card the chips drop under
    /// the app's name**, and the band is still one line: the two tiles and
    /// the two captions share their lines, and the chips sit under the second
    /// name. See [`BandSubject::draw`] for why the captions, and not the
    /// columns' middles, are what line up.
    #[test]
    fn the_chips_drop_under_the_apps_name_and_the_band_stays_one_line() {
        let modal = Modal::over(WINDOWS[0]);
        let painted = modal.frame(&mut draft());
        let item_tile = painted.rect_of("SP");
        let app_tile = painted.rect_of("SL");
        let item_caption = painted.rect_of("VAULT ITEM");
        let app_caption = painted.rect_of("SENDS ONLY TO");
        let process = painted.rect_of("saplogon.exe");
        let class = painted.rect_of("SAPFEWndClass");
        assert!(
            (item_tile.center().y - app_tile.center().y).abs() <= 0.5,
            "the two tiles are not on one line: {item_tile:?} and {app_tile:?}"
        );
        assert!(
            (item_caption.center().y - app_caption.center().y).abs() <= 0.5,
            "the two captions are not on one line: {item_caption:?} and {app_caption:?}"
        );
        assert!(
            app_caption.top() >= app_tile.top() - 12.0,
            "the caption {app_caption:?} starts above its tile {app_tile:?}"
        );
        assert!(app_tile.right() < app_caption.left(), "the caption is not beside its tile");
        assert!(
            process.top() >= app_caption.bottom() && class.top() >= app_caption.bottom(),
            "the chips did not drop under the name: caption {app_caption:?}, chips {process:?} {class:?}"
        );
        assert!(
            (process.center().y - class.center().y).abs() <= 0.5 && process.right() < class.left(),
            "the two chips are not on one line in order"
        );
    }

    /// **An app name the line cannot hold puts the app on a second line, with
    /// its arrow**: the vault item's name above, the app's name below it, and
    /// the arrow's rules on the app's line rather than the item's.
    #[test]
    fn a_long_app_name_takes_the_app_and_its_arrow_to_a_second_line() {
        let modal = Modal::over(WINDOWS[0]);
        let long = "Microsoft Dynamics 365 Business Central Client";
        let painted = modal.frame(&mut draft_named(long));
        let item = painted.rect_of("SAP Production");
        let app = painted.rect_of(long);
        assert!(
            app.top() > item.bottom(),
            "the app's name {app:?} is not under the item's {item:?}"
        );
        // The arrow's two rules: one point tall, `BAND_ARROW_RULE` wide, in
        // `theme::BORDER`.
        let rules: Vec<&PaintedRect> = painted
            .rects
            .iter()
            .filter(|r| {
                r.fill == theme::BORDER
                    && (r.rect.height() - 1.0).abs() < 0.01
                    && (r.rect.width() - BAND_ARROW_RULE).abs() < 0.01
            })
            .collect();
        assert_eq!(rules.len(), 2, "expected the arrow's two rules");
        for rule in rules {
            assert!(
                rule.rect.center().y > item.bottom(),
                "an arrow rule at {:?} is on the item's line, not the app's",
                rule.rect
            );
        }
        // And still inside the card: the whole point of choosing a form.
        let card = painted.card();
        assert!(app.right() <= card.right() + 0.5, "the app's name runs past the card");
    }

    /// **The vault item's tile shows the favicon when there is one**, and its
    /// monogram only when there is not. The app's tile has no icon in this
    /// harness -- the cache is cold and the path is empty -- so its monogram
    /// stays, which is the control.
    #[test]
    fn the_item_tile_shows_the_favicon_and_the_monogram_only_without_one() {
        let bare = Modal::over(WINDOWS[0]).frame(&mut draft());
        assert!(bare.strings().contains(&"SP"), "no monogram without a favicon: {:?}", bare.strings());
        assert!(bare.strings().contains(&"SL"));

        let pictured = Modal::over(WINDOWS[0]).with_item_icon().frame(&mut draft());
        assert!(
            !pictured.strings().contains(&"SP"),
            "the monogram is painted over the favicon: {:?}",
            pictured.strings()
        );
        assert!(pictured.strings().contains(&"SL"), "the app's tile lost its monogram");
    }

    /// **4a's SEQUENCE band**: the caption in capitals, the tally without the
    /// edit form's "total", and the two on the pill's line -- and no card
    /// round it: the tint is square, and nothing with the section card's
    /// radius is painted round the rows.
    #[test]
    fn the_sequence_band_is_a_square_tint_with_the_caption_tally_and_pill_on_one_line() {
        let painted = Modal::over(WINDOWS[0]).frame(&mut draft());
        let caption = painted.rect_of("SEQUENCE");
        let tally = painted
            .texts
            .iter()
            .find(|(t, _)| t.starts_with("5 steps"))
            .map(|(t, r)| (t.clone(), *r))
            .expect("the tally is painted");
        assert!(!tally.0.ends_with("total"), "the band spells the tally {:?}", tally.0);
        let steps = painted.rect_of("Steps");
        let template = painted.rect_of("Template");
        for (what, rect) in [("caption", caption), ("tally", tally.1), ("Template", template)] {
            assert!(
                (rect.center().y - steps.center().y).abs() <= 1.0,
                "the {what} at {rect:?} is off the pill's line at {steps:?}"
            );
        }
        assert!(caption.right() < tally.1.left() && tally.1.right() < steps.left());
        let tint = painted
            .rects
            .iter()
            .find(|r| r.fill == theme::CARD_TINT && r.rect.contains_rect(caption))
            .expect("the band's tint is painted");
        assert_eq!(tint.corners, CornerRadius::ZERO, "the band has a card's corners");
        // Nothing with a section card's edge encloses the band and the rows.
        let row = painted.row_around(painted.rect_of("Tab"));
        assert!(
            !painted.rects.iter().any(|r| {
                r.stroke == theme::HAIRLINE && r.rect.contains_rect(tint.rect) && r.rect.contains_rect(row)
            }),
            "a card is drawn round the SEQUENCE band and its rows"
        );
    }

    /// **Every cell of a step row sits on the row's line**, and there are no
    /// controls on it: a handle, an index, a chip, the step, and nothing
    /// after it -- no rate at the far end and no sentence beside the step,
    /// which the owner asked be taken off the row ("remove these").
    #[test]
    fn every_cell_of_a_step_row_is_on_the_rows_line_and_no_control_is() {
        let painted = Modal::over(WINDOWS[0]).frame(&mut draft());
        // Row two: `Tab`, a keycap. Its index, its `KEY` chip and its `—`.
        let keycap = painted.rect_of("Tab");
        let row = painted.row_around(keycap);
        let on_row = |text: &str| -> egui::Rect {
            let found: Vec<egui::Rect> = painted
                .rects_of(text)
                .into_iter()
                .filter(|r| row.contains_rect(*r))
                .collect();
            assert_eq!(found.len(), 1, "expected one {text:?} on the Tab row, found {}", found.len());
            found[0]
        };
        // Against the ROW's middle, with two points of room: what is painted
        // on one line is each face's ink, and what a galley reports is its
        // box -- ascent plus descent, which an 11-point bold face and a
        // 12-point mono face divide differently. Measured: `KEY`'s box
        // centred on 425 and `Tab`'s on 427, both inks on 425, the row on
        // 426. Three points was the defect; two is the box.
        for text in ["2", "KEY", "Tab"] {
            let rect = on_row(text);
            assert!(
                (rect.center().y - row.center().y).abs() <= 2.0,
                "{text:?} at {rect:?} is off the row's line at {row:?}"
            );
        }
        let grip = painted.grip_of(row);
        assert!((grip.center().y - row.center().y).abs() <= 1.0, "the handle is off the line");
        // In 4a's order across the row.
        let (index, kind) = (on_row("2"), on_row("KEY"));
        assert!(grip.right() <= index.left() && index.right() <= kind.left());
        assert!(kind.right() <= keycap.left());
        // The far cell and the explanation are gone: no dash at the end of a
        // row that has no rate, no rate on the one that has, and no words
        // beside the step saying what it does.
        for gone in ["\u{2014}", "ms/char", "submit", detail_edit::MODIFIER_NOTE] {
            assert!(
                !painted.strings().iter().any(|painted| painted.contains(gone)),
                "the row still carries {gone:?}: {:?}",
                painted.strings()
            );
        }
        for control in ["<", ">", "x"] {
            assert!(
                painted.rects_of(control).is_empty(),
                "the row still carries a {control:?} control"
            );
        }
        assert!(painted.strings().contains(&REORDER_HINT), "the list does not say how it is edited");
    }

    /// **Dragging a step's handle onto another row moves it there.** The
    /// third of five, `Password`, carried up over `Tab` and dropped in the
    /// upper half of `Tab`'s row: the slot above `Tab`.
    #[test]
    fn dragging_a_steps_handle_over_another_row_moves_it_there() {
        let modal = Modal::over(WINDOWS[0]);
        let mut draft = draft();
        let painted = modal.frame(&mut draft);
        let from = painted.row_around(painted.rect_of("Password"));
        let onto = painted.row_around(painted.rect_of("Tab"));
        let grip = painted.grip_of(from);
        let to = egui::pos2(grip.center().x, onto.top() + onto.height() * 0.25);
        let after = modal.drag(&mut draft, grip.center(), to);
        assert_eq!(
            draft.sequence, "{USERNAME}{PASSWORD}{TAB}{DELAY 250}{ENTER}",
            "the drop did not move the step. Painted: {:?}",
            after.strings()
        );
        // And the moved step is the selection, so the keyboard can carry on
        // with it -- where it IS: dropped beside `Username`, the two fields
        // are one text step, and that step is the first.
        assert_eq!(draft.selected, Some(0));
    }

    /// **A drag released off the list moves nothing**: the payload is left
    /// for egui to clear, and the string is untouched.
    #[test]
    fn a_drag_released_off_the_list_moves_nothing() {
        let modal = Modal::over(WINDOWS[0]);
        let mut draft = draft();
        let painted = modal.frame(&mut draft);
        let before = draft.sequence.clone();
        let from = painted.row_around(painted.rect_of("Password"));
        let grip = painted.grip_of(from);
        let card = painted.card();
        let _ = modal.drag(&mut draft, grip.center(), egui::pos2(card.left() - 30.0, grip.center().y));
        assert_eq!(draft.sequence, before);
    }

    /// **A clicked row is the selection, and Delete takes it away**; Alt+Down
    /// moves it. The keyboard's parity with the drag, driven through real
    /// key events.
    #[test]
    fn a_selected_step_is_removed_by_delete_and_moved_by_alt_and_an_arrow() {
        let modal = Modal::over(WINDOWS[0]);
        let mut draft = draft();
        let painted = modal.frame(&mut draft);
        let row = painted.row_around(painted.rect_of("Tab"));
        // Click the row's empty right end, which is the row and nothing else
        // -- the far cell that used to be there is gone.
        let _ = modal.click(&mut draft, egui::pos2(row.right() - ROW_END_INSET, row.center().y));
        assert_eq!(draft.selected, Some(1), "the click did not select the row");

        let _ = modal.key(&mut draft, egui::Key::ArrowDown, egui::Modifiers::ALT);
        assert_eq!(draft.sequence, "{USERNAME}{PASSWORD}{TAB}{DELAY 250}{ENTER}");
        // Past `Password`, which merged with `Username` behind it: the list
        // is a step shorter and `Tab` is its second, not its third.
        assert_eq!(draft.selected, Some(1), "the selection did not follow the step");

        let _ = modal.key(&mut draft, egui::Key::Delete, egui::Modifiers::NONE);
        assert_eq!(draft.sequence, "{USERNAME}{PASSWORD}{DELAY 250}{ENTER}");
        assert_eq!(draft.selected, None);
    }

    /// **A boxed run is centred in its own box.** The kind chip, the field
    /// pill and the keycap each paint their own galley, and each one's INK
    /// -- not its galley box, which a face never fills -- sits on the middle
    /// of the box round it. They inked a point over it while [`run_top`]
    /// still lifted every run to the line two egui-laid cells used to be on;
    /// those cells are gone. The owner: "pills text not centered".
    #[test]
    fn a_chip_a_pill_and_a_keycap_ink_on_the_middle_of_their_own_box() {
        let painted = Modal::over(WINDOWS[0]).frame(&mut draft());
        for run in ["WAIT", "Username", "Tab"] {
            let ink = painted.ink_of(run);
            let box_round_it = painted
                .rects
                .iter()
                .filter(|r| r.rect.contains_rect(ink) && r.rect.height() <= 30.0)
                .map(|r| r.rect)
                .min_by(|a, b| a.area().total_cmp(&b.area()))
                .unwrap_or_else(|| panic!("nothing is painted round {run:?} at {ink:?}"));
            assert!(
                (ink.center().y - box_round_it.center().y).abs() <= 0.25,
                "{run:?} inks {:.2}..{:.2} in a box {:.2}..{:.2}: {:+.2} off its middle",
                ink.top(),
                ink.bottom(),
                box_round_it.top(),
                box_round_it.bottom(),
                ink.center().y - box_round_it.center().y
            );
        }
    }

    /// **The bands butt, and their rules are the joins**: no white strip
    /// above the SEQUENCE tint and none below it -- the owner: "white spaces
    /// on top and below Sequnce".
    #[test]
    fn the_bands_butt_with_their_rules_as_the_joins() {
        let painted = Modal::over(WINDOWS[0]).frame(&mut draft());
        let tint = painted
            .rects
            .iter()
            .find(|r| r.fill == theme::CARD_TINT && r.rect.contains_rect(painted.rect_of("SEQUENCE")))
            .expect("the band's tint is painted")
            .rect;
        let rules: Vec<egui::Rect> = painted
            .rects
            .iter()
            .filter(|r| {
                r.fill == theme::HAIRLINE
                    && (r.rect.height() - 1.0).abs() < 0.1
                    && (r.rect.width() - tint.width()).abs() < 0.1
            })
            .map(|r| r.rect)
            .collect();
        let joined = |edge: f32| rules.iter().any(|r| (r.center().y - edge).abs() <= 0.6);
        assert!(joined(tint.top()), "white above the tint at {tint:?}: rules {rules:?}");
        assert!(joined(tint.bottom()), "white below the tint at {tint:?}: rules {rules:?}");
        // And the first step row hangs the band's own padding below that
        // lower rule, not padding plus a seam.
        let first = painted.row_around(painted.rect_of("Username"));
        assert!(
            (first.top() - (tint.bottom() + 1.0 + f32::from(BAND_PAD_Y))).abs() <= 0.6,
            "the rows start at {} and the rule ends at {}",
            first.top(),
            tint.bottom() + 1.0
        );
    }

    /// **The value menu opens wide enough for its pills to sit on one
    /// line**, as long as one line fits inside the card it hangs off: the
    /// owner, of a four-field palette that wrapped, "should be one line of
    /// pills since popup is not wider than Fill rule modal itlsef".
    ///
    /// Four fields, two of them secrets -- which is the case that failed:
    /// a secret's pill is BOLD and the width was measured semibold, so the
    /// menu came out a few points short and the last pill dropped.
    #[test]
    fn the_value_menu_holds_its_pills_on_one_line() {
        let palette = vec![
            FieldRef::Username,
            FieldRef::Password,
            FieldRef::Totp,
            FieldRef::Custom("Native App Filler".to_string()),
        ];
        let modal = Modal::over(WINDOWS[1]).with_palette(palette.clone());
        let mut draft = draft();
        let painted = modal.frame(&mut draft);
        let at = painted.rect_of(ADD_TEXT_LABEL).center();
        let _ = modal.click(&mut draft, at);
        let open = modal.frame(&mut draft);

        // The menu's own pills, not the step list's: a run of the same name
        // is drawn in both, and the menu is the one under the add row.
        let floor = open.rect_of(ADD_TEXT_LABEL).bottom();
        let in_the_menu = |name: &str| -> egui::Rect {
            let found: Vec<egui::Rect> =
                open.rects_of(name).into_iter().filter(|r| r.top() > floor).collect();
            assert_eq!(found.len(), 1, "expected one {name:?} in the menu, found {}", found.len());
            found[0]
        };
        let names: Vec<String> = palette.iter().map(|field| field.label()).collect();
        let first = in_the_menu(names[0].as_str());
        for name in &names[1..] {
            let pill = in_the_menu(name.as_str());
            assert!(
                (pill.center().y - first.center().y).abs() <= 1.0,
                "{name:?} at {pill:?} is not on the line {first:?} the first pill is on"
            );
        }
        // ...and the menu is no wider than the card it belongs to.
        let card = open.card();
        let last = in_the_menu(names[names.len() - 1].as_str());
        assert!(
            last.right() - first.left() <= card.width(),
            "the pills run {} wide against a card of {}",
            last.right() - first.left(),
            card.width()
        );
    }

    /// **Escape discards the builder** -- "Esc doesn't close modal" -- and
    /// **an open add menu takes the first press**: "Esc should close A value
    /// from this item popup first and second Esc exits Fill rule".
    #[test]
    fn escape_shuts_an_open_menu_first_and_the_builder_second() {
        let modal = Modal::over(WINDOWS[0]);
        let mut draft = draft();
        let painted = modal.frame(&mut draft);
        assert_eq!(
            modal.key_action(&mut draft, egui::Key::Escape, egui::Modifiers::NONE),
            BuilderAction::Discard,
            "Escape did not close the builder"
        );
        // The control: an ordinary key is not a way out.
        assert_eq!(
            modal.key_action(&mut draft, egui::Key::A, egui::Modifiers::NONE),
            BuilderAction::None
        );

        // With `+ Text` open, the first Escape is the menu's.
        let at = painted.rect_of(ADD_TEXT_LABEL).center();
        let _ = modal.click(&mut draft, at);
        assert!(
            egui::Popup::is_any_open(&modal.ctx),
            "the premise failed: clicking + Text opened no menu"
        );
        assert_eq!(
            modal.key_action(&mut draft, egui::Key::Escape, egui::Modifiers::NONE),
            BuilderAction::None,
            "Escape closed the card out from under an open menu"
        );
        // ...and once it has shut, the next one is the card's.
        assert!(!egui::Popup::is_any_open(&modal.ctx), "the menu did not take the press");
        assert_eq!(
            modal.key_action(&mut draft, egui::Key::Escape, egui::Modifiers::NONE),
            BuilderAction::Discard,
            "the second Escape did not close the builder"
        );
    }

    /// **An edited rule asks before it is thrown away**, and an untouched
    /// one just closes. The owner: "if dirty rule and Esc - should ask
    /// about being reset".
    #[test]
    fn escape_on_an_edited_rule_asks_before_it_discards() {
        let modal = Modal::over(WINDOWS[0]);

        // Untouched: Escape closes, and asks nothing.
        let mut clean = draft();
        let _ = modal.frame(&mut clean);
        assert!(!clean.changed(), "the fixture opened dirty");
        assert_eq!(
            modal.key_action(&mut clean, egui::Key::Escape, egui::Modifiers::NONE),
            BuilderAction::Discard
        );
        assert!(!clean.discard_prompt, "an untouched rule asked a question about nothing");

        // Edited: Escape asks, and does NOT close behind the question.
        let mut dirty = draft();
        dirty.sequence = format!("{}{{ENTER}}", dirty.sequence);
        let _ = modal.frame(&mut dirty);
        assert!(dirty.changed(), "the premise: the rule has been edited");
        assert_eq!(
            modal.key_action(&mut dirty, egui::Key::Escape, egui::Modifiers::NONE),
            BuilderAction::None,
            "an edited rule was thrown away without a word"
        );
        assert!(dirty.discard_prompt, "nothing was asked");
        // ...and the card behind the question does not take a second press
        // as its own: that one belongs to the question.
        assert_eq!(
            modal.key_action(&mut dirty, egui::Key::Escape, egui::Modifiers::NONE),
            BuilderAction::None
        );
    }

    /// **A wait row's duration inks on the row's line**, like the boxed cells
    /// beside it: the owner, of `1s` beside a `WAIT` chip, "1 s is not
    /// centered".
    #[test]
    fn a_waits_duration_inks_on_the_rows_line() {
        let painted = Modal::over(WINDOWS[0]).frame(&mut draft());
        let chip = painted.ink_of("WAIT");
        let duration = painted.ink_of("0.3s");
        assert!(
            (chip.center().y - duration.center().y).abs() <= 0.3,
            "the chip inks {:.2}..{:.2} and the duration {:.2}..{:.2}",
            chip.top(),
            chip.bottom(),
            duration.top(),
            duration.bottom()
        );
    }

    /// **A wait row says the duration once.** The kind chip says WAIT and
    /// the step says `0.3s`, not `Wait 0.3s`.
    #[test]
    fn a_wait_row_does_not_say_wait_twice() {
        let painted = Modal::over(WINDOWS[0]).frame(&mut draft());
        assert!(painted.strings().contains(&"WAIT"), "no wait row: {:?}", painted.strings());
        assert!(painted.strings().contains(&"0.3s"), "the duration: {:?}", painted.strings());
        assert!(
            !painted.strings().iter().any(|run| run.contains("Wait 0.3s")),
            "the row still says the word beside the chip: {:?}",
            painted.strings()
        );
        // The word itself is not wrong everywhere -- only beside the chip.
        assert_eq!(wait_duration("Wait 0.3s"), "0.3s");
        assert_eq!(wait_duration("Wait 20 ms"), "20 ms");
        // Anything not spelled that way is left exactly as it is.
        assert_eq!(wait_duration("250 ms"), "250 ms");
    }

    /// How far inside a row's right edge a click lands on the row itself:
    /// past the row's padding, and short of anything drawn in it.
    const ROW_END_INSET: f32 = 6.0;

    /// **4a's add row is three dashed buttons on one line**, in the design's
    /// order, under the last step -- and the section card that used to hold
    /// four stacked palettes is gone: "No Add a step section - just buttons
    /// like this below, that's it".
    #[test]
    fn the_add_row_is_three_dashed_buttons_under_the_list() {
        let painted = Modal::over(WINDOWS[0]).frame(&mut draft());
        let (text, key, wait) = (
            painted.rect_of(ADD_TEXT_LABEL),
            painted.rect_of(ADD_KEY_LABEL),
            painted.rect_of(ADD_WAIT_LABEL),
        );
        for (what, rect) in [("+ Key", key), ("+ Wait", wait)] {
            assert!(
                (rect.center().y - text.center().y).abs() <= 1.0,
                "{what} at {rect:?} is off the add row's line at {text:?}"
            );
        }
        assert!(text.right() < key.left() && key.right() < wait.left(), "4a's order");
        // Under the last step, and nothing titled like the card that went.
        let last = painted.row_around(painted.rect_of("Enter"));
        assert!(text.top() > last.bottom(), "the add row is not under the list");
        assert!(
            !painted.strings().iter().any(|painted| *painted == "Add a step"),
            "the Add a step card is still drawn"
        );
    }

    // -----------------------------------------------------------------------
    // 4c's line: the chips under the template
    // -----------------------------------------------------------------------

    /// One token of the template line as it was laid: the rows its glyphs
    /// landed on, the glyphs' own extent, and the ink of its capitals and
    /// digits -- the band a reader sees as the word, and the band its chip
    /// is asserted to be centred on.
    struct LaidToken {
        word: String,
        rows: Vec<usize>,
        left: f32,
        right: f32,
        caps: egui::Rect,
    }

    impl Painted {
        /// The template line's galley: the one whose text is `template`,
        /// with the space inside a wait laid as the no-break one the
        /// layouter puts there. The `INSERT` row's pills are one token each
        /// and cannot be mistaken for a template of two.
        fn template_galley(&self, template: &str) -> (egui::Pos2, &egui::Galley) {
            let found: Vec<&(egui::Pos2, std::sync::Arc<egui::Galley>)> = self
                .galleys
                .iter()
                .filter(|(_, galley)| galley.text().replace('\u{a0}', " ") == template)
                .collect();
            assert_eq!(found.len(), 1, "expected the template line once, found {}", found.len());
            (found[0].0, &found[0].1)
        }

        /// The box 4c's line is typed in: the white, hairlined,
        /// 8-radius frame round `inside`.
        fn template_box(&self, inside: egui::Rect) -> egui::Rect {
            self.rects
                .iter()
                .filter(|r| {
                    r.corners.nw == detail_edit::TEMPLATE_BOX_RADIUS
                        && r.fill == theme::CARD
                        && r.rect.contains_rect(inside)
                })
                .map(|r| r.rect)
                .min_by(|a, b| a.height().total_cmp(&b.height()))
                .unwrap_or_else(|| panic!("no template box is painted round {inside:?}"))
        }

        /// The chips painted under the line inside `template_box`, in
        /// reading order: 4c's 4-radius grounds and nothing else in the box
        /// has those corners.
        fn template_chips(&self, template_box: egui::Rect) -> Vec<egui::Rect> {
            let mut chips: Vec<egui::Rect> = self
                .rects
                .iter()
                .filter(|r| {
                    r.corners.nw == detail_edit::TEMPLATE_TOKEN_RADIUS
                        && template_box.contains_rect(r.rect)
                })
                .map(|r| r.rect)
                .collect();
            chips.sort_by(|a, b| (a.top(), a.left()).partial_cmp(&(b.top(), b.left())).unwrap());
            chips
        }

        /// Every `{...}` token of the template line, laid. See [`LaidToken`].
        fn laid_tokens(&self, template: &str) -> Vec<LaidToken> {
            let (pos, galley) = self.template_galley(template);
            // Character index -> (row, glyph), the way the layouter counts:
            // a newline is a character and no glyph.
            let mut laid = Vec::new();
            for (r, row) in galley.rows.iter().enumerate() {
                laid.extend(row.row.glyphs.iter().map(|g| Some((r, g))));
                if row.ends_with_newline {
                    laid.push(None);
                }
            }
            let chars: Vec<char> = galley.text().chars().collect();
            let mut tokens = Vec::new();
            let mut i = 0;
            while i < chars.len() {
                let Some(close) = (chars[i] == '{').then(|| chars[i..].iter().position(|c| *c == '}')).flatten()
                else {
                    i += 1;
                    continue;
                };
                let (from, to) = (i, i + close + 1);
                let mut token = LaidToken {
                    word: chars[from..to].iter().collect(),
                    rows: Vec::new(),
                    left: f32::INFINITY,
                    right: f32::NEG_INFINITY,
                    caps: egui::Rect::NOTHING,
                };
                for (r, g) in laid[from..to].iter().flatten() {
                    let row = &galley.rows[*r];
                    let origin = pos + row.pos.to_vec2();
                    token.rows.push(*r);
                    token.left = token.left.min(origin.x + g.pos.x);
                    token.right = token.right.max(origin.x + g.max_x());
                    if (g.chr.is_ascii_uppercase() || g.chr.is_ascii_digit()) && !g.uv_rect.is_nothing() {
                        let at = origin + g.pos.to_vec2() + g.uv_rect.offset;
                        token.caps = token.caps.union(egui::Rect::from_min_size(at, g.uv_rect.size));
                    }
                }
                token.rows.dedup();
                tokens.push(token);
                i = to;
            }
            tokens
        }
    }

    /// The builder with the template view on and `template` in the box.
    fn template_frame(modal: &Modal, template: &str) -> Painted {
        let mut draft = draft();
        draft.template_view = true;
        draft.template_draft = template.into();
        draft.sequence = template.into();
        // **Clicked into, so the box has focus.** The caret is the box's own
        // now -- egui's is switched off, see `detail_edit::template_editor`
        // -- and a caret is only painted for a box that has focus, which is
        // also the only state in which its height is a claim worth making.
        let first = modal.frame(&mut draft);
        let (pos, galley) = first.template_galley(template);
        let line = egui::Rect::from_min_size(pos, galley.size());
        let box_rect = first.template_box(line);
        let _ = modal.click(&mut draft, box_rect.center());
        modal.frame(&mut draft)
    }

    /// **Each chip sits round its own token**: centred on the word's cap
    /// band, from its first glyph to its last plus 4c's padding, with the
    /// box's own white between it and the next -- and the line it is on is
    /// the face's own row, which is what the caret is drawn at.
    ///
    /// Two numbers here were wrong and are pinned. The chip's middle was
    /// 4.5 points under its token's (the chips were centred on a line box
    /// whose extra height egui puts entirely under the ink -- "text not
    /// centered again"), and two adjacent chips overlapped by 8 (a token's
    /// end was read as the next glyph's start, past the gap between them
    /// -- "pills gets one on each other"). The owner asked for the hairline
    /// by name: "make sure there is a white hairline between pills
    /// horizontally". `{DELAY 3000}` is here for its space: "delay should
    /// also be gray pill".
    #[test]
    fn the_template_chips_sit_round_their_tokens() {
        let template = "{USERNAME}{ENTER}{DELAY 3000}{PASSWORD}";
        let modal = Modal::over(WINDOWS[1]);
        let painted = template_frame(&modal, template);
        let tokens = painted.laid_tokens(template);
        let (pos, galley) = painted.template_galley(template);
        let line = egui::Rect::from_min_size(pos, galley.size());
        let bx = painted.template_box(line);
        let chips = painted.template_chips(bx);
        assert_eq!(tokens.len(), 4);
        assert_eq!(chips.len(), tokens.len(), "one chip per token: {chips:?}");
        for (token, chip) in tokens.iter().zip(&chips) {
            assert_eq!(token.rows.len(), 1, "{} is on more than one row", token.word);
            assert!(
                (chip.center().y - token.caps.center().y).abs() <= 0.5,
                "{}'s chip {:.2}..{:.2} is {:+.2} off its caps {:.2}..{:.2}",
                token.word,
                chip.top(),
                chip.bottom(),
                chip.center().y - token.caps.center().y,
                token.caps.top(),
                token.caps.bottom(),
            );
            assert!(
                (chip.left() - (token.left - detail_edit::TEMPLATE_TOKEN_PAD_X)).abs() <= 0.01
                    && (chip.right() - (token.right + detail_edit::TEMPLATE_TOKEN_PAD_X)).abs() <= 0.01,
                "{}'s chip {:.2}..{:.2} is not its glyphs {:.2}..{:.2} plus the padding",
                token.word,
                chip.left(),
                chip.right(),
                token.left,
                token.right,
            );
        }
        let hairline = detail_edit::TEMPLATE_TOKEN_GAP - 2.0 * detail_edit::TEMPLATE_TOKEN_PAD_X;
        for pair in chips.windows(2) {
            let white = pair[1].left() - pair[0].right();
            assert!(
                white >= 1.0 && white <= hairline + 0.5,
                "{white:.2} between {:?} and {:?}: not the hairline of about {hairline}",
                pair[0],
                pair[1]
            );
        }
        // **The row is a chip and the air it leaves the next row** -- it has
        // to be, now that the chips are taller than the face's own line
        // ("pills should be higher as per design") -- and the CARET is no
        // longer that row: the box draws its own at the chip's height, which
        // is what three rounds of "text cursor is still way to big" settled.
        let row = modal.ctx.fonts_mut(|f| f.row_height(&detail_edit::template_font()));
        assert!(
            galley.rows[0].height() > row,
            "the row is {:.2} in a {row:.2} face: it cannot hold a chip",
            galley.rows[0].height()
        );
        let caret = painted
            .rects
            .iter()
            .filter(|rect| {
                rect.fill == theme::INK
                    && (rect.rect.width() - detail_edit::TEMPLATE_CARET_WIDTH).abs() < 0.1
            })
            .map(|rect| rect.rect)
            .next()
            .expect("the focused box paints no caret");
        assert!(
            (caret.height() - chips[0].height()).abs() <= 0.25,
            "the caret is {:.2} tall against a chip of {:.2}",
            caret.height(),
            chips[0].height()
        );
        // 4c's padding 12, measured to the chip, the same both ways.
        let (above, below) = (chips[0].top() - bx.top(), bx.bottom() - chips[0].bottom());
        assert!(
            (above - below).abs() <= 0.25 && (above - 13.0).abs() <= 0.25,
            "{above:.2} over the chip and {below:.2} under it: not 12 inside a one-point stroke"
        );
    }

    /// **A token wraps whole, and its chip with it**, over every length from
    /// one row to two. epaint breaks a spaceless line at the latest
    /// punctuation and `{` is punctuation: seven `{TAB}`s before an
    /// `{ENTER}` left `{` on one row and `ENTER}` on the next, each with
    /// half a chip -- the owner: "make sure only full pill goes there and
    /// there is enough space in between of lines to draw pills". Two rows
    /// of chips must not touch, and the air over the first row is the air
    /// under the last.
    #[test]
    fn a_token_wraps_whole_and_its_chip_with_it() {
        let modal = Modal::over(WINDOWS[1]);
        let mut wrapped = 0;
        for tabs in 0..16 {
            let template = format!("{{USERNAME}}{}{{PASSWORD}}{{ENTER}}", "{TAB}".repeat(tabs));
            let painted = template_frame(&modal, &template);
            let tokens = painted.laid_tokens(&template);
            let (pos, galley) = painted.template_galley(&template);
            let bx = painted.template_box(egui::Rect::from_min_size(pos, galley.size()));
            let chips = painted.template_chips(bx);
            assert_eq!(tokens.len(), tabs + 3);
            for token in &tokens {
                assert_eq!(token.rows.len(), 1, "{} is split across rows {:?} with {tabs} tabs", token.word, token.rows);
            }
            assert_eq!(chips.len(), tokens.len(), "a token without its chip with {tabs} tabs");
            for (a, b) in chips.iter().flat_map(|a| chips.iter().map(move |b| (a, b))) {
                if a.top() < b.top() {
                    assert!(
                        b.top() - a.bottom() >= 0.5,
                        "chips {a:?} and {b:?} touch across rows with {tabs} tabs"
                    );
                }
            }
            let last = chips.last().unwrap();
            let (above, below) = (chips[0].top() - bx.top(), bx.bottom() - last.bottom());
            assert!(
                (above - below).abs() <= 0.25,
                "{above:.2} over the first chip and {below:.2} under the last with {tabs} tabs"
            );
            if galley.rows.len() > 1 {
                wrapped += 1;
            }
        }
        assert!(wrapped >= 8, "only {wrapped} of the sixteen templates wrapped: the case is not exercised");
    }
}

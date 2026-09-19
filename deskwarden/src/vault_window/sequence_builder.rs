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
    self, sequence_tally, step_rows, ChipEdit, StepKind, StepRow,
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
                    label.push_str(&m);
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
/// `10px` on a wait row -- the explanation's `padding-left: 4px`, and the
/// space between the middle's lines when it wraps (not 4a's; it never
/// wraps).
const STEP_KEY_GAP: f32 = 7.0;
const STEP_TEXT_GAP: f32 = 8.0;
const STEP_WAIT_GAP: f32 = 10.0;
const STEP_ASIDE_INSET: f32 = 4.0;
const STEP_WRAP_GAP: f32 = 3.0;

/// The runs: `font-size: 12px` on the explanation, the value and the far
/// cell's dash, mono `13px` on a wait, mono `11px` on a rate.
const STEP_TEXT_PX: f32 = 12.0;
const STEP_WAIT_PX: f32 = 13.0;
const STEP_RATE_PX: f32 = 11.0;

/// Between the row's three move-and-remove controls, which 4a does not draw.
const STEP_CONTROL_GAP: f32 = 6.0;

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
        },
        |ui| {
            // **The body scrolls and the CARD does not grow.** A sequence of
            // twenty steps would otherwise make a modal taller than the window
            // it floats over. `auto_shrink` is off on BOTH axes: on the y it
            // does not mean "cap at `max_height`", it means "be as tall as
            // your content", which is the opposite -- measured once already at
            // 842 points in a 740-point window.
            egui::ScrollArea::vertical()
                .max_height(height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    destination_band(ui, draft, item_icon, app_icon.as_ref());
                    ui.add_space(theme::SECTION_GAP);
                    steps_column(ui, draft, palette, source);
                });
        },
        |ui| theme::primary_button_enabled(ui, label, Some("CTRL+S"), saveable),
    );

    if chorded || press.confirmed {
        BuilderAction::Save
    } else if press.dismissed {
        BuilderAction::Discard
    } else {
        BuilderAction::None
    }
}

/// How wide the card is against this window.
///
/// **Narrower than 4a**, at the owner's word. The design is drawn at 1240
/// points across two columns; this is one column in a modal, and a modal has
/// to leave the window it floats over visible at the edges. 640 is a form's
/// width -- the widest thing in it is a step row -- and the `min` is what
/// keeps it inside the 900-point window this app can open at.
const CARD_WIDTH: f32 = 640.0;
const CARD_GUTTER: f32 = 48.0;

fn card_width(ctx: &egui::Context) -> f32 {
    CARD_WIDTH.min((ctx.content_rect().width() - 2.0 * CARD_GUTTER).max(320.0))
}

/// The tallest the body may be before it scrolls.
///
/// **Taller than it was**, also at the owner's word: the card is narrower, so
/// the steps need the room back vertically. What is subtracted is the chrome
/// the body sits between -- `theme::modal_card`'s header band and its footer,
/// plus the gutter that keeps the card off the window's edges.
const CARD_CHROME: f32 = 132.0;

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
            // 4a's `align-items: center`, which in egui means giving each
            // line its height before anything is placed on it -- see
            // [`centred_row`] for the measurement.
            match form {
                BandForm::OneLine(chips_go) => {
                    let height = item.size(ui, ChipsGo::Beside).y.max(app.size(ui, chips_go).y);
                    centred_row(ui, height, |ui| {
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
                        centred_row(ui, app.size(ui, chips_go).y, |ui| {
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

    /// What the whole subject lays out as: the tile, its gap, and the column
    /// beside it, as tall as the taller of the two.
    fn size(&self, ui: &egui::Ui, go: ChipsGo) -> egui::Vec2 {
        let column = self.metrics(ui, go).column();
        egui::vec2(BAND_TILE + BAND_TILE_GAP + column.x, BAND_TILE.max(column.y))
    }

    fn draw(&self, ui: &mut egui::Ui, go: ChipsGo) {
        let metrics = self.metrics(ui, go);
        let column = metrics.column();
        let size = egui::vec2(BAND_TILE + BAND_TILE_GAP + column.x, BAND_TILE.max(column.y));
        // The subject's own row at its own size, and the column inside it at
        // ITS own size: a `vertical` of unknown height dropped into a centred
        // row is placed by the height egui guesses for it and grows downward
        // from there, which is the tile a third of the way up its column
        // that the probe found (the app's tile at 192..226 against a column
        // running 192..245).
        ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = BAND_TILE_GAP;
            match self.icon {
                Some(texture) => {
                    let tile = theme::avatar_artwork_tile(ui, BAND_TILE, self.accent);
                    theme::avatar_image(ui, tile, texture, self.accent);
                }
                None => theme::avatar(ui, &theme::initials(self.name), BAND_TILE, self.accent),
            }
            ui.allocate_ui_with_layout(column, egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.spacing_mut().item_spacing.y = BAND_COLUMN_GAP;
                ui.add(egui::Label::new(self.caption_job()).extend());
                let line = metrics.line();
                ui.allocate_ui_with_layout(
                    line,
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = BAND_NAME_GAP;
                        ui.add(
                            egui::Label::new(
                                theme::bold(self.name, BAND_NAME_PX).color(theme::INK),
                            )
                            .extend(),
                        );
                        match (&self.beside, go) {
                            (BandBeside::Folder(folder), _) if !folder.is_empty() => {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(*folder)
                                            .size(BAND_FOLDER_PX)
                                            .color(theme::TEXT_FAINT),
                                    )
                                    .extend(),
                                );
                            }
                            (BandBeside::Chips(chips), ChipsGo::Beside) => {
                                for chip in *chips {
                                    band_chip(ui, chip);
                                }
                            }
                            _ => {}
                        }
                    },
                );
                if let (BandBeside::Chips(chips), ChipsGo::Under) = (&self.beside, go) {
                    ui.add_space(BAND_CHIPS_UNDER_GAP - BAND_COLUMN_GAP);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = BAND_NAME_GAP;
                        for chip in *chips {
                            band_chip(ui, chip);
                        }
                    });
                }
            });
        });
    }
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

    /// The column: caption over line over whatever is under, at 4a's `gap:
    /// 2px` and [`BAND_CHIPS_UNDER_GAP`].
    fn column(&self) -> egui::Vec2 {
        let line = self.line();
        let under = if self.under.y > 0.0 { BAND_CHIPS_UNDER_GAP + self.under.y } else { 0.0 };
        egui::vec2(
            self.caption.x.max(line.x).max(self.under.x),
            self.caption.y + BAND_COLUMN_GAP + line.y + under,
        )
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

/// The size of `text` set in `font` on one line: what an `extend`ed label of
/// it is laid out as.
fn run_size(ui: &egui::Ui, text: &str, font: egui::FontId) -> egui::Vec2 {
    ui.painter().layout_no_wrap(text.to_string(), font, egui::Color32::BLACK).size()
}

/// 4a's mono chip beside a name in the band: `font-size: 11px; background:
/// #f3f2f2; border-radius: 5px; padding: 2px 7px`.
fn band_chip(ui: &mut egui::Ui, text: &str) {
    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        egui::FontId::new(BAND_CHIP_PX, egui::FontFamily::Monospace),
        theme::TEXT_SECONDARY,
    );
    let size = egui::vec2(
        galley.size().x + BAND_CHIP_PAD_X * 2.0,
        galley.size().y + BAND_CHIP_PAD_Y * 2.0,
    );
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(BAND_CHIP_RADIUS), theme::CANVAS);
    ui.painter().galley(
        egui::pos2(rect.left() + BAND_CHIP_PAD_X, rect.top() + BAND_CHIP_PAD_Y),
        galley,
        theme::TEXT_SECONDARY,
    );
}

/// What [`band_chip`] will allocate for `text`, for [`BandSubject::metrics`].
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
fn steps_column(
    ui: &mut egui::Ui,
    draft: &mut SequenceDraft,
    palette: &[FieldRef],
    source: &ResolveSource<'_>,
) {
    theme::section_card(ui, |ui| {
        ui.set_width(ui.available_width());
        let summary = match sequence_tally(&draft.sequence, source) {
            Some(tally) => detail_edit::tally_short(&tally),
            None => detail_edit::TALLY_REFUSED.to_string(),
        };
        if let Some(wants_template) = sequence_band(ui, &summary, draft.template_view) {
            // Seeded verbatim, every time the view is entered -- so a user
            // who opens the template view and closes it again has changed
            // nothing at all.
            if wants_template {
                draft.template_draft = draft.sequence.clone();
            }
            draft.template_view = wants_template;
        }
        // At the band's `20`, not the section card's own 12: 4a's rows sit
        // in `padding: 16px 20px`, under a band padded the same.
        theme::section_card_body_at(ui, BAND_PAD_X, |ui| {
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
                    source,
                    |ui, rows| {
                        let _ = step_list(ui, rows, false);
                    },
                );
            } else if let Some(edit) = step_list(
                ui,
                &step_rows(&draft.sequence, source, draft.revealing),
                true,
            ) {
                draft.sequence = detail_edit::apply_chip_edit(&draft.sequence, edit);
            }
        });
    });
    ui.add_space(theme::SECTION_GAP);

    // The palette belongs to the step list. In the template view the insert
    // chips ARE the palette, and a second set of Add buttons writing to a
    // string the user is editing by hand would fight the cursor.
    if !draft.template_view {
        theme::section_card(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section_card_header(ui, "Add a step", "", false);
            theme::section_card_body(ui, |ui| {
                ui.set_width(ui.available_width());
                palette_body(ui, draft, palette);
            });
        });
        ui.add_space(theme::SECTION_GAP);
    }
}

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
/// color: #9b9797`, a `flex: 1` spacer, and the Steps/Template pill.
///
/// The pill is [`detail_edit::view_toggle`] as it stands, which is already
/// 4a's declaration (`border: 1px solid #d7d3d3; border-radius: 7px`, the
/// lit cell `#1b3fa0` behind white at `4px 11px`) drawn through the crate's
/// one segmented control -- see its doc for the two points of height it keeps
/// over the design, and why. It moved up here from the body, where it sat
/// over the first row; the edit form's inline builder still draws its own
/// copy below the tally, and that one stays where it is because that form
/// has no band to put it in.
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
        // The card's own rounding on the two corners this band owns, and none
        // on the two it shares with the rows: a square tint over a rounded
        // card would show at both top corners.
        .corner_radius(CornerRadius {
            nw: theme::SECTION_CARD_RADIUS,
            ne: theme::SECTION_CARD_RADIUS,
            sw: 0,
            se: 0,
        })
        .inner_margin(Margin::symmetric(SEQUENCE_BAND_PAD_X, SEQUENCE_BAND_PAD_Y))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // At the pill's height, which is the band's: 4a's `align-items:
            // center` puts the caption on the pill's middle, and egui does
            // that only for a row that knows its height first.
            centred_row(ui, theme::SEGMENT_HEIGHT, |ui| {
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
                    wants = detail_edit::view_toggle(ui, template_view);
                });
            });
        });
    theme::hairline(ui);
    wants
}

// ---------------------------------------------------------------------------
// 4a's step rows
// ---------------------------------------------------------------------------

/// The step list as 4a draws it: one row per token, and the controls that
/// move and remove it.
///
/// **The builder's own, and not [`detail_edit::sequence_steps`].** That list
/// is drawn in the edit form's `Fill rule` card at a pane 298 points wide,
/// and 4a's row does not fit there: the handle, the index, the 52-point kind
/// chip, the three `gap: 12px` between them and the `padding: 11px 14px`
/// round them come to 150 before a step is drawn, and that card's rows have
/// about 200. One function drawing both would be two layouts behind an `if`,
/// which is two idioms with one name -- so the form keeps its wrapped line,
/// and this list is 4a's. The two share everything that is not geometry:
/// [`detail_edit::step_rows`] decides what a row says, [`ChipEdit`] what its
/// controls do, and [`detail_edit::apply_chip_edit`] what that does to the
/// string.
///
/// The owner, with the old rows on screen: "draggable tiles are also
/// different in design". They were -- an index, a badge and the step's words
/// on one wrapped line, with the note under it.
///
/// `editable` is false in the template view, where the list is the read-out
/// of what the string became rather than the thing being edited. Returns the
/// one edit clicked, applied by the caller after the loop.
fn step_list(ui: &mut egui::Ui, rows: &[StepRow], editable: bool) -> Option<ChipEdit> {
    let mut edit = None;
    ui.scope(|ui| {
        // 4a's `gap: 8px` between rows, in place of the body's spacing.
        ui.spacing_mut().item_spacing.y = STEP_ROW_GAP;
        for row in rows {
            if let Some(pressed) = step_row(ui, row, rows.len(), editable) {
                edit = Some(pressed);
            }
        }
    });
    edit
}

/// One of 4a's rows: `display: flex; align-items: center; gap: 12px;
/// background: #ffffff; border: 1px solid #eae7e7; border-radius: 10px;
/// padding: 11px 14px`. The handle, the index and the kind chip at fixed
/// widths, the step in the `flex: 1` middle, and the far cell at the end.
///
/// **The far end is laid first, right to left, and the middle takes what is
/// left.** That is 4a's `flex: 1` without measuring the trailing cells by
/// hand: the far cell and the three controls claim their own widths, and the
/// nested left-to-right scope inside them is handed the remainder as a lane.
/// The middle wraps inside that lane (see [`step_middle`]), so a row that is
/// too long for the card grows downward and its other cells stay on its
/// first line -- the prefix reads with the step, and the explanation hangs
/// under it.
///
/// A secret step wears [`theme::secret_band`], 4a's fifth row: the danger
/// wash and edge, the index in [`theme::DANGER_QUIET`] (`#a2554d`), the kind
/// chip in [`theme::ERROR`] behind white, the pill the same, and its runs in
/// [`theme::SECRET_INK`]. The hatch is still not drawn -- see
/// `detail_edit::SECRET_STEP_RADIUS` for the argument -- and the sub-band
/// under it (`Sends only if the focused control is a masked field`) is not:
/// that gate is `preflight`'s and is not a per-step setting in this build.
///
/// The controls at the row's end are not 4a's, which has none: its rows are
/// dragged. This app's are moved by `<` and `>` and taken away by `x`, and
/// the handle is drawn as 4a's mark for a row that moves; it is not yet a
/// drag source, and a drag on it does nothing.
fn step_row(ui: &mut egui::Ui, row: &StepRow, count: usize, editable: bool) -> Option<ChipEdit> {
    let mut edit = None;
    let ground = if row.secret {
        theme::secret_band()
    } else {
        egui::Frame::new().fill(theme::CARD).stroke(Stroke::new(1.0, theme::HAIRLINE))
    };
    ground
        .corner_radius(CornerRadius::same(STEP_ROW_RADIUS))
        .inner_margin(Margin::symmetric(STEP_ROW_PAD_X, STEP_ROW_PAD_Y))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let height = step_row_height(ui, editable);
            centred_row(ui, height, |ui| {
                ui.spacing_mut().item_spacing.x = STEP_ROW_GAP_X;
                step_grip(ui, row.secret);
                step_index(ui, row);
                step_kind_chip(ui, row);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if editable {
                        // Right to left, so they read `< > x` on the screen.
                        ui.spacing_mut().item_spacing.x = STEP_CONTROL_GAP;
                        let index = row.number - 1;
                        if ui.add(detail_edit::small_chip_button("x")).clicked() {
                            edit = Some(ChipEdit::Remove(index));
                        }
                        if ui
                            .add_enabled(row.number < count, detail_edit::small_chip_button(">"))
                            .clicked()
                        {
                            edit = Some(ChipEdit::Forward(index));
                        }
                        if ui.add_enabled(index > 0, detail_edit::small_chip_button("<")).clicked()
                        {
                            edit = Some(ChipEdit::Back(index));
                        }
                    }
                    ui.spacing_mut().item_spacing.x = STEP_ROW_GAP_X;
                    step_far_cell(ui, row);
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        step_middle(ui, row, height);
                    });
                });
            });
        });
    edit
}

/// The row's band: its tallest cell, known before the row is laid so every
/// cell is centred on the same line (see [`centred_row`]). The controls when
/// the row has them, and otherwise the field pill, which stands a point over
/// the keycap; the middle may still grow past this by wrapping, and then it
/// grows downward from the line the other cells are on.
fn step_row_height(ui: &egui::Ui, editable: bool) -> f32 {
    let control = if editable { detail_edit::small_chip_button_height(ui) } else { 0.0 };
    let row = |px: f32, family: &str| {
        ui.ctx()
            .fonts_mut(|f| f.row_height(&egui::FontId::new(px, egui::FontFamily::Name(family.into()))))
    };
    let pill = row(FIELD_PILL_PX, theme::BOLD) + FIELD_PILL_PAD_Y * 2.0 + 2.0;
    let keycap = row(KEYCAP_PX, theme::MONO_BOLD) + KEYCAP_PAD_Y * 2.0 + KEYCAP_EDGE + KEYCAP_FOOT;
    control.max(pill).max(keycap)
}

/// The face the row's egui-laid runs are set in -- the explanation, the far
/// cell -- and therefore the line every hand-painted run has to land on.
fn step_line_font() -> egui::FontId {
    egui::FontId::proportional(STEP_TEXT_PX)
}

/// **Where a hand-painted galley's top goes so its ink sits on the row's
/// line.** [`theme::face_ink_middle`] puts this face's cap-middle where asked,
/// and [`theme::line_lift`] asks for the point above the geometric middle
/// where egui puts every label's -- the rule `detail_edit::sequence_chip`
/// settled in three attempts, reused rather than re-derived.
fn run_top(ui: &egui::Ui, middle: f32, font: &egui::FontId) -> f32 {
    middle - theme::face_ink_middle(ui, font) - theme::line_lift(ui, &step_line_font())
}

/// 4a's drag handle: a `width: 20px` column of three bars, `width: 12px;
/// height: 2px; border-radius: 2px; background: #d7d3d3`, `gap: 3px`.
///
/// Drawn, not a glyph, for the crate's standing reason: `⋮` resolves to
/// nothing in Archivo and a mark out of a fallback face brings its own weight
/// and baseline. On the secret row 4a tints the bars `#e0b2ac`, which is not
/// in the palette; [`theme::DANGER_EDGE`] is the band's own edge, one shade
/// over, and the band's edge is what the bars are.
fn step_grip(ui: &mut egui::Ui, secret: bool) {
    let height = STEP_GRIP_BAR.y * 3.0 + STEP_GRIP_GAP * 2.0;
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(STEP_GRIP_WIDTH, height), egui::Sense::hover());
    let color = if secret { theme::DANGER_EDGE } else { theme::BORDER_STRONG };
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
}

/// The index: mono `font-size: 12px; color: #9b9797` in a `width: 14px`
/// column, so the chips after it line up down the list whatever the count.
fn step_index(ui: &mut egui::Ui, row: &StepRow) {
    let ink = if row.secret { theme::DANGER_QUIET } else { theme::TEXT_GHOST };
    let font = egui::FontId::new(STEP_INDEX_PX, egui::FontFamily::Monospace);
    let galley = ui.painter().layout_no_wrap(row.number.to_string(), font.clone(), ink);
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
fn step_kind_chip(ui: &mut egui::Ui, row: &StepRow) {
    let (fill, ink) = match row.kind {
        _ if row.secret => (theme::ERROR, egui::Color32::WHITE),
        StepKind::Text => (theme::BLUE_WASH, theme::BLUE_DEEP),
        StepKind::Wait | StepKind::Rate => (theme::CANVAS, theme::TEXT_FAINT),
        StepKind::Key | StepKind::Raw => (theme::CANVAS, theme::TEXT_SECONDARY),
    };
    let font = egui::FontId::new(STEP_KIND_PX, egui::FontFamily::Name(theme::BOLD.into()));
    let job = theme::letterspaced(
        row.kind.badge(),
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

/// The `flex: 1` middle: the step itself, what it resolves to, and the
/// explanation beside it.
///
/// Wrapped, at 4a's own gap for the kind (`7px` on a key row, `8px` on a
/// text row, `10px` on a wait row), so a row longer than the lane drops its
/// explanation under the step rather than pushing the far cell off the card.
/// Measured: the secret row -- pill, mask, `hidden — never shown here`, then
/// `3 ms/char` and the three controls -- is 630 in the card's 600, so at this
/// width that explanation is on a second line. The step's own runs are
/// `extend`ed and never break mid-word; only the sentence flows.
fn step_middle(ui: &mut egui::Ui, row: &StepRow, height: f32) {
    let gap = match row.kind {
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
        match row.kind {
            StepKind::Key => keycap(ui, &row.label),
            StepKind::Text => match &row.field {
                Some(field) => field_pill(ui, field, &row.label, row.secret),
                // A literal: the text itself, in the row's own ink. 4a draws
                // no literal, and a pill round typed text would make it look
                // like a field.
                None => {
                    ui.add(plain(&row.label, theme::INK));
                }
            },
            // 4a's `250 ms`: mono `font-size: 13px; font-weight: 600`. The
            // words are the row's own -- `Wait 0.3s`, in the seconds the
            // owner asked to set waits in -- rather than the design's.
            StepKind::Wait => {
                ui.add(
                    egui::Label::new(
                        RichText::new(row.label.clone())
                            .size(STEP_WAIT_PX)
                            .family(egui::FontFamily::Name(theme::MONO_BOLD.into()))
                            .color(theme::INK),
                    )
                    .extend(),
                );
            }
            StepKind::Rate => {
                ui.add(plain(&row.label, theme::INK));
            }
            StepKind::Raw => {
                let label = ui.add(plain(&row.label, theme::TEXT_FAINT));
                if !row.understood {
                    label.on_hover_text(detail_edit::SEQUENCE_UNKNOWN_TIP);
                }
            }
        }
        if !row.payload.is_empty() {
            // 4a's value after the pill: `font-size: 12px; color: #9b9797`
            // for a value, and the mask in mono `#8c3c33` on the secret row.
            let text = RichText::new(row.payload.clone()).size(STEP_TEXT_PX);
            let text = if row.secret {
                text.family(egui::FontFamily::Monospace).color(theme::SECRET_INK)
            } else {
                text.color(theme::TEXT_GHOST)
            };
            ui.add(egui::Label::new(text).extend());
        }
        if !row.aside.is_empty() {
            // 4a's explanation: `font-size: 12px; color: #7d7979; padding-left:
            // 4px` -- the inset spent as space beside the gap egui has already
            // put in, because netting it the other way is a negative space.
            ui.add_space(STEP_ASIDE_INSET);
            let ink = if row.secret { theme::SECRET_INK } else { theme::TEXT_FAINT };
            ui.add(
                egui::Label::new(RichText::new(row.aside.clone()).size(STEP_TEXT_PX).color(ink))
                    .wrap(),
            );
        }
    });
}

/// The far cell: 4a's `—` at `font-size: 12px; color: #9b9797` on a row with
/// no rate, and the rate in mono `font-size: 11px; color: #7d7979` on a text
/// row (`#8c3c33` on the secret one).
fn step_far_cell(ui: &mut egui::Ui, row: &StepRow) {
    let text = if row.note == detail_edit::NO_NOTE {
        RichText::new(detail_edit::NO_NOTE).size(STEP_TEXT_PX).color(theme::TEXT_GHOST)
    } else {
        let ink = if row.secret { theme::SECRET_INK } else { theme::TEXT_FAINT };
        RichText::new(row.note.clone())
            .size(STEP_RATE_PX)
            .family(egui::FontFamily::Monospace)
            .color(ink)
    };
    ui.add(egui::Label::new(text).extend());
}

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
    let font = egui::FontId::new(KEYCAP_PX, egui::FontFamily::Name(theme::MONO_BOLD.into()));
    let galley = ui.painter().layout_no_wrap(text.to_string(), font.clone(), theme::INK);
    let size = egui::vec2(
        galley.size().x + KEYCAP_PAD_X * 2.0 + KEYCAP_EDGE * 2.0,
        galley.size().y + KEYCAP_PAD_Y * 2.0 + KEYCAP_EDGE + KEYCAP_FOOT,
    );
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(KEYCAP_RADIUS), theme::BORDER_STRONG);
    let face = egui::Rect::from_min_max(
        rect.min + egui::vec2(KEYCAP_EDGE, KEYCAP_EDGE),
        rect.max - egui::vec2(KEYCAP_EDGE, KEYCAP_FOOT),
    );
    painter.rect_filled(face, CornerRadius::same(KEYCAP_RADIUS - 1), theme::CARD);
    painter.galley(
        egui::pos2(face.left() + KEYCAP_PAD_X, run_top(ui, rect.center().y, &font)),
        galley,
        theme::INK,
    );
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
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
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

/// The four palettes, and the eye.
fn palette_body(ui: &mut egui::Ui, draft: &mut SequenceDraft, palette: &[FieldRef]) {
    ui.label(RichText::new("Add a value").size(11.0).color(theme::TEXT_FAINT));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        for field in palette {
            if ui.add(detail_edit::palette_button(&field.label())).clicked() {
                draft.sequence =
                    detail_edit::sequence_with(&draft.sequence, Token::Field(field.clone()));
            }
        }
        if palette.is_empty() {
            ui.label(
                RichText::new("This item has no fields to reference yet.")
                    .size(11.0)
                    .color(theme::TEXT_FAINT),
            );
        }
    });
    ui.add_space(8.0);

    ui.label(RichText::new("Add a key").size(11.0).color(theme::TEXT_FAINT));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        for key in key_sequence::KEYS.iter().filter(|k| k.palette) {
            if ui.add(detail_edit::palette_button(key.label)).clicked() {
                draft.sequence = detail_edit::sequence_with(&draft.sequence, Token::Key(key));
            }
        }
    });
    ui.add_space(8.0);

    ui.label(RichText::new("Add text").size(11.0).color(theme::TEXT_FAINT));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        ui.add(egui::TextEdit::singleline(&mut draft.literal_draft).desired_width(160.0));
        if theme::secondary_button(ui, "Add text").clicked() {
            // Escaping is this app's job, not the user's.
            if let Some(next) =
                detail_edit::sequence_with_literal(&draft.sequence, &draft.literal_draft)
            {
                draft.sequence = next;
                draft.literal_draft.clear();
            }
        }
    });
    ui.add_space(8.0);

    ui.label(RichText::new("Add a wait").size(11.0).color(theme::TEXT_FAINT));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        ui.add(egui::TextEdit::singleline(&mut draft.wait_draft).desired_width(56.0));
        ui.label(RichText::new("seconds").size(11.0).color(theme::TEXT_FAINT));
        let addable = key_sequence::wait_ms_from_seconds(&draft.wait_draft).is_some();
        ui.add_enabled_ui(addable, |ui| {
            if theme::secondary_button(ui, "Add wait").clicked() {
                if let Some(next) =
                    detail_edit::sequence_with_wait(&draft.sequence, &draft.wait_draft)
                {
                    draft.sequence = next;
                }
            }
        });
        if !addable {
            ui.label(RichText::new(WAIT_REFUSAL).size(11.0).color(theme::TEXT_FAINT));
        }
    });
    ui.add_space(10.0);

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
        let caption = if draft.revealing { HIDE } else { REVEAL };
        if theme::secondary_button(ui, caption).clicked() {
            draft.revealing = !draft.revealing;
        }
        if !draft.sequence.is_empty() && theme::secondary_button(ui, "Use the default").clicked() {
            // The empty string, not the default's own spelling: an empty
            // stored value IS the default, and rendering it would turn an item
            // that inherits into one that pins.
            draft.sequence = String::new();
        }
    });
}

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
        rects: Vec<PaintedRect>,
    }

    impl Painted {
        fn strings(&self) -> Vec<&str> {
            self.texts.iter().map(|(t, _)| t.as_str()).collect()
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
    }

    fn walk(shape: &egui::Shape, painted: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => painted.texts.push((
                text.galley.text().to_string(),
                egui::Rect::from_min_size(text.pos, text.galley.size()),
            )),
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
    }

    impl Modal {
        fn over(window: egui::Vec2) -> Self {
            let ctx = egui::Context::default();
            let modal = Self { ctx, window, clock: std::cell::Cell::new(0.0), icon: None };
            // A font set registered during a frame is usable from the next
            // one on, so two throwaway frames -- every harness in this crate
            // runs them.
            let _ = modal.ctx.run_ui(modal.input(&[]), |_ui| {});
            theme::apply(&modal.ctx);
            let _ = modal.ctx.run_ui(modal.input(&[]), |_ui| {});
            modal
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
            let palette = vec![FieldRef::Username, FieldRef::Password];
            let totp = crate::vault_window::detail::TotpState::NoSecret;
            let source = ResolveSource {
                username: "a.novak@ledgerline.com",
                password: "correct-horse-battery",
                custom: Vec::new(),
                totp: &totp,
            };
            let mut apps = AppIdentityCache::default();
            let output = self.ctx.run_ui(self.input(events), |ui| {
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

        /// The builder at rest: an `egui::Area` is laid out from the size it
        /// had on the previous frame, so the frame read is the third.
        fn frame(&self, draft: &mut SequenceDraft) -> Painted {
            let _ = self.frame_with(draft, &[]);
            let _ = self.frame_with(draft, &[]);
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
    /// the app's name**, and the band is still one line: the two tiles share
    /// the row's middle and the chips sit under the second name.
    ///
    /// The two NAMES do not share a line, and are not asserted to: 4a's
    /// `align-items: center` centres each subject's column on the row, and
    /// the app's column is a chip row taller than the item's, so the item's
    /// name sits lower by half of that. Measured: `SAP Production` centred
    /// on 216 and `SAP Logon 760` on 205.5, the tiles both on 209.
    #[test]
    fn the_chips_drop_under_the_apps_name_and_the_band_stays_one_line() {
        let modal = Modal::over(WINDOWS[0]);
        let painted = modal.frame(&mut draft());
        let item_tile = painted.rect_of("SP");
        let app_tile = painted.rect_of("SL");
        let app = painted.rect_of("SAP Logon 760");
        let process = painted.rect_of("saplogon.exe");
        let class = painted.rect_of("SAPFEWndClass");
        assert!(
            (item_tile.center().y - app_tile.center().y).abs() <= 0.5,
            "the two tiles are not on one line: {item_tile:?} and {app_tile:?}"
        );
        assert!(app_tile.right() < app.left(), "the app's name is not beside its tile");
        assert!(
            process.top() >= app.bottom() && class.top() >= app.bottom(),
            "the chips did not drop under the name: name {app:?}, chips {process:?} {class:?}"
        );
        assert!(
            (process.center().y - class.center().y).abs() <= 0.5 && process.right() < class.left(),
            "the two chips are not on one line in order"
        );
        // 4a's small capitals, from sentence-case constants.
        assert!(painted.strings().contains(&"VAULT ITEM"));
        assert!(painted.strings().contains(&"SENDS ONLY TO"));
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
    /// edit form's "total", and the two on the pill's line.
    #[test]
    fn the_sequence_band_is_4as_caption_tally_and_pill_on_one_line() {
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
    }

    /// **Every cell of a step row sits on the row's line.** The defect this
    /// holds against was found by measuring and not by looking: egui centres
    /// each child on the band as it stands when that child is placed, so
    /// the grip, index and kind chip -- placed first -- sat three points
    /// above the controls placed after them. See [`centred_row`].
    #[test]
    fn every_cell_of_a_step_row_is_on_the_rows_line() {
        let painted = Modal::over(WINDOWS[0]).frame(&mut draft());
        // Row two: `Tab`, a keycap. Its index, its `KEY` chip, its `—`, and
        // its three controls.
        let keycap = painted.rect_of("Tab");
        let on_row = |text: &str| -> egui::Rect {
            let found: Vec<egui::Rect> = painted
                .rects_of(text)
                .into_iter()
                .filter(|r| (r.center().y - keycap.center().y).abs() < 12.0)
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
        let row = painted
            .rects
            .iter()
            .find(|r| {
                r.fill == theme::CARD && r.stroke == theme::HAIRLINE && r.rect.contains_rect(keycap)
            })
            .map(|r| r.rect)
            .expect("the Tab row's frame is painted");
        for text in ["2", "KEY", "\u{2014}", "<", ">", "x", "Tab"] {
            let rect = on_row(text);
            assert!(
                (rect.center().y - row.center().y).abs() <= 2.0,
                "{text:?} at {rect:?} is off the row's line at {row:?}"
            );
        }
        // And in 4a's order across the row.
        let (index, kind, dash, back, forward, remove) =
            (on_row("2"), on_row("KEY"), on_row("\u{2014}"), on_row("<"), on_row(">"), on_row("x"));
        assert!(index.right() <= kind.left() && kind.right() <= keycap.left());
        assert!(keycap.right() <= dash.left() && dash.right() <= back.left());
        assert!(back.right() <= forward.left() && forward.right() <= remove.left());
    }

    /// **A row's own `x` takes away that row's step.** The third of five, so
    /// neither the first nor the last: a control bound to the wrong index in
    /// either direction is visible.
    #[test]
    fn a_rows_own_remove_control_takes_away_that_step() {
        let modal = Modal::over(WINDOWS[0]);
        let mut draft = draft();
        let painted = modal.frame(&mut draft);
        let password = painted.rect_of("Password");
        let remove: Vec<egui::Rect> = painted
            .rects_of("x")
            .into_iter()
            .filter(|r| (r.center().y - password.center().y).abs() < 12.0)
            .collect();
        assert_eq!(remove.len(), 1, "expected one `x` on the Password row");
        let _ = modal.click(&mut draft, remove[0].center());
        assert_eq!(draft.sequence, "{USERNAME}{TAB}{DELAY 250}{ENTER}");
    }
}

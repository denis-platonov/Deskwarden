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

use crate::key_sequence::{self, FieldRef, ResolveSource, Token};
use crate::theme;
use crate::vault_window::detail_edit::{
    self, duration_label, sequence_tally, step_rows, StepKind,
};
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText};
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
                acts.push(Act { kind: StepKind::Key, label, secret: false });
                typing = false;
            }
            Token::Delay(_) => {
                acts.push(Act {
                    kind: StepKind::Wait,
                    label: token.chip_label(),
                    secret: false,
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

/// The rails' width. 4a's are 260 and 280 against a 1040 canvas; one width for
/// both keeps the steps column centred, which is what the picture reads as.
const RAIL_WIDTH: f32 = 252.0;

/// The gap between the three columns.
const COLUMN_GAP: f32 = 16.0;

/// The steps column's floor. Below this the rails are stacked under it
/// instead: three columns whose middle one is narrower than the edit-form pane
/// this screen exists to escape would be a worse version of the thing it
/// replaces.
const STEPS_FLOOR: f32 = 420.0;

/// The header strip's height.
const HEADER_HEIGHT: f32 = 56.0;

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

/// The sentence under the app card. The rule the gate really enforces, in the
/// design's own words.
pub const REFUSED_ELSEWHERE: &str =
    "Any window that is not this process will be refused at send time, even on the hotkey.";

/// The note under the rehearse button -- 4d's own sentence, which the edit
/// form had no room for and this screen does.
pub const REHEARSE_NOTE: &str =
    "Types sample-user and not-a-real-password into a scratch window so you can watch the \
     timing. No secret leaves the vault.";

/// The eye's two captions, the edit form's words.
const REVEAL: &str = "Show what it types";
const HIDE: &str = "Hide what it types";

/// Draws 4a. Returns what the user asked for, which the window acts on.
pub fn draw_sequence_builder(
    ui: &mut egui::Ui,
    draft: &mut SequenceDraft,
    palette: &[FieldRef],
    source: &ResolveSource<'_>,
) -> BuilderAction {
    theme::paint_window_background(ui);
    let mut action = header(ui, draft);

    // **CTRL+S, which is 4a's own caption on the Save button.**
    //
    // The one other `egui::Key::S` in this app's production is the read
    // pane's Send-a-record chord, and the two do not compete: they differ by
    // SHIFT, `consume_shortcut` compares the WHOLE modifier set, and this
    // screen is a `DetailMode` that replaces the read pane rather than
    // sitting beside it. `send_create_wiring`'s
    // `the_record_chord_is_a_key_no_other_binding_takes` names this file and
    // checks that shape, so a third binding -- or this one going loose --
    // still fails the suite.
    //
    // Read before the body so a press cannot be swallowed by a text box that
    // happens to have focus: the box takes a bare `s`, never the chord.
    let save_chord = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::S);
    if draft.saveable() && ui.input_mut(|i| i.consume_shortcut(&save_chord)) {
        action = BuilderAction::Save;
    }

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(Margin::symmetric(16, 14)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let wide = ui.available_width() >= STEPS_FLOOR + (RAIL_WIDTH + COLUMN_GAP) * 2.0;
            if wide {
                let total = ui.available_width();
                let steps = total - (RAIL_WIDTH + COLUMN_GAP) * 2.0;
                ui.horizontal_top(|ui| {
                    column(ui, RAIL_WIDTH, |ui| left_rail(ui, draft));
                    ui.add_space(COLUMN_GAP);
                    column(ui, steps, |ui| steps_column(ui, draft, palette, source));
                    ui.add_space(COLUMN_GAP);
                    column(ui, RAIL_WIDTH, |ui| {
                        if let Some(asked) = right_rail(ui, draft, source) {
                            action = asked;
                        }
                    });
                });
            } else {
                left_rail(ui, draft);
                steps_column(ui, draft, palette, source);
                if let Some(asked) = right_rail(ui, draft, source) {
                    action = asked;
                }
            }
        });
    });
    action
}

/// One column of the wide layout, at an exact width and in its own top-down
/// layout.
///
/// `allocate_ui_with_layout` and not `allocate_ui`: the latter hands the child
/// egui's DEFAULT layout, which inside the `horizontal_top` above is
/// left-to-right -- so every card in the column would be laid beside the last
/// one instead of under it.
fn column<R>(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(width);
            add(ui)
        },
    )
    .inner
}

/// The strip across the top: what is being edited, and the two ways out.
fn header(ui: &mut egui::Ui, draft: &mut SequenceDraft) -> BuilderAction {
    let mut action = BuilderAction::None;
    egui::Frame::new()
        .fill(theme::CARD)
        .inner_margin(Margin::symmetric(16, 0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(HEADER_HEIGHT);
            ui.horizontal_centered(|ui| {
                ui.label(theme::bold(SCREEN_TITLE, 15.0).color(theme::INK));
                ui.label(RichText::new("\u{b7}").size(15.0).color(theme::TEXT_GHOST));
                ui.label(RichText::new(draft.item_name.clone()).size(15.0).color(theme::TEXT_SECONDARY));
                // Right-aligned, so the two controls sit where every footer in
                // this app puts them however wide the window is.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let blocked = draft.fault().is_some();
                    let label = if blocked { SAVE_BLOCKED_LABEL } else { SAVE_LABEL };
                    if theme::primary_button_enabled(ui, label, Some("CTRL+S"), draft.saveable())
                        .clicked()
                    {
                        action = BuilderAction::Save;
                    }
                    if theme::secondary_button(ui, DISCARD_LABEL).clicked() {
                        action = BuilderAction::Discard;
                    }
                    if draft.changed() {
                        theme::status_pill(ui, theme::CAUTION_MARK, theme::CHANGED_PILL);
                    }
                });
            });
        });
    theme::hairline(ui);
    action
}

/// 4a's left rail: what the rule belongs to, and what it is allowed to type
/// into.
fn left_rail(ui: &mut egui::Ui, draft: &SequenceDraft) {
    theme::section_card(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section_card_header(ui, VAULT_ITEM_HEADING, "", false);
        theme::section_card_body(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::avatar(ui, &draft.item_name, 30.0, false);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(theme::semibold(draft.item_name.clone(), 13.0).color(theme::INK));
                    if !draft.folder.is_empty() {
                        ui.label(
                            RichText::new(draft.folder.clone())
                                .size(11.0)
                                .color(theme::TEXT_FAINT),
                        );
                    }
                });
            });
        });
    });
    ui.add_space(theme::SECTION_GAP);

    theme::section_card(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section_card_header(ui, SENDS_TO_HEADING, "", false);
        theme::section_card_body(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            ui.horizontal(|ui| {
                theme::avatar(ui, &draft.app_name, 30.0, false);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(theme::semibold(draft.app_name.clone(), 13.0).color(theme::INK));
                    ui.label(
                        RichText::new(draft.app.process.clone())
                            .size(11.0)
                            .color(theme::TEXT_FAINT),
                    );
                    // Only when it is really part of the rule: a title needle
                    // is read for a hosted window and ignored otherwise, so
                    // printing one here on an ordinary process would name a
                    // condition that does not apply.
                    if draft.app.hosted && !draft.app.title.is_empty() {
                        ui.label(
                            RichText::new(draft.app.title.clone())
                                .size(11.0)
                                .color(theme::TEXT_FAINT),
                        );
                    }
                });
            });
            ui.add_space(8.0);
            ui.label(RichText::new(REFUSED_ELSEWHERE).size(11.0).color(theme::TEXT_GHOST));
        });
    });
    ui.add_space(theme::SECTION_GAP);
}

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
            Some(tally) => detail_edit::tally_label(&tally),
            None => detail_edit::TALLY_REFUSED.to_string(),
        };
        theme::section_card_header(ui, SEQUENCE_HEADING, &summary, false);
        theme::section_card_body(ui, |ui| {
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

            if let Some(wants_template) = detail_edit::view_toggle(ui, draft.template_view) {
                // Seeded verbatim, every time the view is entered -- so a user
                // who opens the template view and closes it again has changed
                // nothing at all.
                if wants_template {
                    draft.template_draft = draft.sequence.clone();
                }
                draft.template_view = wants_template;
            }
            ui.add_space(8.0);

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
                );
            } else if let Some(edit) = detail_edit::sequence_steps(
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

/// 4a's right rail: timing, checks, budget, and the rehearsal.
fn right_rail(
    ui: &mut egui::Ui,
    draft: &mut SequenceDraft,
    source: &ResolveSource<'_>,
) -> Option<BuilderAction> {
    let mut action = None;

    theme::section_card(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section_card_header(ui, TIMING_HEADING, "", false);
        theme::section_card_body(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            match timing_spans(&draft.sequence, source) {
                Some(spans) if !spans.is_empty() => timing_strip(ui, &spans),
                // Not a blank card: the reason is already on the steps column
                // and this says which state it is in.
                _ => {
                    ui.label(
                        RichText::new(detail_edit::TALLY_REFUSED)
                            .size(11.0)
                            .color(theme::TEXT_FAINT),
                    );
                }
            }
        });
    });
    ui.add_space(theme::SECTION_GAP);

    theme::section_card(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section_card_header(ui, CHECKS_HEADING, "", false);
        theme::section_card_body(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            let mut fix = None;
            for check in checks(&draft.sequence) {
                if let Some(asked) = check_row(ui, &check) {
                    fix = Some(asked);
                }
            }
            if let Some(CheckFix::PauseAfterEnter(index)) = fix {
                draft.sequence = with_pause_after(&draft.sequence, index);
            }
        });
    });
    ui.add_space(theme::SECTION_GAP);

    theme::section_card(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section_card_header(ui, BUDGET_HEADING, "", false);
        theme::section_card_body(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            match sequence_tally(&draft.sequence, source) {
                Some(tally) => {
                    budget_bar(ui, "Total length", tally.total, crate::injector::sequence::MAX_SEQUENCE);
                    ui.add_space(8.0);
                    budget_bar(ui, "Largest burst", tally.burst, crate::injector::sequence::MAX_BURST);
                }
                None => {
                    ui.label(
                        RichText::new(detail_edit::TALLY_REFUSED)
                            .size(11.0)
                            .color(theme::TEXT_FAINT),
                    );
                }
            }
        });
    });
    ui.add_space(theme::SECTION_GAP);

    theme::section_card(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section_card_header(ui, "Rehearsal", "", false);
        theme::section_card_body(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            if theme::secondary_button(ui, detail_edit::APP_SEQUENCE_REHEARSE).clicked() {
                action = Some(BuilderAction::Rehearse);
            }
            ui.add_space(6.0);
            ui.label(RichText::new(REHEARSE_NOTE).size(11.0).color(theme::TEXT_GHOST));
        });
    });
    ui.add_space(theme::SECTION_GAP);
    action
}

/// The height of one bar in the timing strip.
const BAR_HEIGHT: f32 = 12.0;

/// The smallest a bar is drawn however short its act is: a 10ms keypress on a
/// 40-second sequence is a bar 0.03pt wide, which is nothing on screen and
/// nothing to hover. A floor makes the strip a picture of the ORDER of the
/// acts as well as their length, which is what it is read for.
const BAR_FLOOR: f32 = 3.0;

/// 4a's strip: one bar per act, the acts named down the side, and the scale
/// under them.
fn timing_strip(ui: &mut egui::Ui, spans: &[TimingSpan]) {
    let total: Duration = spans.iter().map(|s| s.len).sum();
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, BAR_HEIGHT), egui::Sense::hover());
    let painter = ui.painter();
    let scale = if total.is_zero() { 0.0 } else { width / total.as_secs_f32() };
    for span in spans {
        let x = rect.left() + span.at.as_secs_f32() * scale;
        let w = (span.len.as_secs_f32() * scale).max(BAR_FLOOR);
        let bar = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(w, BAR_HEIGHT));
        painter.rect_filled(bar, CornerRadius::same(3), bar_colour(span));
    }
    ui.add_space(4.0);

    // The scale. Drawn as a row of marks rather than as ticks under the bar,
    // because a tick two points from its neighbour is a smudge and a number
    // beside it is unreadable -- and because the numbers are the thing being
    // read here, not their exact x.
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 2.0);
        for mark in timing_ticks(total) {
            ui.label(RichText::new(duration_label(mark)).size(10.0).color(theme::TEXT_GHOST));
        }
    });
    ui.add_space(8.0);

    for span in spans {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 2.0);
            let dot = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover()).0;
            ui.painter().rect_filled(dot, CornerRadius::same(2), bar_colour(span));
            ui.label(
                RichText::new(span.act.label.clone())
                    .size(11.0)
                    .color(if span.act.secret { theme::SECRET_INK } else { theme::INK }),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(duration_label(span.len)).size(11.0).color(theme::TEXT_FAINT),
                );
            });
        });
        ui.add_space(2.0);
    }
}

/// A bar's colour. The one exception to the blue is the one the rest of this
/// app already makes: a secret is red, and nothing else is.
fn bar_colour(span: &TimingSpan) -> Color32 {
    if span.act.secret {
        theme::SECRET_INK
    } else {
        match span.act.kind {
            StepKind::Wait => theme::BLUE_SOFT,
            _ => theme::BLUE,
        }
    }
}

/// One budget line: the words, and a bar showing how much of the limit is
/// spent.
fn budget_bar(ui: &mut egui::Ui, label: &str, used: Duration, limit: Duration) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(11.0).color(theme::TEXT_FAINT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                theme::semibold(
                    format!("{} of {}", duration_label(used), duration_label(limit)),
                    11.0,
                )
                .color(theme::TEXT_SECONDARY),
            );
        });
    });
    ui.add_space(4.0);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 6.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(3), theme::BLUE_WASH);
    let share = if limit.is_zero() {
        0.0
    } else {
        (used.as_secs_f32() / limit.as_secs_f32()).clamp(0.0, 1.0)
    };
    if share > 0.0 {
        let filled = egui::Rect::from_min_size(rect.min, egui::vec2(width * share, rect.height()));
        ui.painter().rect_filled(filled, CornerRadius::same(3), theme::BLUE);
    }
}

/// One line of the checks card. Returns the fix if its button was pressed.
fn check_row(ui: &mut egui::Ui, check: &Check) -> Option<CheckFix> {
    let mut asked = None;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 2.0);
        let (mark, ink) = match check.tone {
            CheckTone::Pass => ("\u{2713}", theme::DONE_MARK),
            CheckTone::Caution => ("!", theme::CAUTION_MARK),
            CheckTone::Fact => ("\u{b7}", theme::TEXT_GHOST),
        };
        ui.label(theme::semibold(mark, 11.0).color(ink));
        ui.vertical(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            ui.label(
                RichText::new(check.line.clone()).size(11.0).color(match check.tone {
                    CheckTone::Caution => theme::CAUTION_INK,
                    CheckTone::Fact => theme::TEXT_FAINT,
                    CheckTone::Pass => theme::TEXT_SECONDARY,
                }),
            );
            if !check.note.is_empty() {
                ui.label(RichText::new(check.note.clone()).size(11.0).color(theme::TEXT_GHOST));
            }
            if check.fix.is_some() && theme::secondary_button(ui, ADD_PAUSE_LABEL).clicked() {
                asked = check.fix;
            }
        });
    });
    ui.add_space(8.0);
    asked
}

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
}

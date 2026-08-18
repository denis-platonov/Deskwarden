//! **4d -- rehearsal: prove the sequence against fake data before the real
//! password is ever in play.**
//!
//! Three parts, and the split is the crate's usual one.
//!
//! [`substitute`] is the guarantee, and it is pure. It takes the steps a real
//! [`crate::injector::sequence::plan`] produced and returns the same list with
//! **every** [`Step::Text`] payload replaced by a fixed sample. `Step::Key`
//! and `Step::Wait` come through untouched, so the shape and the pauses of the
//! rehearsal are the shape and the pauses of the real run.
//!
//! [`rehearsal_plan`] puts that list back through
//! [`crate::injector::sequence::replan`], so the rehearsal is chunked against
//! `MAX_BURST` and bounded by `MAX_SEQUENCE` exactly as a real fill is, and
//! then goes to the **ordinary sender**. Nothing here simulates typing: the
//! timing a user watches is the timing they will get.
//!
//! [`SCRATCH_TITLE`]/[`scratch_target`] are the target rule: a rehearsal types
//! into a window **this process owns**, found by title through
//! `foreground::own_window_titled`, and does not run at all if that window
//! cannot be found. There is no fallback to the foreground, deliberately.
//!
//! # Where the window is
//!
//! [`crate::scratch_window`], at crate root rather than here. It opens under
//! [`SCRATCH_TITLE`], raises itself, and drives this module's
//! [`rehearsal_plan`] through `Injector::fill_sequence` -- the ordinary sender
//! -- while pumping its own message loop, which is what lets a user watch the
//! timing rather than receive the whole burst at the end. It is a plain Win32
//! window and not an `eframe` one because a rehearsal is started from inside
//! the vault window's event loop; see that module's header.
//!
//! What is drawn there comes from here: [`transcript`] and [`acts`] for the
//! plan that was sent, [`arrived_panel`] and [`report_text`] for what really
//! landed, and [`elapsed_label`] for the total.
//!
//! # Why the assertions in this file are positive
//!
//! The property is "no payload here came from the vault". Written as a
//! negative -- `assert!(steps.iter().all(|s| s.text() != password))` -- it is
//! satisfied by an empty step list, which is exactly the vacuous shape this
//! crate has shipped twice. So the tests below assert that the samples ARE
//! there, at the right indices, with the original count of text steps, and
//! that the plan really ran.

use crate::injector::sequence::{self, Plan, Step};
use crate::key_sequence::{FieldRef, Token};
use std::time::Duration;

// ---------------------------------------------------------------------------
// The substitution
// ---------------------------------------------------------------------------

/// The design's own sample for the first thing a sequence types.
pub const SAMPLE_USER: &str = "sample-user";

/// The design's own sample for everything a sequence types after the first
/// thing. **Named so that a user who sees it in a log, a screenshot or a
/// support ticket knows immediately that it is not a credential.**
pub const SAMPLE_PASSWORD: &str = "not-a-real-password";

/// The sample a text step at `text_step_index` (counted among text steps only)
/// is given.
///
/// The rule is the design's: the first thing typed stands for the username and
/// everything after it stands for the password. **The safety property does not
/// rest on this choice.** It rests on the fact that every arm returns a
/// constant -- there is no branch here that can return an argument, so no
/// payload from the caller can survive [`substitute`] whatever the sequence
/// looked like.
pub fn sample_for(text_step_index: usize) -> &'static str {
    if text_step_index == 0 {
        SAMPLE_USER
    } else {
        SAMPLE_PASSWORD
    }
}

/// Replaces every typed payload with a sample, and changes nothing else.
///
/// `rate` is carried through unchanged: it is what the keyboard sleeps
/// between characters, and rewriting it would make the rehearsal's timing a
/// story about a sequence the user is not going to send.
pub fn substitute(steps: &[Step]) -> Vec<Step> {
    let mut text_seen = 0usize;
    steps
        .iter()
        .map(|step| match step {
            Step::Text { rate, .. } => {
                let text = sample_for(text_seen).to_string();
                text_seen += 1;
                Step::Text { text, rate: *rate }
            }
            // Untouched, deliberately: the pauses and the key presses are the
            // half of the timing a user is rehearsing to check.
            Step::Key { key, mods } => Step::Key { key, mods: *mods },
            Step::Wait(d) => Step::Wait(*d),
        })
        .collect()
}

/// The plan a rehearsal actually sends: [`substitute`], re-chunked and
/// re-bounded.
///
/// Takes `&Plan` rather than `&[Step]` so that no caller can hand this a step
/// list it assembled itself and skip the substitution.
pub fn rehearsal_plan(real: &Plan) -> Result<Plan, sequence::Refusal> {
    sequence::replan(substitute(real.steps()))
}

/// The sample a `{TOTP}` resolves to for a rehearsal. Six zeros: a shape a
/// user recognises as a code, and a value no authenticator will ever emit.
pub const SAMPLE_TOTP: &str = "000000";

/// **The plan a rehearsal starts from, built without the vault.**
///
/// [`substitute`] would replace every payload anyway, so the obvious thing --
/// plan the user's sequence against the real item and then substitute -- makes
/// one more copy of the password than the feature needs, on the UI thread, for
/// no gain: the *shape* a rehearsal reproduces is its keys, its waits and its
/// rates, and none of those depend on what a field resolves to. `MAX_BURST`
/// chunking does depend on payload length, and that is precisely what
/// [`sequence::replan`] redoes on the substituted text, so planning against the
/// real values would have its answer thrown away.
///
/// So the sequence is resolved against the samples from the start, and
/// [`substitute`] then runs over the result anyway. The two are not redundant:
/// this is the reason no vault value is ever *near* a rehearsal, and that is
/// the reason no vault value can *survive* one.
///
/// An empty sequence is the item's default, exactly as a real fill treats it --
/// rehearsing "nothing" for an item that fills perfectly well would be a
/// rehearsal of the wrong thing.
pub fn sample_plan(sequence: &str) -> Result<Plan, sequence::Refusal> {
    let sequence =
        if sequence.is_empty() { crate::key_sequence::DEFAULT_SEQUENCE } else { sequence };
    let tokens = crate::key_sequence::parse(sequence);
    // Every custom field the sequence names, given a sample. Collected from the
    // tokens rather than from the item, because there is no item here: a
    // `{S:PIN}` must rehearse whether or not this vault has a `PIN`.
    let custom: Vec<(&str, &str)> = tokens
        .iter()
        .filter_map(|token| match token {
            Token::Field(FieldRef::Custom(name)) => Some((name.as_str(), SAMPLE_PASSWORD)),
            _ => None,
        })
        .collect();
    sequence::plan(
        &tokens,
        &sequence::Resolved {
            username: SAMPLE_USER,
            password: SAMPLE_PASSWORD,
            totp: Some(SAMPLE_TOTP),
            custom,
        },
    )
}

// ---------------------------------------------------------------------------
// The transcript
// ---------------------------------------------------------------------------

/// One line of the design's "WHAT ARRIVED" list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arrival {
    /// Characters that were typed.
    Typed(String),
    /// A key that was pressed, in the words the step list uses.
    Pressed(String),
    /// A pause.
    Paused(Duration),
}

/// The transcript, read off the plan that was **actually sent**.
///
/// Not off the plan that was *going* to be sent: the argument is the
/// rehearsal plan, after substitution and after chunking, so a transcript can
/// never show something the sender was not given.
///
/// Adjacent text chunks are joined back up, because the chunking is an
/// artefact of `MAX_BURST` and not something the user typed -- a
/// `not-a-real-password` split across two bursts arrived as one word.
pub fn transcript(sent: &Plan) -> Vec<Arrival> {
    let mut out: Vec<Arrival> = Vec::new();
    for step in sent.steps() {
        match step {
            Step::Text { text, .. } => match out.last_mut() {
                Some(Arrival::Typed(existing)) => existing.push_str(text),
                _ => out.push(Arrival::Typed(text.clone())),
            },
            Step::Key { key, .. } => out.push(Arrival::Pressed(key.token.to_string())),
            Step::Wait(d) => out.push(Arrival::Paused(*d)),
        }
    }
    out
}

/// The design's timing readout: `2.1 s`, or `250 ms` under a second.
///
/// Shares [`crate::vault_window::detail_edit::duration_label`] with the
/// builder's own budget line, so the number a user reads before rehearsing and
/// the number they read after cannot be written in two different ways.
pub fn elapsed_label(elapsed: Duration) -> String {
    super::detail_edit::duration_label(elapsed)
}

/// How many acts the rehearsal will perform, for the line above the
/// transcript.
///
/// **Counted off the transcript and not off `Plan::len()`**, which is the
/// chunk count: `sequence::plan` splits text at `MAX_BURST`, so a six-act
/// sequence plans as nine steps and a user told "9 steps" is being told about
/// an implementation detail.
pub fn acts(sent: &Plan) -> usize {
    transcript(sent).len()
}

// ---------------------------------------------------------------------------
// The scratch window
// ---------------------------------------------------------------------------

/// The title of the window a rehearsal types into.
///
/// **A rehearsal never types into whatever happened to be focused.** The
/// window is created by this process, raised, and handed to the sender by
/// handle; [`crate::foreground::own_window_titled`] resolves the handle by
/// searching *this process's* windows, so a same-titled window belonging to
/// anything else cannot be found by it.
pub const SCRATCH_TITLE: &str = "Deskwarden \u{2014} rehearsal scratch";

/// The handle a rehearsal may send to, or `None`.
///
/// `None` is the whole safety story of this function and the reason it is not
/// written as `own_window_titled(..).unwrap_or_else(GetForegroundWindow)`: if
/// the scratch window cannot be found, there is no fallback target that is
/// safe, so the rehearsal does not run. The alternative -- typing
/// `not-a-real-password` into a colleague's chat window -- is embarrassing
/// rather than dangerous, which is precisely why it would be easy to ship.
pub fn scratch_target() -> Option<isize> {
    crate::foreground::own_window_titled(SCRATCH_TITLE)
}

// ---------------------------------------------------------------------------
// The readout
// ---------------------------------------------------------------------------

/// What the readout says while the sequence is still being typed.
pub const WAITING_NOTE: &str = "Rehearsing\u{2026} watch the panel below.";

/// The design's heading for the panel showing what really landed.
pub const ARRIVED_HEADING: &str = "WHAT ARRIVED";

/// The glyph for a Tab that arrived.
///
/// **Not the design's U+21E5.** 4d draws these two marks in a browser, which
/// falls through to a system font that has them; this crate's monospace stack
/// is Consolas over egui's Hack, and `has_glyph` answers `false` for U+21E5,
/// U+23CE *and* U+21B5 in both. A codepoint the stack lacks is drawn as a tofu
/// box -- which is exactly the blank a rehearsal must not answer with, and the
/// same trap [`crate::theme::close_glyph`] exists for. Bundling a fourth face
/// for two characters was rejected against changing the two characters.
///
/// U+2192 and U+00B6 both render, and
/// `scratch_window::the_tab_and_enter_glyphs_are_drawable_in_the_panel_font`
/// asks the real font stack rather than assuming -- so a stack change that
/// took these away is red rather than a rectangle in a screenshot.
pub const ARRIVED_TAB: char = '\u{2192}';

/// The glyph for an Enter that arrived. See [`ARRIVED_TAB`] for why this is a
/// pilcrow and not the design's U+23CE.
pub const ARRIVED_ENTER: char = '\u{00b6}';

/// The characters that really landed, with the two invisible keys drawn.
///
/// **Read off the edit control, not off the plan.** [`transcript`] says what
/// was *sent*; this says what *arrived*, and the whole value of a rehearsal is
/// that those two can differ -- a control that eats a Tab, or a dialog that was
/// not ready for the first character, shows up here and nowhere else.
///
/// A Tab and an Enter are otherwise invisible in a transcript, and a rehearsal
/// whose readout cannot tell "the Tab arrived" from "the Tab did not" is a
/// readout that answers the one question it was opened to answer with a blank.
/// The carriage return of a Windows line ending is dropped rather than drawn:
/// it is how the edit control stores a line break, not a key anybody sent.
pub fn arrived_panel(arrived: &str) -> String {
    let mut out = String::with_capacity(arrived.len());
    for ch in arrived.chars() {
        match ch {
            '\t' => out.push(ARRIVED_TAB),
            '\r' => {}
            '\n' => {
                out.push(ARRIVED_ENTER);
                out.push('\n');
            }
            other => out.push(other),
        }
    }
    out
}

/// The line above the transcript: the design's `Rehearsal finished · 2.1 s`,
/// with the act count beside it.
///
/// **Acts, not `Plan::len()`.** See [`acts`]: `plan` chops text at
/// [`crate::injector::sequence::MAX_BURST`], so a six-act sequence plans as
/// nine steps and a user told "9" is being told an implementation detail.
pub fn finished_line(elapsed: Duration, acts: usize) -> String {
    let unit = if acts == 1 { "act" } else { "acts" };
    format!("Rehearsal finished \u{b7} {} \u{b7} {acts} {unit}", elapsed_label(elapsed))
}

/// The whole readout, as the one string the scratch window's panel shows.
///
/// Windows line endings because its one consumer is a Win32 edit control,
/// which draws a bare `\n` as a box rather than as a line break.
pub fn report_text(arrived: &str, elapsed: Duration, acts: usize) -> String {
    format!(
        "{}\r\n\r\n{ARRIVED_HEADING}\r\n{}",
        finished_line(elapsed, acts),
        arrived_panel(arrived).replace('\n', "\r\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::injector::sequence::{plan, Resolved, DEFAULT_RATE};
    use crate::key_sequence::parse;

    const REAL_USER: &str = "a.novak@ledgerline.com";
    const REAL_PASSWORD: &str = "Tr0ub4dor&3-correct-horse";

    fn real_values() -> Resolved<'static> {
        Resolved {
            username: REAL_USER,
            password: REAL_PASSWORD,
            totp: Some("482913"),
            custom: Vec::new(),
        }
    }

    /// The six-act sequence from the design, with `{CTRL+A}` and `{WAIT=}`
    /// spelled the way this crate's grammar actually spells them (`+{TAB}` is
    /// not needed here; `{CTRL+A}` is unrepresentable and deliberately
    /// deferred -- see the send-hardening follow-ups plan).
    const DESIGN_SEQUENCE: &str = "{USERNAME}{TAB}{DELAY 250}{PASSWORD}{ENTER}";

    fn real_plan() -> Plan {
        plan(&parse(DESIGN_SEQUENCE), &real_values()).expect("the fixture must plan")
    }

    /// **The property, asserted positively.**
    ///
    /// Not "no step equals the password" -- that is true of an empty list, and
    /// this crate has shipped that test twice. This says the samples are
    /// there, in the right places, and that there are exactly as many text
    /// steps after substitution as before.
    #[test]
    fn rehearsal_never_resolves_a_real_field() {
        let real = real_plan();
        let before: Vec<&Step> =
            real.steps().iter().filter(|s| matches!(s, Step::Text { .. })).collect();
        assert_eq!(
            before.len(),
            2,
            "the fixture must type two things, or this test proves nothing about the second"
        );

        let rehearsed = substitute(real.steps());
        let typed: Vec<&str> = rehearsed
            .iter()
            .filter_map(|s| match s {
                Step::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();

        // Positive: these exact values, in this order, and nothing else typed.
        assert_eq!(
            typed,
            [SAMPLE_USER, SAMPLE_PASSWORD],
            "a rehearsal must type the samples and only the samples"
        );
    }

    /// The other half: the acts that are NOT text survive untouched, so the
    /// timing a user watches is the timing of the real sequence.
    #[test]
    fn the_keys_and_the_pauses_are_the_real_ones() {
        let real = real_plan();
        let rehearsed = substitute(real.steps());
        assert_eq!(rehearsed.len(), real.len(), "substitution changed the shape of the sequence");
        for (a, b) in rehearsed.iter().zip(real.steps()) {
            match (a, b) {
                (Step::Text { rate: x, .. }, Step::Text { rate: y, .. }) => {
                    assert_eq!(x, y, "a text step's typing rate was rewritten");
                }
                (x, y) => assert_eq!(x, y, "a key or a pause was not carried through untouched"),
            }
        }
        assert!(
            rehearsed.iter().any(|s| matches!(s, Step::Wait(d) if *d == Duration::from_millis(250))),
            "the fixture's 250 ms wait is what makes the timing worth rehearsing"
        );
    }

    /// The rehearsal goes through the ordinary compiler on the way to the
    /// ordinary sender: a substituted list that is too long, or empty, is
    /// refused here exactly as a real one would be.
    #[test]
    fn the_rehearsal_plan_is_a_real_plan() {
        let real = real_plan();
        let sent = rehearsal_plan(&real).expect("the substituted sequence must plan");
        let typed: String = sent
            .steps()
            .iter()
            .filter_map(|s| match s {
                Step::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            typed,
            format!("{SAMPLE_USER}{SAMPLE_PASSWORD}"),
            "the chunked plan must still carry the samples and nothing else"
        );
        assert!(
            sequence::replan(Vec::new()).is_err(),
            "an empty rehearsal must be refused, not sent"
        );
    }

    /// **`Plan::len()` is not the act count.** The transcript, and the number
    /// beside it, are counted the way a user counts.
    #[test]
    fn the_transcript_reads_as_acts_and_not_as_bursts() {
        let real = real_plan();
        let sent = rehearsal_plan(&real).expect("plans");
        assert_eq!(
            transcript(&sent),
            [
                Arrival::Typed(SAMPLE_USER.to_string()),
                Arrival::Pressed("TAB".to_string()),
                Arrival::Paused(Duration::from_millis(250)),
                Arrival::Typed(SAMPLE_PASSWORD.to_string()),
                Arrival::Pressed("ENTER".to_string()),
            ],
            "the transcript must show what arrived, not how it was chunked"
        );
        assert_eq!(acts(&sent), 5);
    }

    /// A burst-splitting sequence: the transcript joins the chunks back up, so
    /// `acts` stays a count of acts even when the plan is longer than it.
    #[test]
    fn a_chunked_sample_still_reads_as_one_arrival() {
        // A rate that makes `MAX_BURST` hold only a handful of characters, so
        // `not-a-real-password` is provably split.
        let steps = vec![Step::Text {
            text: "x".repeat(200),
            rate: Duration::from_millis(40),
        }];
        let sent = sequence::replan(substitute(&steps)).expect("plans");
        assert!(
            sent.len() > 1,
            "the fixture did not split, so this test cannot see the joining"
        );
        assert_eq!(
            transcript(&sent),
            [Arrival::Typed(SAMPLE_USER.to_string())],
            "the chunk boundaries leaked into the transcript"
        );
        assert_eq!(acts(&sent), 1);
    }

    /// **The plan a rehearsal starts from never asks the vault for anything.**
    ///
    /// Positive on both halves: every text step that comes out holds a sample,
    /// and the fields a real fill would refuse to resolve -- a `{TOTP}` on an
    /// item with no seed, a `{S:PIN}` on an item with no `PIN` -- plan here
    /// rather than refusing, because a rehearsal is about the shape and the
    /// timing and not about what this particular item happens to hold.
    #[test]
    fn the_starting_plan_is_built_from_samples_and_not_from_an_item() {
        let planned = sample_plan("{USERNAME}{TAB}{PASSWORD}{TAB}{TOTP}{TAB}{S:PIN}{ENTER}")
            .expect("a sequence naming every kind of field must still rehearse");
        let typed: Vec<&str> = planned
            .steps()
            .iter()
            .filter_map(|s| match s {
                Step::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            typed,
            [SAMPLE_USER, SAMPLE_PASSWORD, SAMPLE_TOTP, SAMPLE_PASSWORD],
            "the starting plan resolved something other than a sample"
        );
        // The control that stops the assertion above being vacuous: the very
        // same sequence, planned the way a real fill plans it, REFUSES on an
        // item with no one-time code -- so the values above came from here.
        assert!(
            sequence::plan(
                &crate::key_sequence::parse("{TOTP}"),
                &sequence::Resolved {
                    username: "u",
                    password: "p",
                    totp: None,
                    custom: Vec::new(),
                },
            )
            .is_err(),
            "control: the real planner resolves a `{{TOTP}}` this item has not got"
        );
    }

    /// An empty sequence is the item's default, because that is what a real
    /// fill types for it. Rehearsing nothing would rehearse the wrong thing.
    #[test]
    fn an_empty_sequence_rehearses_the_default_fill() {
        let planned = sample_plan("").expect("the default must plan");
        assert_eq!(
            transcript(&planned),
            transcript(
                &sample_plan(crate::key_sequence::DEFAULT_SEQUENCE).expect("the default plans")
            ),
            "an unset sequence rehearsed something other than the default fill"
        );
        assert!(
            transcript(&planned).contains(&Arrival::Pressed("TAB".to_string())),
            "control: the default fill really does press Tab, so the comparison above is \
             between two non-trivial transcripts"
        );
    }

    /// **The two invisible keys are visible in the readout.**
    ///
    /// Asserted positively, on the glyphs, at the positions they arrived at --
    /// and with a control that the sample text itself came through unaltered,
    /// so a renderer that dropped everything would fail here rather than pass
    /// by saying nothing.
    #[test]
    fn a_tab_and_an_enter_that_arrived_are_drawn_rather_than_left_blank() {
        // Exactly what a Win32 edit control holds after the design's sequence:
        // Tab is stored as a tab, Enter as a Windows line ending.
        let arrived = format!("{SAMPLE_USER}\t\r\n{SAMPLE_PASSWORD}\r\n");
        let panel = arrived_panel(&arrived);
        assert_eq!(
            panel,
            format!("{SAMPLE_USER}{ARRIVED_TAB}{ARRIVED_ENTER}\n{SAMPLE_PASSWORD}{ARRIVED_ENTER}\n")
        );
        // The control: a Tab that did NOT arrive reads differently. Without
        // this, a renderer that drew the glyph unconditionally would pass.
        assert!(
            !arrived_panel(&format!("{SAMPLE_USER}\r\n")).contains(ARRIVED_TAB),
            "a Tab that never arrived was drawn anyway, so the readout cannot answer the one \
             question a rehearsal is opened to answer"
        );
        assert!(panel.contains(SAMPLE_USER) && panel.contains(SAMPLE_PASSWORD));
    }

    /// The heading counts **acts**, and the whole readout carries the design's
    /// words plus the arrival panel.
    #[test]
    fn the_report_says_how_long_it_took_and_what_landed() {
        let real = real_plan();
        let sent = rehearsal_plan(&real).expect("plans");
        assert_eq!(
            finished_line(Duration::from_millis(2100), acts(&sent)),
            "Rehearsal finished \u{b7} 2.1 s \u{b7} 5 acts",
            "the count must be acts and not `Plan::len()`, which chunks"
        );
        assert_eq!(finished_line(Duration::from_millis(250), 1), "Rehearsal finished \u{b7} 250 ms \u{b7} 1 act");

        let report = report_text(&format!("{SAMPLE_USER}\t"), Duration::from_millis(2100), 5);
        assert!(report.starts_with("Rehearsal finished \u{b7} 2.1 s"), "{report:?}");
        assert!(report.contains(ARRIVED_HEADING), "{report:?}");
        assert!(report.contains(&format!("{SAMPLE_USER}{ARRIVED_TAB}")), "{report:?}");
        // Every line break in the readout is a Windows one: a bare `\n` is
        // drawn as a box by the edit control this is written for. Controlled by
        // the assertion just above that the report has line breaks at all.
        assert!(report.contains("\r\n"), "control: the readout has no line breaks to check");
        for line in report.split("\r\n") {
            assert!(!line.contains('\n'), "a bare newline survived into {line:?}");
        }
    }

    #[test]
    fn the_readout_is_the_builders_own_words() {
        assert_eq!(elapsed_label(Duration::from_millis(2100)), "2.1 s");
        assert_eq!(elapsed_label(Duration::from_millis(250)), "250 ms");
    }

    /// Every arm of [`sample_for`] returns a constant, which is what makes
    /// [`substitute`] total. Stated as a test so that adding an arm that
    /// returns an argument is red rather than merely regrettable.
    #[test]
    fn every_sample_is_a_constant() {
        for index in 0..8 {
            assert!(
                [SAMPLE_USER, SAMPLE_PASSWORD].contains(&sample_for(index)),
                "sample {index} was not one of the two fixed samples"
            );
        }
        assert_eq!(sample_for(0), SAMPLE_USER);
        assert_ne!(sample_for(0), sample_for(1), "every step typing the same thing would hide a \
             substitution that only replaced the first");
    }

    #[test]
    fn a_rehearsal_has_a_window_of_its_own_to_type_into() {
        // The scratch window is not open in a test, so the handle is `None` --
        // and `None` is the answer that stops a rehearsal, rather than one
        // that falls back to the foreground. Asserting it here pins that there
        // is no fallback arm to fall into.
        assert_eq!(scratch_target(), None);
        assert!(SCRATCH_TITLE.contains("Deskwarden"));
    }

    /// `DEFAULT_RATE` is what an unqualified `{PASSWORD}` types at; naming it
    /// keeps the fixture honest about which rate is being carried through.
    #[test]
    fn the_default_rate_is_what_the_fixture_carries() {
        let real = real_plan();
        assert!(
            real.steps().iter().any(|s| matches!(s, Step::Text { rate, .. } if *rate == DEFAULT_RATE)),
            "the fixture types at a rate other than the default, so `the_keys_and_the_pauses_are_the_real_ones` \
             is comparing something the shipped sequence never uses"
        );
    }
}

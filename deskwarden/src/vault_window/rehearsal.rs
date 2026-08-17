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
//! # What is not here yet
//!
//! The scratch window itself -- the eframe host that opens under
//! [`SCRATCH_TITLE`], receives the keystrokes and shows the transcript -- is
//! **not built**. Everything it would need is: [`rehearsal_plan`] for what to
//! send, [`scratch_target`] for where, `Injector::fill_sequence` for the send,
//! and [`transcript`]/[`acts`]/[`elapsed_label`] for the readout. Until that
//! host exists there is no way to start a rehearsal from the UI, so nothing in
//! this module is reachable from a running app.
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

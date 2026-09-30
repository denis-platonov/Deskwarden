//! **The send gate: whether a fill may type, and what the user is told when
//! it may not.**
//!
//! This file used to have two halves, and the split was the one
//! `injector::target` makes: a DECISION, and a SURFACE that put the decision
//! in front of the user and asked them to confirm it. **The surface is gone.**
//!
//! # The confirmation was removed deliberately, and this is the argument
//!
//! Design 4b drew a card in front of every bare-secret fill -- "About to type
//! into", "Will send", a step list, and a hold on the space bar before a
//! single keystroke left the app. The owner's instruction was to take it out:
//! *"Confirm before sending - remove that popup completely, user intentionally
//! sends the whatever needed - their right."*
//!
//! The reasoning is theirs and it is a good one. A fill is not something that
//! happens *to* a user. They put the caret in a field, pressed the fill
//! hotkey, or picked an item and then picked a field off `picker_prompt`'s
//! card -- two deliberate acts, by name, before anything was typed. Asking
//! again is the app second-guessing a decision it has just watched the user
//! make twice. And the second question buys less than it looks like it does:
//! a person who has already asked answers *yes* on reflex, which is how a
//! confirmation stops being a check and becomes a keystroke on the way to the
//! thing they wanted. Held-to-send or not, the card's real effect on a user
//! who fills fifty times a day is 800 ms of delay and a learned reflex.
//!
//! **What was removed is the asking, and only the asking.** Three things on
//! this path were never the confirmation, and all three are still here:
//!
//! 1. **The gate.** [`verdict`] decides whether a fill is safe to attempt at
//!    all, and [`dispatch_with`] is the position it decides from. A fill the
//!    engine judges unsafe still does not happen -- and it now *cannot* be
//!    waved through, because there is no longer a human in the loop to wave
//!    it. Removing the card made the gate strictly more decisive, not less.
//! 2. **The refusal.** [`refusal_notice`] is the report that nothing was
//!    typed. It is not a confirmation -- nobody is being asked anything -- and
//!    without it a refused fill would be indistinguishable from a hotkey that
//!    never registered. See the next section for where it goes now.
//! 3. **The foreground re-check.** The card's footnote said "Sending stops the
//!    moment focus leaves this window." That sentence *described* a runtime
//!    behaviour it did not implement: `injector::sequence::run` re-reads the
//!    foreground before **every** step and abandons the plan the instant the
//!    target window stops being in front. The sentence went with the card. The
//!    behaviour is untouched, and it is the thing that was actually load
//!    bearing.
//!
//! # Where the refusal goes now
//!
//! It goes where [`Gated::Refused`] and [`Gated::NoTarget`] have always sent
//! it: through [`crate::injector::sequence::Notifier`], which production wires
//! to `sequence::REAL_NOTIFIER` -- a task-modal box on its own thread -- and
//! which a test wires to a recorder. That path already existed, was already
//! tested, and did not depend on the card.
//!
//! What changed is that it is now the **only** path, and it is now always
//! taken. Before this pass a refused verdict was computed *twice*: once in
//! `app::confirmed_by_preflight`, to decide which shape the card took, and
//! again inside `dispatch_with`. In production only the first one ever reached
//! the user -- the card said "Nothing sent", the user pressed *Dismiss*, the
//! fill returned, and `dispatch_with` was never called at all, so the notifier
//! never fired. Deleting the card did not delete the refusal; it deleted the
//! duplicate, and the survivor is the one with a test on it
//! (`app::fill_dispatch_tests::a_password_fill_types_nothing_when_the_preflight_refuses`
//! asserts the notifier was told, on both refusing shapes).
//!
//! [`REFUSED_HEADING`] -- "Nothing sent", design 4b's own word for this state
//! -- survived the card by moving into those sentences, so it is still the
//! first thing the user reads when a fill is declined. The rest of 4b's string
//! table went with the screen that painted it.
//!
//! # The masking refusal went the same way, on the same ruling
//!
//! The gate used to refuse twice. Besides the wrong window it had `NotMasked`:
//! a bare password or one-time code was not typed unless UI Automation
//! reported the control holding focus as a masked field -- "Deskwarden will
//! not type a password into a box that echoes it." The owner had that taken
//! out too: *"allow this - our app can enter TOTP or whatever to web pages as
//! well - if official Bitwarden fails to do - which it does a lot"*, and *"user
//! can open the app and copy and paste if logged in - it is same anyways"*.
//!
//! Both halves are true, and between them they leave the check guarding
//! nothing:
//!
//! - **It refused exactly the fills it was wanted for.** A browser does not
//!   reliably report a web page's password input as masked through UI
//!   Automation, and a one-time-code box is never masked at all -- it is an
//!   ordinary text input on every site that has one. So the refusal's
//!   commonest verdict was a correct fill declined: the code, typed into the
//!   box the site asked for it in, on the very pages the official client's
//!   own autofill misses.
//! - **It protected a value that is one copy away.** The user who pressed the
//!   hotkey is signed in, and the same secret is on the vault window's copy
//!   button and on `picker_prompt`'s. A paste into a box that echoes shows
//!   exactly what a typed fill shows. A refusal the clipboard walks around is
//!   not a boundary; it is friction, charged to the user who is entitled to
//!   the value.
//! - **It was the confirmation's argument over again.** The user put the caret
//!   in the box and asked for this value by name. *"User intentionally sends
//!   the whatever needed - their right"* is the ruling that removed the card,
//!   and it covers which box they chose as much as whether they are asked
//!   twice.
//!
//! **What stays is a wrong-WINDOW guard, which is a different thing.**
//! [`Refusal::WrongProcess`] is not about how a field draws what is typed into
//! it but about whose field it is: an item whose app-match rule names
//! `saplogon.exe` is not typed into Slack, because the likeliest way that
//! happens is focus moving between the pick and the keystroke -- a fill the
//! user did not ask for, into a window they did not choose. [`Gated::NoTarget`]
//! stays with it: a foreground that cannot be described cannot be checked
//! against a rule, and an unknown target must not read as a safe one. Neither
//! needs UI Automation, so `injector::target::describe_foreground` no longer
//! asks it anything, and a provider that fails to answer can no longer turn
//! into a refused fill.

use crate::injector::target::SendTarget;

// ---------------------------------------------------------------------------
// The decision
// ---------------------------------------------------------------------------

/// Why the gate declined a fill.
///
/// **One reason, and it used to be two.** The second was `NotMasked` -- an
/// unmasked control holding focus -- and it is gone on the owner's ruling; see
/// the module doc. It is still an enum rather than a unit struct because the
/// reason is what [`refusal_notice`] words, and a gate that grows a reason
/// again should grow a variant and a sentence, not a boolean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The focused window is not the process this rule was written for.
    WrongProcess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allowed,
    Refused(Refusal),
}

/// The whole gate, as a pure function so it can be tested without a window.
///
/// **Its one refusal is outright, and was never "ask the user".** That
/// distinction mattered while a card stood in front of this function, so it is
/// worth writing down what the card did and did not do: it *rendered* the
/// verdict, in one of two shapes, and it could never change it. A
/// `Verdict::Refused` laid out no hold affordance at all -- there was nothing
/// on the refused card a user could press to send anyway -- and `run_with`
/// answered `Cancel` for a send event in that state as a third barrier. So
/// there is no arm here that loses its meaning when the human leaves: removing
/// the confirmation removed a screen, not a branch.
///
/// **What it asks is whose window this is, and nothing about the control in
/// it.** It used to take a `sequence_has_secret` flag and refuse a secret into
/// a control UI Automation did not report as masked. Every caller passed
/// `true`, and the refusal it guarded went on the owner's ruling (see the
/// module doc), so the flag went with it: a parameter that no longer changes
/// the answer is one a reader would reasonably believe still does.
pub fn verdict(target: &SendTarget, rule_image: &str) -> Verdict {
    if !crate::injector::target::matches_rule(target, rule_image) {
        return Verdict::Refused(Refusal::WrongProcess);
    }
    Verdict::Allowed
}

// ---------------------------------------------------------------------------
// The gate, in the position that gates
// ---------------------------------------------------------------------------

/// What a gated send did, or did not do.
///
/// [`Self::NoTarget`] is its own arm rather than a `Refused`: it means the
/// foreground could not be described at all, which is not the wrong-window
/// refusal the design words and must not be reported as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gated<T> {
    Sent(T),
    Refused(Refusal),
    NoTarget,
}

/// The one place a secret-typing fill may reach the sender, **and it is a
/// function rather than an `if` inside a fill** for the reason
/// `updater::installer_is_launchable` records in full: a pin on a pure
/// decision cannot see whether the decision is in a gating position. Measured
/// on this crate, neutralising such a gate to a `let _ = decision(..);`
/// survived the entire suite at zero warnings.
///
/// So the gate lives behind a seam that a test can drive end to end. `describe`
/// is `injector::target::describe_foreground` in production and a fixture in a
/// test; `send` is the real fill in production and a recorder in a test. The
/// assertions are then about ROUTING -- that `send` is NOT reached for a wrong
/// process or an undescribable foreground, and IS reached for the allowed case,
/// which since the owner's ruling includes an unmasked control in the right
/// process. Deleting the refusal branch, or neutralising it, breaks those
/// assertions; a pin on [`verdict`] alone would break on neither.
///
/// # One field, and it used to be two
///
/// The second was `confirm`, the seam design 4b's card was hosted behind. It
/// is gone with the card -- see the module doc for the owner's reasoning. The
/// struct is kept for the field that is left, because that field is the whole
/// reason a live Win32 foreground lookup can be driven from a test at all.
///
/// # `fn` pointer rather than `impl Fn` for `describe`
///
/// A seam that is itself unpinned only MOVES the hole, so
/// [`SendGate::production`] hands over the real `describe_foreground` by
/// identity and [`production_holds_the_real_foreground_lookup`] asserts that
/// with `std::ptr::fn_addr_eq`. A wrapper, a forwarder or a flag-gated no-op
/// is a different address and fails there, whatever it is spelled.
pub struct SendGate {
    /// [`crate::injector::target::describe_foreground`] in production.
    describe: fn() -> Option<SendTarget>,
}

impl SendGate {
    pub fn production() -> Self {
        Self { describe: crate::injector::target::describe_foreground }
    }

    /// The foreground, through the gate's own seam.
    ///
    /// Public because the seam is the point: a caller that wants to know where
    /// a fill would land must make the **same** observation [`dispatch_with`]
    /// will make, rather than a second one taken from somewhere else.
    pub fn describe(&self) -> Option<SendTarget> {
        (self.describe)()
    }
}

/// Whether this particular fill is one the preflight speaks for.
///
/// **[`Self::NotRequired`] does not call `describe` at all**, and that is not
/// an optimisation. Describing the foreground is a Win32 round trip that can
/// come back empty -- a window gone between the hotkey and the look, a process
/// whose image name cannot be read -- and an empty answer is a refusal. So
/// asking the question about a fill the gate has nothing to say about would
/// turn a foreground Win32 could not name into a broken fill for every item in
/// the vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard<'a> {
    /// This fill types a secret into **whatever holds focus**, which is the
    /// case section 4b is written about. `rule_image` is the image name the
    /// item's app-match rule records, when it records one.
    Preflight { rule_image: Option<&'a str> },
    /// Not a bare-secret fill. See the type doc.
    NotRequired,
}

/// Runs `send` **only** when the foreground can be described and [`verdict`]
/// allows it.
///
/// `send` is `FnOnce` so it cannot be run twice and cannot be run at all
/// without being consumed -- a refusal drops it unused, which is the state the
/// compiler makes visible.
///
/// **Nothing stands between the caller and this function any more.** A fill
/// the user asked for reaches the sender on the first pass, with no window
/// opened, no key held and no second question: the only thing between the
/// hotkey and the keystroke is this function's own arithmetic.
pub fn dispatch_with<T>(
    gate: &SendGate,
    guard: Guard<'_>,
    send: impl FnOnce() -> T,
) -> Gated<T> {
    let rule_image = match guard {
        Guard::NotRequired => return Gated::Sent(send()),
        Guard::Preflight { rule_image } => rule_image,
    };
    let Some(target) = (gate.describe)() else {
        return Gated::NoTarget;
    };
    // No rule was ever recorded for this item, so there is no process claim to
    // check and `verdict` is satisfied by the target's own image. What a
    // rule-less item is still gated on is the line above: a foreground that
    // could be described at all. Nothing is asked about the control holding
    // focus, for either kind of item -- see the module doc for the owner's
    // ruling that took the masking check out.
    let rule = rule_image.unwrap_or(target.image_name.as_str());
    match verdict(&target, rule) {
        Verdict::Refused(why) => Gated::Refused(why),
        Verdict::Allowed => Gated::Sent(send()),
    }
}

/// **Design 4b's word for a fill that did not happen**, and the one string
/// from that card's table which outlived it.
///
/// The card painted it across the top of its refused shape. Nothing paints it
/// now, so it leads every sentence [`refusal_notice`] composes instead -- the
/// user still reads "Nothing sent" first, in the one place a refusal now
/// reaches them.
pub const REFUSED_HEADING: &str = "Nothing sent";

/// What the user is told when a gated fill did not happen. Reaches them
/// through the same [`crate::injector::sequence::Notifier`] every other
/// refusal uses -- a fill that quietly does nothing is indistinguishable from
/// a hotkey that never registered.
///
/// **This is now the only channel, and that is the whole of what the card's
/// removal cost here.** It used to be the second of two: a refused verdict was
/// computed in `app::confirmed_by_preflight` as well, the card was drawn in its
/// refusal shape, and the fill returned before `dispatch_with` was ever
/// reached -- so in production this function's text was what nobody saw. The
/// surviving channel is the tested one, and it fires on both refusing shapes:
/// the wrong window, and a window that could not be told at all.
///
/// **There were three sentences, and the third is gone with its refusal.** "The
/// control holding focus is not a masked field" was the box a user met when
/// they asked for a one-time code in the one-time-code box of a web page -- the
/// fill the owner most wanted to work. It is not reworded into a warning; there
/// is no longer anything to warn about.
///
/// What is **not** offered any more is *Copy instead*, the escape the card put
/// beside its refusal. It was an affordance of a screen, and it needed that
/// screen: a button, the value in hand, and a user standing in front of both.
/// The value is still one chord away in the vault window and in
/// `picker_prompt`, which is where every other copy in this app already lives.
pub fn refusal_notice(gated_reason: Option<Refusal>) -> String {
    let why = match gated_reason {
        Some(Refusal::WrongProcess) => "The window in front is not the one this item's rule was \
             written for. Deskwarden will not type a password there.",
        None => "Deskwarden could not tell which window is in front, so it did not type anything.",
    };
    format!("{REFUSED_HEADING}. {why}")
}

/// A gate whose foreground is a **fixture**, for the tests that drive a whole
/// fill. Not available in a shipping build: `describe_foreground` is a live
/// Win32 round trip, and a test that reached it would be asking the
/// machine it runs on where the mouse is.
///
/// Written down here, below everything production, so that a source pin that
/// reads the region above the first gate still sees the whole of the
/// production half.
#[cfg(test)]
impl SendGate {
    pub fn describing(describe: fn() -> Option<SendTarget>) -> Self {
        Self { describe }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::injector::target::SendTarget;

    /// A target in `image`. **There is no masking bit to set**: `SendTarget`
    /// stopped carrying one when the refusal that read it was removed, so
    /// every fixture here is equally a password box, a user-name box or a
    /// web page's one-time-code input -- which is the point.
    fn t(image: &str) -> SendTarget {
        SendTarget {
            title: "SAP Logon 760 - Sign in".into(),
            image_name: image.into(),
            pid: 7412,
            class_name: "SAPFEWndClass".into(),
        }
    }

    #[test]
    fn a_secret_needs_the_right_process_and_nothing_about_the_control() {
        assert_eq!(
            verdict(&t("saplogon.exe"), "saplogon.exe"),
            Verdict::Allowed,
            "the rule's own process, whatever box holds focus in it, is where the user asked \
             for this value to go"
        );
        assert_eq!(
            verdict(&t("slack.exe"), "saplogon.exe"),
            Verdict::Refused(Refusal::WrongProcess),
            "the design's own example: a password must not reach a chat box"
        );
    }

    /// **The refusal still says what happened, now that the card that said it
    /// is gone.**
    ///
    /// This is the one surviving channel for a declined fill, so both
    /// sentences are asserted here rather than left to the surface that used
    /// to paint them. Each one leads with [`REFUSED_HEADING`] -- design 4b's
    /// own word for this state -- and each names the fact that is wrong, so a
    /// user who pressed the hotkey knows which of "press it again" and "you
    /// are in the wrong window" is their next move.
    #[test]
    fn every_refusal_says_nothing_was_sent_and_why() {
        let wrong = refusal_notice(Some(Refusal::WrongProcess));
        let unknown = refusal_notice(None);
        for notice in [&wrong, &unknown] {
            assert!(
                notice.starts_with(REFUSED_HEADING),
                "{notice:?} does not lead with the design's word for this state"
            );
            // The masking refusal is gone on the owner's ruling, so no notice
            // may still blame the control -- a user told "not a masked field"
            // would go looking for a setting that no longer exists.
            assert!(!notice.contains("masked"), "{notice:?} still words the masking refusal");
        }
        assert!(wrong.contains("not the one this item's rule was written for"), "{wrong:?}");
        assert!(unknown.contains("could not tell which window is in front"), "{unknown:?}");
        // Control on the instrument: the two are distinguishable, so a
        // `refusal_notice` that answered one string for every reason would
        // fail here rather than pass the assertions above.
        assert_ne!(wrong, unknown);
    }

    // -- the gate, in the position that gates ------------------------------
    //
    // Every test below drives `dispatch_with` and asks whether `send` RAN.
    // That is the question `updater::installer_is_launchable` records as the
    // one a pin on a pure decision cannot answer: neutralising a gate to a
    // `let _ = decision(..);` was measured surviving this crate's whole suite
    // at zero warnings, because nothing observed the gate's POSITION.

    /// The rule's own process with the caret in an ordinary, echoing box -- a
    /// one-time-code input, or a web page's password field a browser does not
    /// report as masked. The fill this gate used to refuse and now sends.
    fn an_unmasked_box_in_the_right_process() -> Option<SendTarget> {
        Some(t("saplogon.exe"))
    }
    fn a_chat_box() -> Option<SendTarget> {
        Some(t("slack.exe"))
    }
    fn nothing_at_all() -> Option<SendTarget> {
        None
    }
    /// A `describe` that must never be called: `Guard::NotRequired` may not
    /// pay for a foreground lookup, and a fill it does not speak for may not be
    /// broken by a foreground Win32 could not name.
    fn must_not_be_asked() -> Option<SendTarget> {
        panic!("the foreground was described for a fill the preflight does not speak for");
    }

    /// Runs a gated send that records whether it ran, and hands back both.
    fn run(describe: fn() -> Option<SendTarget>, guard: Guard<'_>) -> (Gated<()>, bool) {
        let mut sent = false;
        let gated = dispatch_with(&SendGate::describing(describe), guard, || sent = true);
        (gated, sent)
    }

    const RULE: Guard<'static> = Guard::Preflight { rule_image: Some("saplogon.exe") };

    /// **An allowed fill goes straight through**, which is the owner's
    /// instruction stated as a test: one call, no confirmation, and the sender
    /// ran. There is nothing to hold, nothing to press and nothing to dismiss
    /// between a fill the user asked for and the keystrokes it types.
    #[test]
    fn the_sender_runs_only_for_an_allowed_verdict() {
        let (gated, sent) = run(an_unmasked_box_in_the_right_process, RULE);
        assert_eq!(gated, Gated::Sent(()));
        assert!(sent, "the allowed case did not reach the sender -- the gate is shut on everything");
    }

    #[test]
    fn no_path_reaches_the_sender_without_an_allowed_verdict() {
        // Wrong process: the design's own example, a password toward a chat box.
        let (gated, sent) = run(a_chat_box, RULE);
        assert_eq!(gated, Gated::Refused(Refusal::WrongProcess));
        assert!(!sent, "a password reached the sender with the wrong window in front");

        // Nothing describable: an unknown target must not read as a safe one.
        let (gated, sent) = run(nothing_at_all, RULE);
        assert_eq!(gated, Gated::NoTarget);
        assert!(!sent, "a password reached the sender with no idea where it was going");
    }

    /// **The owner's ruling, stated as a test: an unmasked control in the
    /// right process is SENT.** "Allow this - our app can enter TOTP or
    /// whatever to web pages as well" -- the one-time-code box of a web page is
    /// never masked, and this is the fill that used to answer "Nothing sent".
    ///
    /// Asserted for a ruled item and a rule-less one, because the old refusal
    /// applied to both: a rule-less item was gated on the masking half alone,
    /// so it is the case a half-removed check would still break.
    #[test]
    fn an_unmasked_control_in_the_right_process_is_sent() {
        let (gated, sent) = run(an_unmasked_box_in_the_right_process, RULE);
        assert_eq!(gated, Gated::Sent(()));
        assert!(sent, "a ruled item was refused into its own process's echoing box");

        let unruled = Guard::Preflight { rule_image: None };
        let (gated, sent) = run(an_unmasked_box_in_the_right_process, unruled);
        assert_eq!(gated, Gated::Sent(()));
        assert!(sent, "a rule-less item was refused into an echoing box");
    }

    /// The rule-less item is still GATED, just not on the control: with no
    /// process claim to check, what is left is that the foreground could be
    /// described at all. Without this the test above would pass against a
    /// `Guard::Preflight { rule_image: None }` that had quietly become
    /// `NotRequired`.
    #[test]
    fn an_item_with_no_rule_is_still_refused_an_undescribable_foreground() {
        let unruled = Guard::Preflight { rule_image: None };
        let (gated, sent) = run(nothing_at_all, unruled);
        assert_eq!(gated, Gated::NoTarget);
        assert!(!sent, "a rule-less item typed a password with no idea where it was going");
    }

    /// `NotRequired` sends without describing anything at all -- asserted by
    /// handing it a `describe` that panics if it is reached.
    #[test]
    fn a_fill_the_gate_does_not_speak_for_is_sent_without_asking_the_foreground() {
        let (gated, sent) = run(must_not_be_asked, Guard::NotRequired);
        assert_eq!(gated, Gated::Sent(()));
        assert!(sent);
    }

    /// The seam itself, pinned by ADDRESS. A seam that is unpinned only moves
    /// the hole: production could hand over a wrapper that always answers
    /// "the rule's own process" and every routing test above would still pass.
    #[test]
    fn production_holds_the_real_foreground_lookup() {
        assert!(
            std::ptr::fn_addr_eq(
                SendGate::production().describe,
                crate::injector::target::describe_foreground
                    as fn() -> Option<crate::injector::target::SendTarget>
            ),
            "the production gate does not look at the real foreground window"
        );
    }
}

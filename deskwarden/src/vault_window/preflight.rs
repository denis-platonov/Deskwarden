//! **4b -- the only screen between a sequence and a real password.**
//!
//! Two halves, and the split is the same one `injector::target` makes.
//!
//! The DECISION is [`verdict`]: a pure function of a [`SendTarget`], the image
//! name the rule was written for, and whether the sequence types a secret. It
//! has no window, no COM and no vault in it, so every branch is reachable from
//! a unit test -- and, more importantly, so the gate can be exercised from
//! *the position it gates* (see [`dispatch_with`]).
//!
//! The SURFACE is [`draw`]: it names the window rather than the rule, lists
//! the steps with secrets masked, asks for a *hold* rather than a click, and
//! when the verdict is a refusal it paints no send affordance at all.
//!
//! # The step list is not built here
//!
//! [`PreflightState::new`] asks
//! [`crate::vault_window::detail_edit::step_rows`] for its rows and this file
//! contains no other way to make one. That is deliberate and it is pinned
//! ([`the_step_list_is_the_editors_and_is_never_rebuilt_here`]): `step_rows`
//! writes [`crate::vault_window::detail_edit::SECRET_MASK`] for a password in
//! an `if` whose `else` is the only branch that can resolve a value, so the
//! masking is unconditional BY CONSTRUCTION rather than by a `reveal` argument
//! being passed `false`. A second row builder here -- even one that also
//! masked -- would be a second place for that property to stop being true.

use super::detail_edit::{step_rows, StepRow};
use crate::injector::target::SendTarget;
use crate::key_sequence::ResolveSource;
use crate::theme;
use eframe::egui::{self, CornerRadius, Margin, Ui};
use std::time::Duration;

// ---------------------------------------------------------------------------
// The decision
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The focused window is not the process this rule was written for.
    WrongProcess,
    /// The focused control is not a masked field, and this sequence types a secret.
    NotMasked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allowed,
    Refused(Refusal),
}

/// The whole gate, as a pure function so it can be tested without a window.
///
/// Order matters for the message the user sees: naming the wrong process is
/// more useful than naming the wrong control, because it is the more likely
/// mistake and the more dangerous one.
pub fn verdict(target: &SendTarget, rule_image: &str, sequence_has_secret: bool) -> Verdict {
    if !crate::injector::target::matches_rule(target, rule_image) {
        return Verdict::Refused(Refusal::WrongProcess);
    }
    if sequence_has_secret && !target.focused_is_masked {
        return Verdict::Refused(Refusal::NotMasked);
    }
    Verdict::Allowed
}

// ---------------------------------------------------------------------------
// The gate, in the position that gates
// ---------------------------------------------------------------------------

/// What a gated send did, or did not do.
///
/// [`Self::NoTarget`] is its own arm rather than a `Refused`: it means the
/// foreground could not be described at all, which is neither of the two
/// refusals the design words and must not be reported as one.
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
/// process, an unmasked control or an undescribable foreground, and IS reached
/// for the allowed case. Deleting the refusal branch, or neutralising it,
/// breaks those assertions; a pin on [`verdict`] alone would break on neither.
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
    /// [`crate::preflight_host::show_preflight`] in production: the modal that
    /// hosts [`draw`]. See [`Self::confirm`] for why it is here and not inside
    /// [`dispatch_with`].
    confirm: fn(PreflightState, zeroize::Zeroizing<String>) -> Option<PreflightAction>,
}

impl SendGate {
    pub fn production() -> Self {
        Self {
            describe: crate::injector::target::describe_foreground,
            confirm: crate::preflight_host::show_preflight,
        }
    }

    /// The foreground, through the gate's own seam.
    ///
    /// Public so that the caller can build a [`PreflightState`] out of the
    /// **same** observation [`dispatch_with`] will make, rather than a second
    /// one taken from somewhere else -- a preflight that named one window and
    /// a gate that checked another would be worse than no preflight.
    pub fn describe(&self) -> Option<SendTarget> {
        (self.describe)()
    }

    /// Puts the 4b confirmation on screen and answers what the user did.
    ///
    /// # This is *ahead of* the gate and never *instead of* it
    ///
    /// The confirmation cannot decide anything. Its only affirmative answer is
    /// [`PreflightAction::Send`], and all that answer does is let the caller
    /// go on to call [`dispatch_with`], which describes the foreground again
    /// and refuses on its own terms. So there is no ordering of clicks, holds
    /// or window switches that reaches a sender without the refusal arms in
    /// `dispatch_with` having allowed it, and the mutation measurement those
    /// arms carry is unchanged by hosting the surface -- the tests drive this
    /// seam with a stub that always answers `Send`, so what they measure is
    /// still the gate alone.
    ///
    /// **The measurement is not quoted here.** It used to be, as
    /// "neutralise: 3 red, delete: 2 red", and it was not reproducible: the
    /// prose did not pin the mutants closely enough for two readers to write
    /// the same ones. They now live as anchored source replacements under
    /// `mutations/cases/`, and `mutations/run.ps1` applies each to a
    /// throwaway worktree and prints the count and the killing test names.
    /// The names are the part worth reading -- a count that moved says
    /// nothing on its own about whether the same escape is still caught.
    ///
    /// The other direction is a real gain: [`draw`]'s refusal state paints no
    /// hold affordance at all, so a refused target never even offers the user
    /// a way to ask.
    pub fn confirm(
        &self,
        state: PreflightState,
        copy_payload: zeroize::Zeroizing<String>,
    ) -> Option<PreflightAction> {
        (self.confirm)(state, copy_payload)
    }
}

/// Whether this particular fill is one the preflight speaks for.
///
/// **[`Self::NotRequired`] does not call `describe` at all**, and that is not
/// an optimisation. Describing the foreground is a COM round trip that can
/// fail, and a failure is a refusal -- so asking the question about a fill the
/// gate has nothing to say about would turn an unreachable UI Automation
/// provider into a broken fill for every item in the vault.
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
    // check and the process half of `verdict` is satisfied by the target's own
    // image. The masking half -- the one that matters for a bare secret typed
    // at the caret -- is asked either way. Written as an explicit `None` arm
    // rather than an `unwrap_or` so that "this item has no rule" is a state
    // named in the source and not an argument that happens to compare equal.
    let rule = rule_image.unwrap_or(target.image_name.as_str());
    match verdict(&target, rule, true) {
        Verdict::Refused(why) => Gated::Refused(why),
        Verdict::Allowed => Gated::Sent(send()),
    }
}

/// What the user is told when a gated fill did not happen. Reaches them
/// through the same [`crate::injector::sequence::Notifier`] every other
/// refusal uses -- a fill that quietly does nothing is indistinguishable from
/// a hotkey that never registered.
pub fn refusal_notice(gated_reason: Option<Refusal>) -> String {
    match gated_reason {
        Some(Refusal::WrongProcess) => "The window in front is not the one this item's rule was \
             written for. Deskwarden will not type a password there."
            .to_string(),
        Some(Refusal::NotMasked) => "The control holding focus is not a masked field. Deskwarden \
             will not type a password into a box that echoes it."
            .to_string(),
        None => "Deskwarden could not tell which window is in front, so it did not type anything."
            .to_string(),
    }
}

// ---------------------------------------------------------------------------
// The surface
// ---------------------------------------------------------------------------

/// What the user did with the preflight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreflightAction {
    /// The hold completed. The only value that may reach a sender.
    Send,
    Cancel,
    /// The escape the design offers beside the refusal: put the value on the
    /// clipboard and let the user place it themselves.
    CopyInstead,
}

/// How long the send key must be held down before [`PreflightAction::Send`] is
/// emitted.
///
/// Long enough that it cannot be a stray keypress on a window that has just
/// taken focus, short enough not to read as a hang. A click is not an option
/// at all -- see [`draw`].
pub const HOLD_TO_SEND: Duration = Duration::from_millis(800);

/// The design's own words, kept as constants so the tests assert on the same
/// strings the surface paints rather than on copies of them.
pub const HEADING_TARGET: &str = "About to type into";
pub const HEADING_STEPS: &str = "Will send";
pub const HOLD_HINT: &str = "Hold Space to send";
pub const CANCEL_LABEL: &str = "Cancel \u{b7} Esc";
pub const COPY_INSTEAD_LABEL: &str = "Copy instead";
pub const DISMISS_LABEL: &str = "Dismiss";
pub const FOOTNOTE: &str = "Sending stops the moment focus leaves this window.";
pub const REFUSED_HEADING: &str = "Nothing sent";
/// The label on the masked step, in the design's words.
pub const MASKED_ONLY: &str = "masked field only";

/// Everything the surface needs, and nothing it does not.
///
/// It holds no vault item and no resolved secret: the rows were built once by
/// [`step_rows`], which never puts a password in one.
pub struct PreflightState {
    pub target: SendTarget,
    pub rule_image: String,
    pub rows: Vec<StepRow>,
    pub verdict: Verdict,
    /// How long the send key has been held down, accumulated across frames.
    /// Reset to zero the moment the key comes up, so a series of taps never
    /// adds up to a send.
    pub held: Duration,
}

impl PreflightState {
    /// The rows come from the editor's [`step_rows`] with the eye SHUT, which
    /// is the only call to it in this file. See the module doc.
    pub fn new(
        target: SendTarget,
        rule_image: &str,
        sequence: &str,
        source: &ResolveSource<'_>,
    ) -> Self {
        let rows = step_rows(sequence, source, false);
        let has_secret = rows.iter().any(|r| r.secret);
        let verdict = verdict(&target, rule_image, has_secret);
        Self {
            target,
            rule_image: rule_image.to_string(),
            rows,
            verdict,
            held: Duration::ZERO,
        }
    }

    /// Whether this sequence types something that must never be echoed in
    /// clear. Read off the rows rather than re-parsed, so it cannot disagree
    /// with what the list shows.
    pub fn has_secret(&self) -> bool {
        self.rows.iter().any(|r| r.secret)
    }
}

/// The line under the window title: the image, the pid, and whether the rule
/// claims this process.
pub fn target_line(state: &PreflightState) -> String {
    let claim = if crate::injector::target::matches_rule(&state.target, &state.rule_image) {
        "matches this rule"
    } else {
        "does not match this rule"
    };
    format!("{} \u{b7} pid {} \u{b7} {claim}", state.target.image_name, state.target.pid)
}

/// The refusal, in words that name the window the user is actually looking at.
///
/// Both refusals say plainly that the sequence types a password and that it
/// will not be sent here; what differs is which fact is wrong, and the design
/// says both when both are.
pub fn refusal_message(state: &PreflightState, why: Refusal) -> String {
    let wrong_process = matches!(why, Refusal::WrongProcess);
    let unmasked = !state.target.focused_is_masked;
    let mut reasons = Vec::new();
    if wrong_process {
        reasons.push(format!(
            "The focused window is {}, not {}",
            state.target.image_name, state.rule_image
        ));
    }
    if unmasked {
        reasons.push("the focused control is not masked".to_string());
    }
    format!(
        "{}. This sequence types a password \u{2014} Deskwarden will not send it here.",
        reasons.join(", and ")
    )
}

/// The preflight, drawn.
///
/// **There is no send button.** The most dangerous action in the app must not
/// be reachable by a stray click on a window that just took focus, so the send
/// is a held key: `held` accumulates while the key is down and is thrown away
/// the moment it is not. Nothing here returns [`PreflightAction::Send`] on a
/// click, and no code path returns it while the verdict is a refusal.
pub fn draw(ui: &mut Ui, state: &mut PreflightState) -> Option<PreflightAction> {
    let mut action = None;
    egui::Frame::new()
        .fill(theme::CARD)
        .stroke(egui::Stroke::new(1.0, theme::HAIRLINE))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| match state.verdict {
            Verdict::Allowed => action = draw_allowed(ui, state),
            Verdict::Refused(why) => action = draw_refused(ui, state, why),
        });
    // Esc cancels from either state -- the one control the design gives both.
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = Some(PreflightAction::Cancel);
    }
    action
}

fn draw_allowed(ui: &mut Ui, state: &mut PreflightState) -> Option<PreflightAction> {
    ui.label(egui::RichText::new(HEADING_TARGET).size(11.0).color(theme::TEXT_MUTED));
    ui.label(egui::RichText::new(&state.target.title).size(15.0).color(theme::INK));
    ui.label(egui::RichText::new(target_line(state)).size(11.0).color(theme::TEXT_SECONDARY));
    ui.add_space(12.0);
    ui.label(egui::RichText::new(HEADING_STEPS).size(11.0).color(theme::TEXT_MUTED));
    for row in &state.rows {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{}", row.number)).size(11.0).color(theme::TEXT_MUTED),
            );
            ui.label(egui::RichText::new(&row.label).size(12.0).color(theme::INK));
            if !row.payload.is_empty() {
                ui.label(
                    egui::RichText::new(&row.payload).size(12.0).color(theme::TEXT_SECONDARY),
                );
            }
            if row.secret {
                ui.label(egui::RichText::new(MASKED_ONLY).size(11.0).color(theme::TEXT_MUTED));
            }
        });
    }
    ui.add_space(12.0);

    // The hold. `stable_dt` rather than a wall clock so a stalled frame cannot
    // credit the user with time they did not hold the key for.
    let (down, dt) = ui.input(|i| (i.key_down(egui::Key::Space), i.stable_dt));
    let mut action = None;
    if down {
        state.held += Duration::from_secs_f32(dt.max(0.0));
        if state.held >= HOLD_TO_SEND {
            action = Some(PreflightAction::Send);
        }
    } else {
        state.held = Duration::ZERO;
    }

    let fraction = (state.held.as_secs_f32() / HOLD_TO_SEND.as_secs_f32()).clamp(0.0, 1.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(220.0, 30.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(6), theme::BLUE_WASH);
    if fraction > 0.0 {
        let mut filled = rect;
        filled.set_width(rect.width() * fraction);
        ui.painter().rect_filled(filled, CornerRadius::same(6), theme::BLUE_EDGE);
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        HOLD_HINT,
        egui::FontId::proportional(12.0),
        theme::BLUE,
    );

    ui.horizontal(|ui| {
        if ui.add(egui::Button::new(egui::RichText::new(CANCEL_LABEL).size(12.0))).clicked() {
            action = Some(PreflightAction::Cancel);
        }
        if ui.add(egui::Button::new(egui::RichText::new(COPY_INSTEAD_LABEL).size(12.0))).clicked() {
            action = Some(PreflightAction::CopyInstead);
        }
    });
    ui.add_space(8.0);
    ui.label(egui::RichText::new(FOOTNOTE).size(11.0).color(theme::TEXT_MUTED));
    action
}

/// The refusal state. **Paints no hold affordance and reads no key**, so there
/// is no frame on which a held Space can accumulate toward a send.
fn draw_refused(ui: &mut Ui, state: &PreflightState, why: Refusal) -> Option<PreflightAction> {
    ui.label(egui::RichText::new(REFUSED_HEADING).size(15.0).color(theme::ERROR));
    ui.label(egui::RichText::new(&state.target.title).size(13.0).color(theme::INK));
    ui.label(
        egui::RichText::new(format!(
            "{} \u{b7} {} focused",
            state.target.image_name, state.target.class_name
        ))
        .size(11.0)
        .color(theme::TEXT_SECONDARY),
    );
    ui.add_space(10.0);
    ui.label(egui::RichText::new(refusal_message(state, why)).size(12.0).color(theme::INK));
    ui.add_space(12.0);
    let mut action = None;
    ui.horizontal(|ui| {
        if ui.add(egui::Button::new(egui::RichText::new(DISMISS_LABEL).size(12.0))).clicked() {
            action = Some(PreflightAction::Cancel);
        }
        if ui.add(egui::Button::new(egui::RichText::new(COPY_INSTEAD_LABEL).size(12.0))).clicked() {
            action = Some(PreflightAction::CopyInstead);
        }
    });
    action
}

/// A gate whose foreground is a **fixture**, for the tests that drive a whole
/// fill. Not available in a shipping build: `describe_foreground` is a live
/// Win32 + COM round trip, and a test that reached it would be asking the
/// machine it runs on where the mouse is.
///
/// Written down here, below everything production, so that this file's own
/// source pin -- which reads the region above the first gate -- still sees the
/// whole of the production half.
#[cfg(test)]
impl SendGate {
    /// A gate whose foreground is a fixture and whose confirmation **always
    /// says yes**.
    ///
    /// Saying yes is the point: it takes the hosted modal out of the picture
    /// entirely, so every routing assertion built on this constructor is
    /// measuring `dispatch_with`'s refusal arms and nothing else. If the
    /// confirmation could refuse here, a deleted gate would still look green
    /// and the whole measurement would be worthless.
    pub fn describing(describe: fn() -> Option<SendTarget>) -> Self {
        Self { describe, confirm: |_, _| Some(PreflightAction::Send) }
    }

    /// A gate whose confirmation is a fixture too, for the tests that ask
    /// whether the surface is HOSTED -- i.e. whether a gated fill really opens
    /// it before anything is typed.
    /// The confirmation seam, by identity, for the address pin in
    /// `app::fill_dispatch_tests`. A getter and not a `pub` field so that
    /// production code still cannot reach past [`Self::confirm`].
    pub fn confirm_fn(
        &self,
    ) -> fn(PreflightState, zeroize::Zeroizing<String>) -> Option<PreflightAction> {
        self.confirm
    }

    pub fn describing_and_confirming(
        describe: fn() -> Option<SendTarget>,
        confirm: fn(PreflightState, zeroize::Zeroizing<String>) -> Option<PreflightAction>,
    ) -> Self {
        Self { describe, confirm }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::injector::target::SendTarget;

    fn t(image: &str, masked: bool) -> SendTarget {
        SendTarget {
            title: "SAP Logon 760 - Sign in".into(),
            image_name: image.into(),
            pid: 7412,
            class_name: "SAPFEWndClass".into(),
            focused_is_masked: masked,
        }
    }

    #[test]
    fn a_secret_sequence_needs_the_right_process_and_a_masked_control() {
        assert_eq!(verdict(&t("saplogon.exe", true), "saplogon.exe", true), Verdict::Allowed);
        assert_eq!(
            verdict(&t("slack.exe", true), "saplogon.exe", true),
            Verdict::Refused(Refusal::WrongProcess),
            "the design's own example: a password must not reach a chat box"
        );
        assert_eq!(
            verdict(&t("saplogon.exe", false), "saplogon.exe", true),
            Verdict::Refused(Refusal::NotMasked),
            "right window, wrong field -- a password typed into a username box is echoed in clear"
        );
    }

    #[test]
    fn a_sequence_with_no_secret_does_not_require_a_masked_control() {
        // A username-only sequence has nothing to leak into a visible field,
        // and requiring a masked control would make it unusable.
        assert_eq!(verdict(&t("saplogon.exe", false), "saplogon.exe", false), Verdict::Allowed);
    }

    // -- the gate, in the position that gates ------------------------------
    //
    // Every test below drives `dispatch_with` and asks whether `send` RAN.
    // That is the question `updater::installer_is_launchable` records as the
    // one a pin on a pure decision cannot answer: neutralising a gate to a
    // `let _ = decision(..);` was measured surviving this crate's whole suite
    // at zero warnings, because nothing observed the gate's POSITION.

    fn right_and_masked() -> Option<SendTarget> {
        Some(t("saplogon.exe", true))
    }
    fn right_and_unmasked() -> Option<SendTarget> {
        Some(t("saplogon.exe", false))
    }
    fn a_chat_box() -> Option<SendTarget> {
        Some(t("slack.exe", true))
    }
    fn nothing_at_all() -> Option<SendTarget> {
        None
    }
    /// A `describe` that must never be called: `Guard::NotRequired` may not
    /// pay for a COM round trip, and a fill it does not speak for may not be
    /// broken by an unreachable UI Automation provider.
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

    #[test]
    fn the_sender_runs_only_for_an_allowed_verdict() {
        let (gated, sent) = run(right_and_masked, RULE);
        assert_eq!(gated, Gated::Sent(()));
        assert!(sent, "the allowed case did not reach the sender -- the gate is shut on everything");
    }

    #[test]
    fn no_path_reaches_the_sender_without_an_allowed_verdict() {
        // Wrong process: the design's own example, a password toward a chat box.
        let (gated, sent) = run(a_chat_box, RULE);
        assert_eq!(gated, Gated::Refused(Refusal::WrongProcess));
        assert!(!sent, "a password reached the sender with the wrong window in front");

        // Right process, unmasked control: a password echoed in clear.
        let (gated, sent) = run(right_and_unmasked, RULE);
        assert_eq!(gated, Gated::Refused(Refusal::NotMasked));
        assert!(!sent, "a password reached the sender with an unmasked control focused");

        // Nothing describable: an unknown target must not read as a safe one.
        let (gated, sent) = run(nothing_at_all, RULE);
        assert_eq!(gated, Gated::NoTarget);
        assert!(!sent, "a password reached the sender with no idea where it was going");
    }

    /// The rule-less item: no process claim to check, and the masking half
    /// still applies. Both directions, so this is not a gate that is simply
    /// open.
    #[test]
    fn an_item_with_no_rule_is_still_gated_on_the_control_being_masked() {
        let unruled = Guard::Preflight { rule_image: None };
        let (gated, sent) = run(right_and_unmasked, unruled);
        assert_eq!(gated, Gated::Refused(Refusal::NotMasked));
        assert!(!sent, "a rule-less item typed a password into an unmasked control");

        let (gated, sent) = run(right_and_masked, unruled);
        assert_eq!(gated, Gated::Sent(()));
        assert!(sent, "a rule-less item into a masked field was refused, which breaks every fill");
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
    /// "masked" and every routing test above would still pass.
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

    /// **The masking is the editor's, not a copy of it.** `step_rows` writes
    /// `SECRET_MASK` for a password in a branch whose `else` is the only thing
    /// that can resolve a value; a second row builder here would be a second
    /// place for that to stop being true. So this file must build its rows
    /// exactly one way and must never spell a `StepRow` literal.
    #[test]
    fn the_step_list_is_the_editors_and_is_never_rebuilt_here() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/vault_window/preflight.rs"
        ))
        .expect("preflight.rs is readable");
        let production = source
            .split_once(concat!("#[cfg(", "test)]"))
            .map_or(source.as_str(), |(above, _)| above);
        assert!(
            production.len() < source.len(),
            "control: the test gate was not found, so this pin is reading its own fixtures"
        );
        assert_eq!(
            production.matches("step_rows(sequence, source, false)").count(),
            1,
            "the preflight's rows must come from the editor's own builder, with the eye shut"
        );
        assert_eq!(
            production.matches(concat!("StepRow", " {")).count(),
            0,
            "a step row is built by hand here, which is a second place for the masking to be \
             decided"
        );

        // Positive control on both needles: they match the spellings they are
        // meant to match, so a count of 1 is a real call and a count of 0 is a
        // real absence rather than a typo that matches nothing.
        let fixture = concat!("let rows = step_rows(sequence, source, false);\n", "StepRow", " {");
        assert_eq!(fixture.matches("step_rows(sequence, source, false)").count(), 1);
        assert_eq!(fixture.matches(concat!("StepRow", " {")).count(), 1);
    }
}

/// **The surface, drawn.**
///
/// The headless `Context::run_ui` idiom the rest of this crate uses. What
/// these can see is every string painted and every rectangle's geometry; what
/// they cannot see is hover cursors, focus rings or whether it *looks* right,
/// and nothing here pretends otherwise.
#[cfg(test)]
mod painted_tests {
    use super::*;
    use crate::injector::target::SendTarget;
    use crate::vault_bridge::{LoginData, VaultItem};
    use crate::vault_window::detail;
    use eframe::egui::{Pos2, Rect, Vec2};

    const PANE: Vec2 = Vec2::new(520.0, 700.0);
    const USERNAME: &str = "a.novak@ledgerline.com";
    const PASSWORD: &str = "correct-horse-battery-staple-92";
    const SEQUENCE: &str = "{USERNAME}{TAB}{PASSWORD}{ENTER}";

    #[derive(Default)]
    struct Painted {
        texts: Vec<String>,
        rects: Vec<Rect>,
    }

    impl Painted {
        fn strings(&self) -> Vec<&str> {
            self.texts.iter().map(String::as_str).collect()
        }
        fn contains(&self, needle: &str) -> bool {
            self.texts.iter().any(|t| t == needle)
        }
    }

    fn walk(shape: &egui::Shape, p: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => p.texts.push(text.galley.text().to_string()),
            egui::Shape::Rect(rect) => p.rects.push(rect.rect),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, p)),
            _ => {}
        }
    }

    fn item() -> VaultItem {
        VaultItem {
            id: "item-1".to_string(),
            name: "Ledgerline".to_string(),
            fields: Vec::new(),
            login: Some(LoginData {
                username: Some(USERNAME.to_string()),
                password: Some(PASSWORD.to_string().into()),
                totp: None,
                uris: Vec::new(),
                other: serde_json::Map::new(),
            }),
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: Some(1),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    fn target(image: &str, masked: bool) -> SendTarget {
        SendTarget {
            title: "SAP Logon 760 - Sign in".into(),
            image_name: image.into(),
            pid: 7412,
            class_name: "SAPFEWndClass".into(),
            focused_is_masked: masked,
        }
    }

    fn state_for(image: &str, masked: bool) -> PreflightState {
        let item = item();
        let totp = detail::TotpState::NoSecret;
        let login = item.login.as_ref().unwrap();
        let source = super::super::detail_edit::sequence_source(
            login.username.as_deref().unwrap_or(""),
            login.password.as_deref().map_or("", |v| v.as_str()),
            Some(&item),
            &totp,
        );
        PreflightState::new(target(image, masked), "saplogon.exe", SEQUENCE, &source)
    }

    fn raw_input(events: &[egui::Event]) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, PANE)),
            events: events.to_vec(),
            ..Default::default()
        }
    }

    fn styled_context() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw_input(&[]), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(raw_input(&[]), |_ui| {});
        ctx
    }

    fn frame(
        ctx: &egui::Context,
        state: &mut PreflightState,
        events: &[egui::Event],
    ) -> (Painted, Option<PreflightAction>) {
        let mut action = None;
        let output = ctx.run_ui(raw_input(events), |ui| action = draw(ui, state));
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        (painted, action)
    }

    /// A full primary press-and-release at `pos` -- what egui needs to report
    /// a click. A press alone is not one.
    fn click(pos: Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    // -- the allowed state -------------------------------------------------

    #[test]
    fn the_surface_names_the_window_and_not_the_rule() {
        let ctx = styled_context();
        let mut state = state_for("saplogon.exe", true);
        let (painted, action) = frame(&ctx, &mut state, &[]);
        assert!(action.is_none(), "a preflight that nobody touched decided something");
        assert!(painted.contains(HEADING_TARGET), "painted: {:?}", painted.strings());
        assert!(
            painted.contains("SAP Logon 760 - Sign in"),
            "the window's own title is the headline; painted: {:?}",
            painted.strings()
        );
        assert!(
            painted.contains("saplogon.exe \u{b7} pid 7412 \u{b7} matches this rule"),
            "painted: {:?}",
            painted.strings()
        );
        assert!(painted.contains(HOLD_HINT), "painted: {:?}", painted.strings());
        assert!(painted.contains(CANCEL_LABEL));
        assert!(painted.contains(COPY_INSTEAD_LABEL));
        assert!(painted.contains(FOOTNOTE));
    }

    /// The step list, with the secret masked and labelled -- and the password
    /// itself nowhere on the surface, in any string, at any point.
    #[test]
    fn the_password_step_is_masked_and_labelled_and_its_characters_are_never_painted() {
        let ctx = styled_context();
        let mut state = state_for("saplogon.exe", true);
        let (painted, _) = frame(&ctx, &mut state, &[]);

        // Positive control on the instrument: the list really is drawn, so an
        // absent password below is a mask and not an empty surface.
        assert!(painted.contains(HEADING_STEPS), "painted: {:?}", painted.strings());
        assert!(
            painted.contains(super::super::detail_edit::SECRET_MASK),
            "the masked payload was not painted at all; painted: {:?}",
            painted.strings()
        );
        assert!(painted.contains(MASKED_ONLY), "painted: {:?}", painted.strings());

        for s in painted.strings() {
            assert!(!s.contains(PASSWORD), "the password was painted in {s:?}");
        }
    }

    /// **The whole point of hold-to-send.** The most dangerous action in the
    /// app must not be reachable by a stray click on a window that just took
    /// focus, so every rectangle on the allowed surface is clicked and none of
    /// them may send.
    #[test]
    fn one_click_does_not_send() {
        let ctx = styled_context();
        let mut state = state_for("saplogon.exe", true);
        let (painted, _) = frame(&ctx, &mut state, &[]);
        let spots: Vec<Pos2> = painted.rects.iter().map(|r| r.center()).collect();
        assert!(!spots.is_empty(), "control: nothing was painted to click");
        for spot in spots {
            let (_, action) = frame(&ctx, &mut state, &click(spot));
            assert_ne!(
                action,
                Some(PreflightAction::Send),
                "a single click at {spot:?} sent the sequence"
            );
        }
        assert_eq!(state.held, Duration::ZERO, "a click accumulated hold time");
    }

    /// And the control is not merely inert: holding the key long enough does
    /// send, and letting go throws the accumulated time away.
    #[test]
    fn the_send_needs_the_key_held_and_a_release_throws_the_hold_away() {
        let ctx = styled_context();
        let mut state = state_for("saplogon.exe", true);

        // Not yet: one frame's worth of hold is far short of the threshold.
        let held = vec![egui::Event::Key {
            key: egui::Key::Space,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }];
        let (_, action) = frame(&ctx, &mut state, &held);
        assert_eq!(action, None, "one frame of holding sent the sequence");
        assert!(state.held > Duration::ZERO, "control: the hold did not accumulate at all");

        // Let go: the accumulated time goes.
        let released = vec![egui::Event::Key {
            key: egui::Key::Space,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }];
        let (_, action) = frame(&ctx, &mut state, &released);
        assert_eq!(action, None);
        assert_eq!(state.held, Duration::ZERO, "releasing did not reset the hold");

        // Held past the threshold: it sends.
        state.held = HOLD_TO_SEND;
        let (_, action) = frame(&ctx, &mut state, &held);
        assert_eq!(action, Some(PreflightAction::Send));
    }

    #[test]
    fn esc_cancels() {
        let ctx = styled_context();
        let mut state = state_for("saplogon.exe", true);
        let esc = vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }];
        let (_, action) = frame(&ctx, &mut state, &esc);
        assert_eq!(action, Some(PreflightAction::Cancel));
    }

    // -- the refusal state -------------------------------------------------

    /// The refusal names the focused window and says plainly that this
    /// sequence types a password and will not be sent there -- and paints **no
    /// send affordance at all**, so there is no frame on which a held key can
    /// accumulate toward one.
    #[test]
    fn the_refusal_names_the_focused_window_and_offers_no_way_to_send() {
        let ctx = styled_context();
        let mut state = state_for("slack.exe", false);
        assert_eq!(state.verdict, Verdict::Refused(Refusal::WrongProcess));

        let (painted, _) = frame(&ctx, &mut state, &[]);
        assert!(painted.contains(REFUSED_HEADING), "painted: {:?}", painted.strings());
        assert!(
            painted.contains("slack.exe \u{b7} SAPFEWndClass focused"),
            "painted: {:?}",
            painted.strings()
        );
        let sentence = painted
            .strings()
            .into_iter()
            .find(|s| s.contains("will not send it here"))
            .unwrap_or_else(|| panic!("no refusal sentence; painted: {:?}", painted.strings()))
            .to_string();
        assert!(sentence.contains("slack.exe"), "{sentence:?} does not name the focused window");
        assert!(sentence.contains("saplogon.exe"), "{sentence:?} does not name the rule");
        assert!(sentence.contains("the focused control is not masked"), "{sentence:?}");
        assert!(sentence.contains("types a password"), "{sentence:?}");
        assert!(painted.contains(DISMISS_LABEL));
        assert!(painted.contains(COPY_INSTEAD_LABEL));

        assert!(
            !painted.contains(HOLD_HINT),
            "the refusal painted a send affordance; painted: {:?}",
            painted.strings()
        );
        for s in painted.strings() {
            assert!(!s.contains(PASSWORD), "the password was painted in {s:?}");
        }
    }

    /// Holding the send key on the refusal does nothing, on every frame.
    #[test]
    fn the_refusal_cannot_be_held_into_a_send() {
        let ctx = styled_context();
        let mut state = state_for("slack.exe", false);
        let held = vec![egui::Event::Key {
            key: egui::Key::Space,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }];
        for _ in 0..30 {
            let (_, action) = frame(&ctx, &mut state, &held);
            assert_ne!(action, Some(PreflightAction::Send), "the refusal was held into a send");
        }
        assert_eq!(state.held, Duration::ZERO, "the refusal accumulated hold time");
    }
}

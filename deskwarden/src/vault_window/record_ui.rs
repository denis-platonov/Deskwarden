//! **The two surfaces of "send a whole record": the export form and the
//! import form.**
//!
//! The split in this file is the crate's usual one. Every decision either
//! surface makes is a **pure function of a draft** — [`export_problem`],
//! [`export_can_submit`], [`fields_present`], [`needs_passphrase`],
//! [`stale_note`], [`import_can_proceed`] — and the two `draw_*` functions
//! below do nothing but paint those answers. A rule written inside an `eframe`
//! closure is a rule no test in this crate can run, and the three rules that
//! matter most here are exactly the ones a user cannot recover from getting
//! wrong.
//!
//! # Export
//!
//! **This file builds a plan and does not publish it.** [`send_plan_from`]
//! turns a [`Record`] into a [`SendPlan`] with **`hidden: true`**, and the
//! plan then goes to `vault_window`'s existing `send_create_thread`, which is
//! the crate's one route to `bw send create` and is already guarded twice
//! over: `every_mention_of_the_blocking_create_is_sealed_inside_its_own_module`
//! refuses a second mention anywhere in production, and `spawn_send_create`
//! was measured able to have its whole body emptied with every static guard
//! still green, falling only to a behavioural test. A second Send-creating
//! path written here would be a publish behind a guard nobody wrote — and,
//! since this file is drawn from the `eframe` closure, a blocking one on the
//! UI thread.
//!
//! `hidden: true` is a **compromise, not the spec's preference** — the spec
//! wanted a file Send so a browser offers a download rather than rendering a
//! seed on screen, and `send.rs` has no file path at all. A hidden text Send
//! masks the content until the viewer deliberately reveals it. That is weaker,
//! and [`SEED_WARNING`] is what makes it honest.
//!
//! # The seed warning is a safety control, not decoration
//!
//! [`SEED_WARNING`] is pinned by content, the way this crate pins its refusal
//! messages: a seed cannot be rotated, so "whoever opens this link" is a
//! permanent grant and the sentence is the only place the user is told so. A
//! reworded one must be a deliberate edit that reds a test, not a tidy-up.
//! The same goes for [`STALE_TEMPLATE`], for the opposite reason — copy that
//! implies the record expires *on its own* would be a lie, because
//! [`crate::record::import::item_from`] deliberately does not gate on
//! `not_after` and there is a test protecting that.
//!
//! # Import takes a link, never a pasted blob
//!
//! The clipboard is exactly the leak the fill path's password step already
//! refuses to touch. [`ImportDraft`] therefore has a `link` and no payload
//! field, and there is no function here that reads a record out of anything
//! but a fetched Send.
//!
//! # Nothing is created before the user has seen what will be created
//!
//! [`fields_present`] returns **field names and never values** — the same
//! thing `Record`'s redacting `Debug` shows — and [`import_can_proceed`]
//! refuses while a [`Collision::SameName`] has no [`CollisionChoice`] picked.
//! `None` is the starting state and no code here turns it into a default:
//! replacing is the one step in this whole feature that can destroy data the
//! user already had, so it is never the answer to a question nobody was asked.

use crate::record::import::Collision;
use crate::record::payload::{Record, RecordRefusal};
use crate::record::{write_json, RecordSelection, TotpToSend};
use crate::send::SendPlan;
use crate::theme;
use eframe::egui::{self, CornerRadius};
use zeroize::Zeroizing;

/// Height of every button on these two forms, matching the Sends pane.
const BUTTON_HEIGHT: f32 = 26.0;

// ---------------------------------------------------------------------------
// Task 5 -- the plan that travels
// ---------------------------------------------------------------------------

/// The Send a record travels in.
///
/// **`hidden: true` always**, and it is written here rather than left to the
/// caller so that no call site can publish a record whose body a link preview
/// renders on sight. The name is the record's own, so the sender recognises
/// the row in their Sends list; the body is [`write_json`]'s
/// [`Zeroizing`] buffer, moved in rather than copied.
pub fn send_plan_from(record: &Record) -> SendPlan {
    SendPlan {
        name: record.name.clone(),
        text: write_json(record),
        hidden: true,
        ..SendPlan::default()
    }
}

// ---------------------------------------------------------------------------
// Task 6 -- the export surface
// ---------------------------------------------------------------------------

/// The heading over the export form.
pub const EXPORT_HEADING: &str = "Send this record";

/// The tick-box labels, in the order they are drawn.
pub const USERNAME_LABEL: &str = "Username";
/// See [`USERNAME_LABEL`].
pub const PASSWORD_LABEL: &str = "Password";
/// See [`USERNAME_LABEL`].
pub const URI_LABEL: &str = "Website";
/// See [`USERNAME_LABEL`].
pub const NOTES_LABEL: &str = "Notes";
/// See [`USERNAME_LABEL`]. **Not ticked by default.** A seed is not a default.
pub const TOTP_LABEL: &str = "TOTP seed";

/// **The safety control of this whole feature, verbatim.**
///
/// Shown whenever [`TOTP_LABEL`] is ticked, and pinned by content in
/// `the_seed_warning_is_the_spec_s_own_sentence`. A username and a password can
/// be rotated, so sending them is a bargain the sender can undo. A seed cannot
/// be: "rotating" it means re-enrolling the second factor with the service,
/// which this app can neither do nor offer.
pub const SEED_WARNING: &str = "Sending a seed is not sharing a code \u{2014} it is cloning the \
     second factor, permanently. Anyone who opens this can generate valid codes indefinitely. \
     Revoking stops new recipients; it cannot retract what was already fetched.";

/// The hint in the passphrase box that appears with the seed tick.
pub const PASSPHRASE_HINT: &str = "Passphrase for the seed";

/// The line under the passphrase box. The passphrase layer is worth nothing if
/// it travels beside the link, so the surface says where it must not go.
pub const PASSPHRASE_NOTE: &str =
    "Tell the recipient this passphrase some other way. Sending it with the link protects \
     nothing.";

/// The label on the export form's submit button.
pub const EXPORT_SUBMIT_LABEL: &str = "Create link";

/// Why the export button is grey: the seed tick with nothing to seal under.
pub const NEEDS_PASSPHRASE: &str =
    "A seed can only be sent sealed, so it needs a passphrase to seal it under.";

/// Why the export button is grey: an empty record.
pub const NOTHING_TICKED: &str = "Tick at least one field to send.";

/// What the sender ticked, and the passphrase they typed for the seed.
///
/// **[`Default`] is hand-written and that is the point of it.** Deriving would
/// give an all-false [`RecordSelection`], and the design's opening state is
/// username and password ticked with the seed left alone.
pub struct RecordDraft {
    /// Whether the form is on screen.
    pub open: bool,
    /// The ticks.
    pub selection: RecordSelection,
    /// The passphrase the seed will be sealed under. Empty unless
    /// `selection.totp`; see [`RecordDraft::set_totp`].
    pub passphrase: Zeroizing<String>,
}

impl Default for RecordDraft {
    fn default() -> Self {
        Self {
            open: false,
            // Username and password ticked; the seed is not a default, and
            // neither the URI nor the notes are things a sender should have to
            // notice they are sending.
            selection: RecordSelection {
                username: true,
                password: true,
                uri: false,
                notes: false,
                totp: false,
            },
            passphrase: Zeroizing::new(String::new()),
        }
    }
}

impl RecordDraft {
    /// Ticks or unticks the seed, **dropping the passphrase either way**.
    ///
    /// Dropping it zeroizes it, which is the whole reason this is a method
    /// rather than a field assignment: unticking and re-ticking must start
    /// from empty, because a buffer that survives an untick is a passphrase
    /// still in memory for a seed the user decided not to send.
    pub fn set_totp(&mut self, ticked: bool) {
        if ticked != self.selection.totp {
            self.selection.totp = ticked;
            self.passphrase = Zeroizing::new(String::new());
        }
    }

    /// The seed, paired with its passphrase, exactly as
    /// [`crate::record::record_from`] wants it.
    ///
    /// Returns [`TotpToSend::None`] when the tick is off, when the item has no
    /// seed, or when the passphrase is blank. There is no arm of the return
    /// type that can carry a seed without a passphrase, so this cannot leak
    /// one however it is called.
    pub fn totp_to_send<'a>(&'a self, seed: Option<&'a str>) -> TotpToSend<'a> {
        match (self.selection.totp, seed, self.passphrase.trim().is_empty()) {
            (true, Some(seed), false) if !seed.is_empty() => {
                TotpToSend::Sealed { seed, passphrase: self.passphrase.as_str() }
            }
            _ => TotpToSend::None,
        }
    }
}

/// What is wrong with the export draft, phrased for the user, or `None`.
///
/// The seed rule is first because it is the dangerous one: a ticked seed with
/// a blank passphrase is the exact state
/// `record_from` answers with [`TotpToSend::None`], and a user whose button
/// stayed live would publish a record with the seed silently missing.
pub fn export_problem(draft: &RecordDraft) -> Option<&'static str> {
    let sel = &draft.selection;
    if sel.totp && draft.passphrase.trim().is_empty() {
        return Some(NEEDS_PASSPHRASE);
    }
    if !(sel.username || sel.password || sel.uri || sel.notes || sel.totp) {
        return Some(NOTHING_TICKED);
    }
    None
}

/// Whether the export button may be pressed at all.
///
/// A function of two facts rather than a condition inside a widget, for
/// `send_ui::composer_can_submit`'s reason: `in_flight` is the lock against a
/// second `bw send create`, and a lock written inside an `eframe` closure is a
/// lock no test can run.
pub fn export_can_submit(problem: Option<&str>, in_flight: bool) -> bool {
    problem.is_none() && !in_flight
}

/// Whether [`SEED_WARNING`] is on screen. The tick and nothing else.
pub fn warning_is_shown(draft: &RecordDraft) -> bool {
    draft.selection.totp
}

// ---------------------------------------------------------------------------
// Task 10 -- the import surface
// ---------------------------------------------------------------------------

/// The heading over the import form.
pub const IMPORT_HEADING: &str = "Import a record from a Send";

/// The hint in the link box.
pub const LINK_HINT: &str = "The Send link you were given";

/// The line under it. The link is the input; there is no box to paste a
/// payload into, and this says why rather than leaving it looking like an
/// omission.
pub const LINK_NOTE: &str =
    "Paste the link, not the record itself. Deskwarden fetches the contents so the record \
     never has to sit on your clipboard.";

/// The heading over the field-name list shown before anything is created.
pub const WILL_IMPORT_HEADING: &str = "What this record carries";

/// The hint in the import passphrase box, shown only for a sealed seed.
pub const IMPORT_PASSPHRASE_HINT: &str = "Passphrase the sender set for the seed";

/// The label on the import form's submit button.
pub const IMPORT_SUBMIT_LABEL: &str = "Import into my vault";

/// Why the import button is grey: nothing to fetch yet.
pub const NEEDS_LINK: &str = "Paste the Send link to see what it carries.";

/// Why the import button is grey: a sealed seed and no passphrase offered.
pub const NEEDS_SEED_PASSPHRASE: &str =
    "This record carries a sealed one-time code seed. Enter the passphrase the sender set.";

/// Why the import button is grey: a name collision with nothing chosen.
pub const NEEDS_COLLISION_CHOICE: &str =
    "Choose whether to add a second item or replace the one already there.";

/// **The advisory staleness sentence, verbatim, pinned by content.**
///
/// `{}` is the date. A vault item does not expire — the 2026-08-17 decision
/// accepted that knowingly — so `not_after` is *staleness information about
/// the record* and never enforcement. "It will still import" is the load-
/// bearing half of the sentence: copy implying the record lapses on its own
/// would describe behaviour this app does not have and
/// `record::import` has a test specifically forbidding.
pub const STALE_TEMPLATE: &str = "This record was marked stale on {}. It will still import.";

/// The two answers to a name collision. **Neither is a default.**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollisionChoice {
    /// Add another item and leave the existing one alone.
    CreateSecond,
    /// Overwrite the existing item.
    Replace,
}

/// The label on the [`CollisionChoice::CreateSecond`] button.
pub const CREATE_SECOND_LABEL: &str = "Create a second item";

/// The label on the [`CollisionChoice::Replace`] button.
pub const REPLACE_LABEL: &str = "Replace the existing one";

/// The question above the two buttons.
pub fn collision_prompt(name: &str) -> String {
    format!("An item called \u{201c}{name}\u{201d} is already in your vault.")
}

/// The link, the passphrase, and the collision answer.
///
/// `choice` starts as `None` and **nothing in this module ever gives it a
/// value on the user's behalf.** `Option<CollisionChoice>` rather than a
/// `CollisionChoice` with a `Default` is that rule expressed as a type: there
/// is no value to preselect because the unanswered state is a value of its own.
/// **No `Debug`, deliberately.** `Zeroizing<String>` prints its contents, so a
/// derived one would put the seed's passphrase into any log line that formatted
/// a draft — the same reason `Record`, `SendPlan` and `SendSummary` all carry
/// hand-written redacting ones.
#[derive(Default)]
pub struct ImportDraft {
    /// Whether the form is on screen.
    pub open: bool,
    /// The Send link. **Not the payload** — see the module docs.
    pub link: String,
    /// The passphrase for a sealed seed, if the fetched record has one.
    pub passphrase: Zeroizing<String>,
    /// What the user chose about a name collision, or `None` if they have not
    /// been asked yet or have not answered.
    pub choice: Option<CollisionChoice>,
}

/// The field **names** a record carries, in the order they are drawn.
///
/// **Names, never values.** This list is what the user reads before anything
/// is created, and the payload came from someone else: rendering its contents
/// to prove it arrived would put a stranger's text — and the recipient's
/// soon-to-be password — on screen for the sake of a preview. The seed's line
/// says it is sealed, because "one-time code seed" alone reads like something
/// already in hand.
pub fn fields_present(record: &Record) -> Vec<&'static str> {
    let mut out = Vec::new();
    if record.username.is_some() {
        out.push(USERNAME_LABEL);
    }
    if record.password.is_some() {
        out.push(PASSWORD_LABEL);
    }
    if record.uri.is_some() {
        out.push(URI_LABEL);
    }
    if record.notes.is_some() {
        out.push(NOTES_LABEL);
    }
    if record.totp_sealed.is_some() {
        out.push(SEALED_SEED_LABEL);
    }
    out
}

/// How a sealed seed is named in [`fields_present`].
pub const SEALED_SEED_LABEL: &str = "TOTP seed (sealed)";

/// Whether to show the passphrase box at all.
///
/// A sealed seed and nothing else. A prompt shown for a record that carries no
/// seed asks for a secret that opens nothing, which teaches the user that the
/// prompt is noise.
pub fn needs_passphrase(record: &Record) -> bool {
    record.totp_sealed.is_some()
}

/// The advisory staleness line, or `None` when the record is not stale.
///
/// `None` for an absent, unparseable or future `not_after`: the field is
/// advisory, so a date this build cannot read is not a reason to say anything
/// — least of all to refuse, which the spec forbids.
pub fn stale_note(record: &Record, now: &dyn crate::send::SendClock) -> Option<String> {
    let not_after = record.not_after.as_deref()?;
    let at = super::send_ui::parse_iso_utc_millis(not_after)?;
    if at > now.now_unix_millis() {
        return None;
    }
    // The date the sender wrote, as they wrote it. `not_after` is RFC 3339 and
    // its first ten bytes are the calendar date; `parse_iso_utc_millis`
    // succeeding above is what makes that slice safe to take.
    Some(STALE_TEMPLATE.replace("{}", not_after.trim().get(..10).unwrap_or(not_after)))
}

/// The sentence a refusal is shown as.
///
/// A one-line delegation to [`RecordRefusal::sentence`] **and that is the
/// point**: the reasons live with the reader that produces them, so a variant
/// added there cannot be rendered here as a shrug. A refusal that reads as a
/// generic failure teaches the user to retry, which is the opposite of what a
/// rejected payload should teach.
pub fn refusal_sentence(refusal: &RecordRefusal) -> String {
    refusal.sentence()
}

/// What is wrong with the import draft, or `None`.
///
/// `record` is what was fetched from the link, if anything has been yet.
pub fn import_problem(
    record: Option<&Record>,
    draft: &ImportDraft,
    collision: &Collision,
) -> Option<&'static str> {
    if draft.link.trim().is_empty() {
        return Some(NEEDS_LINK);
    }
    let Some(record) = record else {
        return Some(NEEDS_LINK);
    };
    if needs_passphrase(record) && draft.passphrase.trim().is_empty() {
        return Some(NEEDS_SEED_PASSPHRASE);
    }
    // Last, and unskippable: a collision with no answer stops the import
    // however complete the rest of the draft is.
    if matches!(collision, Collision::SameName { .. }) && draft.choice.is_none() {
        return Some(NEEDS_COLLISION_CHOICE);
    }
    None
}

/// Whether the import button may be pressed at all.
pub fn import_can_proceed(
    record: Option<&Record>,
    draft: &ImportDraft,
    collision: &Collision,
    in_flight: bool,
) -> bool {
    import_problem(record, draft, collision).is_none() && !in_flight
}

// ---------------------------------------------------------------------------
// The two forms
// ---------------------------------------------------------------------------

/// What a frame of either form reports back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordUiAction {
    None,
    /// Publish the record the export draft describes.
    SubmitExport,
    /// Fetch the link the import draft holds.
    FetchLink,
    /// Create the item the fetched record describes.
    SubmitImport,
    /// Close the form without doing anything.
    Cancel,
}

fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(12))
        .show(ui, add)
        .inner
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(14.0).color(theme::INK).strong());
    ui.add_space(8.0);
}

fn note(ui: &mut egui::Ui, text: &str, colour: egui::Color32) {
    ui.label(egui::RichText::new(text).size(11.0).color(colour));
}

/// The export form.
///
/// Every decision it paints comes from [`export_problem`] and
/// [`warning_is_shown`]; there is no condition in this function that a test
/// cannot also ask directly.
pub fn draw_export_form(
    ui: &mut egui::Ui,
    draft: &mut RecordDraft,
    item_name: &str,
    in_flight: bool,
) -> RecordUiAction {
    let mut action = RecordUiAction::None;
    let enabled = !in_flight;
    card(ui, |ui| {
        heading(ui, EXPORT_HEADING);
        note(ui, item_name, theme::TEXT_MUTED);
        ui.add_space(8.0);

        for (label, ticked) in [
            (USERNAME_LABEL, &mut draft.selection.username),
            (PASSWORD_LABEL, &mut draft.selection.password),
            (URI_LABEL, &mut draft.selection.uri),
            (NOTES_LABEL, &mut draft.selection.notes),
        ] {
            ui.add_enabled(
                enabled,
                egui::Checkbox::new(
                    ticked,
                    egui::RichText::new(label).size(12.0).color(theme::TEXT_SECONDARY),
                ),
            );
        }

        // The seed's tick goes through `set_totp` rather than a `&mut bool`,
        // so unticking it drops the passphrase.
        let mut totp = draft.selection.totp;
        if ui
            .add_enabled(
                enabled,
                egui::Checkbox::new(
                    &mut totp,
                    egui::RichText::new(TOTP_LABEL).size(12.0).color(theme::TEXT_SECONDARY),
                ),
            )
            .changed()
        {
            draft.set_totp(totp);
        }

        if warning_is_shown(draft) {
            ui.add_space(8.0);
            // Painted in the error colour and at the same size as the labels
            // above it, not as fine print: it is the sentence that decides
            // whether the tick above was a mistake.
            ui.label(egui::RichText::new(SEED_WARNING).size(12.0).color(theme::ERROR));
            ui.add_space(8.0);
            ui.add_enabled(
                enabled,
                egui::TextEdit::singleline(&mut *draft.passphrase)
                    .hint_text(PASSPHRASE_HINT)
                    .password(true)
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(4.0);
            note(ui, PASSPHRASE_NOTE, theme::TEXT_FAINT);
        }

        ui.add_space(12.0);
        let problem = export_problem(draft);
        let can_submit = export_can_submit(problem, in_flight);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    can_submit,
                    egui::Button::new(
                        egui::RichText::new(EXPORT_SUBMIT_LABEL).size(12.0).color(theme::INK),
                    )
                    .min_size(egui::vec2(104.0, BUTTON_HEIGHT)),
                )
                .clicked()
            {
                action = RecordUiAction::SubmitExport;
            }
            ui.add_space(8.0);
            if ui
                .add_enabled(
                    enabled,
                    egui::Button::new(
                        egui::RichText::new("Cancel").size(12.0).color(theme::TEXT_MUTED),
                    )
                    .min_size(egui::vec2(72.0, BUTTON_HEIGHT)),
                )
                .clicked()
            {
                action = RecordUiAction::Cancel;
            }
            // The reason the button is grey, beside the button. A disabled
            // control with no explanation is a control the user reads as
            // broken.
            if let Some(problem) = problem {
                ui.add_space(8.0);
                note(ui, problem, theme::TEXT_FAINT);
            }
        });
    });
    action
}

/// The import form.
///
/// `record` is what the link fetched, once it has: `None` before, `Some(Err)`
/// for a payload that was refused. A refusal is drawn as
/// [`refusal_sentence`], never as a generic failure.
pub fn draw_import_form(
    ui: &mut egui::Ui,
    draft: &mut ImportDraft,
    record: Option<&Result<Record, RecordRefusal>>,
    collision: &Collision,
    in_flight: bool,
    now: &dyn crate::send::SendClock,
) -> RecordUiAction {
    let mut action = RecordUiAction::None;
    let enabled = !in_flight;
    card(ui, |ui| {
        heading(ui, IMPORT_HEADING);
        ui.add_enabled(
            enabled,
            egui::TextEdit::singleline(&mut draft.link)
                .hint_text(LINK_HINT)
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);
        note(ui, LINK_NOTE, theme::TEXT_FAINT);

        let ok = match record {
            Some(Err(refusal)) => {
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new(refusal_sentence(refusal)).size(12.0).color(theme::ERROR),
                );
                None
            }
            Some(Ok(record)) => Some(record),
            None => None,
        };

        if let Some(record) = ok {
            ui.add_space(10.0);
            note(ui, WILL_IMPORT_HEADING, theme::TEXT_MUTED);
            ui.add_space(2.0);
            // Names only. Never a value; see `fields_present`.
            for name in fields_present(record) {
                note(ui, name, theme::TEXT_SECONDARY);
            }

            if let Some(stale) = stale_note(record, now) {
                ui.add_space(8.0);
                note(ui, &stale, theme::TEXT_MUTED);
            }

            if needs_passphrase(record) {
                ui.add_space(8.0);
                ui.add_enabled(
                    enabled,
                    egui::TextEdit::singleline(&mut *draft.passphrase)
                        .hint_text(IMPORT_PASSPHRASE_HINT)
                        .password(true)
                        .desired_width(f32::INFINITY),
                );
            }

            if let Collision::SameName { .. } = collision {
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new(collision_prompt(&record.name))
                        .size(12.0)
                        .color(theme::INK),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    // `selected` is driven by `draft.choice`, which starts
                    // `None`, so neither button is lit until the user lights
                    // one. There is no `unwrap_or` here and there must not be.
                    for (label, choice) in [
                        (CREATE_SECOND_LABEL, CollisionChoice::CreateSecond),
                        (REPLACE_LABEL, CollisionChoice::Replace),
                    ] {
                        let chosen = draft.choice == Some(choice);
                        if ui
                            .add_enabled(
                                enabled,
                                egui::Button::new(
                                    egui::RichText::new(label).size(12.0).color(theme::INK),
                                )
                                .selected(chosen)
                                .min_size(egui::vec2(160.0, BUTTON_HEIGHT)),
                            )
                            .clicked()
                        {
                            draft.choice = Some(choice);
                        }
                    }
                });
            }
        }

        ui.add_space(12.0);
        let problem = import_problem(ok, draft, collision);
        let can_proceed = import_can_proceed(ok, draft, collision, in_flight);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    enabled && !draft.link.trim().is_empty(),
                    egui::Button::new(
                        egui::RichText::new("Fetch").size(12.0).color(theme::TEXT_MUTED),
                    )
                    .min_size(egui::vec2(72.0, BUTTON_HEIGHT)),
                )
                .clicked()
            {
                action = RecordUiAction::FetchLink;
            }
            ui.add_space(8.0);
            if ui
                .add_enabled(
                    can_proceed,
                    egui::Button::new(
                        egui::RichText::new(IMPORT_SUBMIT_LABEL).size(12.0).color(theme::INK),
                    )
                    .min_size(egui::vec2(150.0, BUTTON_HEIGHT)),
                )
                .clicked()
            {
                action = RecordUiAction::SubmitImport;
            }
            if let Some(problem) = problem {
                ui.add_space(8.0);
                note(ui, problem, theme::TEXT_FAINT);
            }
        });
    });
    action
}

// ---------------------------------------------------------------------------
// The way in
// ---------------------------------------------------------------------------

/// What the control that opens the export composer is CALLED.
///
/// Design §5b drew it as a `Send a record` pill in the window titlebar,
/// carrying `CTRL+⇧+S`. That pill is gone at the user's direction, and the
/// judgement was right: it acted on the SELECTED ITEM from a strip whose
/// every other control is global. The control is now
/// `theme::send_record_button`, an envelope in the detail pane's own header
/// strip, so this string is its hover rather than its face.
///
/// Still a named constant, and still paired with the chord below on that
/// hover, for the rule every binding in this app follows: a chord with
/// nothing on screen naming it is a chord nobody finds.
pub const SEND_RECORD_LABEL: &str = "Send a record";

/// See [`SEND_RECORD_LABEL`]. Spelled the way `detail.rs`'s copy chords are,
/// and — like them — **the only place this chord is spelled for a human**.
/// `the_record_chord_is_spelled_the_way_it_is_bound` compares it against
/// `SEND_RECORD_MODIFIERS` and `SEND_RECORD_KEY`, the values the key handler
/// actually matches on, so the hover cannot advertise a binding the code does
/// not have.
pub const SEND_RECORD_SHORTCUT: &str = "CTRL+SHIFT+S";

// `NO_ITEM_SELECTED` — "Select an item in the list to send it." — was the
// titlebar pill's disabled hover, and it is DELETED with that pill rather
// than parked: a lib crate raises no dead-code warning for a `pub` item, so
// it would have sat here indefinitely as a doc comment describing a state
// that can no longer occur. The state is gone, not merely unexplained: the
// control now lives in the detail pane's header strip, which `vault_window`
// only draws for an item resolved out of the live vault — so there is no
// frame in which the control exists and there is nothing to send, and it
// needs no disabled state at all. Same reasoning `theme.rs` recorded when
// the header's "Fill in app" button took `header_primary_button` with it.

/// The composer's per-open state: **which item it was opened against**, and
/// the ticks.
///
/// The id and the name are both held, and neither is redundant. The id is what
/// the caller re-resolves the item by when Create is pressed — the vault can
/// be re-read between the open and the press, so the item the composer was
/// opened from is not necessarily the one that should be published. The name
/// is what the form paints, and it is a copy rather than a second lookup so
/// the heading cannot go blank if the item disappears underneath.
pub struct RecordSend {
    /// The chosen item's id. See the struct doc.
    pub item_id: String,
    /// The chosen item's name, as painted. See the struct doc.
    pub item_name: String,
    /// The ticks and the passphrase.
    pub draft: RecordDraft,
}

impl RecordSend {
    /// Opens the composer against one item.
    ///
    /// `open` is set here rather than left to the caller: a `RecordSend` that
    /// exists at all *is* the composer being on screen, and a second flag the
    /// caller had to remember to set is the second enumeration this crate
    /// keeps losing to.
    pub fn opening(item_id: &str, item_name: &str) -> Self {
        Self {
            item_id: item_id.to_string(),
            item_name: item_name.to_string(),
            draft: RecordDraft { open: true, ..RecordDraft::default() },
        }
    }
}

/// The composer card's width. Design §5a's composer is a narrow column — the
/// tick list is the widest control in it — and the seed warning is a paragraph
/// that has to wrap somewhere.
const MODAL_WIDTH: f32 = 360.0;

/// [`draw_export_form`] over a dimmed scrim, centred, for `vault_window::mod`
/// to call from its frame closure.
///
/// **A modal and not a pane**, deliberately: the composer is opened against
/// the item the user has selected in the list, and a screen that replaced the
/// list — the way the Sends screen does — would take that item off screen at
/// the moment the user is deciding what to send from it.
///
/// Built exactly the way [`super::folder_modal::draw_folder_edit_modal`] is —
/// a full-window click-catching scrim on the `Foreground` layer, then a
/// centred card — because that is this window's established modal, and a
/// second one built differently is two modals that dim, layer and swallow
/// clicks two ways. Nothing about the form itself moves in here: every
/// decision it paints is still [`export_problem`]'s and [`warning_is_shown`]'s.
pub fn draw_export_modal(
    ctx: &egui::Context,
    state: &mut RecordSend,
    in_flight: bool,
) -> RecordUiAction {
    egui::Area::new(egui::Id::new("record-send-scrim"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let screen = ctx.content_rect();
            ui.allocate_response(screen.size(), egui::Sense::click());
            ui.painter().rect_filled(
                screen,
                CornerRadius::ZERO,
                egui::Color32::from_black_alpha(90),
            );
        });

    egui::Area::new(egui::Id::new("record-send-modal"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_max_width(MODAL_WIDTH);
            draw_export_form(ui, &mut state.draft, &state.item_name, in_flight)
        })
        .inner
}

// ---------------------------------------------------------------------------
// The way in, the other direction
// ---------------------------------------------------------------------------

/// The label on the `+ New` menu's import row.
///
/// **The import lives on `+ New` and not beside [`SEND_RECORD_LABEL`] in the
/// titlebar, and the two are not symmetric on purpose.** Sending narrows an
/// item the user has SELECTED, which is why design §5b draws its control in
/// the header next to the account avatar -- the header is where this window
/// puts things that act on the window. Importing SELECTS NOTHING and CREATES
/// AN ITEM, which is the `+ New` button's entire job, so it belongs on the
/// one control this window already has for "make me a new item". Putting it
/// in the titlebar instead would have made the pair look symmetric and read
/// wrong: a user with nothing selected would find the send pill greyed out
/// and an import pill live beside it, two controls that answer to different
/// preconditions sitting in one strip.
///
/// The ellipsis is this app's usual "this opens something and does not act",
/// the way `Export vault...` reads on the account menu.
pub const IMPORT_FROM_SEND_LABEL: &str = "Import from a Send...";

/// The import form's per-open state: the draft, what the link fetched, and
/// whether a `bw send receive` is running.
///
/// **`failure` is a separate field from a refused [`Record`] and must stay
/// one.** `fetched: Some(Err(refusal))` means a payload ARRIVED and was
/// rejected by [`crate::record::payload::read_json`], and the form renders
/// each of those reasons as its own sentence. Everything else that can go
/// wrong -- a link that fetched nothing, a passphrase that would not open the
/// seal, a vault that refused the create -- is not a refusal OF A PAYLOAD,
/// and rendering it through [`refusal_sentence`] would name the wrong reason,
/// which is the whole thing that function exists to prevent. Those land here,
/// each already a sentence from the module that produced it.
///
/// **No `Debug`, deliberately**, for [`ImportDraft`]'s reason: the draft holds
/// a `Zeroizing` passphrase and the fetched record holds a password.
#[derive(Default)]
pub struct RecordImport {
    /// The link, the passphrase and the collision answer.
    pub draft: ImportDraft,
    /// What the link fetched, once it has. `None` before the first fetch and
    /// after a failed one.
    pub fetched: Option<Result<Record, RecordRefusal>>,
    /// Why the last attempt did not end in an item. See the struct doc.
    pub failure: Option<String>,
    /// A `bw send receive` is running for this form.
    pub in_flight: bool,
}

impl RecordImport {
    /// Opens the import form.
    ///
    /// `open` is set here rather than left to the caller, for
    /// [`RecordSend::opening`]'s reason: a `RecordImport` that exists at all
    /// *is* the form being on screen.
    pub fn opening() -> Self {
        Self { draft: ImportDraft { open: true, ..ImportDraft::default() }, ..Self::default() }
    }
}

/// [`draw_import_form`] over a dimmed scrim, centred, for `vault_window::mod`
/// to call from its frame closure.
///
/// Built exactly the way [`draw_export_modal`] is -- which is exactly the way
/// [`super::folder_modal::draw_folder_edit_modal`] is -- because a second
/// modal built differently is two modals that dim, layer and swallow clicks
/// two ways. Its `Id`s differ from the export's so that the two cannot share
/// egui state if both were ever open.
///
/// **The fetch failure is painted HERE and not inside the form**, because it
/// is not a fact about the draft: the form's every other decision is a pure
/// function of what it was handed, and a fetch that never returned a payload
/// is a fact about the last `bw` child instead. See [`RecordImport`].
pub fn draw_import_modal(
    ctx: &egui::Context,
    state: &mut RecordImport,
    collision: &Collision,
    now: &dyn crate::send::SendClock,
) -> RecordUiAction {
    egui::Area::new(egui::Id::new("record-import-scrim"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let screen = ctx.content_rect();
            ui.allocate_response(screen.size(), egui::Sense::click());
            ui.painter().rect_filled(
                screen,
                CornerRadius::ZERO,
                egui::Color32::from_black_alpha(90),
            );
        });

    egui::Area::new(egui::Id::new("record-import-modal"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_max_width(MODAL_WIDTH);
            let action = draw_import_form(
                ui,
                &mut state.draft,
                state.fetched.as_ref(),
                collision,
                state.in_flight,
                now,
            );
            if let Some(why) = &state.failure {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(why).size(12.0).color(theme::ERROR));
            }
            action
        })
        .inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::payload::read_json;
    use crate::record::seal::seal;
    use crate::send::FixedClock;
    use std::sync::OnceLock;

    /// `2026-08-17T12:00:00Z`.
    const NOW: i64 = 1_786_320_000_000;

    fn a_record() -> Record {
        Record {
            name: "SAP Production".to_string(),
            username: Some("dplatonov".to_string()),
            password: Some(Zeroizing::new("hunter2".to_string())),
            uri: Some("https://sap.example".to_string()),
            notes: None,
            totp_sealed: None,
            not_after: None,
        }
    }

    /// One Argon2id derivation for the whole test module, ~0.7 s in debug.
    /// Every sealed-seed fixture below clones this rather than sealing again.
    fn sealed() -> &'static crate::record::SealedSeed {
        static ONCE: OnceLock<crate::record::SealedSeed> = OnceLock::new();
        ONCE.get_or_init(|| seal("JBSWY3DPEHPK3PXP", "correct horse battery staple"))
    }

    fn a_sealed_record() -> Record {
        Record { totp_sealed: Some(sealed().clone()), ..a_record() }
    }

    // -- Task 5 -----------------------------------------------------------

    /// **The body parses back as a record, and the Send is hidden.**
    ///
    /// Both halves matter and neither implies the other: a plan whose text is
    /// garbage would still be hidden, and a plan carrying a perfect record in
    /// the clear is one a link preview renders on sight.
    #[test]
    fn the_plan_carries_the_record_and_is_hidden() {
        let record = a_record();
        let plan = send_plan_from(&record);

        assert!(plan.hidden, "the record Send was not hidden, so a viewer renders the body on sight");

        let back = read_json(&plan.text).expect("the plan's own text must parse back as a record");
        assert_eq!(back.name, record.name);
        assert_eq!(back.username.as_deref(), Some("dplatonov"));
        assert_eq!(back.password.as_deref().map(String::as_str), Some("hunter2"));
        assert_eq!(back.uri.as_deref(), Some("https://sap.example"));
        // The name is the record's, so the sender recognises the row.
        assert_eq!(plan.name, "SAP Production");

        // Control on `hidden`: `SendPlan::default()` is NOT hidden, so the
        // assertion above is about what this function sets and not about a
        // field that is true for everyone.
        assert!(
            !SendPlan::default().hidden,
            "control: a default plan is already hidden, so `plan.hidden` proves nothing"
        );
    }

    /// A record with nothing optional in it still travels hidden, so the
    /// assertion above is not an accident of a full fixture.
    #[test]
    fn even_a_bare_record_travels_hidden() {
        let plan = send_plan_from(&Record {
            name: "x".to_string(),
            username: None,
            password: None,
            uri: None,
            notes: None,
            totp_sealed: None,
            not_after: None,
        });
        assert!(plan.hidden);
        assert!(read_json(&plan.text).is_ok(), "{}", plan.text.as_str());
    }

    /// The production half of this file, for the source pins below.
    fn production() -> String {
        let source = include_str!("record_ui.rs").replace("\r\n", "\n");
        let production = source
            .split(concat!("#[cfg(test)]", "\nmod tests"))
            .next()
            .expect("split always yields one part")
            .to_string();
        assert!(
            production.len() < source.len(),
            "the test-module marker was not found, so every pin built on this reads the \
             test source it was meant to exclude"
        );
        production
    }

    /// **This file builds a plan and never publishes one.**
    ///
    /// The crate already has exactly one route to `bw send create`, sealed
    /// inside `vault_window`'s `send_create_thread` and guarded there both
    /// statically and behaviourally. A record Send started from here would be
    /// a second route, on the `eframe` thread. So the property to pin is an
    /// absence, and it is pinned with a live control first: `send_plan_from`
    /// really is in this file, so "no send call" is not a statement about an
    /// empty one.
    #[test]
    fn this_file_builds_a_plan_and_never_starts_a_send() {
        let production = production();
        assert!(
            production.contains("pub fn send_plan_from(record: &Record) -> SendPlan"),
            "control: the plan builder is gone, so the absences below are vacuous"
        );
        assert!(production.contains("hidden: true"), "control: the plan builder is not building");

        for forbidden in [concat!("cli_send", "_create"), concat!("create", "_send"), "Command::new", "std::process"] {
            assert!(
                !production.contains(forbidden),
                "this file reaches `bw` directly through {forbidden:?}. There is one \
                 Send-creating path in this crate, it is sealed inside \
                 `vault_window::send_create_thread`, and a second one here would be a \
                 blocking `bw send create` on the eframe thread"
            );
        }
    }

    // -- Task 6 -----------------------------------------------------------

    /// **Username and password ticked, the seed unticked.** A seed is not a
    /// default, and the positive half stops "nothing is ticked" passing.
    #[test]
    fn the_export_form_opens_with_username_and_password_and_no_seed() {
        let draft = RecordDraft::default();
        assert!(draft.selection.username, "the username tick is not on by default");
        assert!(draft.selection.password, "the password tick is not on by default");
        assert!(!draft.selection.totp, "the SEED was ticked by default");
        assert!(!draft.selection.uri);
        assert!(!draft.selection.notes);
        assert!(draft.passphrase.is_empty());
        // And the opening draft is one the button is live for, so the
        // defaults are usable rather than merely safe.
        assert_eq!(export_problem(&draft), None);
        assert!(export_can_submit(export_problem(&draft), false));
    }

    /// **The disabled button, as a pure function of the draft.**
    ///
    /// Ticking the seed greys the button until a passphrase is typed, and the
    /// control on the other side is that typing one un-greys it — without
    /// which "always disabled" would pass.
    #[test]
    fn ticking_the_seed_disables_the_button_until_a_passphrase_is_typed() {
        let mut draft = RecordDraft::default();
        assert!(export_can_submit(export_problem(&draft), false), "control: live before the tick");

        draft.set_totp(true);
        assert_eq!(export_problem(&draft), Some(NEEDS_PASSPHRASE));
        assert!(
            !export_can_submit(export_problem(&draft), false),
            "a seed can be published with nothing to seal it under"
        );

        // Whitespace is not a passphrase.
        draft.passphrase = Zeroizing::new("   ".to_string());
        assert_eq!(export_problem(&draft), Some(NEEDS_PASSPHRASE));

        draft.passphrase = Zeroizing::new("correct horse battery staple".to_string());
        assert_eq!(export_problem(&draft), None);
        assert!(export_can_submit(export_problem(&draft), false));

        // And in flight, nothing submits whatever the draft says.
        assert!(!export_can_submit(export_problem(&draft), true));
    }

    /// An empty draft is refused for its own reason, so the seed rule above is
    /// not the only thing `export_problem` can ever say.
    #[test]
    fn a_draft_with_nothing_ticked_says_so() {
        let draft = RecordDraft {
            selection: RecordSelection::default(),
            ..RecordDraft::default()
        };
        assert_eq!(export_problem(&draft), Some(NOTHING_TICKED));
        let ticked = RecordDraft { selection: RecordSelection { uri: true, ..Default::default() }, ..RecordDraft::default() };
        assert_eq!(export_problem(&ticked), None, "control: one tick is enough");
    }

    /// Unticking the seed drops the passphrase, which zeroizes it. Re-ticking
    /// starts from empty rather than from what was typed before.
    #[test]
    fn unticking_the_seed_drops_the_passphrase() {
        let mut draft = RecordDraft::default();
        draft.set_totp(true);
        draft.passphrase = Zeroizing::new("correct horse battery staple".to_string());
        assert!(!draft.passphrase.is_empty(), "control: there is a passphrase to drop");

        draft.set_totp(false);
        assert!(draft.passphrase.is_empty(), "the passphrase survived an untick");
        draft.set_totp(true);
        assert!(draft.passphrase.is_empty(), "a re-tick brought the old passphrase back");
        assert_eq!(export_problem(&draft), Some(NEEDS_PASSPHRASE));
    }

    /// The seed and its passphrase leave this form as one value, so no caller
    /// can carry one without the other.
    #[test]
    fn a_seed_leaves_the_form_only_beside_its_passphrase() {
        const SEED: &str = "JBSWY3DPEHPK3PXP";
        let mut draft = RecordDraft::default();
        assert_eq!(draft.totp_to_send(Some(SEED)), TotpToSend::None, "unticked, nothing travels");

        draft.set_totp(true);
        assert_eq!(
            draft.totp_to_send(Some(SEED)),
            TotpToSend::None,
            "a ticked seed with a blank passphrase must not travel"
        );

        draft.passphrase = Zeroizing::new("pw".to_string());
        assert_eq!(
            draft.totp_to_send(Some(SEED)),
            TotpToSend::Sealed { seed: SEED, passphrase: "pw" },
            "control: with both, the seed does travel -- or the assertions above are vacuous"
        );
        assert_eq!(draft.totp_to_send(None), TotpToSend::None, "an item with no seed sends none");
        assert_eq!(draft.totp_to_send(Some("")), TotpToSend::None);
    }

    /// **The warning is shown exactly when the seed is ticked**, and the tick
    /// is the only thing it turns on.
    #[test]
    fn the_seed_warning_appears_with_the_tick_and_not_before() {
        let mut draft = RecordDraft::default();
        assert!(!warning_is_shown(&draft), "the warning was on screen with no seed ticked");
        draft.set_totp(true);
        assert!(warning_is_shown(&draft), "the seed was ticked and the warning was not shown");
        draft.passphrase = Zeroizing::new("pw".to_string());
        assert!(warning_is_shown(&draft), "typing a passphrase dismissed the warning");
        draft.set_totp(false);
        assert!(!warning_is_shown(&draft));
    }

    /// **The sentence itself, pinned by content.**
    ///
    /// Written out here in full rather than compared against the constant by
    /// name — comparing `SEED_WARNING` with itself is not a pin. Every clause
    /// is load-bearing: *cloning* rather than sharing, *permanently*,
    /// *indefinitely*, and the last clause, which is the only place the user
    /// is told that revoking does not undo what already happened.
    #[test]
    fn the_seed_warning_is_the_specs_own_sentence() {
        assert_eq!(
            SEED_WARNING,
            "Sending a seed is not sharing a code \u{2014} it is cloning the second factor, \
             permanently. Anyone who opens this can generate valid codes indefinitely. \
             Revoking stops new recipients; it cannot retract what was already fetched.",
            "the seed warning was reworded. It is the safety control of this feature, not \
             copy: a seed cannot be rotated, so this sentence is the only place the user is \
             told the grant is permanent. Change it deliberately, here and in the spec, or \
             not at all"
        );
        // And it is the sentence the form actually paints -- see
        // `the_form_paints_the_warning_only_with_the_seed_ticked`.
        assert!(SEED_WARNING.contains("permanently"));
        assert!(SEED_WARNING.contains("cannot retract what was already fetched"));
    }

    // -- Task 10 ----------------------------------------------------------

    /// **Names, never values.**
    #[test]
    fn the_preview_names_the_fields_and_shows_no_value() {
        let record = Record { notes: Some("a note".to_string()), ..a_sealed_record() };
        let names = fields_present(&record);
        assert_eq!(
            names,
            [USERNAME_LABEL, PASSWORD_LABEL, URI_LABEL, NOTES_LABEL, SEALED_SEED_LABEL],
            "the preview must name every field the record carries, in order"
        );
        // No value from the record is anywhere in the preview.
        let shown = names.join(" ");
        for value in ["dplatonov", "hunter2", "https://sap.example", "a note"] {
            assert!(!shown.contains(value), "the preview showed a VALUE: {shown}");
        }

        // The other side: a record carrying less is previewed as carrying
        // less, so the list is read off the record and not a fixed menu.
        let bare = Record {
            username: None,
            password: None,
            uri: None,
            notes: None,
            totp_sealed: None,
            ..a_record()
        };
        assert!(fields_present(&bare).is_empty());
        assert_eq!(fields_present(&a_record()), [USERNAME_LABEL, PASSWORD_LABEL, URI_LABEL]);
    }

    /// The passphrase prompt appears **only** for a sealed seed.
    #[test]
    fn the_passphrase_prompt_appears_only_for_a_sealed_seed() {
        assert!(!needs_passphrase(&a_record()), "a record with no seed asked for a passphrase");
        assert!(needs_passphrase(&a_sealed_record()), "a sealed seed did not ask for one");
    }

    /// **The import takes a link and there is nowhere to paste a payload.**
    ///
    /// Pinned at the source as well as at the type: the clipboard is the leak
    /// the fill path's password step already refuses to touch, and a "paste
    /// the record here" box added later would reopen it.
    #[test]
    fn the_import_form_takes_a_link_and_not_a_pasted_payload() {
        let draft = ImportDraft::default();
        assert_eq!(draft.link, "", "control: the link starts empty");
        assert_eq!(
            import_problem(None, &draft, &Collision::Fresh),
            Some(NEEDS_LINK),
            "an import with no link must not proceed"
        );
        // And a link cleared *after* a fetch is still a refusal. Measured:
        // without this line, deleting the link rule entirely survived the
        // whole module, because every other fixture that had no link also had
        // no record and was caught one branch further down.
        assert_eq!(
            import_problem(Some(&a_record()), &ImportDraft::default(), &Collision::Fresh),
            Some(NEEDS_LINK),
            "a record whose link was cleared imported anyway"
        );
        // Control: the same record, with the link still there, proceeds.
        let with_link =
            ImportDraft { link: "https://send.example/#/x".to_string(), ..Default::default() };
        assert_eq!(import_problem(Some(&a_record()), &with_link, &Collision::Fresh), None);

        let production = production();
        assert!(
            production.contains("pub link: String"),
            "control: `ImportDraft` no longer has a link, so the pin below reads nothing"
        );
        // The API names, not the word: this file's own copy explains what the
        // clipboard is for and why a link is asked for instead, and a pin that
        // refused the word would refuse the explanation.
        for clipboard in ["copy_text", "copied_text", "Clipboard", "paste_text"] {
            assert!(
                !production.contains(clipboard),
                "the import surface reaches the clipboard ({clipboard}), which is exactly the \
                 leak the fill path's password step refuses to touch"
            );
        }
        // And there is no payload field to paste into, which is the shape of
        // the rule rather than a habit of the current code.
        for payload_box in ["pub payload:", "pub blob:", "pub pasted"] {
            assert!(
                !production.contains(payload_box),
                "the import draft grew a field to paste a record into ({payload_box}); the \
                 link is the input precisely so a record never sits on a clipboard"
            );
        }
    }

    /// **A past `not_after` is advisory and says so, verbatim.**
    #[test]
    fn a_stale_record_says_it_will_still_import() {
        let stale = Record {
            not_after: Some("2026-08-01T00:00:00Z".to_string()),
            ..a_record()
        };
        let note = stale_note(&stale, &FixedClock(NOW)).expect("a past date must be reported");
        assert_eq!(
            note,
            "This record was marked stale on 2026-08-01. It will still import.",
            "the staleness line was reworded. \"It will still import\" is the load-bearing \
             half: `record::import::item_from` deliberately does not gate on `not_after`, and \
             copy implying the record lapses on its own describes behaviour this app has not \
             got"
        );

        // The controls, all three, or the sentence above would be shown to
        // everyone: a future date, an absent one, and one this build cannot
        // read all say nothing rather than something wrong.
        let future = Record { not_after: Some("2027-01-01T00:00:00Z".to_string()), ..a_record() };
        assert_eq!(stale_note(&future, &FixedClock(NOW)), None, "a live record was called stale");
        assert_eq!(stale_note(&a_record(), &FixedClock(NOW)), None);
        let junk = Record { not_after: Some("whenever".to_string()), ..a_record() };
        assert_eq!(stale_note(&junk, &FixedClock(NOW)), None);
    }

    /// Staleness never blocks. The same record that reports stale still
    /// imports, which is the decision the spec settled.
    #[test]
    fn a_stale_record_is_not_refused_by_this_surface() {
        let stale = Record {
            not_after: Some("2026-08-01T00:00:00Z".to_string()),
            ..a_record()
        };
        assert!(stale_note(&stale, &FixedClock(NOW)).is_some(), "control: this record IS stale");
        let draft = ImportDraft { link: "https://send.example/#/x".to_string(), ..Default::default() };
        assert_eq!(import_problem(Some(&stale), &draft, &Collision::Fresh), None);
        assert!(import_can_proceed(Some(&stale), &draft, &Collision::Fresh, false));
    }

    /// **Every `RecordRefusal` renders as a sentence naming the reason.**
    ///
    /// The `match` is exhaustive on purpose: a variant added to
    /// `RecordRefusal` stops this compiling, rather than shipping as a shrug.
    #[test]
    fn every_refusal_renders_as_a_sentence_that_names_its_reason() {
        let every = [
            RecordRefusal::NotOurFormat,
            RecordRefusal::UnsupportedVersion(99),
            RecordRefusal::UnknownField("surprise".to_string()),
            RecordRefusal::MissingName,
            RecordRefusal::Malformed("the body is not an object"),
            RecordRefusal::TooLarge,
        ];
        // Exhaustiveness: this reds the BUILD if a variant is added without
        // being added to `every` above.
        for refusal in &every {
            match refusal {
                RecordRefusal::NotOurFormat
                | RecordRefusal::UnsupportedVersion(_)
                | RecordRefusal::UnknownField(_)
                | RecordRefusal::MissingName
                | RecordRefusal::Malformed(_)
                | RecordRefusal::TooLarge => {}
            }
        }

        let mut seen: Vec<String> = Vec::new();
        for refusal in &every {
            let sentence = refusal_sentence(refusal);
            assert!(
                sentence.len() > 20 && sentence.ends_with('.'),
                "{refusal:?} renders as {sentence:?}, which is not a sentence"
            );
            for shrug in ["failed", "error", "try again", "unknown error"] {
                assert!(
                    !sentence.to_lowercase().contains(shrug),
                    "{refusal:?} renders as a generic failure ({shrug:?}), which teaches the \
                     user to retry a payload that will never be accepted: {sentence}"
                );
            }
            assert!(!seen.contains(&sentence), "two refusals render identically: {sentence}");
            seen.push(sentence);
        }
        // Positively: the reasons really are named, each in its own words.
        assert!(seen[1].contains("99"), "the version refused is not in its sentence: {}", seen[1]);
        assert!(
            seen[2].contains("surprise"),
            "the unknown field is not named in its sentence: {}",
            seen[2]
        );
        assert!(seen[4].contains("not an object"), "{}", seen[4]);
    }

    /// **A collision has no preselected answer, and no import happens without
    /// one.** The one step in this feature that can destroy data the user
    /// already had.
    #[test]
    fn a_name_collision_is_asked_about_and_never_defaulted() {
        let record = a_record();
        let collision = Collision::SameName { existing_id: "item-1".to_string() };
        let base = ImportDraft { link: "https://send.example/#/x".to_string(), ..Default::default() };

        assert_eq!(base.choice, None, "a collision answer was preselected");
        assert_eq!(
            import_problem(Some(&record), &base, &collision),
            Some(NEEDS_COLLISION_CHOICE)
        );
        assert!(
            !import_can_proceed(Some(&record), &base, &collision, false),
            "an item would have been overwritten without the user being asked"
        );

        // Either answer unblocks it -- and both must, or "no default" would
        // be indistinguishable from "replace is impossible".
        for choice in [CollisionChoice::CreateSecond, CollisionChoice::Replace] {
            let draft = ImportDraft {
                link: base.link.clone(),
                choice: Some(choice),
                ..Default::default()
            };
            assert_eq!(
                import_problem(Some(&record), &draft, &collision),
                None,
                "{choice:?} did not unblock the import"
            );
            assert!(import_can_proceed(Some(&record), &draft, &collision, false));
        }

        // Control: with no collision, the same unanswered draft proceeds, so
        // the refusal above is about the collision and not about the draft.
        assert!(import_can_proceed(Some(&record), &base, &Collision::Fresh, false));
    }

    /// A sealed seed with no passphrase stops the import, and typing one
    /// releases it.
    #[test]
    fn a_sealed_seed_holds_the_import_until_a_passphrase_is_offered() {
        let record = a_sealed_record();
        let mut draft = ImportDraft { link: "https://send.example/#/x".to_string(), ..Default::default() };
        assert_eq!(
            import_problem(Some(&record), &draft, &Collision::Fresh),
            Some(NEEDS_SEED_PASSPHRASE)
        );
        draft.passphrase = Zeroizing::new("  ".to_string());
        assert_eq!(
            import_problem(Some(&record), &draft, &Collision::Fresh),
            Some(NEEDS_SEED_PASSPHRASE),
            "whitespace was accepted as a passphrase"
        );
        draft.passphrase = Zeroizing::new("correct horse battery staple".to_string());
        assert_eq!(import_problem(Some(&record), &draft, &Collision::Fresh), None);

        // Control: a record with no seed never asks, so the block above is
        // about the seal and not about a passphrase box that is always
        // required.
        let plain = ImportDraft { link: "https://send.example/#/x".to_string(), ..Default::default() };
        assert_eq!(import_problem(Some(&a_record()), &plain, &Collision::Fresh), None);
    }

    /// The two collision labels are the spec's, and they are two different
    /// offers rather than one written twice.
    #[test]
    fn the_collision_offers_the_two_choices_by_name() {
        assert_eq!(CREATE_SECOND_LABEL, "Create a second item");
        assert_eq!(REPLACE_LABEL, "Replace the existing one");
        assert_ne!(CREATE_SECOND_LABEL, REPLACE_LABEL);
        assert!(collision_prompt("SAP Production").contains("SAP Production"));
    }
}

#[cfg(test)]
mod paint_tests {
    //! What the two forms **actually paint**, driven through real frames.
    //!
    //! Two details are baked in, both learned by `send_ui`'s paint tests.
    //! (1) `theme::apply`'s font families only exist from the frame after it
    //! is called, so every fixture runs two warm-up frames first. (2) **egui
    //! culls shapes entirely outside the screen rect**, so a control pushed
    //! off the pane comes back as *nothing at all* -- which is why the
    //! fixtures below are given a generous pane.

    use super::*;
    use crate::record::seal::seal;
    use crate::send::FixedClock;
    use std::sync::OnceLock;

    const NOW: i64 = 1_786_320_000_000;

    struct Painted(Vec<String>);

    impl Painted {
        fn has(&self, needle: &str) -> bool {
            self.0.iter().any(|t| t.contains(needle))
        }
    }

    fn collect(shape: &egui::Shape, out: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => out.0.push(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, out);
                }
            }
            _ => {}
        }
    }

    fn paint(draw: impl FnOnce(&mut egui::Ui)) -> Painted {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 900.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        crate::theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});

        let mut draw = Some(draw);
        let output = ctx.run_ui(input(), |ui| {
            (draw.take().expect("run_ui runs the closure once"))(ui);
        });
        let mut painted = Painted(Vec::new());
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        assert!(
            !painted.0.is_empty(),
            "the form painted no text at all, so every assertion over this list would pass \
             against nothing"
        );
        painted
    }

    /// **The warning is on screen when the seed is ticked, and not before.**
    ///
    /// The pure test says `warning_is_shown` answers correctly; this says the
    /// form draws what it answers. Both halves are needed: a constant nothing
    /// paints is not a safety control.
    #[test]
    fn the_form_paints_the_warning_only_with_the_seed_ticked() {
        let mut draft = RecordDraft::default();
        let before = paint(|ui| {
            draw_export_form(ui, &mut draft, "SAP Production", false);
        });
        assert!(before.has(EXPORT_HEADING), "control: the form drew nothing recognisable");
        assert!(before.has(TOTP_LABEL), "the seed tick is not on the form at all");
        assert!(
            !before.has("cloning the second factor"),
            "the warning was painted with no seed ticked"
        );

        let mut ticked = RecordDraft::default();
        ticked.set_totp(true);
        let after = paint(|ui| {
            draw_export_form(ui, &mut ticked, "SAP Production", false);
        });
        assert!(
            after.has(SEED_WARNING),
            "the seed was ticked and the warning was NOT painted: {:?}",
            after.0
        );
        assert!(after.has(NEEDS_PASSPHRASE), "the greyed button's reason was not painted");
    }

    /// The import form paints the field **names** of a record it was given,
    /// and none of the values.
    #[test]
    fn the_import_form_paints_names_and_never_values() {
        static SEALED: OnceLock<crate::record::SealedSeed> = OnceLock::new();
        let record = Record {
            name: "SAP Production".to_string(),
            username: Some("dplatonov".to_string()),
            password: Some(Zeroizing::new("hunter2".to_string())),
            uri: Some("https://sap.example".to_string()),
            notes: None,
            totp_sealed: Some(SEALED.get_or_init(|| seal("JBSWY3DPEHPK3PXP", "pw")).clone()),
            not_after: Some("2026-08-01T00:00:00Z".to_string()),
        };
        // With the passphrase already typed, so the greyed button's reason is
        // the COLLISION rather than the seal -- `import_problem` answers the
        // seal first, and a fixture that left it blank would never see the
        // collision line at all.
        let mut draft = ImportDraft {
            link: "https://send.example/#/x".to_string(),
            passphrase: Zeroizing::new("pw".to_string()),
            ..Default::default()
        };
        let fetched = Ok(record);
        let painted = paint(|ui| {
            draw_import_form(
                ui,
                &mut draft,
                Some(&fetched),
                &Collision::SameName { existing_id: "item-1".to_string() },
                false,
                &FixedClock(NOW),
            );
        });

        assert!(painted.has(WILL_IMPORT_HEADING), "{:?}", painted.0);
        for name in [USERNAME_LABEL, PASSWORD_LABEL, URI_LABEL, SEALED_SEED_LABEL] {
            assert!(painted.has(name), "{name} was not listed: {:?}", painted.0);
        }
        for value in ["dplatonov", "hunter2", "sap.example"] {
            assert!(!painted.has(value), "a VALUE was painted ({value}): {:?}", painted.0);
        }
        // The staleness line, the two collision offers, and the reason the
        // button is grey -- all on screen, with nothing chosen.
        assert!(painted.has("It will still import."), "{:?}", painted.0);
        assert!(painted.has(CREATE_SECOND_LABEL));
        assert!(painted.has(REPLACE_LABEL));
        assert!(painted.has(NEEDS_COLLISION_CHOICE), "{:?}", painted.0);
        assert_eq!(draft.choice, None, "a frame chose an answer on the user's behalf");
    }

    /// A refused payload is painted as its own sentence, not as a shrug.
    #[test]
    fn a_refusal_is_painted_as_the_sentence_that_names_it() {
        let mut draft =
            ImportDraft { link: "https://send.example/#/x".to_string(), ..Default::default() };
        let refused: Result<Record, RecordRefusal> =
            Err(RecordRefusal::UnknownField("surprise".to_string()));
        let painted = paint(|ui| {
            draw_import_form(ui, &mut draft, Some(&refused), &Collision::Fresh, false, &FixedClock(NOW));
        });
        assert!(painted.has("surprise"), "the refused field was not named: {:?}", painted.0);
        assert!(
            !painted.has(WILL_IMPORT_HEADING),
            "a refused payload was previewed as if it had been read: {:?}",
            painted.0
        );
        // And no field-name line either: the preview is the whole thing that
        // must not appear, not just its heading.
        for name in [USERNAME_LABEL, PASSWORD_LABEL, SEALED_SEED_LABEL] {
            assert!(!painted.has(name), "{name} was previewed for a payload that was refused");
        }
        // Control on the same fixture: the form did draw, so the absences
        // above are not the absences of a blank frame.
        assert!(painted.has(IMPORT_HEADING), "{:?}", painted.0);
    }
}

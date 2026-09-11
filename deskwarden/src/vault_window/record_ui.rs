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
//!
//! # Design §5a, and the four blocks of it that are deliberately not built
//!
//! [`draw_export_form`] **is** design §5a — "Compose a Send", captioned
//! FIELD-LEVEL, NOT WHOLE-RECORD — and after the 2026-09 design pass it
//! carries §5a's `RECORD` chip, its `INCLUDE` block with the running count,
//! its boxed tick list and its primary/secondary footer. Four things §5a
//! draws are still not here, and each is a decision rather than an omission.
//!
//! **§5a's whole `ACCESS` block.** The mockup gives the composer an Expires
//! row (`1 h · 24 h · 7 d · 30 d`), a view-count stepper, an "Open with"
//! password with a Generate beside it, a Recipient address, an "only this
//! address can open it" switch and a "tell me when it is opened" switch.
//! Exactly one of the six is a thing this app can do today.
//! [`send_plan_from`] builds a [`SendPlan`], and a `SendPlan` has
//! `delete_in_days` (whose only legal values are `1`, `7` and `30` — there is
//! no sub-day lifetime to offer under `1 h` or `24 h`), `password` and
//! `max_access_count`. The other three — a recipient, an address lock and an
//! open notification — are **not properties of a Bitwarden Send at all**, and
//! there is no server here to make them ones. Building the two that could be
//! built (a password, a view limit) without the four that cannot would give
//! the user a §5a-shaped block that answers half its own questions, which is
//! worse than the honest absence. It needs the owner's call on what the
//! feature is, not a design pass's guess.
//!
//! **The value previews down the right of the tick list.** §5a shows each
//! row's actual content beside its label — the username, a masked password,
//! "3 apps". This form is handed `item_name` and a [`RecordDraft`] and
//! nothing else, on purpose: it is drawn from `vault_window`'s frame closure
//! against an item that is re-resolved by id at submit time, and a form that
//! held a record's values between frames is a form holding a password it did
//! not need to hold.
//!
//! **The password row's red treatment** (`background: #fdf3f2`, a red tick,
//! and the badge "visible to anyone with the link"). New copy, and safety
//! copy at that; this file already has one pinned safety sentence in
//! [`SEED_WARNING`] and a second belongs to whoever owns the wording.
//!
//! **§5a's field set.** The mockup's five rows are Username, Password,
//! One-time code, Notes and **Autofill targets**; this form's five are
//! Username, Password, **Website**, Notes and TOTP seed. `Autofill targets`
//! is not something [`crate::record::payload::Record`] can carry, and the
//! seed sits last rather than third because [`SEED_WARNING`] and its
//! passphrase field hang off it and belong at the end of the list rather than
//! through the middle of it.

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

/// Design §5a's first section eyebrow, over the record the composer was
/// opened against.
///
/// The design's own word, in the design's own case: `text-transform:
/// uppercase` is applied to the literal `Record` in the mockup, and
/// [`theme::eyebrow`] deliberately does not uppercase for the caller -- see
/// that function -- so the constant carries the case the glyphs are painted
/// in and a paint test can look for it.
pub const RECORD_EYEBROW: &str = "RECORD";

/// Design §5a's second section eyebrow, over the tick list. See
/// [`RECORD_EYEBROW`].
pub const INCLUDE_EYEBROW: &str = "INCLUDE";

/// How many fields the tick list offers, and therefore the denominator in
/// [`include_counter`].
///
/// **Derived from nothing and checked against the drawing**, deliberately:
/// [`RecordSelection`] is a struct of five `bool`s and not a collection, so
/// there is no `len()` to ask, and a hand-written 5 that drifted from the
/// rows actually drawn would put a counter on screen that disagrees with the
/// list under it. `the_include_counter_counts_every_row_the_form_draws` is
/// what holds the two together.
pub const INCLUDE_FIELD_COUNT: usize = 5;

/// Design §5a's `2 of 5 fields`, beside the [`INCLUDE_EYEBROW`].
///
/// **A pure function of the draft**, like every other answer these two forms
/// paint, so the sentence can be asserted without running a frame. It is also
/// the only thing on this form that says how much of the record is about to
/// travel *as a number*: five tick-boxes are five separate facts, and a
/// sender scanning the card before pressing a publish button wants the
/// total, which is exactly the argument §5a makes for putting it there.
///
/// Singular at one, because "1 of 5 fields" is the one count a form like this
/// is most likely to be showing and reading it as a plural is the sort of
/// thing that makes a careful screen look careless. The same rule
/// [`super::send_ui::lifetime_label`] follows for `1 day`.
pub fn include_counter(draft: &RecordDraft) -> String {
    let sel = &draft.selection;
    let ticked = [sel.username, sel.password, sel.uri, sel.notes, sel.totp]
        .into_iter()
        .filter(|on| *on)
        .count();
    let noun = if ticked == 1 { "field" } else { "fields" };
    format!("{ticked} of {INCLUDE_FIELD_COUNT} {noun}")
}

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
///
/// **Not §5a's `Create & copy link`, and that is a refusal rather than an
/// oversight.** Pressing this starts a `bw send create` whose link comes back
/// in `vault_window`'s create banner; nothing on that path touches the
/// clipboard. A button promising a copy that does not happen is worse than
/// one that promises less, and the alternative -- making the copy happen --
/// is a behaviour change (a public URL silently replacing whatever the user
/// had on the clipboard) that belongs to whoever owns the clipboard rules in
/// this app, not to a design pass. See this file's report note.
pub const EXPORT_SUBMIT_LABEL: &str = "Create link";

/// The export form's way out.
///
/// A named constant rather than the bare `"Cancel"` it used to be, for the
/// reason every other label on these two forms is one: it is now the thing
/// `the_export_footer_wears_the_design_systems_two_buttons` finds the
/// secondary button by, and a literal that only exists inside a closure is a
/// literal a paint test has to restate.
pub const EXPORT_CANCEL_LABEL: &str = "Cancel";

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

/// The top strip of either form, measured rather than guessed: [`card`]'s
/// 12-point margin plus the ~18 [`heading`]'s 14px line occupies. It is what
/// `draw_export_modal` and `draw_import_modal` hand
/// [`theme::modal_drag_handle`] as the band the card is dragged by.
///
/// A point short of where the first control begins -- `heading` follows itself
/// with `add_space(8.0)` -- so the grab strip cannot reach one. It is its own
/// number and not `theme::MODAL_PLAIN_HEADER_HEIGHT` because these two cards
/// are the tighter [`card`] frame and not the 20-point one the rest of the
/// hand-built modals use; sharing a constant between two different paddings is
/// how a handle ends up over a text field.
const FORM_HEADER_HEIGHT: f32 = 30.0;

fn note(ui: &mut egui::Ui, text: &str, colour: egui::Color32) {
    ui.label(egui::RichText::new(text).size(11.0).color(colour));
}

/// The tick list's own box, from design §5a: `border: 1px solid #eae7e7;
/// border-radius: 10px`, its rows at `padding: 10px 12px`.
///
/// The 10 is split either side of the row rule as [`TICK_ROW_GAP`] rather
/// than spent as padding on each row, because egui lays these rows out as
/// widgets in a column and there is no per-row box to pad.
const TICK_LIST_RADIUS: u8 = 10;
const TICK_LIST_PAD_X: i8 = 12;
const TICK_LIST_PAD_Y: i8 = 8;
const TICK_ROW_GAP: f32 = 5.0;

/// The monogram tile beside the record's name in §5a's `RECORD` chip.
///
/// 32, the design's own, and the same tile `item_list` draws a row with --
/// the point of the chip is that it is recognisably the row the composer was
/// opened from.
const RECORD_CHIP_TILE: f32 = 32.0;

/// Design §5a's picked-record chip: a monogram tile and the record's name, in
/// a blue-wash box edged in [`theme::BLUE_EDGE`].
///
/// # Three things §5a draws here that are deliberately not built
///
/// **The `Change` link.** §5a's chip ends in a `Change` affordance, which
/// implies a record picker inside the composer. There is none, and there
/// should not be: this composer is opened *against the item the user has
/// selected in the list*, which is why [`draw_export_modal`] is a modal
/// rather than a pane -- the list stays on screen behind it. "Change" is
/// therefore already spelled "press Escape and click the other row", and a
/// second, in-card way to choose a record would be a second selection model
/// in a window that has one.
///
/// **The `Login · Engineering` subtitle.** The kind and the folder are real
/// facts about the item and this function is handed neither -- it takes the
/// name, because the name is what [`RecordSend`] copies at open time
/// precisely so the heading cannot go blank if the item disappears
/// underneath. Passing two more strings through for a subtitle is a change to
/// what the composer is *given*, not to how it draws, and it belongs with the
/// rest of §5a's Access block rather than on its own.
///
/// **The `SP` initials as the design's literal glyphs.** The design's tile
/// says `SP` because its record is called "SAP Production"; ours says
/// whatever [`theme::initials`] makes of the record actually being sent,
/// which is the same function the item list and the delete modal use.
fn record_chip(ui: &mut egui::Ui, item_name: &str) {
    egui::Frame::new()
        .fill(theme::BLUE_WASH)
        .stroke(egui::Stroke::new(1.0, theme::BLUE_EDGE))
        .corner_radius(CornerRadius::same(TICK_LIST_RADIUS))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                theme::avatar(ui, &theme::initials(item_name), RECORD_CHIP_TILE, true);
                ui.add_space(11.0);
                // 13px and semibold in [`theme::BLUE_DEEP`], which is §5a's
                // `#14307a` exactly, and is what this app already paints the
                // chosen row's name in.
                ui.label(theme::semibold(item_name, 13.0).color(theme::BLUE_DEEP));
            });
        });
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

        // **§5a's `RECORD` block: the record is NAMED, not mentioned.**
        //
        // The design opens the composer with a chip -- a monogram tile, the
        // record's name set in the blue the rest of this app gives a chosen
        // thing, and a subtitle -- under an eyebrow that says what the block
        // is. What stood here was the item's name in [`note`]'s 11px grey,
        // which is the size and colour this file uses for fine print: the one
        // line that says WHICH record is about to be published was drawn
        // quieter than the sentence explaining why the button is grey.
        //
        // The tile is `theme::avatar`, which is the same tile the item list
        // and the delete modal draw, so the chip reads as the row it was
        // opened from. `emphasized` is on for §5a's reason: this record is
        // the chosen one, and blue-on-wash is what this app's tiles already
        // say that with.
        theme::eyebrow(ui, RECORD_EYEBROW);
        ui.add_space(6.0);
        record_chip(ui, item_name);
        ui.add_space(12.0);

        // **§5a's `INCLUDE` block**, with the design's own running count
        // beside its eyebrow. See [`include_counter`] for why the number is
        // worth having and why it is a pure function.
        ui.horizontal(|ui| {
            theme::eyebrow(ui, INCLUDE_EYEBROW);
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(include_counter(draft))
                    .size(12.0)
                    .color(theme::TEXT_GHOST),
            );
        });
        ui.add_space(6.0);

        // **The ticks are a LIST, and §5a draws the list as one boxed
        // object**: a 1px hairline outline round the run, a 10px radius, and
        // the lighter `#f3f2f2` rule between one row and the next. Both
        // weights are already this design system's -- `theme::HAIRLINE` is
        // the card border and `theme::row_rule` is the between-rows rule the
        // detail pane draws -- so the box is assembled out of the two
        // dividers the app has rather than a third.
        //
        // It is not decoration. Five loose check-boxes stacked in a column
        // are five independent questions; the same five inside one outline
        // are the answer to "what travels", which is the whole subject of
        // this card and the thing §5a's caption calls out ("each field is an
        // explicit opt-in"). The counter above only reads as a counter of
        // something once the something has an edge.
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, theme::HAIRLINE))
            .corner_radius(CornerRadius::same(TICK_LIST_RADIUS))
            .inner_margin(egui::Margin::symmetric(TICK_LIST_PAD_X, TICK_LIST_PAD_Y))
            .show(ui, |ui| {
                let mut first = true;
                let rule_between = |ui: &mut egui::Ui, first: &mut bool| {
                    if *first {
                        *first = false;
                    } else {
                        ui.add_space(TICK_ROW_GAP);
                        theme::row_rule(ui);
                        ui.add_space(TICK_ROW_GAP);
                    }
                };

                for (label, ticked) in [
                    (USERNAME_LABEL, &mut draft.selection.username),
                    (PASSWORD_LABEL, &mut draft.selection.password),
                    (URI_LABEL, &mut draft.selection.uri),
                    (NOTES_LABEL, &mut draft.selection.notes),
                ] {
                    rule_between(ui, &mut first);
                    ui.add_enabled(
                        enabled,
                        egui::Checkbox::new(
                            ticked,
                            egui::RichText::new(label).size(12.0).color(theme::TEXT_SECONDARY),
                        ),
                    );
                }

                // The seed's tick goes through `set_totp` rather than a
                // `&mut bool`, so unticking it drops the passphrase.
                rule_between(ui, &mut first);
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
            });

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
        // **§5a's footer: a filled primary beside an outlined secondary.**
        //
        // The two answers on this card were two bare `egui::Button`s with the
        // same default fill, the same default outline and the same 26px
        // height, differing only in the colour of their text -- so the
        // publish and the throw-away read as a matched pair, and §5a's
        // one clear point about this footer (a blue button and a white one)
        // was the one thing missing from it. `theme.rs` names this defect on
        // the item form's Save, and `primary_button_enabled` is the half of
        // the design system that exists so a footer needing `add_enabled` has
        // no reason to leave it.
        //
        // These are `theme::BUTTON_HEIGHT`'s 32 and not §5a's 34 -- see
        // `send_ui::draw_composer`'s footer for the argument, which is the
        // same one and is made once there.
        ui.horizontal(|ui| {
            if theme::primary_button_enabled(ui, EXPORT_SUBMIT_LABEL, None, can_submit).clicked() {
                action = RecordUiAction::SubmitExport;
            }
            ui.add_space(8.0);
            if ui
                .add_enabled_ui(enabled, |ui| theme::secondary_button(ui, EXPORT_CANCEL_LABEL))
                .inner
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

    theme::movable_modal(ctx, egui::Area::new(egui::Id::new("record-send-modal")))
        .show(ctx, |ui| {
            // Before the form, not after: a drag strip laid over the form's
            // controls would swallow their clicks. See
            // `theme::modal_drag_handle`.
            theme::modal_drag_handle(ui, FORM_HEADER_HEIGHT);
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

    theme::movable_modal(ctx, egui::Area::new(egui::Id::new("record-import-modal")))
        .show(ctx, |ui| {
            theme::modal_drag_handle(ui, FORM_HEADER_HEIGHT);
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

    /// Everything one frame painted that these tests can ask about: the
    /// glyph runs, where each of them landed, and every filled rectangle with
    /// the colour it was filled in.
    ///
    /// **The fills are what make a claim about a BUTTON possible.** A footer
    /// test that can only see text can say "the words `Create link` are on
    /// screen", which stays true of a bare `egui::Button`, of a label, and of
    /// a control drawn at zero size. Which of the two answers is the primary
    /// one is a fact about the rectangle behind the words and nothing else --
    /// the same measurement `detail_edit`'s
    /// `the_disabled_save_button_does_not_look_enabled` makes, for the same
    /// reason.
    struct Painted {
        text: Vec<String>,
        /// Each painted run, with its visual bounds.
        text_rects: Vec<(String, egui::Rect)>,
        /// Each filled rectangle, with its fill.
        fills: Vec<(egui::Rect, egui::Color32)>,
    }

    impl Painted {
        fn has(&self, needle: &str) -> bool {
            self.text.iter().any(|t| t.contains(needle))
        }

        /// Where a painted run landed. The first match wins, which is what
        /// every caller here wants: the labels these forms draw are distinct.
        fn rect_of(&self, needle: &str) -> Option<egui::Rect> {
            self.text_rects
                .iter()
                .find(|(t, _)| t.contains(needle))
                .map(|(_, r)| *r)
        }

        /// The **smallest** filled rectangle that contains `inner`, and its
        /// fill -- in other words, the control a label is sitting on rather
        /// than the card the control is sitting on.
        ///
        /// Smallest and not first, because the card, the modal's body and the
        /// pane all contain the label too and any of them could be painted
        /// first; the innermost box is the only one that is unambiguously
        /// *this* control's.
        fn fill_behind(&self, inner: egui::Rect) -> Option<(egui::Rect, egui::Color32)> {
            self.fills
                .iter()
                .filter(|(r, _)| r.contains_rect(inner))
                .min_by(|a, b| {
                    let area = |r: &egui::Rect| r.width() * r.height();
                    area(&a.0).partial_cmp(&area(&b.0)).expect("finite rects")
                })
                .copied()
        }

        /// [`Self::fill_behind`], found from a label's own words.
        fn button_under(&self, label: &str) -> (egui::Rect, egui::Color32) {
            let text = self
                .rect_of(label)
                .unwrap_or_else(|| panic!("{label:?} was not painted at all: {:?}", self.text));
            self.fill_behind(text).unwrap_or_else(|| {
                panic!("{label:?} was painted with no filled rectangle behind it at all")
            })
        }
    }

    fn collect(shape: &egui::Shape, out: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => {
                out.text.push(text.galley.text().to_owned());
                out.text_rects
                    .push((text.galley.text().to_owned(), text.visual_bounding_rect()));
            }
            egui::Shape::Rect(rect) => out.fills.push((rect.rect, rect.fill)),
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
        let mut painted =
            Painted { text: Vec::new(), text_rects: Vec::new(), fills: Vec::new() };
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        assert!(
            !painted.text.is_empty(),
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
            after.text
        );
        assert!(after.has(NEEDS_PASSPHRASE), "the greyed button's reason was not painted");
    }

    /// **Design §5a's footer is a primary beside a secondary, measured on the
    /// fills and not on the words.**
    ///
    /// Until this pass the two answers were bare `egui::Button`s that
    /// differed only in the colour of their *text*, which no test could see
    /// and which on screen made the publish and the throw-away one matched
    /// pair. The assertion is therefore on the rectangle behind each label:
    /// [`theme::BLUE`] under the submit, [`theme::CARD`] under the way out.
    /// A regression that reverted either button to egui's default chrome
    /// reds here, where reverting it used to change nothing any test could
    /// name.
    ///
    /// The heights are pinned alongside, because a control drawn at zero size
    /// still paints its galley -- `send_ui`'s paint tests learned that once
    /// -- and "there is a blue rectangle somewhere behind these words" is not
    /// yet a button.
    #[test]
    fn the_export_footer_wears_the_design_systems_two_buttons() {
        // A draft with something ticked, so the submit is LIVE: a disabled
        // primary is faded toward the window colour and this test would then
        // be asserting the fade rather than the fill.
        let mut draft = RecordDraft::default();
        assert!(
            export_problem(&draft).is_none(),
            "the fixture wants a submittable draft, or the fill below is a disabled one"
        );
        let painted = paint(|ui| {
            draw_export_form(ui, &mut draft, "SAP Production", false);
        });

        let (submit, submit_fill) = painted.button_under(EXPORT_SUBMIT_LABEL);
        assert_eq!(
            submit_fill,
            theme::BLUE,
            "the composer's submit is not the design system's filled primary -- §5a's footer \
             is a blue button beside a white one, and a footer whose two answers carry the \
             same chrome says neither is the one the card is for"
        );
        assert_eq!(
            submit.height(),
            theme::BUTTON_HEIGHT,
            "the submit is not this app's action-button height"
        );

        let (cancel, cancel_fill) = painted.button_under(EXPORT_CANCEL_LABEL);
        assert_eq!(
            cancel_fill,
            theme::CARD,
            "the composer's way out is not the design system's outlined secondary"
        );
        assert_eq!(
            cancel.height(),
            theme::BUTTON_HEIGHT,
            "the two footer answers are different heights"
        );
        assert_ne!(
            submit_fill, cancel_fill,
            "the two answers paint the same fill, which is the exact defect this pass fixed"
        );
    }

    /// **§5a's `RECORD` block: the eyebrow, the monogram tile and the record's
    /// own name.**
    ///
    /// The name is the one line on this card that says *which* record is
    /// about to become a public link, and it used to be painted in the same
    /// 11px grey this file uses for fine print. Pinned by the tile's size as
    /// well as by the glyphs, because "the initials are on screen" is true of
    /// two letters dropped anywhere.
    #[test]
    fn the_record_block_names_the_record_under_its_own_eyebrow() {
        let mut draft = RecordDraft::default();
        let painted = paint(|ui| {
            draw_export_form(ui, &mut draft, "SAP Production", false);
        });

        assert!(
            painted.has(RECORD_EYEBROW),
            "§5a's RECORD eyebrow is not on the card: {:?}",
            painted.text
        );
        assert!(
            painted.has("SAP Production"),
            "the composer does not name the record it was opened against: {:?}",
            painted.text
        );

        let initials = theme::initials("SAP Production");
        let tile = painted
            .rect_of(&initials)
            .unwrap_or_else(|| panic!("the monogram {initials:?} was not painted"));
        // The glyphs' own ink is smaller than the tile; what is asserted is
        // that they sit inside a box of the design's 32, which is the tile
        // `avatar` allocated.
        assert!(
            tile.width() <= RECORD_CHIP_TILE && tile.height() <= RECORD_CHIP_TILE,
            "the record chip's monogram does not fit the design's {RECORD_CHIP_TILE}pt tile: \
             {tile:?}"
        );
    }

    /// **The counter agrees with the list under it, for every one of the six
    /// tick counts the form can be in.**
    ///
    /// [`include_counter`] is a pure function over a struct of five `bool`s
    /// and [`INCLUDE_FIELD_COUNT`] is a hand-written 5 beside it; nothing in
    /// the type system holds the two together, and a sixth tick row added
    /// without touching the constant would put a counter on screen that
    /// disagrees with the rows it is counting. This walks the real form and
    /// counts the labels it really drew.
    #[test]
    fn the_include_counter_counts_every_row_the_form_draws() {
        let labels = [USERNAME_LABEL, PASSWORD_LABEL, URI_LABEL, NOTES_LABEL, TOTP_LABEL];
        assert_eq!(
            labels.len(),
            INCLUDE_FIELD_COUNT,
            "the denominator and the list of rows have come apart"
        );

        let mut draft = RecordDraft::default();
        let painted = paint(|ui| {
            draw_export_form(ui, &mut draft, "SAP Production", false);
        });
        for label in labels {
            assert!(painted.has(label), "{label} is not a row on the form: {:?}", painted.text);
        }

        // The default draft is username + password, which is the state §5a
        // itself draws -- "2 of 5 fields".
        assert_eq!(include_counter(&RecordDraft::default()), "2 of 5 fields");
        assert!(
            painted.has("2 of 5 fields"),
            "the counter §5a puts beside INCLUDE is not painted: {:?}",
            painted.text
        );

        // Every count, including the two ends and the singular.
        let with = |username, password, uri, notes, totp| RecordDraft {
            selection: RecordSelection { username, password, uri, notes, totp },
            ..RecordDraft::default()
        };
        assert_eq!(include_counter(&with(false, false, false, false, false)), "0 of 5 fields");
        assert_eq!(include_counter(&with(true, false, false, false, false)), "1 of 5 field");
        assert_eq!(include_counter(&with(true, true, true, false, false)), "3 of 5 fields");
        assert_eq!(include_counter(&with(true, true, true, true, false)), "4 of 5 fields");
        assert_eq!(include_counter(&with(true, true, true, true, true)), "5 of 5 fields");
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

        assert!(painted.has(WILL_IMPORT_HEADING), "{:?}", painted.text);
        for name in [USERNAME_LABEL, PASSWORD_LABEL, URI_LABEL, SEALED_SEED_LABEL] {
            assert!(painted.has(name), "{name} was not listed: {:?}", painted.text);
        }
        for value in ["dplatonov", "hunter2", "sap.example"] {
            assert!(!painted.has(value), "a VALUE was painted ({value}): {:?}", painted.text);
        }
        // The staleness line, the two collision offers, and the reason the
        // button is grey -- all on screen, with nothing chosen.
        assert!(painted.has("It will still import."), "{:?}", painted.text);
        assert!(painted.has(CREATE_SECOND_LABEL));
        assert!(painted.has(REPLACE_LABEL));
        assert!(painted.has(NEEDS_COLLISION_CHOICE), "{:?}", painted.text);
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
        assert!(painted.has("surprise"), "the refused field was not named: {:?}", painted.text);
        assert!(
            !painted.has(WILL_IMPORT_HEADING),
            "a refused payload was previewed as if it had been read: {:?}",
            painted.text
        );
        // And no field-name line either: the preview is the whole thing that
        // must not appear, not just its heading.
        for name in [USERNAME_LABEL, PASSWORD_LABEL, SEALED_SEED_LABEL] {
            assert!(!painted.has(name), "{name} was previewed for a payload that was refused");
        }
        // Control on the same fixture: the form did draw, so the absences
        // above are not the absences of a blank frame.
        assert!(painted.has(IMPORT_HEADING), "{:?}", painted.text);
    }
}

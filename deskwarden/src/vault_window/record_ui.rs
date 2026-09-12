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
//! # Design §5a, and the three blocks of it that are deliberately not built
//!
//! [`draw_export_form`] **is** design §5a — "Compose a Send", captioned
//! FIELD-LEVEL, NOT WHOLE-RECORD — and after the 2026-09 design passes it
//! carries §5a's `RECORD` chip, its `INCLUDE` block with the running count,
//! its boxed tick list, its `ACCESS` block and its primary/secondary footer.
//! Three things §5a draws are still not here, and each is a decision rather
//! than an omission.
//!
//! **§5a's `ACCESS` block is now built — three of its six rows.** The block
//! itself is [`super::send_ui::draw_access_block`], shared with the Sends
//! screen's own composer so that the app's two Send composers answer one
//! question one way; the three rows it draws are the lifetime, the view cap
//! and the share password, and the three it does not are argued in full on
//! that function. In short: a **recipient address** and the **"only this
//! address can open it"** switch are one mechanism on the wire — a Send whose
//! `authType` is `Email`, gated by a code the server mails — and the server
//! this app is built against answers `501 Not Implemented` to every one of
//! them, on create, on update and on access alike, so the switch would either
//! fail the publish or, worse, lock nothing while claiming to. **"Tell me
//! when it is opened"** is not a Send field at all; the only thing behind it
//! would be this app polling `accessCount` on a timer, which is a background
//! job and a notification surface wearing a switch's clothes. §5a's
//! **`Generate`** beside the password is absent for a smaller reason, also
//! given there: a generated share password the sender cannot read is one they
//! cannot tell the recipient, and a reveal would be this app's third
//! treatment of a typed secret.
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

// `BUTTON_HEIGHT` -- 26, "matching the Sends pane" -- is DELETED here rather
// than left unused. It was the height of the four bare `egui::Button`s these
// two forms used to draw, and every one of them is now a
// `theme::primary_button_enabled` or a `theme::secondary_button`, which are
// `theme::BUTTON_HEIGHT`'s 32. Its doc comment had also stopped being true:
// the Sends pane's own footer moved to 32 when the composer did, so this
// constant named a match with something that no longer measured that.

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
///
/// # `access` is MOVED, not copied, and that is what makes the block real
///
/// It is the draft's own [`RecordDraft::access`], taken by value: the share
/// password inside it is a [`Zeroizing`] buffer, and a version of this that
/// took `&RecordDraft` and cloned would leave a second copy of that secret
/// behind in the draft for the life of the modal, wiped only when the modal
/// closed. Taken by value, there is one buffer and it travels.
///
/// This is also the only place the three Access answers meet the record, so
/// it is the one place a test can ask "does what the user chose reach the
/// plan that gets published" -- `the_access_block_reaches_the_plan_that_is_published`
/// is that test. `..access` rather than `..SendPlan::default()` is the whole
/// of the change and would be an easy thing to lose in a refactor; losing it
/// would mean the Access block drew, validated and published nothing, in
/// silence.
pub fn send_plan_from(record: &Record, access: SendPlan) -> SendPlan {
    SendPlan {
        name: record.name.clone(),
        text: write_json(record),
        hidden: true,
        ..access
    }
}

// ---------------------------------------------------------------------------
// Task 6 -- the export surface
// ---------------------------------------------------------------------------

/// The heading over the export form -- §5a's own words.
///
/// It read `Send this record`, which is more precise (it IS this record,
/// named in the chip directly under the band) and is not what the design
/// says. The owner, looking at the two side by side: "also tigheten the UI
/// as per design - it is not 100% as". Precision the design did not ask for
/// is still a departure from it, and this one bought nothing: the chip
/// answers `which record` a line later, in bold, with its own tile.
pub const EXPORT_HEADING: &str = "Send a record";

/// The tick-box labels, in the order they are drawn.
pub const USERNAME_LABEL: &str = "Username";
/// See [`USERNAME_LABEL`].
pub const PASSWORD_LABEL: &str = "Password";
/// See [`USERNAME_LABEL`]. **§5a's word, which is the READ pane's card
/// title too**: a login's addresses are where it is OFFERED, and the row
/// counts them rather than naming one. `Website` was this file's own word
/// for the same field and the singular was wrong the moment a record had
/// two.
pub const URI_LABEL: &str = "Autofill targets";
/// See [`USERNAME_LABEL`].
pub const NOTES_LABEL: &str = "Notes";
/// See [`USERNAME_LABEL`]. **Not ticked by default.** A seed is not a
/// default.
///
/// §5a's word, and the one the rest of this app uses for the same thing --
/// the edit form's card, the read pane's row and 6d's card all say
/// `One-time code`. `TOTP seed` was accurate and was the only place in the
/// app that said it; the sentence under the row is where the word SEED
/// belongs, because that is the sentence about what a seed is.
pub const TOTP_LABEL: &str = "One-time code";

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
/// [`crate::send::lifetime_label`] follows for `1 hour` and `1 day`.
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
/// answered.** This used to read `Create link`, on the argument that a
/// button promising a copy that does not happen is worse than one promising
/// less -- and that making the copy happen was a behaviour change belonging
/// to whoever owns this app's clipboard rules rather than to a design pass.
///
/// The copy happens now. `vault_window::drain_send_create` puts the access
/// URL on the clipboard through `clipboard::copy_secret`, under the same
/// clearing timer a password gets, because that URL carries the Send's
/// decryption key in its fragment. So the design's own words are true and
/// this says them.
///
/// What settled it was not the design: until that copy existed the ONLY
/// copy of a new link was a sentence in a toast -- a label, unselectable,
/// gone when dismissed. The button was honest about an app that could not
/// hand the user the thing it had just made.
pub const EXPORT_SUBMIT_LABEL: &str = "Create & copy link";

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
    /// Design §5a's `ACCESS` block: how long the link lives, the share
    /// password, and the view cap.
    ///
    /// # Why the draft carries a whole [`SendPlan`] for three fields
    ///
    /// The alternative was three loose fields here and an assembly step in
    /// [`send_plan_from`], and an assembly step is precisely where the value
    /// the form validated and the value that gets published come apart --
    /// `send_ui::SendComposer`'s own doc records that reasoning, and it holds
    /// a `SendPlan` for the same reason. The two Send composers in this app
    /// now hold the same type for the same three answers, so a rule written
    /// about one is true of the other.
    ///
    /// The name and body are not filled in until [`send_plan_from`]: they are
    /// the RECORD, which is re-resolved by id at submit time precisely so
    /// this form never holds a record's values between frames. What is here
    /// between frames is three choices the user made about the link, none of
    /// which is a secret about the vault -- except the share password, which
    /// is a `Zeroizing` inside the plan and dies with the draft.
    pub access: SendPlan,
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
            // Seven days, no password, no view cap -- `SendPlan::default`'s
            // own answers, not a second set written here. §5a's Access block
            // opens on exactly this state, and the design's caption says so:
            // "the password is off until you say so".
            access: SendPlan::default(),
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
///
/// It takes a clock for `crate::send::validate_plan`'s reason: one of the
/// Access rules is that a picked date has not gone by, and the past is not a
/// property of a draft.
pub fn export_problem(
    draft: &RecordDraft,
    now: &dyn crate::send::SendClock,
) -> Option<&'static str> {
    let sel = &draft.selection;
    if sel.totp && draft.passphrase.trim().is_empty() {
        return Some(NEEDS_PASSPHRASE);
    }
    if !(sel.username || sel.password || sel.uri || sel.notes || sel.totp) {
        return Some(NOTHING_TICKED);
    }
    // **The Access block's own refusals, in the words `crate::send` already
    // has for them**, and delegated rather than restated for
    // `send_ui::composer_problem`'s reason exactly: the sentence under this
    // form's button and the refusal inside `plan_to_invocation` must be the
    // same function, or this form can call a draft acceptable that the
    // encoder then rejects -- after the modal has closed, on a background
    // thread, with nothing on screen to explain it.
    //
    // It is `crate::send::validate_access` and not `validate_plan`, because
    // the name and the body do not exist yet: the record is re-resolved by id
    // when Create is pressed, which is the whole reason this form holds no
    // record values between frames. `validate_access` is the three Access
    // rules with the name and body rules left out, and `validate_plan` is
    // those rules plus it -- so this form and the encoder still run the same
    // code over the same three answers, and there is no arm of it that only
    // one of them takes.
    crate::send::validate_access(
        draft.access.lifetime,
        draft.access.password.as_deref().map(String::as_str),
        draft.access.max_access_count,
        now,
    )
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

/// The label on the import form's other button: the one that goes and gets
/// the Send the link names.
///
/// **A named constant now rather than the bare `"Fetch"` this form used to
/// pass inline**, because it is the only label on either of these two cards
/// that was not one -- and the paint test that says which of this footer's
/// two buttons is the filled primary has to name both of them to say it.
pub const FETCH_LABEL: &str = "Fetch";

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

/// The card either form sits in, **and the rectangle it measured out to**.
///
/// The rect is new and it is what [`theme::modal_corner_mark`] needs: the
/// dismiss ✕ is inset from the CARD's right-hand edge, and that edge is not
/// the content column's plus the margin on these two cards. The import form
/// sets a 360-wide maximum and overflows it, painting a 449-wide card around a
/// 336-wide column, so arithmetic off the column would have put the mark a
/// hundred points adrift of the corner. The frame has just measured itself and
/// can simply say.
/// It is `theme::form_card` and no longer a `Frame` spelled out here, and
/// that swap is the whole of what this function now is.
///
/// **Why it moved.** §5a's composer is a bordered, rounded, SHADOWED card in
/// three bands -- a header closed by a hairline, a padded body, a tinted
/// footer -- and what stood here was white-on-white with an 8px radius and no
/// edge at all. The Sends screen's text composer had written out the same
/// four lines independently, so the two composers agreed only by coincidence,
/// and the coincidence was that neither was §5a. One function draws the card
/// now; `theme::form_card` argues every part of it.
fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> (egui::Rect, R) {
    theme::form_card(ui, add)
}

/// Either form's first band, answering with **the rectangle the heading
/// occupies** -- the line the dismiss ✕ is hung on once [`card`] has measured
/// itself.
///
/// The 8-point space this used to follow itself with is gone: the header is a
/// band now, and what separates it from the first control is its own padding
/// and the hairline under it rather than a gap.
fn heading(ui: &mut egui::Ui, text: &str) -> egui::Rect {
    theme::form_card_header(ui, text)
}

/// The top strip of either form: what `draw_export_modal` and
/// `draw_import_modal` hand [`theme::modal_drag_handle`] as the band the card
/// is dragged by.
///
/// It is now exactly [`theme::FORM_CARD_HEADER_HEIGHT`] -- the header band,
/// to the hairline that closes it -- rather than a number of this file's own.
/// The band is a real strip with nothing in it but the title, which is
/// precisely what a drag handle wants, and a handle measured off anything
/// else is how one ends up over a text field.
const FORM_HEADER_HEIGHT: f32 = theme::FORM_CARD_HEADER_HEIGHT;

fn note(ui: &mut egui::Ui, text: &str, colour: egui::Color32) {
    ui.label(egui::RichText::new(text).size(11.0).color(colour));
}

/// The tick list's own box, from design §5a: `border: 1px solid #eae7e7;
/// border-radius: 10px`.
///
/// **The list carries no padding of its own**, and that is the change that
/// made these rows §5a's. Its rows declare `padding: 10px 12px` and its rule
/// runs edge to edge between them; a padded list would inset that rule and
/// leave a hairline floating inside a box. So the padding is
/// [`INCLUDE_PAD_X`]/[`INCLUDE_PAD_Y`], applied per row, and the three
/// constants that spent it here -- a list padding and a gap split either side
/// of the rule -- are gone with the column of `egui::Checkbox`es they were
/// measured for.
const TICK_LIST_RADIUS: u8 = 10;

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
fn record_chip(ui: &mut egui::Ui, item_name: &str, kind_and_folder: &str) {
    egui::Frame::new()
        .fill(theme::BLUE_WASH)
        .stroke(egui::Stroke::new(1.0, theme::BLUE_EDGE))
        .corner_radius(CornerRadius::same(TICK_LIST_RADIUS))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::avatar(ui, &theme::initials(item_name), RECORD_CHIP_TILE, true);
                ui.add_space(11.0);
                // **Two lines, which is §5a's own shape.** The name, and
                // under it what the record IS and where it lives -- `Login ·
                // Engineering` in the design. One line was all this drew, and
                // the owner's verdict on the card as a whole was "very plain
                // and not even close to this design".
                //
                // The second line is the READ pane's subtitle, the same two
                // facts in the same order, so the chip reads as the row the
                // composer was opened from rather than as a new naming of it.
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(theme::semibold(item_name, 13.0).color(theme::BLUE_DEEP));
                    if !kind_and_folder.is_empty() {
                        ui.label(
                            egui::RichText::new(kind_and_folder)
                                .size(11.0)
                                .color(theme::TEXT_MUTED),
                        );
                    }
                });
            });
        });
}

/// **One row of §5a's include list.** Answers whether it was clicked.
///
/// The whole row is the target, not the 17-point square: §5a draws a row and
/// a row is what a pointer aims at. `egui::Checkbox` cannot do this -- it
/// draws its own square at its own size in its own palette and senses only
/// itself -- which is why this is built out of `theme::opt_in_box` and an
/// `interact` over the band.
///
/// **The password row is tinted, labelled and masked in §5a's red**, and the
/// three together are the design making one point in the only three ways a
/// row has: this is the field that travels in the clear. The pill beside the
/// label says it in words.
fn include_row(ui: &mut egui::Ui, row: &IncludeRow, enabled: bool) -> bool {
    let danger = row.field == IncludeField::Password && row.ticked;
    // §5a's `#b42318` on the ticked password, which is `theme::ERROR`
    // exactly -- a step louder than the `DANGER_INK` its label is set in, so
    // the three red things on this row do not all shout at one pitch.
    let tone = if danger { theme::ERROR } else { theme::BLUE };
    let mut clicked = false;
    let band = egui::Frame::new()
        .fill(if danger { theme::DANGER_WASH } else { egui::Color32::TRANSPARENT })
        .inner_margin(egui::Margin::symmetric(INCLUDE_PAD_X, INCLUDE_PAD_Y))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                theme::opt_in_box(ui, row.ticked, tone);
                ui.add_space(INCLUDE_GAP);
                let ink = if danger { theme::DANGER_INK } else { theme::INK };
                if danger {
                    ui.label(theme::semibold(row.label, INCLUDE_LABEL_PX).color(ink));
                    ui.add_space(7.0);
                    visible_pill(ui);
                } else {
                    ui.label(
                        egui::RichText::new(row.label).size(INCLUDE_LABEL_PX).color(ink),
                    );
                }
                // The value hangs off the right edge, which is §5a's own
                // `flex: 1` on the label with the value after it. Measured and
                // right-aligned rather than pushed, so a long address cannot
                // walk the label off the row.
                if !row.value.is_empty() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let text = if row.secret {
                            theme::letterspaced_mono_in(
                                &row.value,
                                INCLUDE_VALUE_PX,
                                0.0,
                                theme::DANGER_QUIET,
                                theme::ascent_of(
                                    ui.ctx(),
                                    &egui::FontId::new(
                                        INCLUDE_VALUE_PX,
                                        egui::FontFamily::Monospace,
                                    ),
                                ),
                            )
                        } else {
                            egui::text::LayoutJob::simple_singleline(
                                row.value.clone(),
                                egui::FontId::proportional(INCLUDE_VALUE_PX),
                                theme::TEXT_MUTED,
                            )
                        };
                        ui.label(text);
                    });
                }
            });
        })
        .response
        .rect;
    // Registered after the contents so it is on top of them -- nothing inside
    // a row of this list is separately clickable, so there is nothing for it
    // to steal. Greyed rows are inert, which is what `in_flight` means
    // everywhere else on this card.
    if enabled {
        let hit = ui.interact(band, ui.next_auto_id().with(row.label), egui::Sense::click());
        if hit.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        clicked = hit.clicked();
    }
    clicked
}

/// §5a's `visible to anyone with the link` pill, beside the password's name.
///
/// Its own drawing rather than [`theme::state_pill`]: that one is a 20-point
/// bordered capsule for a STATE -- Waiting, Used, Changed -- and this is a
/// `border-radius: 5px` tag with no border and no mark, sitting inline in a
/// row of text. Two shapes that say different things about a row.
fn visible_pill(ui: &mut egui::Ui) {
    egui::Frame::new()
        .fill(theme::DANGER_PILL)
        .corner_radius(CornerRadius::same(5))
        .inner_margin(egui::Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(VISIBLE_PILL).size(11.0).color(theme::DANGER_QUIET),
            );
        });
}

/// §5a's caution band, minus the promise this build cannot keep.
///
/// The design reads "This Send contains a password. Rotate it after the
/// recipient is done — Deskwarden will offer to when the Send expires."
/// The second clause describes a feature that does not exist: nothing here
/// watches a Send's expiry or offers anything when it passes. Shipping the
/// whole sentence would have been a promise the app does not keep, which is
/// the defect this project has now shipped twice and has notes about in two
/// other modules. The first clause is true, is actionable, and is what a
/// reader does something about.
pub const PASSWORD_TRAVELS: &str =
    "This Send contains a password. Rotate it after the recipient is done.";

/// §5a's own words on the password row's pill.
pub const VISIBLE_PILL: &str = "visible to anyone with the link";

/// §5a's note beside the one-time code row.
pub const TOTP_ROW_NOTE: &str = "rotates \u{2014} the recipient sees a live code";

/// §5a's `padding: 10px 12px` on an include row.
const INCLUDE_PAD_X: i8 = 12;
/// See [`INCLUDE_PAD_X`].
const INCLUDE_PAD_Y: i8 = 10;
/// §5a's `gap: 11px` between the tick and the name.
const INCLUDE_GAP: f32 = 11.0;
/// §5a's `font-size: 13px` on a row's name.
const INCLUDE_LABEL_PX: f32 = 13.0;
/// §5a's `font-size: 12px` on the value beside it.
const INCLUDE_VALUE_PX: f32 = 12.0;

/// Which of [`RecordSelection`]'s five a row is about.
///
/// An enum rather than a `&mut bool` per row, because the seed's tick is not
/// a plain assignment -- `RecordDraft::set_totp` drops the passphrase when it
/// goes off -- and a list whose rows held `&mut bool` would have to make that
/// one row a special case at the point of drawing. The write happens at the
/// call site, where `draft` is not already borrowed by the row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IncludeField {
    Username,
    Password,
    Uri,
    Notes,
    Totp,
}

/// One row of §5a's include list: the tick, the name, and what would travel.
pub struct IncludeRow {
    pub field: IncludeField,
    /// §5a's label for the field.
    pub label: &'static str,
    /// Whether it is going.
    pub ticked: bool,
    /// The right-hand text: the value itself where the field has one, and
    /// §5a's own note where it does not (`rotates — the recipient sees a live
    /// code`, `3 apps`). Empty draws nothing, which is what §5a does on Notes.
    pub value: String,
    /// Whether the value is a secret, and therefore masked and set in the
    /// monospace face §5a masks it in.
    pub secret: bool,
}

/// **§5a's five rows, in §5a's order, with §5a's values beside them.**
///
/// Pure, and taking the item rather than reading one, for this file's
/// standing reason: a decision reachable only from inside a paint closure is
/// a decision no test can call. Every string a row shows is decided here.
///
/// **The value is elided, never the fact of it.** A field the record does not
/// carry still gets its row -- §5a draws five rows whatever the record holds,
/// and a list that grew and shrank with the item would make the counter
/// beside its eyebrow a moving denominator.
pub fn include_rows(
    draft: &RecordDraft,
    item: Option<&crate::vault_bridge::VaultItem>,
) -> Vec<IncludeRow> {
    let login = item.and_then(|i| i.login.as_ref());
    let username = login.and_then(|l| l.username.as_deref()).unwrap_or_default().to_string();
    // §5a's `3 apps` -- a COUNT and not the addresses themselves, because the
    // list is unbounded and the row is one line. The word agrees with the
    // number, so a record with one website does not read `1 apps`.
    let uris = login.map(|l| l.uris.len()).unwrap_or(0);
    let has_seed = login.and_then(|l| l.totp.as_ref()).is_some();
    vec![
        IncludeRow {
            field: IncludeField::Username,
            label: USERNAME_LABEL,
            ticked: draft.selection.username,
            value: username,
            secret: false,
        },
        IncludeRow {
            field: IncludeField::Password,
            label: PASSWORD_LABEL,
            ticked: draft.selection.password,
            // **A fixed mask, not one that tracks the password's length.**
            // §5a draws twelve bullets and this draws ten, and the difference
            // is a rule this app already has: `theme::MASKED_BULLETS`'s doc
            // forbids a length-tracking mask outright, because it tells a
            // shoulder-surfer how many characters to expect. The row's job is
            // to say a password is in the parcel, which a constant mask says
            // exactly as well.
            value: theme::masked_readout(),
            secret: true,
        },
        IncludeRow {
            field: IncludeField::Totp,
            label: TOTP_LABEL,
            ticked: draft.selection.totp,
            value: if has_seed { TOTP_ROW_NOTE.to_string() } else { String::new() },
            secret: false,
        },
        IncludeRow {
            field: IncludeField::Notes,
            label: NOTES_LABEL,
            ticked: draft.selection.notes,
            // §5a draws nothing beside Notes, and nor does this: a note is a
            // paragraph, and a one-line preview of one is a paragraph's first
            // few words pretending to be a summary.
            value: String::new(),
            secret: false,
        },
        IncludeRow {
            field: IncludeField::Uri,
            label: URI_LABEL,
            ticked: draft.selection.uri,
            value: uri_count(uris),
            secret: false,
        },
    ]
}

/// §5a's `3 apps`, with the word agreeing with the number and nothing at all
/// where there is nothing to count.
pub fn uri_count(uris: usize) -> String {
    match uris {
        0 => String::new(),
        1 => "1 address".to_string(),
        n => format!("{n} addresses"),
    }
}

/// §5a's second chip line: what the record IS, and where it lives.
///
/// The design reads `Login · Engineering`, and so does the READ pane's own
/// subtitle -- `detail::header_subtitle_parts` builds the same two runs from
/// the same two facts. Built here rather than borrowed from there because
/// that one paints a folder MARK between the runs and this one is a line of
/// plain text inside a chip; what is shared is the order and the separator,
/// which is what makes the chip read as the row it was opened from.
///
/// Empty when there is no item, which is the one state that draws no second
/// line at all rather than a lonely separator.
fn chip_subtitle(item: Option<&crate::vault_bridge::VaultItem>, folder: Option<&str>) -> String {
    let Some(item) = item else {
        return String::new();
    };
    let kind = crate::vault_bridge::ItemKind::of(item).label();
    match folder {
        Some(name) => format!("{kind} \u{b7} {name}"),
        None => kind.to_string(),
    }
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
    // **What §5a draws beside each tick, borrowed for the frame and stored
    // nowhere.**
    //
    // §5a's include list is not five bare labels: every row carries the
    // value that would travel -- the address, the masked password, `3 apps`
    // -- because the question the card asks is "what travels", and a tick
    // beside the word `Password` does not answer it. The owner, of the
    // version without them: "Current UI is very plain and not even close to
    // this design".
    //
    // **This does not break [`RecordDraft`]'s rule**, which is that the form
    // never holds a record's values BETWEEN frames: this is a borrow that
    // lives for one paint, exactly as `detail::draw_detail_read` takes the
    // item it draws. What is still re-resolved by id at submit time is the
    // record that gets PUBLISHED, which is the half of that rule that
    // matters.
    //
    // `None` where the window cannot find the item any more. The rows then
    // draw their labels and no values, which is what this card looked like
    // before -- a degradation, not a panic.
    item: Option<&crate::vault_bridge::VaultItem>,
    // The folder's NAME, already resolved by `sidebar::folder_name` -- see
    // `detail::draw_detail_read`, which takes it for the same line and the
    // same reason.
    folder: Option<&str>,
    in_flight: bool,
    now: &dyn crate::send::SendClock,
    zone: &dyn crate::local_time::LocalOffset,
) -> RecordUiAction {
    let mut action = RecordUiAction::None;
    let enabled = !in_flight;
    let (card_rect, title) = card(ui, |ui| {
        // **§5a's paper plane**, which only this card wears: the import form
        // below is the same band without one, because the design draws the
        // mark on the card that SENDS.
        let title = theme::form_card_header_marked(ui, EXPORT_HEADING, true);
        theme::form_card_body(ui, |ui| {
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
            ui.add_space(theme::EYEBROW_GAP);
            record_chip(ui, item_name, &chip_subtitle(item, folder));
            ui.add_space(theme::BLOCK_GAP);

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
            ui.add_space(theme::EYEBROW_GAP);

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
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                    // **§5a's rows, each a tick, a name and the value that
                    // would travel** -- and the rule between them runs edge to
                    // edge, so the list carries no inner margin of its own and
                    // every padding below is a row's.
                    let rows = include_rows(draft, item);
                    let last = rows.len().saturating_sub(1);
                    for (index, row) in rows.into_iter().enumerate() {
                        if include_row(ui, &row, enabled) {
                            let on = !row.ticked;
                            match row.field {
                                // The seed's tick goes through `set_totp`
                                // rather than a `&mut bool`, so unticking it
                                // drops the passphrase with it.
                                IncludeField::Totp => draft.set_totp(on),
                                IncludeField::Username => draft.selection.username = on,
                                IncludeField::Password => draft.selection.password = on,
                                IncludeField::Uri => draft.selection.uri = on,
                                IncludeField::Notes => draft.selection.notes = on,
                            }
                        }
                        if index != last {
                            theme::row_rule(ui);
                        }
                    }
                });

            if warning_is_shown(draft) {
                ui.add_space(8.0);
                // Painted in the error colour and at the same size as the labels
                // above it, not as fine print: it is the sentence that decides
                // whether the tick above was a mistake.
                ui.label(egui::RichText::new(SEED_WARNING).size(12.0).color(theme::ERROR));
                ui.add_space(8.0);
                // `theme::hinted_field` and not a bare `egui::TextEdit`: this box
                // sat directly under the composer's other boxes wearing egui's
                // frame instead of the design's, which is the same defect the
                // Sends composer's name and body fields had. See that function.
                ui.add_enabled_ui(enabled, |ui| {
                    theme::hinted_field(ui, &mut draft.passphrase, PASSPHRASE_HINT, true)
                });
                ui.add_space(4.0);
                note(ui, PASSPHRASE_NOTE, theme::TEXT_FAINT);
            }

            // **§5a's `ACCESS` block**, drawn by the one function in this app that
            // draws it -- `send_ui::draw_access_block`, which the Sends screen's
            // own composer also calls. Everything about what it draws, what it
            // deliberately does not draw, and why each absence is a decision
            // rather than a to-do is argued there, in one place, rather than
            // halved between this file and that one.
            //
            // It lives in `send_ui` and is called from here, which is the
            // direction that makes sense of the two: `send_ui` is this window's
            // Send module and already owns the composer, the pane and the plan;
            // `record_ui` is a modal that borrows the Send machinery to publish
            // one record. The block is a control for three fields of a
            // `crate::send::SendPlan`, so it belongs beside the screen that is
            // made of `SendPlan`s.
            ui.add_space(12.0);
            super::send_ui::draw_access_block(
                ui,
                super::send_ui::AccessControls {
                    lifetime: &mut draft.access.lifetime,
                    password: &mut draft.access.password,
                    max_access_count: &mut draft.access.max_access_count,
                },
                enabled,
                now,
                zone,
            );
        });

        let problem = export_problem(draft, now);
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
        //
        // **And they are in §5a's footer BAND now**, which they were not:
        // the two answers used to be simply the last things in the body, with
        // the refusal sentence trailing after them as loose prose. Both cards
        // are fixed together and by the same functions, because two composers
        // that publish the same object and end differently is the defect one
        // level up from the one the owner reported.
        // **§5a's caution band**, drawn between the body and the answers, and
        // only when the thing it cautions about is really in the parcel.
        //
        // §5a's own sentence continues "-- Deskwarden will offer to when the
        // Send expires", and that half is NOT drawn: nothing in this build
        // offers a rotation when a Send expires, and a card that promised one
        // would be the drawn-and-dead defect this project has shipped twice
        // already. What is left is the true half, and it is the half a reader
        // does something about.
        if draft.selection.password {
            theme::form_card_caution(ui, PASSWORD_TRAVELS);
        }
        theme::form_card_footer(ui, |ui| {
            // The footer's one right-hand slot, and the three things that
            // want it, exactly as `send_ui::draw_composer` arranges them --
            // the argument is made there, once, and this card obeys it rather
            // than restating it.
            let note_text = problem.unwrap_or(super::send_ui::APPEARS_IN_SENDS);
            let mut beside = false;
            ui.horizontal(|ui| {
                if theme::primary_button_enabled(ui, EXPORT_SUBMIT_LABEL, None, can_submit)
                    .clicked()
                {
                    action = RecordUiAction::SubmitExport;
                }
                ui.add_space(theme::FORM_FOOTER_GAP);
                if ui
                    .add_enabled_ui(enabled, |ui| theme::secondary_button(ui, EXPORT_CANCEL_LABEL))
                    .inner
                    .clicked()
                {
                    action = RecordUiAction::Cancel;
                }
                if theme::form_footer_note_width(ui, note_text) + theme::FORM_FOOTER_GAP
                    <= ui.available_width()
                {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        theme::form_footer_note(ui, note_text);
                    });
                    beside = true;
                }
            });
            if !beside {
                ui.add_space(theme::FORM_FOOTER_GAP);
                theme::form_footer_note(ui, note_text);
            }
        });
        title
    });
    // **The ✕ answers exactly what [`EXPORT_CANCEL_LABEL`] answers, and is
    // gated by exactly what gates that button.** A mark that stayed live while
    // Cancel was greyed would be a second exit from a state the card's own
    // exit refuses -- the hazard this mark exists to remove rather than to
    // create. Greyed, it still PAINTS: a control that vanished for the
    // duration of a send and came back would read as the card changing shape.
    //
    // **Neither of this file's two modals binds Escape at all**, here or in
    // `vault_window::mod`, so there is no keyboard gesture for the mark to
    // have to agree with -- and one is deliberately not invented in passing,
    // because binding a key to a card is a decision about that card's
    // keyboard rather than about its corner.
    if theme::modal_corner_mark_gated(ui, card_rect, title, enabled) {
        action = RecordUiAction::Cancel;
    }
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
    let (card_rect, title) = card(ui, |ui| {
        let title = heading(ui, IMPORT_HEADING);
        let ok = theme::form_card_body(ui, |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                theme::hinted_field(ui, &mut draft.link, LINK_HINT, false)
            });
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
                    ui.add_enabled_ui(enabled, |ui| {
                        theme::hinted_field(ui, &mut draft.passphrase, IMPORT_PASSPHRASE_HINT, true)
                    });
                }

                if let Collision::SameName { .. } = collision {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(collision_prompt(&record.name))
                            .size(12.0)
                            .color(theme::INK),
                    );
                    ui.add_space(4.0);
                    // **One control with two positions, and not two buttons.**
                    //
                    // This was a `ui.horizontal` of two `egui::Button`s with
                    // `.selected()` on whichever was chosen -- separated by
                    // egui's item spacing, each with its own outline, each 160
                    // points wide whatever its label said. That is the exact
                    // shape `theme::segmented_control`'s own documentation
                    // records this app moving away from, and the Sends composer's
                    // lifetime row was moved off it a pass ago; this prompt was
                    // the last `.selected()` pair left in the two record forms.
                    //
                    // It matters more here than it did there. This is the one
                    // question in this whole feature whose wrong answer destroys
                    // data the user already had, and `.selected()` paints the
                    // chosen button in egui's own selection fill with
                    // [`theme::INK`] over it -- a grey that is easy to miss at a
                    // glance across two identical 160-point boxes. The segmented
                    // run fills the answer in force with [`theme::BLUE`] behind
                    // white, which is the weight this app gives a primary button
                    // and is the weight "you are about to replace an item" should
                    // have.
                    //
                    // **Neither cell is lit until the user lights one.**
                    // `draft.choice` starts `None`, both `selected` flags are
                    // therefore `false`, and there is no `unwrap_or` here and
                    // must not be: `import_can_proceed` refuses while the choice
                    // is `None`, and a default would answer a question nobody
                    // was asked.
                    let choices = [
                        (CREATE_SECOND_LABEL, CollisionChoice::CreateSecond),
                        (REPLACE_LABEL, CollisionChoice::Replace),
                    ];
                    let segments: Vec<theme::Segment<'_>> = choices
                        .iter()
                        .map(|(label, choice)| theme::Segment {
                            label,
                            selected: draft.choice == Some(*choice),
                        })
                        .collect();
                    // `segmented_control_disabled` and not `add_enabled`, for the
                    // reason that split exists: the inert run senses hover only,
                    // so while an import is running there is no path by which a
                    // cell can be pressed, and the answer already given stays
                    // legible in the wash rather than greying away.
                    if enabled {
                        if let Some(index) = theme::segmented_control(ui, &segments) {
                            draft.choice = Some(choices[index].1);
                        }
                    } else {
                        theme::segmented_control_disabled(ui, &segments);
                    }
                }
            }
            // Handed back out of the body band because the footer's own refusal
            // sentence is a fact about what the link fetched.
            ok
        });

        let problem = import_problem(ok, draft, collision);
        let can_proceed = import_can_proceed(ok, draft, collision, in_flight);
        // **The import footer is the design system's two buttons**, and until
        // this pass it was neither -- two bare `egui::Button`s differing only
        // in the colour of their text and the width of their box, the exact
        // defect the export form's footer was fixed for one pass earlier.
        // The two modals are opened from the same window, sit in the same
        // card frame and answer the same shape of question; leaving one on
        // the design system and one off it made them look like they came from
        // different applications, for no reason anybody could name.
        //
        // **Which button is the primary moved, and that is the point of
        // doing this rather than swapping the widgets.** `Fetch` was drawn
        // first and in [`theme::TEXT_MUTED`], and `IMPORT_SUBMIT_LABEL` --
        // the press that actually creates an item -- was drawn second in
        // plain ink. The create is the answer this form is FOR, so it takes
        // the filled primary and `Fetch` takes the outlined secondary, which
        // is exactly the arrangement `EXPORT_SUBMIT_LABEL` and
        // `EXPORT_CANCEL_LABEL` have on the other card.
        //
        // The order stays `Fetch` then create, because that is the order the
        // steps happen in: you cannot import a record you have not fetched,
        // and a footer that put the second step first would read as a form
        // you could submit straight away. `primary_button_enabled` carries
        // the greying, so the create is visibly not pressable until
        // `import_can_proceed` says it is.
        //
        // These are `theme::BUTTON_HEIGHT`'s 32 and not this file's old 26 --
        // see `draw_export_form`'s footer, where that argument is made once.
        //
        // **And in §5a's footer BAND**, for the export card's reason: these
        // two cards are opened from the same window and sit in the same
        // frame, so one of them growing a banded footer and the other keeping
        // a row of buttons at the end of its body would be the same
        // divergence this pass exists to close, one card over.
        theme::form_card_footer(ui, |ui| {
            let mut beside = false;
            ui.horizontal(|ui| {
                if ui
                    .add_enabled_ui(enabled && !draft.link.trim().is_empty(), |ui| {
                        theme::secondary_button(ui, FETCH_LABEL)
                    })
                    .inner
                    .clicked()
                {
                    action = RecordUiAction::FetchLink;
                }
                ui.add_space(theme::FORM_FOOTER_GAP);
                if theme::primary_button_enabled(ui, IMPORT_SUBMIT_LABEL, None, can_proceed)
                    .clicked()
                {
                    action = RecordUiAction::SubmitImport;
                }
                // **No standing note on this card**, and the absence is the
                // decision: `APPEARS_IN_SENDS` answers "where does the thing
                // I am publishing end up", and this form publishes nothing --
                // it pulls a record INTO the vault. The slot carries the
                // refusal when there is one and is empty when there is not,
                // rather than being filled with a sentence invented to
                // occupy it.
                if let Some(problem) = problem {
                    if theme::form_footer_note_width(ui, problem) + theme::FORM_FOOTER_GAP
                        <= ui.available_width()
                    {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            theme::form_footer_note(ui, problem);
                        });
                        beside = true;
                    }
                }
            });
            if let Some(problem) = problem {
                if !beside {
                    ui.add_space(theme::FORM_FOOTER_GAP);
                    theme::form_footer_note(ui, problem);
                }
            }
        });
        title
    });
    // **On this card the ✕ is the ONLY way out**, which makes it the one mark
    // in this pass that is a fix rather than a consistency. This form has no
    // Cancel button -- the export's footer has one, this one's holds `Fetch`
    // and the create -- and neither this function nor `vault_window::mod`
    // binds Escape for it. `mod` has always had the `RecordUiAction::Cancel`
    // arm that closes the card; nothing in the form ever produced one, so a
    // user who opened `Import from a Send...` and changed their mind had no
    // gesture at all that put it away. This is that gesture, and it reports
    // the arm that was already waiting for it.
    //
    // Gated on `enabled` for the export card's reason, which is the same one:
    // a `bw send receive` in flight is a child writing into state that goes
    // away with the card.
    if theme::modal_corner_mark_gated(ui, card_rect, title, enabled) {
        action = RecordUiAction::Cancel;
    }
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
/// a full-window click-catching scrim on `theme::SCRIM_ORDER`, then a
/// centred card — because that is this window's established modal, and a
/// second one built differently is two modals that dim, layer and swallow
/// clicks two ways. Nothing about the form itself moves in here: every
/// decision it paints is still [`export_problem`]'s and [`warning_is_shown`]'s.
pub fn draw_export_modal(
    ctx: &egui::Context,
    state: &mut RecordSend,
    // The item this composer was opened against, re-found by the window on
    // every frame and borrowed for the paint -- see [`draw_export_form`].
    item: Option<&crate::vault_bridge::VaultItem>,
    folder: Option<&str>,
    in_flight: bool,
    now: &dyn crate::send::SendClock,
    zone: &dyn crate::local_time::LocalOffset,
) -> RecordUiAction {
    theme::modal_scrim(ctx, egui::Area::new(egui::Id::new("record-send-scrim")));

    theme::movable_modal(ctx, egui::Area::new(egui::Id::new("record-send-modal")))
        .show(ctx, |ui| {
            // Before the form, not after: a drag strip laid over the form's
            // controls would swallow their clicks. See
            // `theme::modal_drag_handle`.
            theme::modal_drag_handle(ui, FORM_HEADER_HEIGHT);
            ui.set_max_width(MODAL_WIDTH);
            draw_export_form(
                ui,
                &mut state.draft,
                &state.item_name,
                item,
                folder,
                in_flight,
                now,
                zone,
            )
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
    theme::modal_scrim(ctx, egui::Area::new(egui::Id::new("record-import-scrim")));

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
        let plan = send_plan_from(&record, SendPlan::default());

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
        let plan = send_plan_from(
            &Record {
                name: "x".to_string(),
                username: None,
                password: None,
                uri: None,
                notes: None,
                totp_sealed: None,
                not_after: None,
            },
            SendPlan::default(),
        );
        assert!(plan.hidden);
        assert!(read_json(&plan.text).is_ok(), "{}", plan.text.as_str());
    }

    /// **What the user chose in §5a's `ACCESS` block reaches the plan that is
    /// published.**
    ///
    /// This is the assertion the whole block turns on, and the one nothing
    /// else in this file makes: `send_plan_from` fills in the name, the body
    /// and `hidden`, and everything else it must carry through from the
    /// draft. A version that had kept `..SendPlan::default()` would draw the
    /// block, validate the block, and publish a seven-day Send with no
    /// password and no cap -- in silence, with the user's own choices visible
    /// on the card they pressed the button on.
    ///
    /// All three are set to something OTHER than the default, so a function
    /// that ignored its second argument entirely would fail on every one.
    #[test]
    fn the_access_block_reaches_the_plan_that_is_published() {
        let access = SendPlan {
            lifetime: crate::send::SendLifetime::Hours(1),
            password: Some(Zeroizing::new("share-pw-9271".to_string())),
            max_access_count: Some(3),
            ..SendPlan::default()
        };
        // Control: every one of the three really does differ from what a
        // default plan carries, so the assertions below cannot pass by
        // accident.
        let default = SendPlan::default();
        assert_ne!(access.lifetime, default.lifetime);
        assert!(default.password.is_none() && default.max_access_count.is_none());

        let plan = send_plan_from(&a_record(), access);
        assert_eq!(
            plan.lifetime,
            crate::send::SendLifetime::Hours(1),
            "the lifetime the user chose did not travel"
        );
        assert_eq!(
            plan.password.as_deref().map(String::as_str),
            Some("share-pw-9271"),
            "the share password did not travel, so the link opens for anyone who has it"
        );
        assert_eq!(plan.max_access_count, Some(3), "the view cap did not travel");

        // And the three the function itself owns are still its own, so
        // carrying the Access answers through did not hand a caller the
        // ability to publish an unhidden record.
        assert!(plan.hidden, "the record Send stopped being hidden");
        assert_eq!(plan.name, "SAP Production");
        assert!(read_json(&plan.text).is_ok(), "{}", plan.text.as_str());
    }

    /// **The export form's button answers to the Access block's refusals, in
    /// `crate::send`'s own words.**
    ///
    /// The danger this guards is the quiet one: a form that validated only
    /// its tick-boxes would call a draft submittable, close the modal, and
    /// hand `plan_to_invocation` a plan it refuses -- on a background thread,
    /// with nothing left on screen to say why nothing was published.
    #[test]
    fn the_export_form_refuses_what_the_encoder_would_refuse() {
        let good = RecordDraft::default();
        assert_eq!(export_problem(&good, &FixedClock(NOW)), None, "control: the opening draft is refused");

        for (why, access) in [
            (
                "a lifetime that is not one of the picker's cells",
                SendPlan {
                    lifetime: crate::send::SendLifetime::Hours(3),
                    ..SendPlan::default()
                },
            ),
            (
                "a password that is present and empty",
                SendPlan {
                    password: Some(Zeroizing::new(String::new())),
                    ..SendPlan::default()
                },
            ),
            (
                "a view cap of zero",
                SendPlan { max_access_count: Some(0), ..SendPlan::default() },
            ),
        ] {
            let draft = RecordDraft { access, ..RecordDraft::default() };
            let problem = export_problem(&draft, &FixedClock(NOW))
                .unwrap_or_else(|| panic!("{why} was accepted by the form"));
            assert!(
                !export_can_submit(Some(problem), false),
                "{why} left the publish button live"
            );
            // The same sentence the encoder would have answered with, which
            // is the whole point of delegating rather than restating.
            assert_eq!(
                Some(problem),
                crate::send::validate_access(
                    draft.access.lifetime,
                    draft.access.password.as_deref().map(String::as_str),
                    draft.access.max_access_count,
                    &FixedClock(NOW),
                ),
                "{why} is refused here in words `crate::send` does not use"
            );
        }

        // The tick-box rules still come first: a draft that is wrong in both
        // ways names the seed, which is the dangerous one.
        let both = RecordDraft {
            selection: RecordSelection { totp: true, ..RecordDraft::default().selection },
            access: SendPlan { max_access_count: Some(0), ..SendPlan::default() },
            ..RecordDraft::default()
        };
        assert_eq!(export_problem(&both, &FixedClock(NOW)), Some(NEEDS_PASSPHRASE));
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
            production.contains("pub fn send_plan_from(record: &Record, access: SendPlan) -> SendPlan"),
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
        assert_eq!(export_problem(&draft, &FixedClock(NOW)), None);
        assert!(export_can_submit(export_problem(&draft, &FixedClock(NOW)), false));
    }

    /// **The disabled button, as a pure function of the draft.**
    ///
    /// Ticking the seed greys the button until a passphrase is typed, and the
    /// control on the other side is that typing one un-greys it — without
    /// which "always disabled" would pass.
    #[test]
    fn ticking_the_seed_disables_the_button_until_a_passphrase_is_typed() {
        let mut draft = RecordDraft::default();
        assert!(export_can_submit(export_problem(&draft, &FixedClock(NOW)), false), "control: live before the tick");

        draft.set_totp(true);
        assert_eq!(export_problem(&draft, &FixedClock(NOW)), Some(NEEDS_PASSPHRASE));
        assert!(
            !export_can_submit(export_problem(&draft, &FixedClock(NOW)), false),
            "a seed can be published with nothing to seal it under"
        );

        // Whitespace is not a passphrase.
        draft.passphrase = Zeroizing::new("   ".to_string());
        assert_eq!(export_problem(&draft, &FixedClock(NOW)), Some(NEEDS_PASSPHRASE));

        draft.passphrase = Zeroizing::new("correct horse battery staple".to_string());
        assert_eq!(export_problem(&draft, &FixedClock(NOW)), None);
        assert!(export_can_submit(export_problem(&draft, &FixedClock(NOW)), false));

        // And in flight, nothing submits whatever the draft says.
        assert!(!export_can_submit(export_problem(&draft, &FixedClock(NOW)), true));
    }

    /// An empty draft is refused for its own reason, so the seed rule above is
    /// not the only thing `export_problem` can ever say.
    #[test]
    fn a_draft_with_nothing_ticked_says_so() {
        let draft = RecordDraft {
            selection: RecordSelection::default(),
            ..RecordDraft::default()
        };
        assert_eq!(export_problem(&draft, &FixedClock(NOW)), Some(NOTHING_TICKED));
        let ticked = RecordDraft { selection: RecordSelection { uri: true, ..Default::default() }, ..RecordDraft::default() };
        assert_eq!(export_problem(&ticked, &FixedClock(NOW)), None, "control: one tick is enough");
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
        assert_eq!(export_problem(&draft, &FixedClock(NOW)), Some(NEEDS_PASSPHRASE));
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

    /// The timezone every paint test below stands in: UTC, injected.
    ///
    /// The export form now prints the instant its link dies, in the user's
    /// own day -- so without a fixed offset here, a test that read a date off
    /// the painted glyphs would pass or fail depending on where the machine
    /// running `cargo test` happens to be. `send::expiry_wording`'s own tests
    /// make the same arrangement for the same reason.
    const UTC: crate::local_time::FixedOffset = crate::local_time::FixedOffset(0);

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

    /// **§5a's include list carries the values, and the password carries its
    /// warning.**
    ///
    /// The owner's verdict on the list of bare labels this replaces: "Current
    /// UI is very plain and not even close to this design". §5a's rows answer
    /// "what travels" -- the address, a mask, `3 apps` -- and a tick beside
    /// the word `Password` answers it only if you already know what is in the
    /// record.
    ///
    /// Asserted on the pure function rather than on a painted frame, because
    /// every string a row shows is decided there; the PAINTING of the
    /// password's red is the next test's business.
    #[test]
    fn every_include_row_says_what_would_travel() {
        let item = a_login();
        let rows = include_rows(&RecordDraft::default(), Some(&item));
        let by = |label: &str| {
            rows.iter()
                .find(|r| r.label == label)
                .unwrap_or_else(|| panic!("no {label:?} row: {:?}", rows.iter().map(|r| r.label).collect::<Vec<_>>()))
        };
        assert_eq!(by(USERNAME_LABEL).value, "a.novak@ledgerline.com");
        assert_eq!(by(URI_LABEL).value, "1 address", "the word has to agree with the number");
        assert_eq!(by(TOTP_LABEL).value, TOTP_ROW_NOTE);
        // §5a draws nothing beside Notes even on a record that has one.
        assert!(item.notes.is_some(), "control: the fixture has a note to have elided");
        assert!(by(NOTES_LABEL).value.is_empty());

        // **The mask does not track the password's length.** §5a draws twelve
        // bullets; `theme::MASKED_BULLETS`'s doc forbids a mask that counts,
        // because it tells a shoulder-surfer how many characters to expect.
        let password = by(PASSWORD_LABEL);
        assert!(password.secret, "the password row is not marked a secret");
        assert_eq!(password.value, theme::masked_readout());
        let real = item.login.as_ref().and_then(|l| l.password.as_ref()).expect("fixture");
        assert_ne!(
            password.value.chars().count(),
            real.chars().count(),
            "control: the mask is the same length as the password, so this fixture cannot \
             tell a counting mask from a constant one"
        );

        // A record with nothing in a field still gets the row: §5a draws five
        // whatever the record holds, and a list that grew and shrank would
        // make the counter beside its eyebrow a moving denominator.
        let bare = include_rows(&RecordDraft::default(), None);
        assert_eq!(bare.len(), rows.len(), "the list changed length with the record");
        assert!(
            bare.iter().all(|r| r.value.is_empty() || r.secret),
            "a row invented a value for a record that is not there: {:?}",
            bare.iter().map(|r| (r.label, r.value.clone())).collect::<Vec<_>>()
        );
    }

    /// **The password row is red in all three of §5a's ways**, and only when
    /// it is really going.
    ///
    /// The tint, the tick and the tag: the design makes one point in the only
    /// three ways a row has, and any one of them alone would pass against a
    /// row that had lost the other two.
    #[test]
    fn the_password_row_is_red_only_while_the_password_is_going() {
        let item = a_login();
        // **The password IS ticked by default** -- `RecordDraft::default`
        // says so and argues it: a user sending a login is sending a
        // password. So the quiet frame is one with it deliberately turned
        // OFF, and the assertion below is that the red goes with it.
        assert!(
            RecordDraft::default().selection.password,
            "control: this test has the default the wrong way round"
        );
        let quiet = paint(|ui| {
            let mut draft = RecordDraft::default();
            draft.selection.password = false;
            draw_export_form(
                ui,
                &mut draft,
                "SAP Production",
                Some(&item),
                Some("Engineering"),
                false,
                &FixedClock(NOW),
                &UTC,
            );
        });
        assert!(
            !quiet.has(VISIBLE_PILL),
            "the tag is on a row whose password is not going"
        );
        assert!(
            !quiet.has(PASSWORD_TRAVELS),
            "the caution band is up over a Send with no password in it"
        );

        let mut sending = RecordDraft::default();
        assert!(sending.selection.password, "control: the loud frame is not sending one");
        let loud = paint(|ui| {
            draw_export_form(
                ui,
                &mut sending,
                "SAP Production",
                Some(&item),
                Some("Engineering"),
                false,
                &FixedClock(NOW),
                &UTC,
            );
        });
        assert!(
            loud.has(VISIBLE_PILL),
            "the tag is missing from a password that is going: {:?}",
            loud.text
        );
        assert!(
            loud.has(PASSWORD_TRAVELS),
            "§5a's caution band is missing from a Send carrying a password"
        );
        // The tint, which no glyph reader can see: §5a's `#fdf3f2` is painted
        // on this card only by the password's row and by the caution band, and
        // one of the two is enough to prove the colour reached the frame --
        // so both are counted, and the quiet frame must have neither.
        let washes = |p: &Painted| {
            p.fills.iter().filter(|(_, fill)| *fill == theme::DANGER_WASH).count()
        };
        assert_eq!(washes(&quiet), 0, "the danger wash is on a card sending no password");
        assert!(
            washes(&loud) >= 2,
            "only {} danger-washed bands on a card sending a password; §5a has the row and \
             the caution strip",
            washes(&loud)
        );
    }

    /// The record these shots are about: a login with something in every
    /// field §5a's include list draws a value for.
    ///
    /// A real `VaultItem` and not a hand-made row list, because the rows are
    /// built by [`include_rows`] out of an item and a test that fed it
    /// something else would be asserting about a fixture rather than about
    /// the function.
    fn a_login() -> crate::vault_bridge::VaultItem {
        serde_json::from_str(
            r#"{"id":"send-1","type":1,"name":"SAP Production","fields":[],
                "notes":"Finance approves new seats on the first Monday.",
                "login":{"username":"a.novak@ledgerline.com",
                         "password":"correct-horse-battery-staple-7",
                         "totp":"JBSWY3DPEHPK3PXP",
                         "uris":[{"uri":"https://app.ledgerline.eu"}]}}"#,
        )
        .expect("the send fixture is valid item JSON")
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

    /// **The record composer is the same banded card the Sends composer is**,
    /// and this test exists because the two coming apart is the defect one
    /// level up from the one that was reported.
    ///
    /// The owner screenshotted `send_ui`'s text composer and said it was not
    /// as per design: no card, bare inputs, a footer of loose buttons and
    /// floating prose. Every one of those was equally true here -- the two
    /// forms had each written out `Frame::new().fill(CARD).corner_radius(8)`
    /// in their own file, so they agreed only by coincidence, and the
    /// coincidence was that neither was §5a. Fixing one of them and not the
    /// other would have produced two composers that publish the same object
    /// and look like two applications.
    ///
    /// Asserted on this card's own paint rather than by comparing two
    /// screens' pixels: the shared thing is `theme::form_card` and its three
    /// bands, so what each card has to show is that it went through them.
    #[test]
    fn the_export_card_is_the_banded_card_and_not_a_form_laid_flat() {
        let mut draft = RecordDraft::default();
        let painted = paint(|ui| {
            draw_export_form(
                ui,
                &mut draft,
                "SAP Production",
                Some(&a_login()),
                Some("Engineering"),
                false,
                &FixedClock(NOW),
                &UTC,
            );
        });

        let heading = painted.rect_of(EXPORT_HEADING).expect("the heading was not painted");
        let submit = painted.rect_of(EXPORT_SUBMIT_LABEL).expect("the submit was not painted");

        // The footer band: §5a's `#fbfaf9`, which is `theme::CARD_TINT`, and
        // nothing else on this card is painted in it.
        let band = painted
            .fills
            .iter()
            .filter(|(_, colour)| *colour == theme::CARD_TINT)
            .map(|(rect, _)| *rect)
            .max_by(|a, b| a.bottom().total_cmp(&b.bottom()))
            .expect("no tinted footer band was painted -- the answers are still the last \
                     things in the body");
        assert!(
            band.contains_rect(submit),
            "the submit at {submit:?} is outside the footer band at {band:?}"
        );
        assert!(
            !band.contains_rect(heading),
            "the heading is inside the footer band, so the 'band' found is the whole card"
        );

        // §5a's standing note, in the footer's right-hand slot, spelled once
        // for both composers.
        assert!(
            painted.has(crate::vault_window::send_ui::APPEARS_IN_SENDS),
            "the record composer's footer does not say where the Send ends up, and the Sends \
             composer's does -- one fact, two cards, two answers: {:?}",
            painted.text
        );

        // The header band's closing rule. `theme::hairline` is the one weight
        // §5a draws under a card's title, and a card with no rule under its
        // heading is a heading with a gap under it.
        //
        // Bounded by the heading above and the body's first line below rather
        // than by an arithmetic guess at the band's height: the heading's rect
        // is its GLYPH ink, which stops short of the line box, so "one padding
        // below the heading" is not where the rule is.
        let first_block =
            painted.rect_of(RECORD_EYEBROW).expect("the RECORD eyebrow was not painted");
        let rule = painted.fills.iter().any(|(rect, colour)| {
            *colour == theme::HAIRLINE
                && (rect.height() - 1.0).abs() < 0.5
                && rect.top() > heading.bottom()
                && rect.bottom() < first_block.top()
        });
        assert!(rule, "no hairline closes the header band off from the body");
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
            draw_export_form(
                ui,
                &mut draft,
                "SAP Production",
                Some(&a_login()),
                Some("Engineering"),
                false,
                &FixedClock(NOW),
                &UTC,
            );
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
            draw_export_form(
                ui,
                &mut ticked,
                "SAP Production",
                Some(&a_login()),
                Some("Engineering"),
                false,
                &FixedClock(NOW),
                &UTC,
            );
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
            export_problem(&draft, &FixedClock(NOW)).is_none(),
            "the fixture wants a submittable draft, or the fill below is a disabled one"
        );
        let painted = paint(|ui| {
            draw_export_form(
                ui,
                &mut draft,
                "SAP Production",
                Some(&a_login()),
                Some("Engineering"),
                false,
                &FixedClock(NOW),
                &UTC,
            );
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
            draw_export_form(
                ui,
                &mut draft,
                "SAP Production",
                Some(&a_login()),
                Some("Engineering"),
                false,
                &FixedClock(NOW),
                &UTC,
            );
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
            draw_export_form(
                ui,
                &mut draft,
                "SAP Production",
                Some(&a_login()),
                Some("Engineering"),
                false,
                &FixedClock(NOW),
                &UTC,
            );
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

    /// **§5a's `ACCESS` block is on this card: the eyebrow, three row labels,
    /// four lifetime cells, and the date the link dies.**
    ///
    /// It is worth a paint test and not only the pure ones because every
    /// other assertion about this block is about a value in a struct. A
    /// `draw_access_block` that had been left out of `draw_export_form`
    /// entirely would leave `export_problem`, `view_limit_from` and
    /// `send_plan_from` all green and all pointless: the user would have no
    /// way to set any of the three.
    #[test]
    fn the_access_block_is_on_the_record_composer() {
        let mut draft = RecordDraft::default();
        let painted = paint(|ui| {
            draw_export_form(
                ui,
                &mut draft,
                "SAP Production",
                Some(&a_login()),
                Some("Engineering"),
                false,
                &FixedClock(NOW),
                &UTC,
            );
        });

        assert!(
            painted.has(super::super::send_ui::ACCESS_EYEBROW),
            "§5a's ACCESS eyebrow is not on the card: {:?}",
            painted.text
        );
        for label in [
            super::super::send_ui::EXPIRES_LABEL,
            super::super::send_ui::VIEWS_LABEL,
            super::super::send_ui::OPEN_WITH_LABEL,
        ] {
            assert!(painted.has(label), "the {label:?} row is missing: {:?}", painted.text);
        }

        // The lifetime control, shut, saying the answer in force. It is a
        // dropdown rather than a run of cells -- see
        // `send_ui::draw_access_block` for the measurement that forced the
        // change -- so what the card carries is one label and not nine.
        assert!(
            painted.has(&crate::send::lifetime_label(
                crate::send::DEFAULT_LIFETIME,
                &UTC
            )),
            "the lifetime control does not say the answer in force: {:?}",
            painted.text
        );

        // The sentence under the control, naming the day this link dies -- at
        // the UTC offset these tests stand in, and from `send.rs`'s own
        // `expiry_wording` rather than from a second formatter here.
        assert!(
            painted.has(&crate::send::expiry_wording(
                crate::send::DEFAULT_LIFETIME,
                &FixedClock(NOW),
                &UTC,
            )),
            "the composer does not say when the link stops working: {:?}",
            painted.text
        );
        assert!(
            painted.has("17 Aug 2026"),
            "control: that sentence names some other day than the one seven days from `NOW`, \
             so the assertion above is comparing a formatter with itself: {:?}",
            painted.text
        );

        // Neither optional control is switched on, and the notes beside them
        // say so rather than leaving two blank boxes.
        assert!(painted.has(super::super::send_ui::NO_VIEW_LIMIT_NOTE), "{:?}", painted.text);
        assert_eq!(draft.access.password, None, "a frame put a password on the Send");
        assert_eq!(draft.access.max_access_count, None, "a frame capped the Send");

        // And §5a's three unbuildable rows are not on the card under any
        // label. A control that cannot do what it says is the one thing this
        // block must not grow, and the three are named here so that adding
        // one reds a test rather than shipping.
        for absent in [
            "Recipient",
            "Only this address can open it",
            "Tell me when it is opened",
        ] {
            assert!(
                !painted.has(absent),
                "{absent:?} is drawn on the composer, and there is no server behind it -- see \
                 `send_ui::draw_access_block` on why all three are absences and not to-dos"
            );
        }
    }

    /// **§5a's Access block fits the modal it is drawn in.**
    ///
    /// The block's widest row is the lifetime control sitting to the right of
    /// a 96-point label column, inside a card that is [`MODAL_WIDTH`] wide
    /// with a 12-point margin each side. That leaves **226 points**, and it is
    /// a real constraint that is not obvious by inspection.
    ///
    /// **This test is why the lifetime row is a dropdown.** It used to loop
    /// over a segmented run's cells, because a run sizes each cell to its own
    /// label plus `theme::SEGMENT_PADDING` -- so the run grew when the picker
    /// went from three cells to four, and when the set reached nine there was
    /// no setting of any constant that fitted it in 226 points.
    /// `send_ui::EXPIRY_FIELD_WIDTH` is a fixed box that cannot grow at all,
    /// and this measures the box rather than a label inside it, so the claim
    /// survives a reworded answer.
    ///
    /// **This is the measurement that has cost this project rework before.**
    /// The design page is content-box, so §5a's `width: 690px` column and its
    /// `height: 30px` boxes are all inside their borders; reading any of them
    /// as a border-box number puts a control two points wider than it is,
    /// which is invisible in a screenshot until the row it is in overflows.
    /// The other paint tests here run in a 640-point pane, where nothing
    /// overflows and nothing is learned.
    #[test]
    fn the_access_block_fits_inside_the_modal_it_is_drawn_in() {
        let mut draft = RecordDraft::default();
        let painted = paint(|ui| {
            // The modal's own constraint, applied the way `draw_export_modal`
            // applies it, so this measures the card the user actually sees.
            ui.set_max_width(MODAL_WIDTH);
            draw_export_form(
                ui,
                &mut draft,
                "SAP Production",
                Some(&a_login()),
                Some("Engineering"),
                false,
                &FixedClock(NOW),
                &UTC,
            );
        });

        // The card's own left edge, taken from a control that is definitely
        // in it, so the budget below is measured from the drawing and not
        // from an assumption about where the pane starts.
        let eyebrow = painted
            .rect_of(super::super::send_ui::ACCESS_EYEBROW)
            .expect("control: the ACCESS eyebrow did not paint");
        let right_edge = eyebrow.left() + MODAL_WIDTH;

        // The lifetime control, measured as a BOX and not as a label. The
        // label is found first because that is the only thing the paint
        // harness can see, and the box is then reconstructed round it: the
        // text starts one `DROPDOWN_PAD_X` inside the left edge, and the box
        // runs `EXPIRY_FIELD_WIDTH` from there. Measuring the label alone
        // would pass for a box twice as wide as the card.
        let answer = crate::send::lifetime_label(crate::send::DEFAULT_LIFETIME, &UTC);
        let label = painted
            .rect_of(&answer)
            .unwrap_or_else(|| panic!("the {answer:?} answer did not paint"));
        let box_right =
            label.left() - theme::DROPDOWN_PAD_X + super::super::send_ui::EXPIRY_FIELD_WIDTH;
        assert!(
            box_right <= right_edge,
            "the lifetime control runs {}pt past the right edge of a {MODAL_WIDTH}pt modal. \
             §5a's Access rows are a 96-point label column plus a control, which leaves 226 \
             points -- and a control wider than that overflows the card before it overflows \
             anything a screenshot shows",
            box_right - right_edge
        );

        // The notes beside the two optional controls are the other things on
        // these rows that can push right, and they are sentences rather than
        // labels.
        for note in [
            super::super::send_ui::NO_VIEW_LIMIT_NOTE,
            super::super::send_ui::OPEN_WITH_LABEL,
        ] {
            let rect =
                painted.rect_of(note).unwrap_or_else(|| panic!("{note:?} did not paint"));
            assert!(
                rect.right() <= right_edge,
                "{note:?} runs {}pt past the modal's right edge",
                rect.right() - right_edge
            );
        }
    }

    /// **The import footer is a primary beside a secondary, and the CREATE is
    /// the primary.**
    ///
    /// The twin of `the_export_footer_wears_the_design_systems_two_buttons`,
    /// and it exists because the two cards were left in different states: the
    /// export footer was moved onto the design system a pass ago and this one
    /// was not, so two modals opened from one window looked like they came
    /// from two applications. The fill is also the whole of the fix -- both
    /// buttons here used to be bare `egui::Button`s differing only in the
    /// colour of their text, which no test could see.
    #[test]
    fn the_import_footer_wears_the_design_systems_two_buttons() {
        // A fetched record with no collision and no seal, so the create is
        // LIVE: a disabled primary is faded toward the window colour and this
        // test would then be asserting the fade.
        let record = Record {
            name: "SAP Production".to_string(),
            username: Some("dplatonov".to_string()),
            password: None,
            uri: None,
            notes: None,
            totp_sealed: None,
            not_after: None,
        };
        let mut draft =
            ImportDraft { link: "https://send.example/#/x".to_string(), ..Default::default() };
        let fetched = Ok(record);
        assert!(
            import_can_proceed(fetched.as_ref().ok(), &draft, &Collision::Fresh, false),
            "the fixture wants a proceedable draft, or the fill below is a disabled one"
        );
        let painted = paint(|ui| {
            draw_import_form(
                ui,
                &mut draft,
                Some(&fetched),
                &Collision::Fresh,
                false,
                &FixedClock(NOW),
            );
        });

        let (submit, submit_fill) = painted.button_under(IMPORT_SUBMIT_LABEL);
        assert_eq!(
            submit_fill,
            theme::BLUE,
            "the import's create is not the design system's filled primary. It is the answer \
             this card is FOR, and it used to be the second of two identical boxes while \
             `Fetch` -- a step on the way -- was drawn first"
        );
        assert_eq!(submit.height(), theme::BUTTON_HEIGHT);

        let (fetch, fetch_fill) = painted.button_under(FETCH_LABEL);
        assert_eq!(
            fetch_fill,
            theme::CARD,
            "`Fetch` is not the design system's outlined secondary"
        );
        assert_eq!(fetch.height(), theme::BUTTON_HEIGHT, "the two answers are different heights");
        assert_ne!(
            submit_fill, fetch_fill,
            "the import footer's two answers paint the same fill, which is the exact defect \
             this pass fixed on the export footer"
        );
        // `Fetch` comes first, because that is the order the steps happen in.
        assert!(
            fetch.left() < submit.left(),
            "the create is drawn before the fetch, so the card reads as something you could \
             submit before fetching anything"
        );
    }

    /// **The collision prompt is one segmented run, and the answer in force
    /// is filled blue.**
    ///
    /// This is the one question in this feature whose wrong answer destroys
    /// an item the user already had, and it was drawn as two `.selected()`
    /// buttons -- egui's own grey selection fill over two identical
    /// 160-point boxes. The three facts below are the ones that tell a
    /// segmented run from a pair of buttons, and all three are invisible to a
    /// test that only reads glyphs.
    #[test]
    fn the_collision_prompt_is_one_segmented_run_and_not_two_buttons() {
        let record = Record {
            name: "SAP Production".to_string(),
            username: Some("dplatonov".to_string()),
            password: None,
            uri: None,
            notes: None,
            totp_sealed: None,
            not_after: None,
        };
        let fetched = Ok(record);
        let collision = Collision::SameName { existing_id: "item-1".to_string() };

        // Nothing chosen: neither cell may be lit.
        let mut untouched =
            ImportDraft { link: "https://send.example/#/x".to_string(), ..Default::default() };
        let before = paint(|ui| {
            draw_import_form(ui, &mut untouched, Some(&fetched), &collision, false, &FixedClock(NOW));
        });
        for label in [CREATE_SECOND_LABEL, REPLACE_LABEL] {
            let (cell, fill) = before.button_under(label);
            assert_eq!(
                cell.height(),
                theme::SEGMENT_HEIGHT,
                "{label:?} is not a segmented-control cell -- it is {}pt tall against the \
                 run's {}",
                cell.height(),
                theme::SEGMENT_HEIGHT
            );
            assert_ne!(
                fill,
                theme::BLUE,
                "{label:?} is lit with nothing chosen. `draft.choice` starts `None` and no \
                 frame may turn that into a default: replacing is the one press in this \
                 feature that destroys data the user already had"
            );
        }
        let (first, _) = before.button_under(CREATE_SECOND_LABEL);
        let (second, _) = before.button_under(REPLACE_LABEL);
        let gap = second.left() - first.right();
        assert!(
            gap.abs() <= theme::SEGMENT_SEAM + 0.5,
            "the two offers are {gap}pt apart, so they are two buttons in a row rather than \
             one control with two positions"
        );

        // Replace chosen: that cell, and only that cell, is filled blue.
        let mut replacing = ImportDraft {
            link: "https://send.example/#/x".to_string(),
            choice: Some(CollisionChoice::Replace),
            ..Default::default()
        };
        let after = paint(|ui| {
            draw_import_form(ui, &mut replacing, Some(&fetched), &collision, false, &FixedClock(NOW));
        });
        assert_eq!(
            after.button_under(REPLACE_LABEL).1,
            theme::BLUE,
            "the chosen answer is not filled in the design's blue, so the press that replaces \
             an item is as quiet on screen as the one that does not"
        );
        assert_ne!(after.button_under(CREATE_SECOND_LABEL).1, theme::BLUE);
    }

    // -----------------------------------------------------------------------
    // The dismiss ✕
    //
    // Driven through `draw_export_modal` / `draw_import_modal` and NOT through
    // the two `draw_*_form` functions everything above uses, and that is the
    // point: the mark sits under the modal's drag strip, which only exists in
    // the modal wrapper. A test that pressed the mark on a bare form would
    // pass against precisely the arrangement this app has shipped broken
    // before -- a drag-sensing strip laid over a click-sensing mark, which
    // egui resolves by swallowing the click.
    // -----------------------------------------------------------------------

    fn modal_input(events: &[egui::Event]) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 900.0),
            )),
            events: events.to_vec(),
            ..Default::default()
        }
    }

    fn modal_context() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(modal_input(&[]), |_ui| {});
        crate::theme::apply(&ctx);
        let _ = ctx.run_ui(modal_input(&[]), |_ui| {});
        ctx
    }

    fn modal_click(at: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    /// Every line segment a frame laid down.
    fn segments(output: &egui::FullOutput) -> Vec<[egui::Pos2; 2]> {
        fn walk(shape: &egui::Shape, out: &mut Vec<[egui::Pos2; 2]>) {
            match shape {
                egui::Shape::LineSegment { points, .. } => out.push(*points),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    /// Where the dismiss ✕ crosses on a card, found from the paint alone.
    ///
    /// The mark draws no galley, so it is two diagonals exactly
    /// [`theme::CLOSE_MARK_SPAN`] across -- and nothing else on either of
    /// these forms strokes a diagonal at all.
    fn dismiss_mark(output: &egui::FullOutput) -> egui::Pos2 {
        let arms: Vec<[egui::Pos2; 2]> = segments(output)
            .into_iter()
            .filter(|[a, b]| {
                ((b.x - a.x).abs() - theme::CLOSE_MARK_SPAN).abs() < 0.01
                    && ((b.y - a.y).abs() - theme::CLOSE_MARK_SPAN).abs() < 0.01
            })
            .collect();
        assert_eq!(
            arms.len(),
            2,
            "the card strokes {} arms the size of a ✕; it needs exactly two, and none is a \
             modal with no mark in its corner",
            arms.len()
        );
        let centre = arms[0][0].lerp(arms[0][1], 0.5);
        assert!(
            (arms[1][0].lerp(arms[1][1], 0.5) - centre).length() < 0.01,
            "the two arms do not cross, so this is not a ✕"
        );
        centre
    }

    /// The mark is [`theme::MODAL_CLOSE_INSET`] off the card's own right-hand
    /// edge, where the card's rect is the one egui measured for the modal's
    /// `Area` -- not a number re-derived here from the form's padding.
    fn assert_shared_inset(ctx: &egui::Context, area: &str, at: egui::Pos2) {
        let card = egui::AreaState::load(ctx, egui::Id::new(area))
            .expect("the modal has never been drawn")
            .rect();
        let box_right = at.x + theme::CLOSE_MARK_HIT / 2.0;
        assert!(
            (card.right() - box_right - theme::MODAL_CLOSE_INSET).abs() < 0.5,
            "on {area} the mark's hit box ends {} points inside the card, not \
             `theme::MODAL_CLOSE_INSET`'s {}",
            card.right() - box_right,
            theme::MODAL_CLOSE_INSET
        );
    }

    /// **The composer's ✕ is drawn, and pressing it cancels -- the same answer
    /// its own Cancel button gives.**
    ///
    /// **Neither of these two modals binds Escape**, here or in
    /// `vault_window::mod`, so there is no keyboard gesture for the mark to
    /// have to match; what it matches is [`EXPORT_CANCEL_LABEL`], which is the
    /// card's existing way out. One is deliberately not invented in passing.
    #[test]
    fn the_composers_dismiss_mark_is_drawn_and_cancels() {
        let ctx = modal_context();
        let mut state = RecordSend::opening("itm-1", "SAP Production");
        let run = |events: &[egui::Event], state: &mut RecordSend| {
            let mut action = RecordUiAction::None;
            let output = ctx.run_ui(modal_input(events), |ui| {
                action = draw_export_modal(
                    ui.ctx(),
                    state,
                    Some(&a_login()),
                    Some("Engineering"),
                    false,
                    &FixedClock(NOW),
                    &UTC,
                );
            });
            (action, output)
        };

        // One sizing pass, then a live one -- an anchored `Area` paints
        // nothing until egui has measured it.
        let _ = run(&[], &mut state);
        let (idle, drawn) = run(&[], &mut state);
        assert_eq!(idle, RecordUiAction::None, "the card reported a press with no input");
        let at = dismiss_mark(&drawn);
        assert_shared_inset(&ctx, "record-send-modal", at);

        let (action, _) = run(&modal_click(at), &mut state);
        assert_eq!(
            action,
            RecordUiAction::Cancel,
            "the composer's ✕ at {at:?} reported nothing -- either the card's drag strip \
             swallowed the click, or the mark is drawn and dead"
        );
    }

    /// **A composer with a `bw send create` in flight does not answer its
    /// ✕**, for the same reason it greys its Cancel: walking away from the
    /// card leaves a child writing into state nobody is watching. A mark that
    /// stayed live beside a greyed Cancel would be a second exit from a state
    /// the card's own exit refuses -- the exact hazard this pass exists to
    /// remove rather than to add.
    #[test]
    fn the_composers_dismiss_mark_is_dead_while_the_send_is_in_flight() {
        let ctx = modal_context();
        let mut state = RecordSend::opening("itm-1", "SAP Production");
        let run = |events: &[egui::Event], in_flight: bool, state: &mut RecordSend| {
            let mut action = RecordUiAction::None;
            let output = ctx.run_ui(modal_input(events), |ui| {
                action = draw_export_modal(
                    ui.ctx(),
                    state,
                    Some(&a_login()),
                    Some("Engineering"),
                    in_flight,
                    &FixedClock(NOW),
                    &UTC,
                );
            });
            (action, output)
        };

        let _ = run(&[], true, &mut state);
        let (_, drawn) = run(&[], true, &mut state);
        // It is still PAINTED -- a control that vanished for the duration
        // would read as the card changing shape.
        let at = dismiss_mark(&drawn);

        let (action, _) = run(&modal_click(at), true, &mut state);
        assert_eq!(
            action,
            RecordUiAction::None,
            "the ✕ closed a composer with a send in flight, which its own Cancel button \
             refuses to do"
        );
    }

    /// **The import card's ✕ is drawn, and pressing it cancels.**
    ///
    /// On this card the mark is not a consistency but a FIX. The import form
    /// has no Cancel button -- its footer holds `Fetch` and the create -- and
    /// nothing binds Escape for it, so before this mark existed a user who
    /// opened `Import from a Send...` and changed their mind had no gesture at
    /// all that put the card away. `vault_window::mod` has always had the
    /// `RecordUiAction::Cancel` arm that closes it; nothing ever produced one.
    #[test]
    fn the_import_cards_dismiss_mark_is_drawn_and_cancels() {
        let ctx = modal_context();
        let mut state = RecordImport::opening();
        let run = |events: &[egui::Event], state: &mut RecordImport| {
            let mut action = RecordUiAction::None;
            let output = ctx.run_ui(modal_input(events), |ui| {
                action = draw_import_modal(
                    ui.ctx(),
                    state,
                    &Collision::Fresh,
                    &FixedClock(NOW),
                );
            });
            (action, output)
        };

        let _ = run(&[], &mut state);
        let (idle, drawn) = run(&[], &mut state);
        assert_eq!(idle, RecordUiAction::None, "the card reported a press with no input");
        let at = dismiss_mark(&drawn);
        assert_shared_inset(&ctx, "record-import-modal", at);

        let (action, _) = run(&modal_click(at), &mut state);
        assert_eq!(
            action,
            RecordUiAction::Cancel,
            "the import card's ✕ at {at:?} reported nothing -- and it is this card's only way \
             out, so a dead mark here is a modal a user cannot leave"
        );
    }
}

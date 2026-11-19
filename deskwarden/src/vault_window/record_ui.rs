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
    /// the form validated and the value that gets published come apart: the
    /// form says "7 days" and the JSON says thirty, and no test that checks
    /// the form or the JSON alone sees it. Holding the plan's own type means
    /// the Access block writes the very fields `send_plan_from` moves into
    /// the plan that is published.
    ///
    /// The name and body are not filled in until [`send_plan_from`]: they are
    /// the RECORD, which is re-resolved by id at submit time precisely so
    /// this form never holds a record's values between frames. What is here
    /// between frames is three choices the user made about the link, none of
    /// which is a secret about the vault -- except the share password, which
    /// is a `Zeroizing` inside the plan and dies with the draft.
    pub access: SendPlan,
    /// Who the Send is for, as typed or picked -- filed in `sent_history`
    /// under the Send it makes. Not secret; see that module.
    pub recipient: String,
    /// A suggestion whose forget mark was pressed this frame, for the window
    /// to confirm and act on. Taken by the window.
    pub forget: Option<String>,
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
            recipient: String::new(),
            forget: None,
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

/// **What a whole-record Send carries**: username, password, websites and
/// notes -- and not the one-time code's seed. See [`WHOLE_RECORD_LEAD`].
///
/// The seed is left out because sending one is cloning the authenticator,
/// which the card warns about and seals under a passphrase the sender types
/// ([`SEED_WARNING`]); a default that did that without asking would be the
/// one thing the whole-record default must not do. It travels only with
/// advanced sharing, where it is a tick of its own.
pub fn whole_record_selection() -> RecordSelection {
    RecordSelection { username: true, password: true, uri: true, notes: true, totp: false }
}

/// The lead-in on the whole-record line, before its pills.
pub const WHOLE_RECORD_LEAD: &str = "The whole record";

/// The pill for a record's websites on the whole-record line. The tick list
/// calls the same field [`URI_LABEL`]; a pill is a noun for what travels.
pub const WEBSITES_LABEL: &str = "Websites";

/// Said under the whole-record line when the record has a seed that is not
/// going with it.
pub const SEED_STAYS_NOTE: &str = "The one-time code's seed is not included. Turn on Use \
    advanced sharing in Preferences to send it, sealed.";

/// **The whole-record line's pills: what this item actually has**, in the
/// order the tick list draws them, with the password marked secret. An item
/// without a password shows no password pill, because none travels.
pub fn whole_record_fields(
    item: Option<&crate::vault_bridge::VaultItem>,
) -> Vec<(&'static str, bool)> {
    let Some(item) = item else {
        return Vec::new();
    };
    let login = item.login.as_ref();
    let filled = |value: Option<&str>| value.is_some_and(|v| !v.trim().is_empty());
    let mut fields = Vec::new();
    if filled(login.and_then(|l| l.username.as_deref())) {
        fields.push((USERNAME_LABEL, false));
    }
    if filled(login.and_then(|l| l.password.as_ref().map(|p| p.as_str()))) {
        fields.push((PASSWORD_LABEL, true));
    }
    if login.is_some_and(|l| l.uris.iter().any(|u| filled(u.uri.as_deref()))) {
        fields.push((WEBSITES_LABEL, false));
    }
    if filled(item.notes.as_ref().map(|n| n.as_str())) {
        fields.push((NOTES_LABEL, false));
    }
    fields
}

/// **The names of the fields that go with this Send**, for
/// `sent_history`: what the selection asks for AND the item has, in the
/// whole-record line's order, plus the seed when it travels sealed. Labels
/// only -- never a value.
pub fn fields_travelling(
    selection: &RecordSelection,
    item: &crate::vault_bridge::VaultItem,
    seed_sealed: bool,
) -> Vec<String> {
    let mut labels: Vec<String> = whole_record_fields(Some(item))
        .into_iter()
        .filter(|(label, _)| match *label {
            USERNAME_LABEL => selection.username,
            PASSWORD_LABEL => selection.password,
            WEBSITES_LABEL => selection.uri,
            NOTES_LABEL => selection.notes,
            _ => false,
        })
        .map(|(label, _)| label.to_string())
        .collect();
    if seed_sealed {
        labels.push(TOTP_LABEL.to_string());
    }
    labels
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
    // has for them**, and delegated rather than restated: the sentence under
    // this form's button and the refusal inside `plan_to_invocation` must be
    // the same function, or this form can call a draft acceptable that the
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
/// A function of two facts rather than a condition inside a widget:
/// `in_flight` is the lock against a second `bw send create`, and a lock
/// written inside an `eframe` closure is a lock no test can run.
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
    /// The done state's "See in Shared": the Sends screen.
    SeeInShared,
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
///
/// **`pub(crate)` for the Share dialog** (design 6a), whose `RECORD` block is
/// this chip exactly -- 6a's `.rchip` is §5a's -- and which calls this rather
/// than a copy of it, so the two modals name their record one way.
pub(crate) fn record_chip(ui: &mut egui::Ui, item_name: &str, kind_and_folder: &str) {
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
///
/// `pub(crate)` beside [`record_chip`], for the Share dialog.
pub(crate) fn chip_subtitle(item: Option<&crate::vault_bridge::VaultItem>, folder: Option<&str>) -> String {
    let Some(item) = item else {
        return String::new();
    };
    let kind = crate::vault_bridge::ItemKind::of(item).label();
    match folder {
        Some(name) => format!("{kind} \u{b7} {name}"),
        None => kind.to_string(),
    }
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
    pub item_id: String,
    pub item_name: String,
    pub draft: RecordDraft,
    /// Create link was pressed and the Send is being made: the modal stays
    /// up, its button says so, until the window hears back.
    pub waiting: bool,
    /// The made Send's link: the modal shows "Link ready" (spec B §6.2).
    pub done: Option<zeroize::Zeroizing<String>>,
    /// "More options" (0.15's access block and recipient row) is open.
    pub more: bool,
    /// The done state's summary line, taken when Create link is pressed
    /// (the draft's access answers are handed over to the Send then).
    pub summary: String,
}

impl RecordSend {
    /// Spec B §6.1's defaults: the link works for a day and opens once.
    pub fn opening(item_id: &str, item_name: &str) -> Self {
        let mut draft = RecordDraft { open: true, ..RecordDraft::default() };
        draft.access.lifetime = crate::send::SendLifetime::Hours(24);
        draft.access.max_access_count = Some(1);
        Self {
            item_id: item_id.to_string(),
            item_name: item_name.to_string(),
            draft,
            waiting: false,
            done: None,
            more: false,
            summary: String::new(),
        }
    }
}

/// The scrim's area name (listed in `item_list::MODAL_SCRIM_AREAS`).
pub const ID: &str = "record-send";
pub const SHARE_SUB: &str = "Send a secure link. Revoke it any time.";
pub const WHAT_TO_SHARE: &str = "What to share";
pub const WHAT_TO_SHARE_TIP: &str =
    "Only what you pick travels. The person never gets access to the rest of your vault.";
pub const LINK_WORKS_FOR: &str = "Link works for";
pub const OPEN_ONCE: &str = "Open only once";
pub const OPEN_ONCE_TIP: &str = "The link stops working right after it's opened, even if it hasn't expired.";
pub const CREATE_LINK: &str = "Create link";
pub const CREATING_LINK: &str = "Creating link\u{2026}";
pub const NOTHING_PICKED: &str = "Pick at least one thing to share";
pub const MORE_OPTIONS: &str = "More options";
pub const LINK_READY: &str = "Link ready";
pub const LINK_COPIED: &str = "Link copied";
pub const COPY_LINK: &str = "Copy";
pub const DONE: &str = "Done";
pub const SEE_IN_SHARED: &str = "See in Shared";
const SHARE_WIDTH: f32 = 560.0;

/// Spec B §6.1's expiry choices.
pub const LINK_LIFETIMES: [(&str, u32); 4] = [("1 hour", 1), ("1 day", 24), ("7 days", 168), ("30 days", 720)];

/// Which of [`LINK_LIFETIMES`] `lifetime` is, if any.
pub fn lifetime_choice(lifetime: &crate::send::SendLifetime) -> Option<usize> {
    match lifetime {
        crate::send::SendLifetime::Hours(h) => LINK_LIFETIMES.iter().position(|(_, hours)| hours == h),
        _ => None,
    }
}

/// The chips, in order: label, which field, whether it is the password.
pub fn share_chips() -> [(&'static str, IncludeField); 5] {
    [
        (USERNAME_LABEL, IncludeField::Username),
        (PASSWORD_LABEL, IncludeField::Password),
        ("2FA code", IncludeField::Totp),
        (WEBSITES_LABEL, IncludeField::Uri),
        (NOTES_LABEL, IncludeField::Notes),
    ]
}

fn chip_on(selection: &RecordSelection, field: IncludeField) -> bool {
    match field {
        IncludeField::Username => selection.username,
        IncludeField::Password => selection.password,
        IncludeField::Totp => selection.totp,
        IncludeField::Uri => selection.uri,
        IncludeField::Notes => selection.notes,
    }
}

/// Spec B §6.2's summary: what was picked, how long, and "opens once".
pub fn done_summary(draft: &RecordDraft, advanced: bool) -> String {
    let picked: Vec<&str> = if advanced {
        share_chips().iter().filter(|(_, f)| chip_on(&draft.selection, *f)).map(|(l, _)| *l).collect()
    } else {
        vec![WHOLE_RECORD_LEAD]
    };
    let mut line = picked.join(", ");
    if let Some(i) = lifetime_choice(&draft.access.lifetime) {
        line.push_str(" \u{b7} ");
        line.push_str(LINK_LIFETIMES[i].0);
    }
    if draft.access.max_access_count == Some(1) {
        line.push_str(" \u{b7} opens once");
    }
    line
}

/// One chip id, for tests.
pub fn chip_id(field: IncludeField) -> egui::Id {
    egui::Id::new(("share-chip", field as u8))
}

pub fn create_link_id() -> egui::Id {
    egui::Id::new("share-create-link")
}

pub fn copy_link_id() -> egui::Id {
    egui::Id::new("share-copy-link")
}

/// **Design §5a's own 690**, measured off the design rather than described from
/// it.
///
/// The number that stood here was 360, under a doc saying "§5a's composer is a
/// narrow column". It is not one. §5a is a 690-point figure and the card fills
/// it, and 330 points of missing width is most of what the owner was looking
/// at when they said the built card was not the design: every §5a include row
/// is `[tick] [name -------- flex] [value]`, and at 360 there is no value
/// column left for the address, the masked password or `3 apps` to sit in --
/// so they crowd the name instead of aligning down a right edge. The Access
/// rows lose the same way: a 96-point label column and a 14-point gap out of
/// 336 of content leaves the control less than two thirds of the row.
///
/// It fits the smallest window this app opens. `settings::MIN_VAULT_WINDOW_SIZE`
/// is 900 x 600 and this card is centred in the WINDOW, not in the detail
/// column, so 690 leaves 105 points either side.
///
/// **This does not reopen the Expires dropdown**, which
/// `send_ui::draw_access_block` argues at length: that decision is about NINE
/// choices not fitting a segmented run, and nine cells sized to their own
/// labels do not fit 544 points any more than they fit 226.
const MODAL_WIDTH: f32 = 690.0;


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
    // every frame and borrowed for the paint.
    item: Option<&crate::vault_bridge::VaultItem>,
    folder: Option<&str>,
    advanced: bool,
    history: &crate::sent_history::SentHistory,
    in_flight: bool,
    now: &dyn crate::send::SendClock,
    zone: &dyn crate::local_time::LocalOffset,
) -> RecordUiAction {
    let _ = folder;
    let title = format!("Share {}", state.item_name);
    let busy = in_flight || state.waiting;
    let mut dialog = crate::kit::modal::Dialog::new(ID, SHARE_WIDTH);
    if state.done.is_none() {
        dialog = dialog.title(&title).sub(SHARE_SUB);
    }
    let (outcome, action) = dialog.show(ctx, |ui| match state.done.as_ref() {
        Some(link) => draw_share_done(ui, &state.summary, link),
        None => draw_share_form(ui, state, item, advanced, history, busy, now, zone),
    });
    match (action, outcome) {
        (RecordUiAction::None, crate::kit::modal::ModalOutcome::Closed) if !busy => RecordUiAction::Cancel,
        (action, _) => action,
    }
}

/// Spec B §6.1.
#[allow(clippy::too_many_arguments)]
pub fn draw_share_form(
    ui: &mut egui::Ui,
    state: &mut RecordSend,
    item: Option<&crate::vault_bridge::VaultItem>,
    advanced: bool,
    history: &crate::sent_history::SentHistory,
    busy: bool,
    now: &dyn crate::send::SendClock,
    zone: &dyn crate::local_time::LocalOffset,
) -> RecordUiAction {
    let mut action = RecordUiAction::None;
    let draft = &mut state.draft;
    if !advanced {
        draft.selection = whole_record_selection();
        draft.set_totp(false);
    }
    let label = |ui: &mut egui::Ui, text: &str, tip: Option<&str>| {
        ui.horizontal(|ui| {
            ui.label(theme::bold(text, 15.0).color(theme::INK));
            if let Some(tip) = tip {
                ui.label(egui::RichText::new("?").color(theme::TEXT_FAINT)).on_hover_text(tip);
            }
        });
    };

    label(ui, WHAT_TO_SHARE, Some(WHAT_TO_SHARE_TIP));
    ui.add_space(10.0);
    if advanced {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            for (text, field) in share_chips() {
                let on = chip_on(&draft.selection, field);
                if share_chip(ui, text, field, on, field == IncludeField::Password, !busy) {
                    match field {
                        IncludeField::Totp => draft.set_totp(!on),
                        IncludeField::Username => draft.selection.username = !on,
                        IncludeField::Password => draft.selection.password = !on,
                        IncludeField::Uri => draft.selection.uri = !on,
                        IncludeField::Notes => draft.selection.notes = !on,
                    }
                }
            }
        });
        if warning_is_shown(draft) {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(SEED_WARNING).size(13.0).color(theme::ERROR));
            ui.add_space(6.0);
            ui.add_enabled_ui(!busy, |ui| theme::hinted_field(ui, &mut draft.passphrase, PASSPHRASE_HINT, true));
            ui.add_space(4.0);
            note(ui, PASSPHRASE_NOTE, theme::TEXT_FAINT);
        }
    } else {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(WHOLE_RECORD_LEAD).size(15.0).color(theme::INK));
            ui.add_space(4.0);
            super::send_ui::field_pills(ui, &whole_record_fields(item));
        });
    }
    ui.add_space(22.0);

    label(ui, LINK_WORKS_FOR, None);
    ui.add_space(10.0);
    let labels: Vec<&str> = LINK_LIFETIMES.iter().map(|(l, _)| *l).collect();
    let chosen = lifetime_choice(&draft.access.lifetime).unwrap_or(usize::MAX);
    if let Some(picked) = ui
        .add_enabled_ui(!busy, |ui| crate::kit::controls::segmented(ui, &labels, chosen, crate::kit::controls::SEG_EXPIRY))
        .inner
    {
        draft.access.lifetime = crate::send::SendLifetime::Hours(LINK_LIFETIMES[picked].1);
    }
    ui.add_space(22.0);

    ui.horizontal(|ui| {
        label(ui, OPEN_ONCE, Some(OPEN_ONCE_TIP));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut once = draft.access.max_access_count == Some(1);
            if ui.add_enabled_ui(!busy, |ui| crate::kit::controls::switch(ui, &mut once)).inner.changed() {
                draft.access.max_access_count = once.then_some(1);
            }
        });
    });
    ui.add_space(16.0);

    // 0.15's access block (a password, a date, a view cap) and the
    // recipient row, folded under one link: nothing 0.15 offered is gone.
    if theme::action_link(ui, MORE_OPTIONS, 14.0).clicked() {
        state.more = !state.more;
    }
    let draft = &mut state.draft;
    if state.more {
        ui.add_space(8.0);
        super::send_ui::draw_access_block(
            ui,
            super::send_ui::AccessControls {
                lifetime: &mut draft.access.lifetime,
                password: &mut draft.access.password,
                max_access_count: &mut draft.access.max_access_count,
            },
            !busy,
            now,
            zone,
        );
        if let super::send_ui::RecipientAnswer::Forget(email) =
            super::send_ui::draw_recipient_row(ui, &mut draft.recipient, history, !busy)
        {
            draft.forget = Some(email);
        }
    }
    ui.add_space(16.0);

    let problem = export_problem(draft, now);
    if draft.selection.password {
        note(ui, PASSWORD_TRAVELS, theme::DANGER_INK);
        ui.add_space(8.0);
    }
    if let Some(problem) = problem.filter(|p| *p != NOTHING_TICKED) {
        note(ui, problem, theme::ERROR);
        ui.add_space(8.0);
    }
    let size = crate::kit::controls::ButtonSize { height: 54.0, pad_x: 22.0, radius: 16, font_px: 17.0 };
    let mut button_ui = ui.new_child(
        egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 54.0))),
    );
    if busy {
        button_ui.disable();
    }
    let text = if busy { CREATING_LINK } else { CREATE_LINK };
    let button = crate::kit::controls::button(&mut button_ui, crate::kit::controls::ButtonKind::Primary, size, text, None);
    let _ = ui.interact(button.rect, create_link_id(), egui::Sense::hover());
    ui.advance_cursor_after_rect(egui::Rect::from_min_size(button.rect.min, egui::vec2(ui.available_width(), 54.0)));
    if button.clicked() {
        if advanced && !share_chips().iter().any(|(_, f)| chip_on(&draft.selection, *f)) {
            crate::kit::toast::show(ui.ctx(), NOTHING_PICKED);
        } else if export_can_submit(problem, busy) {
            action = RecordUiAction::SubmitExport;
        }
    }
    action
}

/// One "What to share" chip; true when clicked.
fn share_chip(ui: &mut egui::Ui, text: &str, field: IncludeField, on: bool, secret: bool, enabled: bool) -> bool {
    let font = egui::FontId::new(15.0, egui::FontFamily::Name(theme::BOLD.into()));
    let (edge, fill, ink) = match (on, secret) {
        (true, true) => (theme::ERROR, theme::DANGER_WASH, theme::DANGER_INK),
        (true, false) => (theme::BLUE, egui::Color32::from_rgb(0xf5, 0xf7, 0xfd), egui::Color32::from_rgb(0x14, 0x30, 0x7a)),
        (false, _) => (egui::Color32::from_rgb(0xe8, 0xe6, 0xe3), theme::CARD, egui::Color32::from_rgb(0x6b, 0x68, 0x66)),
    };
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, ink);
    let check = if on { 16.0 + 8.0 } else { 0.0 };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(16.0 + check + galley.size().x + 16.0, 42.0), egui::Sense::click());
    let _ = ui.interact(rect, chip_id(field), egui::Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(12), fill, egui::Stroke::new(2.0, edge), egui::StrokeKind::Inside);
    let mut x = rect.left() + 16.0;
    if on {
        crate::kit::icon::stroke(
            ui.painter(),
            egui::Rect::from_center_size(egui::pos2(x + 8.0, rect.center().y), egui::Vec2::splat(16.0)),
            crate::kit::icon::glyph::TICK,
            3.0,
            ink,
        );
        x += check;
    }
    ui.painter().galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, ink);
    enabled && response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Spec B §6.2.
pub fn draw_share_done(ui: &mut egui::Ui, summary: &str, link: &str) -> RecordUiAction {
    let mut action = RecordUiAction::None;
    ui.vertical_centered(|ui| {
        ui.add_space(10.0);
        let (circle, _) = ui.allocate_exact_size(egui::Vec2::splat(76.0), egui::Sense::hover());
        ui.painter().circle_filled(circle.center(), 38.0, egui::Color32::from_rgb(0xe8, 0xf3, 0xec));
        crate::kit::icon::stroke(
            ui.painter(),
            egui::Rect::from_center_size(circle.center(), egui::Vec2::splat(38.0)),
            crate::kit::icon::glyph::TICK,
            2.6,
            egui::Color32::from_rgb(0x1b, 0x7a, 0x3f),
        );
        ui.add_space(16.0);
        ui.label(
            egui::RichText::new(LINK_READY)
                .font(egui::FontId::new(26.0, egui::FontFamily::Name(theme::EXTRABOLD.into())))
                .color(theme::INK),
        );
        ui.add_space(6.0);
        ui.label(egui::RichText::new(summary).size(16.0).color(egui::Color32::from_rgb(0x7d, 0x79, 0x79)));
    });
    ui.add_space(16.0);
    // The link bar.
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(0xf3, 0xf2, 0xf0))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(egui::Margin { left: 18, right: 8, top: 7, bottom: 7 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let size = crate::kit::controls::ButtonSize { height: 42.0, pad_x: 18.0, radius: 12, font_px: 15.0 };
                let copy_w = crate::kit::controls::button_width(ui.ctx(), crate::kit::controls::ButtonKind::Primary, size, COPY_LINK, false);
                let room = (ui.available_width() - copy_w - 12.0).max(40.0);
                ui.add_sized(
                    egui::vec2(room, 42.0),
                    egui::Label::new(
                        egui::RichText::new(link)
                            .font(egui::FontId::new(15.0, egui::FontFamily::Monospace))
                            .color(egui::Color32::from_rgb(0x4a, 0x47, 0x45)),
                    )
                    .truncate(),
                );
                let copy = crate::kit::controls::button(ui, crate::kit::controls::ButtonKind::Primary, size, COPY_LINK, None);
                let _ = ui.interact(copy.rect, copy_link_id(), egui::Sense::hover());
                if copy.clicked() {
                    crate::clipboard::copy_secret(link);
                    crate::kit::toast::show(ui.ctx(), LINK_COPIED);
                }
            });
        });
    ui.add_space(16.0);
    ui.columns(2, |columns| {
        let size = crate::kit::controls::ButtonSize { height: 48.0, pad_x: 18.0, radius: 14, font_px: 16.0 };
        if crate::kit::controls::button(&mut columns[0], crate::kit::controls::ButtonKind::Outline, size, DONE, None).clicked() {
            action = RecordUiAction::Cancel;
        }
        if crate::kit::controls::button(&mut columns[1], crate::kit::controls::ButtonKind::Outline, size, SEE_IN_SHARED, None).clicked() {
            action = RecordUiAction::SeeInShared;
        }
    });
    action
}

/// **Forgetting a person who still holds shares is asked first.** The owner:
/// "if records shared - we ask like these are shared, deleting will remove all
/// shares yes\no".
pub fn forget_title(email: &str) -> String {
    format!("Forget {email}?")
}

/// Names the records, so the answer is given knowing what goes.
pub fn forget_body(names: &[String]) -> String {
    let listed = match names {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    };
    let noun = if names.len() == 1 { "record is" } else { "records are" };
    format!(
        "{} {noun} shared with this person: {listed}. Deleting them removes all these shares; \
         the links stop working at once.",
        names.len()
    )
}

/// The destructive answer, with its count.
pub fn forget_confirm_label(count: usize) -> String {
    format!("Yes, delete {count} share{}", if count == 1 { "" } else { "s" })
}

/// The safe answer.
pub const FORGET_KEEP: &str = "No";

/// What [`draw_forget_confirm`] was answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgetAnswer {
    DeleteShares,
    Keep,
}

/// The confirmation, over a scrim, as this window's other confirmations are
/// drawn (`detail_edit::draw_discard_confirm`). `No` answers Escape.
pub fn draw_forget_confirm(ctx: &egui::Context, email: &str, names: &[String]) -> Option<ForgetAnswer> {
    let mut answer = None;
    theme::modal_scrim(ctx, egui::Area::new(egui::Id::new("record-forget-scrim")));
    theme::movable_modal(ctx, egui::Area::new(egui::Id::new("record-forget-modal"))).show(ctx, |ui| {
        theme::modal_drag_handle(ui, theme::MODAL_PLAIN_HEADER_HEIGHT);
        egui::Frame::new()
            .fill(theme::CARD)
            .corner_radius(CornerRadius::same(10))
            .stroke(egui::Stroke::new(1.0, theme::BORDER))
            .inner_margin(egui::Margin::same(20))
            .show(ui, |ui| {
                ui.set_width(380.0);
                ui.label(theme::bold(&forget_title(email), 15.0).color(theme::INK));
                ui.add_space(10.0);
                ui.label(egui::RichText::new(forget_body(names)).size(12.0).color(theme::TEXT_SECONDARY));
                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    if theme::destructive_button(ui, &forget_confirm_label(names.len())).clicked() {
                        answer = Some(ForgetAnswer::DeleteShares);
                    }
                    ui.add_space(theme::FORM_FOOTER_GAP);
                    if theme::secondary_button(ui, FORGET_KEEP).clicked() {
                        answer = Some(ForgetAnswer::Keep);
                    }
                });
            });
    });
    if answer.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        answer = Some(ForgetAnswer::Keep);
    }
    answer
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

    /// **What the sent log files is what actually went**: the fields the
    /// selection asks for AND the item has, the seed only when it travels
    /// sealed. Labels, never values.
    #[test]
    fn the_fields_filed_are_the_ones_that_went() {
        let item = a_login();
        assert_eq!(
            fields_travelling(&whole_record_selection(), &item, false),
            [USERNAME_LABEL, PASSWORD_LABEL, WEBSITES_LABEL, NOTES_LABEL]
        );
        let mut narrow = whole_record_selection();
        narrow.password = false;
        narrow.notes = false;
        assert_eq!(fields_travelling(&narrow, &item, true), [USERNAME_LABEL, WEBSITES_LABEL, TOTP_LABEL]);
    }

    /// **The confirmation names what goes, and counts it.** The owner: "we
    /// ask like these are shared, deleting will remove all shares yes\no".
    #[test]
    fn forgetting_someone_with_shares_says_which_and_how_many() {
        assert_eq!(forget_title("m.reyes@ledgerline.com"), "Forget m.reyes@ledgerline.com?");
        let two = ["SAP Production".to_string(), "Postgres Prod".to_string()];
        assert_eq!(
            forget_body(&two),
            "2 records are shared with this person: SAP Production and Postgres Prod. \
             Deleting them removes all these shares; the links stop working at once."
        );
        assert!(forget_body(&two[..1]).starts_with("1 record is shared with this person: SAP Production."));
        let three = ["A".to_string(), "B".to_string(), "C".to_string()];
        assert!(forget_body(&three).contains("A, B and C."));
        assert_eq!(forget_confirm_label(1), "Yes, delete 1 share");
        assert_eq!(forget_confirm_label(2), "Yes, delete 2 shares");
        assert_eq!(super::super::send_ui::sent_count_words(3), "3 sent");
    }

    /// **The pills are what the item has.** A record without a password
    /// shows no password pill, because none travels; no item, no pills.
    #[test]
    fn the_whole_record_pills_are_only_what_the_record_holds() {
        assert_eq!(
            whole_record_fields(Some(&a_login())),
            [(USERNAME_LABEL, false), (PASSWORD_LABEL, true), (WEBSITES_LABEL, false), (NOTES_LABEL, false)]
        );
        let bare: crate::vault_bridge::VaultItem = serde_json::from_str(
            r#"{"id":"b","type":1,"name":"Bare","fields":[],"login":{"username":"u","uris":[]}}"#,
        )
        .expect("valid item JSON");
        assert_eq!(whole_record_fields(Some(&bare)), [(USERNAME_LABEL, false)]);
        assert!(whole_record_fields(None).is_empty());
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

    /// The window these modal tests are drawn in.
    ///
    /// **900 wide, which is `settings::MIN_VAULT_WINDOW_SIZE`'s width** -- the
    /// narrowest window this app opens, and therefore the narrowest one the
    /// composer has to be reachable in. It was 640, which was wider than the
    /// card until the card became design 5a's own 690; a harness narrower than
    /// the thing it measures puts the card's right-hand controls off screen,
    /// where egui culls them and every click test reports nothing.
    ///
    /// The height is deliberately not 600: these tests are about where
    /// controls sit across the card, and a viewport that forces the body to
    /// scroll measures the scroll rather than the layout.
    fn modal_input(events: &[egui::Event]) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 900.0),
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

#[cfg(test)]
mod v2_share_tests {
    //! Spec B §6: the Share modal.
    use super::*;
    use crate::kit::testing::{self, click_at, Painted};
    use eframe::egui;

    const SCREEN: egui::Vec2 = egui::vec2(1280.0, 900.0);

    fn frame(ctx: &egui::Context, time: f64, state: &mut RecordSend, events: Vec<egui::Event>) -> (RecordUiAction, Painted) {
        let history = crate::sent_history::SentHistory::default();
        let mut action = RecordUiAction::None;
        let p = testing::frame(ctx, time, SCREEN, events, |ui| {
            action = draw_export_modal(
                ui.ctx(),
                state,
                None,
                None,
                true,
                &history,
                false,
                &crate::send::SystemClock,
                &crate::local_time::SystemZone,
            );
        });
        (action, p)
    }

    fn settled(state: &mut RecordSend) -> (egui::Context, Painted) {
        let ctx = testing::ctx();
        let _ = frame(&ctx, 0.0, state, vec![]);
        let _ = frame(&ctx, 0.1, state, vec![]);
        let (_, p) = frame(&ctx, 0.2, state, vec![]);
        (ctx, p)
    }

    fn click_id(ctx: &egui::Context, time: f64, state: &mut RecordSend, id: egui::Id) -> RecordUiAction {
        let at = ctx.read_response(id).unwrap_or_else(|| panic!("nothing under {id:?}")).rect.center();
        let action = frame(ctx, time, state, click_at(at)).0;
        // The pointer leaves, and the frame after settles.
        let _ = frame(ctx, time + 0.05, state, vec![egui::Event::PointerGone]);
        let _ = frame(ctx, time + 0.1, state, vec![]);
        action
    }

    #[test]
    fn no_field_picked_asks_for_one() {
        let mut state = RecordSend::opening("1", "Ledgerline");
        let (ctx, _) = settled(&mut state);
        let _ = click_id(&ctx, 0.3, &mut state, chip_id(IncludeField::Username));
        let _ = click_id(&ctx, 0.5, &mut state, chip_id(IncludeField::Password));
        assert!(!state.draft.selection.username && !state.draft.selection.password);
        let action = click_id(&ctx, 0.7, &mut state, create_link_id());
        assert_eq!(action, RecordUiAction::None, "nothing is sent");
        assert_eq!(crate::kit::toast::current(&ctx).as_deref(), Some(NOTHING_PICKED));
    }

    #[test]
    fn the_password_chip_is_red_when_picked() {
        let mut state = RecordSend::opening("1", "Ledgerline");
        let (_, p) = settled(&mut state);
        assert!(state.draft.selection.password, "the password is picked by default");
        assert!(p.rects.iter().any(|r| r.fill == theme::DANGER_WASH && r.stroke.color == theme::ERROR && r.rect.height() == 42.0));
    }

    #[test]
    fn the_plan_maps_expiry_and_once() {
        let mut state = RecordSend::opening("1", "Ledgerline");
        assert_eq!(state.draft.access.lifetime, crate::send::SendLifetime::Hours(24), "a day by default");
        assert_eq!(state.draft.access.max_access_count, Some(1), "opens once by default");
        let (ctx, p) = settled(&mut state);
        let week = p.texts.iter().find(|(t, _, _)| t == "7 days").expect("the 7 days segment").2.center();
        let _ = frame(&ctx, 0.3, &mut state, click_at(week));
        assert_eq!(state.draft.access.lifetime, crate::send::SendLifetime::Hours(168));
        assert_eq!(done_summary(&state.draft, true), "Username, Password \u{b7} 7 days \u{b7} opens once");
    }

    #[test]
    fn done_shows_the_link_and_copies_it() {
        let mut state = RecordSend::opening("1", "Ledgerline");
        state.summary = "Username, Password \u{b7} 1 day \u{b7} opens once".into();
        state.done = Some(zeroize::Zeroizing::new("https://vault.example/#/send/abc/def".into()));
        let (ctx, p) = settled(&mut state);
        for text in [LINK_READY, "https://vault.example/#/send/abc/def", COPY_LINK, DONE, SEE_IN_SHARED, "Username, Password \u{b7} 1 day \u{b7} opens once"] {
            assert!(p.texts.iter().any(|(t, _, _)| t == text), "no {text}");
        }
        assert!(ctx.read_response(copy_link_id()).is_some(), "the link bar's Copy");
        let see = p.texts.iter().find(|(t, _, _)| t == SEE_IN_SHARED).unwrap().2.center();
        assert_eq!(frame(&ctx, 0.3, &mut state, click_at(see)).0, RecordUiAction::SeeInShared);
    }
}

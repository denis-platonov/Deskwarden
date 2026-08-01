//! The vault window's right pane in read mode (design 4.8 "Detail pane"):
//! title bar, a per-kind body card, the NOTES card, the AUTOFILL TARGETS
//! card, and the metadata strip.
//!
//! **Every decision that varies by item kind is a pure function here**
//! ([`kind_offers_fill`], [`kind_offers_edit`], [`detail_body_for`],
//! [`notes_text`], [`metadata_line_for`]), and `draw_detail_read` does
//! nothing but obey them. That split is not stylistic: the pane used to
//! hardcode "Login" under every item's name and offer a Fill button on a card
//! that has neither a username nor a password to fill, and the reason it
//! survived several rounds of inspection is that all of it lived inside an
//! `egui` closure no test could call. The tests at the bottom of this file
//! therefore do both -- call the decisions directly *and* render the pane
//! headlessly and read back what it painted, because a decision that is right
//! and a renderer that ignores it is the exact pair the last two findings
//! against this file were.
//!
//! Edit mode is `detail_edit.rs` (Task 8) -- kept separate
//! because the two have almost no shared state (read mode is passive
//! display + copy actions; edit mode owns a draft `VaultItem` and validates
//! it), and the read-mode file was already large enough on its own.

use crate::password_strength;
use crate::theme;
use crate::vault_bridge::{CardData, IdentityData, ItemKind, VaultItem};
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke};

/// The One-time code row's single source of truth. Replaces a bare
/// `Option<String>` (`Some(code)` / `None`), which could not tell apart
/// three genuinely different situations: no TOTP secret configured on this
/// item, a live code, and "the backend could not be reached to find out
/// which of the other two is true". Collapsing those onto one `Option`
/// is what let three consecutive commits each fix one confusion between
/// them and introduce another (independent review of a7b33cb) -- a stale
/// code kept rendering after its secret was removed elsewhere, and later a
/// backend outage made the row vanish entirely, looking identical to "no
/// TOTP here" and inviting a needless 2FA re-enrolment.
///
/// Computed in exactly one place (`vault_window::mod`'s per-frame TOTP
/// block). `draw_detail_read` no longer matches on it at all: the exhaustive
/// match moved into `totp_row_for`, which turns this state into the
/// `Option<TotpRow>` that is the single decision about whether the One-time
/// code row exists (review 14's Important). That match has no catch-all
/// arm, so a future variant is a compile error there rather than a
/// silently-unhandled case -- and "does this item look like it has no 2FA"
/// is a question a unit test asks directly instead of one buried in an egui
/// closure.
#[derive(Debug, Clone, PartialEq)]
pub enum TotpState {
    /// This item has no TOTP secret configured -- the row is omitted
    /// entirely, same as before TOTP existed in this pane at all.
    ///
    /// *Derived from the item*, every frame, by
    /// `vault_window::mod::totp_state_for_secret_presence`: it is whatever
    /// the item we currently hold says, so a secret removed on another
    /// device clears the row in the same frame the reload lands. It is
    /// deliberately **not** a conclusion drawn from a poll -- that is
    /// `NoCodeReported`, below.
    NoSecret,
    /// This item *does* have a TOTP secret, but the poll for its current
    /// code is a background thread now (see `totp_poll_in_flight`'s doc in
    /// `vault_window::mod`) and hasn't reported back yet -- typically just
    /// the first frame or two after selecting this item, but as long as
    /// ~10s (`ureq`'s read timeout) if a *different* item's poll is still
    /// outstanding and holding the one-poll-at-a-time gate. Distinct from
    /// `NoSecret` for the same reason `Unavailable` is (review 12's
    /// Important 3): rendering no row at all here is pixel-identical to "no
    /// TOTP configured", when this item plainly has one -- the code just
    /// hasn't arrived yet.
    Fetching,
    /// A live code, fetched from `bw serve` on the last successful poll.
    /// `seconds_left` is derived from the wall clock (the 30s TOTP window),
    /// not from the fetch, and is refreshed every frame regardless of
    /// whether a poll happened this tick.
    Code { code: String, seconds_left: u8 },
    /// This item *does* have a TOTP secret configured, but the last poll
    /// could not reach `bw serve` (or it answered with an error other than
    /// "no TOTP configured") to fetch the current code. Distinct from
    /// `NoSecret` specifically so the row stays visible with an honest
    /// "unavailable" state instead of vanishing and reading as "not set up".
    Unavailable,
    /// A poll *answered*, successfully, that there is no current code for
    /// this item (`get_totp` -> `Ok(None)`; `bw serve` returns `400` for
    /// this, see `VaultBridge::get_totp`). Keeps its own row
    /// (`totp_no_code_row`), distinct from both `NoSecret`'s absent row and
    /// `Unavailable`'s: at the live call site this state can *only* mean a
    /// disagreement -- `get_totp` is reachable only for an item whose own
    /// login data carries a seed, so "this item has no TOTP" is not one of
    /// the things `Ok(None)` can be saying (review 14's Important; 48cff27
    /// rendered it as no row and that justification did not survive contact
    /// with the call site).
    ///
    /// It is also a *separate variant* from `NoSecret` so the per-frame
    /// presence derivation (`totp_state_for_secret_presence`) structurally
    /// cannot see it and cannot promote it, and so the poll gate
    /// (`totp_state_wants_poll`) can stop asking.
    ///
    /// Review 13's Important: this used to share `NoSecret`, and the two
    /// situations behind that one value pulled in opposite directions.
    /// `NoSecret` is *derived from the item* and must be re-derived every
    /// frame (review 9's fix -- a remotely removed secret has to clear
    /// immediately), which meant the unconditional derivation promoted a
    /// just-polled `Ok(None)` straight back to `Fetching`, the poll gate
    /// fired again a second later, and an item whose stored seed the backend
    /// rejects (removed on another device before a sync landed, or
    /// malformed) sat on "One-time code / Fetching..." forever while issuing
    /// one HTTP round-trip per second, indefinitely. Splitting the two
    /// situations into two variants is what dissolves that, the same move
    /// that dissolved `TotpState` itself, `PickerItemsResult`,
    /// `BackendReadiness` and `VaultReadyOutcome`.
    ///
    /// Reset on selection change (`run`'s reset block sets `NoSecret`, which
    /// the derivation then promotes to `Fetching`), so selecting the item
    /// again polls normally.
    NoCodeReported,
}

/// What the LOGIN CREDENTIALS card's One-time code row actually shows --
/// the *render* layer's own vocabulary, derived from [`TotpState`] by
/// [`totp_row_for`] and nothing else.
///
/// This exists because the render layer had its own instance of the bug that
/// [`TotpState`]'s own variants were introduced to kill: several distinct
/// situations sharing one representation. `NoSecret` and `NoCodeReported`
/// were two different facts about an item that both drew *nothing*, and
/// "nothing" is not a neutral rendering -- it is the pixel-for-pixel
/// rendering of "this item has no 2FA at all", which is what made a
/// previous reviewer point out a user would re-enrol.
///
/// `Option<TotpRow>` is the single source of truth for whether the row
/// appears: `draw_detail_read` cannot omit a row without `totp_row_for`
/// having said `None`, so the decision is unit-testable directly rather than
/// living inside an `egui` closure no test can call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TotpRow<'a> {
    /// [`TotpState::Fetching`].
    Fetching,
    /// [`TotpState::Code`].
    Code { code: &'a str, seconds_left: u8 },
    /// [`TotpState::Unavailable`].
    Unavailable,
    /// [`TotpState::NoCodeReported`].
    NoCode,
}

/// The One-time code row for a given [`TotpState`], or `None` for the one
/// state that genuinely means "this item has no one-time codes".
///
/// Exhaustive with no catch-all, so a new `TotpState` variant is a compile
/// error here rather than silently inheriting some other variant's pixels.
///
/// **Only `NoSecret` may return `None`.** Review 14's Important: at the live
/// call site (`vault_window::mod`'s per-frame TOTP block), `get_totp` is
/// only ever called for an item whose *own login data carries a seed*, so
/// the one situation in which `Ok(None)` could have meant "this item has no
/// TOTP" is unreachable -- every `NoCodeReported` that can actually occur is
/// a *disagreement* between the cached item and `bw serve`. Drawing that as
/// an absent row is the same "reads as: this item has no TOTP" conflation
/// reviews 8 and 12 forced out of `Unavailable` and `Fetching`; it had
/// simply reappeared one layer down.
pub fn totp_row_for(totp: &TotpState) -> Option<TotpRow<'_>> {
    match totp {
        TotpState::NoSecret => None,
        TotpState::Fetching => Some(TotpRow::Fetching),
        TotpState::Code { code, seconds_left } => Some(TotpRow::Code {
            code: code.as_str(),
            seconds_left: *seconds_left,
        }),
        TotpState::Unavailable => Some(TotpRow::Unavailable),
        TotpState::NoCodeReported => Some(TotpRow::NoCode),
    }
}

/// Which of the read pane's masked values are currently revealed.
///
/// **This is owned by `vault_window::mod`'s `run`, in its per-selection state
/// block next to where the lone `reveal_password` bool used to sit, and is
/// cleared by the same selection-change reset.** That placement is the whole
/// point of the type existing. A `let mut revealed = false` declared inside
/// the pane closure is reset on every frame: the Reveal click flips it, the
/// frame ends, the binding is dropped, and the next frame draws the value
/// masked again -- a toggle that visibly does nothing. That exact bug was
/// found and fixed once already in `detail_edit.rs`, and adding two more
/// masked rows is exactly the moment it would come back.
///
/// One struct rather than three `&mut bool` parameters so adding a fourth
/// masked row cannot quietly reuse another row's flag: two rows sharing one
/// bool would reveal both at once.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RevealState {
    /// A login's password row.
    pub password: bool,
    /// A card's number row.
    pub card_number: bool,
    /// A card's security-code row.
    pub card_code: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailAction {
    None,
    Edit,
    Fill,
    CopyUsername,
    CopyPassword,
    CopyTotp,
    /// A card's number was copied. Named rather than carrying the value, for
    /// the same reason [`Self::CopyPassword`] is: the caller already holds the
    /// item and can read the `Zeroizing<String>` out of it, so the plaintext
    /// secret never gets a second, non-zeroizing home inside this enum.
    CopyCardNumber,
    /// A card's security code was copied. See [`Self::CopyCardNumber`].
    CopyCardCode,
    /// A non-secret row was copied, carrying its own already-rendered value --
    /// the card's cardholder name, brand and expiry, and every identity field.
    ///
    /// These carry the value because naming them would mean twenty-odd
    /// variants and a second copy of the field-to-value mapping in
    /// `vault_window::mod`, which is how the two would drift. The two card
    /// secrets deliberately do *not* use this door; nothing that reaches here
    /// is `Zeroizing` in the model either.
    CopyValue(String),
    OpenWebsite(String),
    /// The header's Delete button was clicked. `vault_window::mod`'s
    /// two-click `confirm_click` gates whether this click is armed or
    /// confirming -- `draw_detail_read` itself only reports the click, via
    /// `delete_pending` (see that param's doc comment) for which label/state
    /// to show.
    Delete,
}

/// Whether this kind can be filled into an application.
///
/// Login-only by explicit design decision (see the spec): the fill path
/// resolves exactly a username and a password, so every other kind would type
/// two empty strings into whatever window is focused. This gates the "Fill in
/// app" button, the AUTOFILL TARGETS card, the Ctrl+Shift+F equivalent in
/// `vault_window::mod`, and the metadata strip's fill count and password
/// strength -- one predicate, so those five cannot drift apart.
///
/// Exhaustive with no catch-all: a future `ItemKind` variant must be a
/// compile error here, not silently inherit "yes, fill this".
pub fn kind_offers_fill(kind: ItemKind) -> bool {
    match kind {
        ItemKind::Login => true,
        ItemKind::SecureNote
        | ItemKind::Card
        | ItemKind::Identity
        | ItemKind::SshKey
        | ItemKind::Unknown(_) => false,
    }
}

/// Whether this kind may be opened in the edit form.
///
/// **Login-only, and this is a stopgap with a date on it.** The read pane
/// became kind-aware before the edit form did, and `EditDraft` is still
/// login-shaped in both directions: `from_item` reads only `item.login`, and
/// `apply_to` does `updated.login.unwrap_or_default()` unconditionally, so
/// saving a card from that form gives it an empty `login` object it never had
/// -- an item carrying two type objects, the exact risk the spec names. The
/// form's own heading says "Edit login".
///
/// Offering the button anyway would mean offering to corrupt the item, so
/// until the plan's kind-aware `EditDraft` lands (Task 6) the button is not
/// drawn for kinds the form cannot honestly edit. **Delete is deliberately
/// not gated** -- deleting a card means exactly what deleting a login means.
///
/// When `EditDraft` becomes kind-aware, this becomes `true` for the kinds it
/// handles; it is a separate predicate from [`kind_offers_fill`] precisely so
/// that change cannot accidentally re-enable filling too.
pub fn kind_offers_edit(kind: ItemKind) -> bool {
    match kind {
        ItemKind::Login => true,
        ItemKind::SecureNote
        | ItemKind::Card
        | ItemKind::Identity
        | ItemKind::SshKey
        | ItemKind::Unknown(_) => false,
    }
}

/// The item's note body, or `None` when there is nothing worth a card.
///
/// Empty and whitespace-only are *absent*: rendering the card for either
/// produces an empty box under a heading, which reads as a note whose
/// contents failed to load rather than as an item with no note. Trimmed, not
/// rejected, when there is real text either side of the whitespace.
pub fn notes_text(item: &VaultItem) -> Option<&str> {
    item.notes
        .as_deref()
        .map(|n| n.as_str())
        .map(str::trim)
        .filter(|n| !n.is_empty())
}

/// The heading and body of a pane for an item this build cannot show the
/// contents of. Two kinds reach it, for two different reasons -- see
/// [`detail_body_for`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedPane {
    pub heading: &'static str,
    pub message: String,
}

/// What the read pane's body is, for one [`ItemKind`].
///
/// The same shape as [`TotpRow`] and for the same reason: the decision is a
/// value a unit test can call for, so "which body does a type-5 item get" is
/// answered directly instead of being inferred from an `egui` closure no test
/// can reach. Every finding this file has collected lived in that gap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailBody {
    /// Today's LOGIN CREDENTIALS card, unchanged, including the One-time
    /// code row.
    LoginCredentials,
    /// No body card of its own: a secure note *is* its note, and the NOTES
    /// card below the dispatch is where that renders. Deliberately not an
    /// empty card -- a "SECURE NOTE" heading over nothing would read as a
    /// note that failed to load.
    NotesOnly,
    /// Task 4 fills this in; today it is the heading and nothing else.
    Card,
    /// Task 4 fills this in; today it is the heading and nothing else.
    Identity,
    /// This build cannot show the item's own data, and says so.
    Unsupported(UnsupportedPane),
}

/// The read pane's body for a kind. The one place that decision is made.
///
/// Exhaustive, no catch-all, so a new `ItemKind` variant is a compile error
/// here rather than silently inheriting whatever the neighbouring arm draws.
///
/// **`SshKey` has its own arm on purpose, and the plan was wrong about it.**
/// The plan assumed a type-5 item would fall through to `Unknown` and get the
/// unsupported pane for free. It does not: `ItemKind::SshKey` is a real
/// variant, so `ItemKind::of` maps type 5 straight onto it. Without this arm
/// it would have rendered whatever sat next to it. What it gets instead is
/// the unsupported pane with its *own* message, because the two situations
/// are genuinely different: an unknown type is one Deskwarden does not
/// recognise, while an SSH key is one it recognises and still cannot show --
/// `VaultItem` deliberately carries no `ssh_key` field, since type 5's wire
/// shape is the one this repo could not verify and modelling it from memory
/// is how a modelled field and its `other` copy start disagreeing (see
/// `VaultItem::notes`' doc). Rendering an SSH pane with blank rows would say
/// the key is missing; it is not, it is riding `other` intact.
pub fn detail_body_for(kind: ItemKind) -> DetailBody {
    match kind {
        ItemKind::Login => DetailBody::LoginCredentials,
        ItemKind::SecureNote => DetailBody::NotesOnly,
        ItemKind::Card => DetailBody::Card,
        ItemKind::Identity => DetailBody::Identity,
        ItemKind::SshKey => DetailBody::Unsupported(UnsupportedPane {
            heading: "SSH KEY",
            message: "Deskwarden can't show SSH keys yet. This item's key is unchanged \
                      and safe -- open it in the Bitwarden web vault or app to view or \
                      edit it."
                .to_string(),
        }),
        ItemKind::Unknown(item_type) => DetailBody::Unsupported(UnsupportedPane {
            heading: "UNSUPPORTED ITEM",
            message: format!(
                "Deskwarden doesn't know how to show item type {item_type} yet. Its \
                 contents are unchanged and safe -- open it in the Bitwarden web vault \
                 or app to view or edit it."
            ),
        }),
    }
}

/// Trimmed text, or `None` when there is nothing to render. The single
/// empty-suppression rule this file's panes share: an empty string and an
/// absent key are the same thing to a reader, and a blank label is worse than
/// no row.
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

/// A card's expiry as `MM/YYYY`, with either half allowed to be missing.
///
/// Both halves are strings on the wire and either may be absent, so "what does
/// a card with a month but no year show" is a decision rather than an
/// accident: it renders whichever half exists rather than a half-formed
/// `03/`, which reads as data loss.
///
/// **The month is padded only when it needs padding.** Bitwarden's own
/// `item.card` template sends `expMonth: "04"` -- already zero-padded (see
/// `.superpowers/sdd/item-shapes-capture.md`) -- so a formatter that blindly
/// prefixed a `0` would render `004`. Parsing first and reformatting handles
/// both shapes; anything that does not parse as a month (`"xx"`, `"13"`,
/// `"0"`) is passed through untouched, because showing an unexpected value
/// beats silently dropping it.
fn card_expiry_text(month: Option<&str>, year: Option<&str>) -> Option<String> {
    let month = non_empty(month).map(|m| match m.parse::<u8>() {
        Ok(n) if (1..=12).contains(&n) => format!("{n:02}"),
        _ => m.to_string(),
    });
    let year = non_empty(year).map(str::to_string);
    match (month, year) {
        (Some(m), Some(y)) => Some(format!("{m}/{y}")),
        (Some(m), None) => Some(m),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

/// Every value the card pane shows, formatted and emptiness-suppressed once.
///
/// Pure, and the *only* producer of a card's displayed text. Two separate
/// findings share that requirement.
///
/// **The emptiness rule.** `card_rows` used to open-code "is this card empty"
/// as a five-way `is_none()` conjunction sitting next to five per-row `if let
/// Some` checks. They agreed, but nothing made them: a sixth field added to
/// the render list and not to the conjunction yields a pane that draws a row
/// *and* says "No card details on this item." [`CardFields::is_empty`] is now
/// that rule, expressed once over the same struct the rows are drawn from, and
/// it destructures rather than reading fields by name so a sixth field is a
/// compile error there rather than a silent omission.
///
/// **Displayed and copied must not diverge.** The number and the security code
/// render through [`non_empty`], which *trims*, while `vault_window::mod`'s
/// Copy handler used to read `card.number`/`card.code` raw off the item. A
/// number stored as `" 4242… "` displayed trimmed and copied with the
/// whitespace, which some payment forms reject. Both sides now come from here,
/// so what is copied is exactly what is shown.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CardFields {
    pub cardholder: Option<String>,
    pub brand: Option<String>,
    /// Masked until revealed. Plain `String` rather than `Zeroizing`: this is
    /// the same already-formatted copy the pane paints, and the wrapped field
    /// on the item is untouched (see the module's recorded "zeroize is leaky
    /// beyond the wrapped fields" deferral).
    pub number: Option<String>,
    pub expiry: Option<String>,
    /// Masked until revealed. See [`Self::number`].
    pub code: Option<String>,
}

impl CardFields {
    /// True when this card has nothing at all to draw -- the one rule, so the
    /// "No card details on this item." note and the absence of every row are
    /// the same decision rather than two that happen to agree.
    pub fn is_empty(&self) -> bool {
        let Self {
            cardholder,
            brand,
            number,
            expiry,
            code,
        } = self;
        cardholder.is_none() && brand.is_none() && number.is_none() && expiry.is_none() && code.is_none()
    }
}

/// See [`CardFields`].
pub fn card_fields(data: &CardData) -> CardFields {
    CardFields {
        cardholder: non_empty(data.cardholder_name.as_deref()).map(str::to_string),
        brand: non_empty(data.brand.as_deref()).map(str::to_string),
        number: non_empty(data.number.as_deref().map(|n| n.as_str())).map(str::to_string),
        expiry: card_expiry_text(data.exp_month.as_deref(), data.exp_year.as_deref()),
        code: non_empty(data.code.as_deref().map(|c| c.as_str())).map(str::to_string),
    }
}

/// The identity pane's displayed text: named groups, each with its surviving
/// label/value rows, empty fields and whole empty groups already suppressed.
/// See [`identity_groups`], its only producer, and [`identity_rows`] for why
/// the pane takes this rather than an [`IdentityData`].
type IdentityGroups = Vec<(&'static str, Vec<(&'static str, String)>)>;

/// The identity pane's rows, grouped, with empty fields and empty groups
/// removed.
///
/// Pure so the suppression rule is tested directly rather than inferred from a
/// screenshot. An identity has eighteen fields and a real one populates a
/// handful; without this the pane is mostly blank labels, which is the risk
/// the spec names by name.
///
/// `address3` is in the Address group deliberately. It is absent from
/// Bitwarden's captured template but present in its documented schema, so it
/// is modelled (see [`IdentityData`]'s doc); if a real item carries it, the
/// pane shows it instead of hiding it in `other`. It costs one suppressed row
/// on every item that does not.
fn identity_groups(identity: &IdentityData) -> IdentityGroups {
    let f = |label: &'static str, value: &Option<String>| {
        non_empty(value.as_deref()).map(|v| (label, v.to_string()))
    };
    let group = |name: &'static str, rows: Vec<Option<(&'static str, String)>>| {
        (name, rows.into_iter().flatten().collect::<Vec<_>>())
    };
    let groups = vec![
        group(
            "Name",
            vec![
                f("Title", &identity.title),
                f("First name", &identity.first_name),
                f("Middle name", &identity.middle_name),
                f("Last name", &identity.last_name),
            ],
        ),
        group(
            "Contact",
            vec![
                f("Email", &identity.email),
                f("Phone", &identity.phone),
                f("Username", &identity.username),
                f("Company", &identity.company),
            ],
        ),
        group(
            "Address",
            vec![
                f("Address", &identity.address1),
                f("Address 2", &identity.address2),
                f("Address 3", &identity.address3),
                f("City", &identity.city),
                f("State", &identity.state),
                f("Postal code", &identity.postal_code),
                f("Country", &identity.country),
            ],
        ),
        group(
            "Government IDs",
            vec![
                f("SSN", &identity.ssn),
                f("Passport number", &identity.passport_number),
                f("Licence number", &identity.license_number),
            ],
        ),
    ];
    groups
        .into_iter()
        .filter(|(_, rows)| !rows.is_empty())
        .collect()
}

/// The metadata strip's text: "Updated N days ago · Filled N times ·
/// Strength: X". `updated_days_ago` is `None` when the item carries no
/// parseable `revisionDate` (shows "Updated recently" rather than
/// fabricating a number).
///
/// This is the *login* strip. [`metadata_line_for`] is what the pane calls;
/// this stays as it was so a login's strip is provably byte-identical to what
/// it rendered before kinds existed.
pub fn metadata_line(updated_days_ago: Option<i64>, fill_count: u32, password: &str) -> String {
    let updated = updated_text(updated_days_ago);
    let filled = if fill_count == 1 {
        "Filled 1 time".to_string()
    } else {
        format!("Filled {fill_count} times")
    };
    let strength = password_strength::rate(password).label();
    format!("{updated} \u{b7} {filled} \u{b7} Strength: {strength}")
}

fn updated_text(updated_days_ago: Option<i64>) -> String {
    match updated_days_ago {
        Some(0) => "Updated today".to_string(),
        Some(1) => "Updated 1 day ago".to_string(),
        Some(n) => format!("Updated {n} days ago"),
        None => "Updated recently".to_string(),
    }
}

/// The metadata strip for one kind.
///
/// A fill count and a password strength are *login* facts: a card has no
/// password to rate, and no kind but a login can be filled, so both halves
/// are gated on the same [`kind_offers_fill`] the buttons are. Left
/// ungated, a secure note rendered "Filled 0 times \u{b7} Strength: Weak"
/// under it -- the same "every item is a login" claim the subtitle was
/// making, one line further down, and with the added insult of rating the
/// strength of a password that does not exist.
pub fn metadata_line_for(
    kind: ItemKind,
    updated_days_ago: Option<i64>,
    fill_count: u32,
    password: &str,
) -> String {
    if kind_offers_fill(kind) {
        metadata_line(updated_days_ago, fill_count, password)
    } else {
        updated_text(updated_days_ago)
    }
}

pub fn draw_detail_read(
    ui: &mut egui::Ui,
    item: &VaultItem,
    fill_count: u32,
    totp: &TotpState,
    // Whether *this* item currently has a delete armed (its first click
    // already happened and the confirm window hasn't expired) -- purely for
    // what the Delete button shows; `vault_window::mod`'s `confirm_click` is
    // what actually decides whether a click here is arming or confirming.
    delete_pending: bool,
    // Owned by `vault_window::mod`'s `run` and reset on selection change --
    // see `RevealState`'s doc for why it cannot live inside this function.
    reveal: &mut RevealState,
    // This item's favicon texture, if `vault_window::mod`'s icon cache has
    // already loaded one -- mirrors `item_list.rs`'s `item_row`, which uses
    // the exact same `Some(tex)`/`None` pattern for its row avatar. `None`
    // falls back to the colored-initials monogram, same as every other
    // avatar in this app when no favicon is available.
    icon: Option<&egui::TextureHandle>,
) -> DetailAction {
    let mut action = DetailAction::None;
    // Derived once, here, and passed to the pure decisions below -- not
    // re-derived per widget, so the header, the chrome, the body and the
    // metadata strip cannot disagree about what this item is.
    let kind = ItemKind::of(item);
    let login = item.login.as_ref();
    let username = login.and_then(|l| l.username.as_deref()).unwrap_or("");
    let password = login
        .and_then(|l| l.password.as_deref())
        .map(|p| p.as_str())
        .unwrap_or("");

    ui.horizontal(|ui| {
        match icon {
            Some(tex) => {
                // Rounded to match `theme::avatar`'s initials-tile treatment
                // (same `size * 0.25` formula) -- see `item_list.rs`'s
                // matching fix for why an unrounded favicon in an identical
                // box reads as visually heavier than the monogram fallback.
                const SIZE: f32 = 44.0;
                ui.add(
                    egui::Image::new((tex.id(), tex.size_vec2()))
                        .fit_to_exact_size(egui::Vec2::splat(SIZE))
                        .corner_radius(CornerRadius::same((SIZE * 0.25) as u8)),
                );
            }
            None => theme::avatar(ui, &theme::initials(&item.name), 44.0, true),
        }
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.label(theme::bold(&item.name, 22.0).color(theme::INK));
            ui.label(RichText::new(kind.label()).size(12.0).color(theme::TEXT_FAINT));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if kind_offers_edit(kind) && theme::secondary_button(ui, "Edit").clicked() {
                action = DetailAction::Edit;
            }
            // Not drawn for a kind that cannot be filled: the fill path
            // resolves exactly a username and a password, so this button on a
            // card would type two empty strings into whatever window happens
            // to be focused. See `kind_offers_fill`.
            if kind_offers_fill(kind)
                && theme::primary_button(ui, "Fill in app", Some("CTRL+SHIFT+F")).clicked()
            {
                action = DetailAction::Fill;
            }
            let (delete_label, delete_hover, delete_color) = if delete_pending {
                (
                    "Delete? Click to confirm",
                    "Click again to delete this item. It may still be recoverable from \
                     bitwarden.com or another Bitwarden client afterward.",
                    theme::ERROR,
                )
            } else {
                ("Delete", "Delete this item", theme::INK)
            };
            let delete_button = egui::Button::new(theme::semibold(delete_label, 13.0).color(delete_color))
                .fill(theme::CARD)
                .stroke(Stroke::new(1.0, if delete_pending { theme::ERROR } else { theme::BORDER_STRONG }))
                .corner_radius(CornerRadius::same(7))
                .min_size(egui::Vec2::new(0.0, 32.0));
            if ui.add(delete_button).on_hover_text(delete_hover).clicked() {
                action = DetailAction::Delete;
            }
        });
    });
    ui.add_space(14.0);

    // Which body this item gets is decided by `detail_body_for` and nowhere
    // else, so "what does a type-5 item render" is a question a unit test
    // asks directly. Exhaustive on purpose -- no catch-all arm -- so a new
    // `DetailBody` variant fails to compile here instead of silently
    // inheriting whatever the last arm happened to draw.
    match detail_body_for(kind) {
        DetailBody::LoginCredentials => {
            card(ui, "LOGIN CREDENTIALS", |ui| {
                credential_row(ui, "Username", username, "Copy", &mut action, DetailAction::CopyUsername);
                theme::hairline(ui);
                password_row(ui, password, &mut reveal.password, &mut action);
                // Whether there is a row at all is decided by `totp_row_for` and
                // nowhere else (see its doc), so "this item looks like it has no
                // 2FA" is a decision a unit test can call directly instead of one
                // buried in an `egui` closure. Exhaustive on purpose -- no catch-all
                // arm -- so a new `TotpRow` variant fails to compile here instead of
                // silently inheriting whatever the last arm happened to draw.
                //
                // Unchanged by the kind dispatch, other than moving inside
                // this arm: only a login reaches it, and only a login ever
                // could -- `totp_state_for_secret_presence` forces `NoSecret`
                // (the one state that draws no row) for any item whose own
                // login data carries no seed, and a non-login has no login
                // data at all.
                if let Some(row) = totp_row_for(totp) {
                    theme::hairline(ui);
                    match row {
                        TotpRow::Fetching => totp_fetching_row(ui),
                        TotpRow::Code { code, seconds_left } => totp_code_row(ui, code, seconds_left, &mut action),
                        TotpRow::Unavailable => totp_unavailable_row(ui),
                        TotpRow::NoCode => totp_no_code_row(ui),
                    }
                }
            });
            ui.add_space(10.0);
        }
        // The body is the NOTES card below, and that card is shared with
        // every other kind rather than duplicated here.
        DetailBody::NotesOnly => {}
        DetailBody::Card => {
            card(ui, "CARD DETAILS", |ui| {
                card_rows(ui, item.card.as_ref().map(card_fields), reveal, &mut action);
            });
            ui.add_space(10.0);
        }
        DetailBody::Identity => {
            card(ui, "IDENTITY", |ui| {
                identity_rows(ui, item.identity.as_ref().map(identity_groups), &mut action);
            });
            ui.add_space(10.0);
        }
        DetailBody::Unsupported(pane) => {
            unsupported_card(ui, &pane);
            ui.add_space(10.0);
        }
    }

    if let Some(notes) = notes_text(item) {
        card(ui, "NOTES", |ui| {
            ui.label(RichText::new(notes).size(13.0).color(theme::INK));
        });
        ui.add_space(10.0);
    }

    let website = login
        .and_then(|l| l.uris.first())
        .and_then(|u| u.uri.as_deref())
        .unwrap_or("");
    // Gated on the kind as well as on there being a URI: this card is the
    // autofill *targets* card, and advertising targets for an item the fill
    // path will not fill is the same false promise the Fill button was.
    if kind_offers_fill(kind) && !website.is_empty() {
        card(ui, "AUTOFILL TARGETS", |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(website).size(13.0).color(theme::TEXT_SECONDARY));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::secondary_button(ui, "Open").clicked() {
                        action = DetailAction::OpenWebsite(website.to_string());
                    }
                });
            });
        });
        ui.add_space(10.0);
    }

    let updated_days_ago = item
        .other
        .get("revisionDate")
        .and_then(|v| v.as_str())
        .and_then(days_since);
    ui.label(
        RichText::new(metadata_line_for(kind, updated_days_ago, fill_count, password))
            .size(11.0)
            .color(theme::TEXT_GHOST),
    );

    action
}

/// A pane for an item this build cannot show the contents of: it states the
/// fact and nothing more, rather than rendering fabricated or blank fields.
///
/// One widget for both situations that produce an [`UnsupportedPane`] (an
/// unknown type, and an SSH key), because they differ only in their words --
/// which is what [`detail_body_for`] decides, and what the tests read.
fn unsupported_card(ui: &mut egui::Ui, pane: &UnsupportedPane) {
    card(ui, pane.heading, |ui| {
        ui.label(
            RichText::new(pane.message.as_str())
                .size(12.0)
                .color(theme::TEXT_FAINT),
        );
    });
}

fn card(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0, theme::HAIRLINE))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::letterspaced(title, 10.0, theme::SEMIBOLD, 1.2, theme::TEXT_GHOST));
            ui.add_space(8.0);
            contents(ui);
        });
}

fn credential_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &str,
    copy_label: &str,
    action: &mut DetailAction,
    on_copy: DetailAction,
) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(label).size(11.0).color(theme::TEXT_FAINT));
            ui.label(RichText::new(value).size(13.0).color(theme::INK));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::secondary_button(ui, copy_label).clicked() {
                *action = on_copy;
            }
        });
    });
    ui.add_space(6.0);
}

fn password_row(ui: &mut egui::Ui, password: &str, revealed: &mut bool, action: &mut DetailAction) {
    masked_row(ui, "Password", password, revealed, action, DetailAction::CopyPassword);
}

/// A secret row: monospace, bullets until revealed, with Reveal and Copy.
///
/// Extracted verbatim from `password_row` when the card pane gained two more
/// secrets, so the treatment the spec calls for ("exactly as passwords are")
/// is literally the same code rather than a second copy that drifts. The
/// bullet run is `max(8)` so a short value does not leak its length.
fn masked_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &str,
    revealed: &mut bool,
    action: &mut DetailAction,
    on_copy: DetailAction,
) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(label).size(11.0).color(theme::TEXT_FAINT));
            let shown = if *revealed { value.to_string() } else { "•".repeat(value.chars().count().max(8)) };
            ui.label(RichText::new(shown).size(13.0).color(theme::INK).family(egui::FontFamily::Monospace));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::secondary_button(ui, "Copy").clicked() {
                *action = on_copy;
            }
            if theme::secondary_button(ui, if *revealed { "Hide" } else { "Reveal" }).clicked() {
                *revealed = !*revealed;
            }
        });
    });
    ui.add_space(6.0);
}

/// A short line of body text for a pane that has a heading and no rows.
///
/// The alternative is an empty box under a heading, which `notes_text`'s own
/// doc argues reads as contents that failed to load rather than as an item
/// with nothing in it. Reachable for real: the spec's rule is that a `type: 3`
/// carrying no `card` object is an *empty card*, not an unsupported item.
fn empty_pane_note(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(theme::TEXT_FAINT));
}

/// The CARD DETAILS rows. Empty fields do not render, exactly as the identity
/// pane's do not -- a card populates five fields at most and a blank "Brand"
/// label is noise.
///
/// The number and the security code are the only masked values on either
/// pane, and their reveal flags come from the caller (see [`RevealState`]).
///
/// **It takes [`CardFields`], never a [`CardData`], and that is structural.**
/// [`CardFields`] is documented as the *only* producer of a card's displayed
/// text, and while the raw `CardData` stayed in scope for the whole of this
/// function that was a convention a sixth row could break without noticing: an
/// `if let Some(v) = &data.new_field` drawn here compiles, renders, and is
/// invisible to [`CardFields::is_empty`] -- the pane draws a row *and* says "No
/// card details on this item.", the exact failure the type exists to prevent.
/// The conversion happens at the call site instead, so there is no `data` here
/// to reach for and a sixth field has to go through [`CardFields`] (where
/// `is_empty`'s destructuring makes it a compile error) to reach the pane.
fn card_rows(
    ui: &mut egui::Ui,
    fields: Option<CardFields>,
    reveal: &mut RevealState,
    action: &mut DetailAction,
) {
    let Some(fields) = fields else {
        empty_pane_note(ui, "No card details on this item.");
        return;
    };

    if fields.is_empty() {
        empty_pane_note(ui, "No card details on this item.");
        return;
    }
    let CardFields {
        cardholder,
        brand,
        number,
        expiry,
        code,
    } = fields;

    // `first` tracks whether a hairline is owed, so suppressing a row never
    // leaves a separator with nothing on one side of it.
    let mut first = true;
    let separate = |ui: &mut egui::Ui, first: &mut bool| {
        if *first {
            *first = false;
        } else {
            theme::hairline(ui);
        }
    };
    if let Some(v) = &cardholder {
        separate(ui, &mut first);
        credential_row(ui, "Cardholder name", v, "Copy", action, DetailAction::CopyValue(v.clone()));
    }
    if let Some(v) = &brand {
        separate(ui, &mut first);
        credential_row(ui, "Brand", v, "Copy", action, DetailAction::CopyValue(v.clone()));
    }
    if let Some(v) = &number {
        separate(ui, &mut first);
        masked_row(ui, "Number", v, &mut reveal.card_number, action, DetailAction::CopyCardNumber);
    }
    if let Some(v) = &expiry {
        separate(ui, &mut first);
        credential_row(ui, "Expiry", v, "Copy", action, DetailAction::CopyValue(v.clone()));
    }
    if let Some(v) = &code {
        separate(ui, &mut first);
        masked_row(ui, "Security code", v, &mut reveal.card_code, action, DetailAction::CopyCardCode);
    }
}

/// The IDENTITY rows, grouped by [`identity_groups`] and nothing else -- the
/// suppression of empty fields *and* of whole empty groups is that function's
/// decision, tested directly, and this only draws what it hands back.
///
/// **It takes [`IdentityGroups`], never an [`IdentityData`], and that is
/// structural** -- the same restructuring `card_rows` got, for the identical
/// hazard one pane over. While the raw `identity` stayed in scope here, "every
/// displayed row comes from `identity_groups`" was a convention a new row could
/// break without noticing: a `credential_row(ui, "Website", &identity.website,
/// ..)` written directly off it compiles and renders, and is invisible to the
/// emptiness check below -- so the pane draws a row *and* says "No identity
/// details on this item.", and is invisible to the group suppression too, so it
/// lands outside every heading. The conversion happens at the call site
/// instead, so there is no `identity` here to reach for and a nineteenth field
/// has to go through [`identity_groups`] to reach the pane.
fn identity_rows(ui: &mut egui::Ui, groups: Option<IdentityGroups>, action: &mut DetailAction) {
    let groups = groups.unwrap_or_default();
    if groups.is_empty() {
        empty_pane_note(ui, "No identity details on this item.");
        return;
    }
    for (index, (group_name, rows)) in groups.iter().enumerate() {
        if index > 0 {
            theme::hairline(ui);
            ui.add_space(4.0);
        }
        ui.label(theme::semibold(*group_name, 11.0).color(theme::TEXT_SECONDARY));
        ui.add_space(4.0);
        for (label, value) in rows {
            credential_row(ui, label, value, "Copy", action, DetailAction::CopyValue(value.clone()));
        }
    }
}

fn totp_code_row(ui: &mut egui::Ui, code: &str, seconds_left: u8, action: &mut DetailAction) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("One-time code").size(11.0).color(theme::TEXT_FAINT));
            ui.label(
                RichText::new(code)
                    .size(17.0)
                    .family(egui::FontFamily::Monospace)
                    .color(theme::INK),
            );
            let (rect, _) = ui.allocate_exact_size(egui::vec2(96.0, 4.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(2), theme::HAIRLINE);
            let fraction = (seconds_left as f32 / 30.0).clamp(0.0, 1.0);
            let filled = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * fraction, rect.height()));
            ui.painter().rect_filled(filled, CornerRadius::same(2), theme::BLUE);
            ui.label(RichText::new(format!("{seconds_left}s left")).size(10.0).color(theme::TEXT_GHOST));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::secondary_button(ui, "Copy").clicked() {
                *action = DetailAction::CopyTotp;
            }
        });
    });
}

/// The One-time code row for `TotpState::Fetching`: this item has a TOTP
/// secret and a poll for its current code is already on its way, just not
/// back yet. Keeps the row's label in place, the same shape `Unavailable`'s
/// row does, but reads as an ordinary in-progress state rather than a
/// problem -- this is the everyday, usually sub-second case right after
/// selecting an item, not a backend issue.
fn totp_fetching_row(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("One-time code").size(11.0).color(theme::TEXT_FAINT));
            ui.label(
                RichText::new("Fetching\u{2026}")
                    .size(13.0)
                    .color(theme::TEXT_SECONDARY),
            );
        });
    });
}

/// The One-time code row for `TotpState::Unavailable`: this item has a TOTP
/// secret, but the last attempt to fetch its current code couldn't reach
/// `bw serve`. Keeps the row's label in place (so the item still visibly
/// *has* one-time codes) without a code, a countdown, or a Copy button --
/// there is nothing valid to show or copy right now, and a countdown here
/// would falsely suggest a code is still live. Wording is a plain status,
/// not an alarm: this is very likely `bw serve` still starting up or a
/// transient hiccup, not something the user needs to act on.
fn totp_unavailable_row(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("One-time code").size(11.0).color(theme::TEXT_FAINT));
            ui.label(
                RichText::new("Unavailable right now")
                    .size(13.0)
                    .color(theme::TEXT_SECONDARY),
            );
            ui.label(
                RichText::new("Couldn't reach the vault to get the current code.")
                    .size(10.0)
                    .color(theme::TEXT_GHOST),
            );
        });
    });
}

/// The One-time code row for `TotpState::NoCodeReported`: this item's own
/// login data carries a TOTP seed, but `bw serve` answered -- successfully --
/// that it has no current code for it. That is a *disagreement* between the
/// item we hold and the backend, not "this item has no 2FA", and it is the
/// only thing it can be at the live call site: `get_totp` is never called for
/// an item without a seed (see `vault_window::mod`'s per-frame TOTP block).
///
/// Same shape as `totp_unavailable_row`, deliberately, and same reason: the
/// row has to stay put so the pane is not pixel-identical to an item that
/// never had 2FA -- the reading that had a previous reviewer point out a user
/// would conclude TOTP was not set up and needlessly re-enrol. Different
/// wording, though, because it is a different fact: `Unavailable` means the
/// vault could not be reached and the app is still trying; this one means the
/// vault was reached and had nothing to give. The hint names the usual cause
/// (the seed changed elsewhere and this copy of the item predates it) and the
/// one action that resolves it, since -- unlike `Unavailable` -- this state
/// deliberately stops polling.
fn totp_no_code_row(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("One-time code").size(11.0).color(theme::TEXT_FAINT));
            ui.label(
                RichText::new("No code available for this item")
                    .size(13.0)
                    .color(theme::TEXT_SECONDARY),
            );
            ui.label(
                RichText::new(
                    "The vault has no current code for it. If its authenticator key was \
                     changed on another device, Sync to pick that up.",
                )
                .size(10.0)
                .color(theme::TEXT_GHOST),
            );
        });
    });
}

/// Days between an RFC3339 `revisionDate` (as `bw serve` sends it) and now.
/// `None` on anything unparseable -- the caller shows "Updated recently"
/// rather than a wrong number.
fn days_since(revision_date: &str) -> Option<i64> {
    // A minimal RFC3339 date parse: only the `YYYY-MM-DD` prefix is needed
    // for a day-granularity "N days ago", so this avoids pulling in a full
    // datetime crate for one field. `std::time::SystemTime` supplies "now".
    let date_part = revision_date.get(0..10)?;
    let mut parts = date_part.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    let revision_days = days_from_civil(year, month, day);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let today_days = (now.as_secs() / 86400) as i64;

    Some((today_days - revision_days).max(0))
}

/// Howard Hinnant's civil-from-days algorithm, days-from-civil direction:
/// converts a (year, month, day) into a day count since the Unix epoch,
/// without pulling in a datetime crate for one field.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_bridge::ItemKind;

    /// An item of a given kind, with nothing but a name -- the shape this
    /// pane has to survive, since a real card or note populates none of the
    /// login fields the pane used to assume were there.
    fn an_item(item_type: Option<i64>) -> VaultItem {
        VaultItem {
            id: "id-1".to_string(),
            name: "Sample".to_string(),
            fields: Vec::new(),
            login: None,
            card: None,
            identity: None,
            notes: None,
            item_type,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    /// The type number that produces each kind. `Unknown` needs a number
    /// Bitwarden has not shipped; 9 is arbitrary and deliberately not 5.
    fn item_type_for(kind: ItemKind) -> Option<i64> {
        match kind {
            ItemKind::Login => Some(1),
            ItemKind::SecureNote => Some(2),
            ItemKind::Card => Some(3),
            ItemKind::Identity => Some(4),
            ItemKind::SshKey => Some(5),
            ItemKind::Unknown(t) => Some(t),
        }
    }

    const EVERY_KIND: [ItemKind; 6] = [
        ItemKind::Login,
        ItemKind::SecureNote,
        ItemKind::Card,
        ItemKind::Identity,
        ItemKind::SshKey,
        ItemKind::Unknown(9),
    ];

    /// Every string `draw_detail_read` actually *painted*, gathered from the
    /// frame's own shape list.
    ///
    /// This is the only thing in this file that proves the render matches the
    /// decision rather than merely that the decision is right. Two of the
    /// last findings against this pane were cases where the state machine was
    /// correct and the renderer was not, and a pure-function test cannot see
    /// that gap: it calls the decision directly and never reaches the widget
    /// that is supposed to obey it.
    ///
    /// `theme::apply`'s font set only takes effect at the *start* of the next
    /// frame (see `paint_window_background`'s doc), so this runs a throwaway
    /// frame before the real one -- without it, every `FontFamily::Name`
    /// lookup in the pane resolves against a family that does not exist yet.
    fn painted_text(
        item: &VaultItem,
        totp: &TotpState,
        delete_pending: bool,
        reveal: RevealState,
    ) -> Vec<String> {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 900.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});

        let mut reveal = reveal;
        let output = ctx.run_ui(input(), |ui| {
            draw_detail_read(ui, item, 3, totp, delete_pending, &mut reveal, None);
        });

        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut texts);
        }
        texts
    }

    fn collect_text(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, out);
                }
            }
            // Every other shape is geometry, and this is a test helper, not
            // a decision over a domain enum -- a new `egui::Shape` variant
            // carrying text would be egui's to tell us about, not something
            // an exhaustive match here could catch.
            _ => {}
        }
    }

    /// Same walk as [`collect_text`], but keeping the rectangle egui laid each
    /// string out in -- the only way this file can find a *control* to click,
    /// since `draw_detail_read` returns a `DetailAction` rather than the
    /// widgets' `Response`s.
    fn collect_text_rects(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => out.push((
                text.galley.text().to_string(),
                egui::Rect::from_min_size(text.pos, text.galley.size()),
            )),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text_rects(shape, out);
                }
            }
            _ => {}
        }
    }

    fn painted(item: &VaultItem, totp: &TotpState) -> Vec<String> {
        painted_text(item, totp, false, RevealState::default())
    }

    fn painted_with_reveal(item: &VaultItem, totp: &TotpState, reveal: RevealState) -> Vec<String> {
        painted_text(item, totp, false, reveal)
    }

    fn contains(texts: &[String], needle: &str) -> bool {
        texts.iter().any(|t| t.contains(needle))
    }

    /// The exact hardcoding this task exists to remove: every item, of every
    /// type, was labelled "Login" under its name.
    #[test]
    fn the_header_subtitle_is_the_items_own_kind() {
        for kind in EVERY_KIND {
            let item = an_item(item_type_for(kind));
            let texts = painted(&item, &TotpState::NoSecret);
            assert!(
                texts.contains(&kind.label()),
                "{kind:?} rendered no {:?} subtitle; painted: {texts:?}",
                kind.label()
            );
            if kind != ItemKind::Login {
                assert!(
                    !texts.iter().any(|t| t == "Login"),
                    "{kind:?} still renders the hardcoded \"Login\" subtitle"
                );
            }
        }
    }

    /// A Fill button on a card would type two empty strings into whatever
    /// window is focused, and an AUTOFILL TARGETS card would advertise a
    /// capability the fill path refuses to provide for non-logins.
    #[test]
    fn only_a_login_renders_the_fill_button_and_the_autofill_targets_card() {
        for kind in EVERY_KIND {
            let mut item = an_item(item_type_for(kind));
            // Give every kind a login object carrying a URI, so the autofill
            // card's *other* precondition is satisfied and the only thing
            // that can suppress it is the kind. A non-login item would not
            // really carry one; this is deliberately the hostile fixture.
            item.login = Some(crate::vault_bridge::LoginData {
                username: Some("u".to_string()),
                password: Some("p".to_string().into()),
                totp: None,
                uris: vec![crate::vault_bridge::UriEntry {
                    uri: Some("https://example.com".to_string()),
                    other: serde_json::Map::new(),
                }],
                other: serde_json::Map::new(),
            });
            let texts = painted(&item, &TotpState::NoSecret);
            let expected = kind == ItemKind::Login;

            assert_eq!(
                contains(&texts, "Fill in app"),
                expected,
                "{kind:?}: wrong Fill button presence; painted: {texts:?}"
            );
            assert_eq!(
                contains(&texts, "AUTOFILL TARGETS"),
                expected,
                "{kind:?}: wrong AUTOFILL TARGETS presence"
            );
            assert_eq!(
                contains(&texts, "LOGIN CREDENTIALS"),
                expected,
                "{kind:?}: wrong LOGIN CREDENTIALS presence"
            );
        }
    }

    /// Delete is the one action that means the same thing for every kind, so
    /// gating fill must not take it with it.
    #[test]
    fn every_kind_can_still_be_deleted() {
        for kind in EVERY_KIND {
            let item = an_item(item_type_for(kind));
            let texts = painted(&item, &TotpState::NoSecret);
            assert!(
                contains(&texts, "Delete"),
                "{kind:?} lost its Delete button; painted: {texts:?}"
            );
        }
    }

    /// Notes were invisible for every kind, logins included, because the
    /// field was not modelled and the pane had nowhere to put it.
    #[test]
    fn every_kind_renders_a_notes_card_when_there_is_a_note() {
        for kind in EVERY_KIND {
            let mut item = an_item(item_type_for(kind));
            item.notes = Some("the passphrase".to_string().into());
            let texts = painted(&item, &TotpState::NoSecret);
            assert!(
                contains(&texts, "NOTES"),
                "{kind:?} rendered no NOTES card; painted: {texts:?}"
            );
            assert!(
                contains(&texts, "the passphrase"),
                "{kind:?} rendered a NOTES card without the note in it"
            );
        }
    }

    /// An empty box under a heading is worse than no card at all.
    #[test]
    fn a_whitespace_only_note_renders_no_notes_card() {
        let mut item = an_item(Some(2));
        item.notes = Some("   \n ".to_string().into());
        let texts = painted(&item, &TotpState::NoSecret);
        assert!(
            !contains(&texts, "NOTES"),
            "a whitespace-only note still drew an empty NOTES card: {texts:?}"
        );
    }

    /// A type this build does not know must say so, not render a login-shaped
    /// pane over data that is not a login.
    #[test]
    fn an_unknown_type_renders_an_unsupported_pane_naming_the_type() {
        let item = an_item(Some(9));
        let texts = painted(&item, &TotpState::NoSecret);
        assert!(
            contains(&texts, "UNSUPPORTED ITEM"),
            "an unknown type drew no unsupported card: {texts:?}"
        );
        assert!(
            contains(&texts, "9"),
            "the unsupported card does not name the type number: {texts:?}"
        );
        assert!(
            contains(&texts, "web vault"),
            "the unsupported card does not say where the data can still be seen"
        );
    }

    /// The correction to the plan: `ItemKind::SshKey` is a real variant, so a
    /// type-5 item does *not* fall through to `Unknown` and does not get the
    /// unsupported pane for free. `VaultItem` deliberately has no `ssh_key`
    /// field (the wire shape is the one this repo could not verify), so the
    /// only honest pane is one that says so -- blank rows would read as an
    /// item whose key had been lost.
    #[test]
    fn an_ssh_key_says_it_is_not_supported_yet_rather_than_rendering_blank_rows() {
        let item = an_item(Some(5));
        let texts = painted(&item, &TotpState::NoSecret);
        assert!(
            contains(&texts, "SSH KEY"),
            "an SSH key item drew no SSH card at all: {texts:?}"
        );
        assert!(
            contains(&texts, "web vault"),
            "the SSH key pane does not say the data is still viewable elsewhere: {texts:?}"
        );
        for blank_row_label in ["Public key", "Fingerprint", "Private key"] {
            assert!(
                !contains(&texts, blank_row_label),
                "the SSH key pane rendered a {blank_row_label} row it has no data for"
            );
        }
    }

    /// The plan's own test, over all six variants.
    #[test]
    fn only_logins_offer_to_fill() {
        assert!(kind_offers_fill(ItemKind::Login));
        assert!(!kind_offers_fill(ItemKind::Card));
        assert!(!kind_offers_fill(ItemKind::Identity));
        assert!(!kind_offers_fill(ItemKind::SecureNote));
        assert!(!kind_offers_fill(ItemKind::SshKey));
        assert!(!kind_offers_fill(ItemKind::Unknown(6)));
    }

    /// The edit form is login-shaped, and `EditDraft::apply_to` writes a
    /// `login` object onto whatever it is given. Until that is kind-aware,
    /// offering Edit on a card is offering to corrupt it.
    #[test]
    fn only_logins_offer_to_edit_for_now() {
        assert!(kind_offers_edit(ItemKind::Login));
        for kind in [
            ItemKind::SecureNote,
            ItemKind::Card,
            ItemKind::Identity,
            ItemKind::SshKey,
            ItemKind::Unknown(6),
        ] {
            assert!(!kind_offers_edit(kind), "{kind:?} must not offer Edit yet");
        }
    }

    #[test]
    fn every_kind_has_a_label_and_none_of_them_say_login() {
        for kind in [
            ItemKind::SecureNote,
            ItemKind::Card,
            ItemKind::Identity,
            ItemKind::SshKey,
            ItemKind::Unknown(9),
        ] {
            assert_ne!(kind.label(), "Login", "{kind:?} still labels itself Login");
            assert!(!kind.label().is_empty());
        }
        assert_eq!(ItemKind::Login.label(), "Login");
    }

    #[test]
    fn notes_are_surfaced_only_when_there_is_something_to_show() {
        let with_notes = |n: Option<&str>| {
            let mut item = an_item(Some(1));
            item.notes = n.map(|n| n.to_string().into());
            item
        };
        assert_eq!(notes_text(&with_notes(None)), None);
        assert_eq!(notes_text(&with_notes(Some(""))), None);
        assert_eq!(notes_text(&with_notes(Some("   "))), None);
        assert_eq!(notes_text(&with_notes(Some("\t \n"))), None);
        assert_eq!(notes_text(&with_notes(Some("hi"))), Some("hi"));
        // Trimmed, not rejected: a note the user ended with a newline is
        // still a note, and the card should not lead with blank lines.
        assert_eq!(notes_text(&with_notes(Some("  hi there \n"))), Some("hi there"));
        // Interior whitespace is content and must survive intact.
        assert_eq!(notes_text(&with_notes(Some("a\n\nb"))), Some("a\n\nb"));
    }

    /// The body dispatch, as a value. `SshKey` is the one this pins hardest:
    /// it is a real `ItemKind` variant, so a type-5 item does *not* fall
    /// through to `Unknown`, and without an explicit arm it would have
    /// rendered whatever its neighbour did.
    #[test]
    fn every_kind_dispatches_to_its_own_body() {
        assert_eq!(detail_body_for(ItemKind::Login), DetailBody::LoginCredentials);
        assert_eq!(detail_body_for(ItemKind::SecureNote), DetailBody::NotesOnly);
        assert_eq!(detail_body_for(ItemKind::Card), DetailBody::Card);
        assert_eq!(detail_body_for(ItemKind::Identity), DetailBody::Identity);

        for kind in [ItemKind::SshKey, ItemKind::Unknown(9)] {
            match detail_body_for(kind) {
                DetailBody::Unsupported(pane) => {
                    assert!(
                        pane.message.contains("web vault"),
                        "{kind:?}'s message must point at where the data can still be seen: {:?}",
                        pane.message
                    );
                    assert!(
                        pane.message.contains("unchanged"),
                        "{kind:?}'s message must say the item's own data is intact: {:?}",
                        pane.message
                    );
                }
                other => panic!("{kind:?} dispatched to {other:?}, not an unsupported pane"),
            }
        }
    }

    /// The unknown-type message names the number, so a bug report can say
    /// which type it was rather than "an item type".
    #[test]
    fn the_unknown_pane_names_the_type_number_it_could_not_show() {
        match detail_body_for(ItemKind::Unknown(42)) {
            DetailBody::Unsupported(pane) => assert!(
                pane.message.contains("42"),
                "the message does not name the type: {:?}",
                pane.message
            ),
            other => panic!("Unknown(42) dispatched to {other:?}"),
        }
    }

    /// The SSH pane is not the unknown pane wearing a different number: a
    /// type-5 item is one Deskwarden *recognises* and still cannot show.
    #[test]
    fn the_ssh_pane_and_the_unknown_pane_are_different_messages() {
        let (ssh, unknown) = (
            detail_body_for(ItemKind::SshKey),
            detail_body_for(ItemKind::Unknown(5)),
        );
        assert_ne!(ssh, unknown);
    }

    /// Fill count and password strength are login facts. A secure note with
    /// "Filled 0 times \u{b7} Strength: Weak" under it is the same "everything
    /// is a login" claim as the subtitle was, one line further down.
    #[test]
    fn only_logins_claim_a_fill_count_and_a_password_strength() {
        for kind in EVERY_KIND {
            let line = metadata_line_for(kind, Some(3), 0, "");
            assert!(line.contains("Updated 3 days ago"), "{kind:?}: {line}");
            assert_eq!(
                line.contains("Filled"),
                kind_offers_fill(kind),
                "{kind:?}: {line}"
            );
            assert_eq!(
                line.contains("Strength"),
                kind_offers_fill(kind),
                "{kind:?}: {line}"
            );
        }
    }

    /// The login strip is byte-identical to what it was before the kind
    /// existed -- this whole task must be invisible on a login.
    #[test]
    fn a_logins_metadata_strip_is_unchanged() {
        assert_eq!(
            metadata_line_for(ItemKind::Login, Some(3), 41, "Tr0ub4dor&3xtraLong!"),
            metadata_line(Some(3), 41, "Tr0ub4dor&3xtraLong!")
        );
    }

    /// The drift guard the brief asks for: the *rendered* chrome is compared
    /// against the same predicate the live pane calls, not against a second
    /// copy of the rule written out in the test. A fix that is correct in
    /// `kind_offers_fill` and inert in `draw_detail_read` fails here.
    #[test]
    fn the_rendered_chrome_matches_the_chrome_decision_for_every_kind() {
        for kind in EVERY_KIND {
            let mut item = an_item(item_type_for(kind));
            item.login = Some(crate::vault_bridge::LoginData {
                username: Some("u".to_string()),
                password: Some("p".to_string().into()),
                totp: None,
                uris: vec![crate::vault_bridge::UriEntry {
                    uri: Some("https://example.com".to_string()),
                    other: serde_json::Map::new(),
                }],
                other: serde_json::Map::new(),
            });
            let texts = painted(&item, &TotpState::NoSecret);

            assert_eq!(
                contains(&texts, "Fill in app"),
                kind_offers_fill(kind),
                "{kind:?}: the Fill button disagrees with kind_offers_fill"
            );
            assert_eq!(
                contains(&texts, "AUTOFILL TARGETS"),
                kind_offers_fill(kind),
                "{kind:?}: the autofill card disagrees with kind_offers_fill"
            );
            assert_eq!(
                contains(&texts, "Edit"),
                kind_offers_edit(kind),
                "{kind:?}: the Edit button disagrees with kind_offers_edit"
            );
            assert_eq!(
                contains(&texts, "Strength"),
                kind_offers_fill(kind),
                "{kind:?}: the metadata strip disagrees with kind_offers_fill"
            );
        }
    }

    /// The other half of the drift guard: the body each kind renders is the
    /// one `detail_body_for` chose.
    #[test]
    fn the_rendered_body_matches_the_body_decision_for_every_kind() {
        for kind in EVERY_KIND {
            let item = an_item(item_type_for(kind));
            let texts = painted(&item, &TotpState::NoSecret);
            let heading_present = |h: &str| contains(&texts, h);

            match detail_body_for(kind) {
                DetailBody::LoginCredentials => {
                    assert!(heading_present("LOGIN CREDENTIALS"), "{kind:?}: {texts:?}")
                }
                DetailBody::NotesOnly => {
                    // Nothing but the notes card, and this fixture has no
                    // note -- so no body card at all.
                    for heading in ["LOGIN CREDENTIALS", "CARD DETAILS", "IDENTITY", "NOTES"] {
                        assert!(
                            !heading_present(heading),
                            "{kind:?} drew a {heading} card it has no data for: {texts:?}"
                        );
                    }
                }
                DetailBody::Card => assert!(heading_present("CARD DETAILS"), "{kind:?}: {texts:?}"),
                DetailBody::Identity => assert!(heading_present("IDENTITY"), "{kind:?}: {texts:?}"),
                DetailBody::Unsupported(pane) => {
                    assert!(heading_present(pane.heading), "{kind:?}: {texts:?}");
                    assert!(heading_present(&pane.message), "{kind:?}: {texts:?}");
                }
            }
        }
    }

    /// The dispatch moved the One-time code row inside the login arm, so this
    /// pins what that means for every other kind: no TOTP row, whatever the
    /// state says. Unreachable in the live composition --
    /// `totp_state_for_secret_presence` forces `NoSecret` for an item whose
    /// login data carries no seed, and a non-login has no login data -- which
    /// is exactly why it is worth a test: nothing else would notice if that
    /// stopped being true.
    #[test]
    fn a_non_login_never_renders_a_one_time_code_row_whatever_the_totp_state() {
        for kind in EVERY_KIND {
            if kind == ItemKind::Login {
                continue;
            }
            for totp in [
                TotpState::Fetching,
                TotpState::Code { code: "123456".to_string(), seconds_left: 12 },
                TotpState::Unavailable,
                TotpState::NoCodeReported,
            ] {
                let texts = painted(&an_item(item_type_for(kind)), &totp);
                assert!(
                    !contains(&texts, "One-time code"),
                    "{kind:?} rendered a One-time code row for {totp:?}: {texts:?}"
                );
                assert!(
                    !contains(&texts, "123456"),
                    "{kind:?} rendered another item's live code: {texts:?}"
                );
            }
        }
    }

    /// A login's pane is untouched by all of the above: same rows, same
    /// order, and the TOTP row still lands where it always did.
    #[test]
    fn a_logins_pane_still_renders_every_totp_row_it_did_before() {
        let mut item = an_item(Some(1));
        item.login = Some(crate::vault_bridge::LoginData {
            username: Some("u".to_string()),
            password: Some("p".to_string().into()),
            totp: Some("SEED".to_string().into()),
            uris: Vec::new(),
            other: serde_json::Map::new(),
        });

        for (totp, needle) in [
            (TotpState::Fetching, "Fetching"),
            (TotpState::Code { code: "123456".to_string(), seconds_left: 12 }, "123456"),
            (TotpState::Unavailable, "Unavailable right now"),
            (TotpState::NoCodeReported, "No code available for this item"),
        ] {
            let texts = painted(&item, &totp);
            assert!(
                contains(&texts, "One-time code"),
                "{totp:?} lost its One-time code row: {texts:?}"
            );
            assert!(contains(&texts, needle), "{totp:?} rendered the wrong row: {texts:?}");
        }
        let texts = painted(&item, &TotpState::NoSecret);
        assert!(
            !contains(&texts, "One-time code"),
            "NoSecret must still draw no row: {texts:?}"
        );
    }

    /// Review 14's Important, at the render layer. `NoSecret` is the *only*
    /// state that may draw nothing, because drawing nothing is the exact
    /// pixels of "this item has no 2FA" -- and every other state belongs to
    /// an item that demonstrably does have a seed.
    #[test]
    fn no_secret_is_the_only_state_that_omits_the_one_time_code_row() {
        assert_eq!(totp_row_for(&TotpState::NoSecret), None);

        for state in [
            TotpState::Fetching,
            TotpState::Code { code: "123456".to_string(), seconds_left: 12 },
            TotpState::Unavailable,
            TotpState::NoCodeReported,
        ] {
            assert!(
                totp_row_for(&state).is_some(),
                "{state:?} belongs to an item that HAS a TOTP seed, so omitting the row \
                 renders it pixel-identically to an item with no 2FA at all"
            );
        }
    }

    /// `NoCodeReported` and `Unavailable` are two different messages, not one
    /// row wearing two labels: the first is "the backend answered and has no
    /// code for this", the second is "the backend could not be reached".
    /// Keeping them visually distinct is the property review 8 established
    /// for `Unavailable` and must survive `NoCodeReported` gaining a row.
    #[test]
    fn no_code_reported_and_unavailable_render_as_different_rows() {
        assert_ne!(
            totp_row_for(&TotpState::NoCodeReported),
            totp_row_for(&TotpState::Unavailable)
        );
    }

    /// A card with every field populated. The number is Bitwarden's own
    /// template value, which is what the masking assertions look for.
    fn a_full_card() -> VaultItem {
        let mut item = an_item(Some(3));
        item.card = Some(crate::vault_bridge::CardData {
            cardholder_name: Some("John Doe".to_string()),
            brand: Some("visa".to_string()),
            number: Some("4242424242424242".to_string().into()),
            exp_month: Some("04".to_string()),
            exp_year: Some("2023".to_string()),
            code: Some("123".to_string().into()),
            other: serde_json::Map::new(),
        });
        item
    }

    #[test]
    fn the_card_pane_paints_every_populated_row() {
        let texts = painted(&a_full_card(), &TotpState::NoSecret);
        for label in [
            "CARD DETAILS",
            "Cardholder name",
            "John Doe",
            "Brand",
            "visa",
            "Number",
            "Expiry",
            "04/2023",
            "Security code",
        ] {
            assert!(contains(&texts, label), "the card pane painted no {label:?}: {texts:?}");
        }
    }

    /// **Masked by default.** The assertion is negative on purpose: it is not
    /// enough that a Reveal button exists, the digits must not be in the
    /// frame's own shape list at all. `4242` rather than the whole number, so
    /// a partial mask ("**** 4242") fails too.
    #[test]
    fn the_card_number_and_security_code_are_masked_by_default() {
        let texts = painted(&a_full_card(), &TotpState::NoSecret);
        assert!(
            !contains(&texts, "4242"),
            "the card number was painted in the clear by default: {texts:?}"
        );
        assert!(
            !contains(&texts, "123"),
            "the security code was painted in the clear by default: {texts:?}"
        );
        assert!(
            contains(&texts, "Reveal"),
            "the card pane offers no way to reveal what it masked: {texts:?}"
        );
    }

    /// The other half: the mask is driven by state the *caller* owns, so a
    /// toggle can survive a frame. A `let mut revealed = false` inside the
    /// pane would make this unreachable -- which is precisely the bug this
    /// pins, already found and fixed once in `detail_edit.rs`.
    #[test]
    fn revealing_is_driven_by_caller_owned_state_that_outlives_the_frame() {
        let reveal = RevealState {
            password: false,
            card_number: true,
            card_code: true,
        };
        let texts = painted_with_reveal(&a_full_card(), &TotpState::NoSecret, reveal);
        assert!(
            contains(&texts, "4242424242424242"),
            "a revealed card number did not paint, so the pane ignores the caller's \
             reveal state: {texts:?}"
        );
        assert!(contains(&texts, "123"), "a revealed security code did not paint: {texts:?}");
        assert!(contains(&texts, "Hide"), "a revealed row still offers Reveal: {texts:?}");
    }

    /// **The toggle survives the frame it was clicked in.** Everything else in
    /// this file runs exactly one frame and never clicks anything, which
    /// proves the pane *reads* the caller's state and nothing more. Two
    /// distinct regressions live in that gap: `draw_detail_read` taking
    /// `RevealState` by value (it is `Copy`, so that compiles and silently
    /// drops every toggle), and the Reveal button writing to anything other
    /// than the caller's struct.
    ///
    /// So: ONE `RevealState`, owned out here the way `vault_window::mod`'s
    /// `run` owns it, across three frames. Frame 1 lays the pane out and is
    /// read only to locate the control. Frame 2 delivers a real pointer press
    /// and release on the *number's* Reveal button. Frame 3 is fed no input at
    /// all -- so if the toggle did not outlive frame 2, frame 3 paints bullets.
    #[test]
    fn a_reveal_click_in_one_frame_is_still_revealed_in_the_next() {
        let item = a_full_card();
        let ctx = egui::Context::default();
        let input = |events: Vec<egui::Event>| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 900.0),
            )),
            events,
            ..Default::default()
        };
        // Same two throwaway frames `painted_text` runs, and for the same
        // reason: `theme::apply`'s fonts only exist from the next frame on.
        let _ = ctx.run_ui(input(Vec::new()), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(Vec::new()), |_ui| {});

        let mut reveal = RevealState::default();
        let frame = |events: Vec<egui::Event>, reveal: &mut RevealState| {
            let output = ctx.run_ui(input(events), |ui| {
                draw_detail_read(ui, &item, 3, &TotpState::NoSecret, false, reveal, None);
            });
            let mut rects = Vec::new();
            for clipped in &output.shapes {
                collect_text_rects(&clipped.shape, &mut rects);
            }
            rects
        };

        let laid_out = frame(Vec::new(), &mut reveal);
        // The topmost "Reveal" is the Number row's: the security code's row is
        // drawn below it, and both are inside the same card.
        let reveal_button = laid_out
            .iter()
            .filter(|(text, _)| text == "Reveal")
            .min_by(|a, b| a.1.top().total_cmp(&b.1.top()))
            .map(|(_, rect)| rect.center())
            .unwrap_or_else(|| panic!("the card pane painted no Reveal control: {laid_out:?}"));

        let click = |pos: egui::Pos2| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                },
            ]
        };
        let _ = frame(click(reveal_button), &mut reveal);

        // The click reached the caller's struct, and reached the field that
        // belongs to the row it landed on.
        assert_eq!(
            reveal,
            RevealState {
                password: false,
                card_number: true,
                card_code: false,
            },
            "clicking the card number's Reveal did not write through to the caller's \
             RevealState, or wrote through to the wrong field"
        );

        // And a frame with no input at all still paints the digits.
        let after = frame(Vec::new(), &mut reveal);
        let texts: Vec<String> = after.into_iter().map(|(text, _)| text).collect();
        assert!(
            contains(&texts, "4242424242424242"),
            "the frame after the Reveal click painted the number masked again -- the \
             toggle did not outlive the frame it happened in: {texts:?}"
        );
        assert!(
            !contains(&texts, "123"),
            "the security code was revealed by a click on the number's Reveal: {texts:?}"
        );
    }

    /// **Which flag feeds which row.** `RevealState`'s doc claims the struct
    /// stops two rows sharing one bool -- and it does stop them *sharing* one,
    /// but nothing pinned which field reaches which row until this test. Every
    /// other case in this file sets the two card flags to the same value, so
    /// `&mut reveal.card_number` passed to the Security-code row -- a
    /// one-token slip in `card_rows` -- left the whole suite green while a
    /// click on the number's Reveal unmasked the CVV too.
    ///
    /// Both directions, because one alone only pins that *some* flag reaches
    /// each row.
    #[test]
    fn each_card_secret_is_revealed_only_by_its_own_flag() {
        let number_only = painted_with_reveal(
            &a_full_card(),
            &TotpState::NoSecret,
            RevealState {
                password: false,
                card_number: true,
                card_code: false,
            },
        );
        assert!(
            contains(&number_only, "4242424242424242"),
            "card_number: true did not reveal the number: {number_only:?}"
        );
        assert!(
            !contains(&number_only, "123"),
            "revealing the card NUMBER also unmasked the security code -- the two rows \
             are reading the same flag: {number_only:?}"
        );

        let code_only = painted_with_reveal(
            &a_full_card(),
            &TotpState::NoSecret,
            RevealState {
                password: false,
                card_number: false,
                card_code: true,
            },
        );
        assert!(
            contains(&code_only, "123"),
            "card_code: true did not reveal the security code: {code_only:?}"
        );
        assert!(
            !contains(&code_only, "4242"),
            "revealing the SECURITY CODE also unmasked the number -- the two rows are \
             reading the same flag: {code_only:?}"
        );
    }

    /// Nothing else on these panes is masked -- the requirement is exactly two
    /// fields, so a cardholder name behind bullets would be as wrong as a
    /// number in front of them.
    #[test]
    fn nothing_but_the_number_and_the_code_is_masked_on_a_card() {
        let texts = painted(&a_full_card(), &TotpState::NoSecret);
        for visible in ["John Doe", "visa", "04/2023"] {
            assert!(
                contains(&texts, visible),
                "{visible:?} was masked; only the number and the security code may be: {texts:?}"
            );
        }
    }

    fn a_card_data(number: &str, code: &str) -> CardData {
        CardData {
            cardholder_name: None,
            brand: None,
            number: Some(number.to_string().into()),
            exp_month: None,
            exp_year: None,
            code: Some(code.to_string().into()),
            other: serde_json::Map::new(),
        }
    }

    /// **What is copied is what is shown.** The two card secrets render
    /// through `non_empty`, which trims; `vault_window::mod`'s Copy handler
    /// used to read `card.number`/`card.code` raw off the item, so a number
    /// stored with stray whitespace displayed trimmed and copied untrimmed --
    /// which some payment forms reject. There is now one producer,
    /// [`card_fields`], and both sides take their value from it.
    #[test]
    fn card_fields_trims_the_secrets_the_copy_path_and_the_rows_share() {
        let fields = card_fields(&a_card_data(" 4242424242424242 ", "\t123\n"));
        assert_eq!(fields.number.as_deref(), Some("4242424242424242"));
        assert_eq!(fields.code.as_deref(), Some("123"));
    }

    /// A field that is present but blank is the same as an absent one to a
    /// reader, for the secrets as much as for the plain rows.
    #[test]
    fn card_fields_treats_a_whitespace_only_field_as_absent() {
        let fields = card_fields(&a_card_data("   ", ""));
        assert_eq!(fields.number, None);
        assert_eq!(fields.code, None);
        assert!(fields.is_empty(), "a card of only blanks is not empty: {fields:?}");
    }

    /// **The conjunction, tested rather than assumed.** `card_rows` used to
    /// open-code "is this card empty" beside five per-row `if let Some`
    /// checks; a sixth field added to the rows and not to the check gives a
    /// pane that draws a row *and* says "No card details on this item." One
    /// populated field at a time, each asserted to make the card non-empty:
    /// a check that missed any one of them would call that card empty.
    #[test]
    fn a_card_is_empty_exactly_when_every_field_it_renders_is() {
        assert!(card_fields(&CardData::default()).is_empty());
        let one_field_at_a_time: [(&str, Box<dyn Fn(&mut CardData)>); 5] = [
            ("cardholder", Box::new(|c: &mut CardData| c.cardholder_name = Some("John Doe".into()))),
            ("brand", Box::new(|c: &mut CardData| c.brand = Some("visa".into()))),
            ("number", Box::new(|c: &mut CardData| c.number = Some("4242".to_string().into()))),
            ("expiry month", Box::new(|c: &mut CardData| c.exp_month = Some("04".into()))),
            ("expiry year", Box::new(|c: &mut CardData| c.exp_year = Some("2023".into()))),
            // The security code is the sixth setter and the fifth *field*:
            // `exp_month`/`exp_year` render as one row. Covered below so the
            // array stays a list of independent "this alone is enough" cases.
        ];
        for (name, populate) in one_field_at_a_time {
            let mut data = CardData::default();
            populate(&mut data);
            let fields = card_fields(&data);
            assert!(
                !fields.is_empty(),
                "a card carrying only its {name} was called empty, so that row renders \
                 under a \"No card details on this item.\" note: {fields:?}"
            );
        }
        let mut code_only = CardData::default();
        code_only.code = Some("123".to_string().into());
        assert!(
            !card_fields(&code_only).is_empty(),
            "a card carrying only its security code was called empty"
        );
    }

    /// The render side takes its text from [`card_fields`] and nothing else,
    /// so the trimmed value the copy path hands to the clipboard is character
    /// for character the one on screen.
    #[test]
    fn the_pane_paints_exactly_what_card_fields_returns() {
        let mut item = an_item(Some(3));
        item.card = Some(a_card_data(" 4242424242424242 ", " 123 "));
        let fields = card_fields(item.card.as_ref().expect("just set"));
        let texts = painted_with_reveal(
            &item,
            &TotpState::NoSecret,
            RevealState {
                password: false,
                card_number: true,
                card_code: true,
            },
        );
        for value in [fields.number.expect("number"), fields.code.expect("code")] {
            assert!(
                texts.iter().any(|t| *t == value),
                "the pane painted something other than what card_fields returned for \
                 {value:?}: {texts:?}"
            );
        }
    }

    /// A `type: 3` with no `card` object is an *empty card*, not an unknown
    /// type (the spec's own words). It still gets the heading -- and a line of
    /// body text, because an empty box under a heading reads as a card whose
    /// contents failed to load.
    #[test]
    fn a_card_with_no_card_object_says_it_is_empty_rather_than_drawing_a_blank_box() {
        let texts = painted(&an_item(Some(3)), &TotpState::NoSecret);
        assert!(contains(&texts, "CARD DETAILS"), "{texts:?}");
        assert!(
            contains(&texts, "No card details"),
            "an empty card drew a heading over nothing: {texts:?}"
        );
        for absent in ["Cardholder name", "Number", "Security code", "Reveal"] {
            assert!(
                !contains(&texts, absent),
                "an empty card drew a {absent:?} row it has no data for: {texts:?}"
            );
        }
    }

    #[test]
    fn the_identity_pane_paints_its_groups_and_rows() {
        let mut item = an_item(Some(4));
        item.identity = Some(crate::vault_bridge::IdentityData {
            first_name: Some("Ada".to_string()),
            last_name: Some("Lovelace".to_string()),
            email: Some("ada@example.com".to_string()),
            passport_number: Some("P123".to_string()),
            ..Default::default()
        });
        let texts = painted(&item, &TotpState::NoSecret);
        for label in [
            "IDENTITY",
            "Name",
            "First name",
            "Ada",
            "Last name",
            "Lovelace",
            "Contact",
            "Email",
            "ada@example.com",
            "Government IDs",
            "Passport number",
            "P123",
        ] {
            assert!(contains(&texts, label), "the identity pane painted no {label:?}: {texts:?}");
        }
        // An empty group must not render its heading either.
        assert!(
            !contains(&texts, "Address"),
            "an identity with no address still drew the Address group: {texts:?}"
        );
    }

    /// Eighteen fields, none populated: eighteen blank labels is the failure
    /// mode the suppression rule exists to prevent, and the group headings are
    /// half of it.
    #[test]
    fn an_empty_identity_paints_no_group_headings() {
        let mut item = an_item(Some(4));
        item.identity = Some(crate::vault_bridge::IdentityData::default());
        let texts = painted(&item, &TotpState::NoSecret);
        assert!(contains(&texts, "IDENTITY"), "{texts:?}");
        for heading in ["Name", "Contact", "Address", "Government IDs"] {
            assert!(
                !contains(&texts, heading),
                "an empty identity drew the {heading:?} group heading: {texts:?}"
            );
        }
        assert!(
            contains(&texts, "No identity details"),
            "an empty identity drew a heading over nothing: {texts:?}"
        );
    }

    /// The plan's test, plus the case the plan got wrong. Bitwarden's own
    /// `item.card` template sends `expMonth: "04"` -- already padded -- so a
    /// formatter that only handles the unpadded case would produce `"004"`.
    #[test]
    fn card_expiry_renders_whatever_half_is_present() {
        let t = |m, y| card_expiry_text(m, y);
        assert_eq!(t(Some("3"), Some("2028")), Some("03/2028".to_string()));
        assert_eq!(t(Some("11"), Some("2028")), Some("11/2028".to_string()));
        // The verified capture's own shape: already zero-padded, and it must
        // not be padded a second time.
        assert_eq!(t(Some("04"), Some("2023")), Some("04/2023".to_string()));
        // Either half may be absent, and a half-formed "03/" reads as data
        // loss rather than as a partially-filled item.
        assert_eq!(t(Some("3"), None), Some("03".to_string()));
        assert_eq!(t(None, Some("2028")), Some("2028".to_string()));
        assert_eq!(t(None, None), None);
        assert_eq!(t(Some(""), Some("")), None);
        assert_eq!(t(Some("  "), Some(" ")), None);
        assert_eq!(t(Some(" 4 "), Some(" 2028 ")), Some("04/2028".to_string()));
        // Not a month: show it rather than swallowing it.
        assert_eq!(t(Some("xx"), Some("2028")), Some("xx/2028".to_string()));
        assert_eq!(t(Some("13"), Some("2028")), Some("13/2028".to_string()));
        assert_eq!(t(Some("0"), Some("2028")), Some("0/2028".to_string()));
    }

    #[test]
    fn identity_groups_hide_empty_fields_and_empty_groups() {
        let identity = crate::vault_bridge::IdentityData {
            first_name: Some("Ada".to_string()),
            last_name: Some("Lovelace".to_string()),
            email: Some("ada@example.com".to_string()),
            ..Default::default()
        };
        let groups = identity_groups(&identity);
        let names: Vec<&str> = groups.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, vec!["Name", "Contact"], "an empty group was rendered");

        let name_fields: Vec<&str> = groups[0].1.iter().map(|(l, _)| *l).collect();
        assert_eq!(name_fields, vec!["First name", "Last name"]);
    }

    #[test]
    fn an_entirely_empty_identity_renders_no_groups() {
        assert!(identity_groups(&crate::vault_bridge::IdentityData::default()).is_empty());
    }

    #[test]
    fn whitespace_only_identity_fields_count_as_empty() {
        let identity = crate::vault_bridge::IdentityData {
            company: Some("   ".to_string()),
            ..Default::default()
        };
        assert!(identity_groups(&identity).is_empty());
    }

    /// `address3` is absent from Bitwarden's captured template but present in
    /// its documented schema, so it is modelled on purpose. If a real item
    /// carries it, the pane must show it rather than hide it in `other`.
    #[test]
    fn address3_is_in_the_address_group() {
        let identity = crate::vault_bridge::IdentityData {
            address3: Some("Flat 3".to_string()),
            ..Default::default()
        };
        let groups = identity_groups(&identity);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, "Address");
        assert_eq!(groups[0].1, vec![("Address 3", "Flat 3".to_string())]);
    }

    #[test]
    fn metadata_line_pluralizes_fill_count() {
        assert_eq!(
            metadata_line(Some(3), 41, "Tr0ub4dor&3xtraLong!"),
            "Updated 3 days ago \u{b7} Filled 41 times \u{b7} Strength: Strong"
        );
        assert_eq!(
            metadata_line(Some(1), 1, "weak"),
            "Updated 1 day ago \u{b7} Filled 1 time \u{b7} Strength: Weak"
        );
    }

    #[test]
    fn metadata_line_handles_missing_update_date() {
        assert_eq!(
            metadata_line(None, 0, ""),
            "Updated recently \u{b7} Filled 0 times \u{b7} Strength: Weak"
        );
    }

    #[test]
    fn metadata_line_handles_today() {
        assert_eq!(
            metadata_line(Some(0), 5, "abc"),
            "Updated today \u{b7} Filled 5 times \u{b7} Strength: Weak"
        );
    }
}

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

use super::sidebar::OutOfVault;
use crate::password_strength;
use crate::theme;
use crate::vault_bridge::{
    password_history, CardData, IdentityData, ItemKind, PasswordHistoryEntry, SshKeyData, VaultItem,
};
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke};

// ---------------------------------------------------------------------------
// Design 2b's detail-pane metrics, read off the block marked `2b` in
// `docs/design/Deskwarden.dc.html`. NOT `3f`, which is the macOS variant of
// the same pane and differs in its shortcut glyphs.
// ---------------------------------------------------------------------------

/// The white header strip's `padding: 20px 24px`.
const HEADER_PAD_X: i8 = 24;
const HEADER_PAD_Y: i8 = 20;
/// `gap: 14px` between the strip's avatar, title column and buttons.
const HEADER_GAP: f32 = 14.0;
/// The strip's `width: 44px; height: 44px` avatar tile.
const HEADER_AVATAR: f32 = 44.0;
/// `font-size: 22px` on the item title.
const TITLE_SIZE: f32 = 22.0;
/// `gap: 3px` between the title and its subtitle.
const TITLE_GAP: f32 = 3.0;
/// **The floor under the item title, and the thing the rest of the strip
/// gives way to.** Not from the design: 2b draws this pane at one width and
/// says nothing about what happens below it.
///
/// A truncating title has no natural minimum -- egui will happily elide it to
/// a single "…" and report that as a fitted layout -- so *something* has to
/// state one, and the number is the point below which the title stops being a
/// name. 120pt measures out at eight to ten characters of 22pt ExtraBold plus
/// the ellipsis: enough of a name to recognise, not a mark. See
/// [`header_layout`] for what is sacrificed to keep it, and
/// `the_title_is_never_reduced_to_an_ellipsis_at_any_width` for the sweep
/// that holds it to this.
const TITLE_MIN: f32 = 120.0;
/// The gap between the two lines of the header when it stacks -- see
/// [`header_layout`]. Not [`HEADER_GAP`]: that one is 2b's horizontal `gap`
/// inside a row the design draws, and this is the vertical space in a
/// rearrangement the design does not draw at all.
const HEADER_ROW_GAP: f32 = 10.0;
/// The primary action's label and its chord, named once so the widths
/// [`header_layout`] reserves are measured from the very strings that are
/// later painted.
const FILL_LABEL: &str = "Fill in app";
const FILL_HINT: &str = "CTRL+SHIFT+F";
/// The body below the strip: `padding: 18px 24px`.
const BODY_PAD_X: i8 = 24;
const BODY_PAD_Y: i8 = 18;
/// `gap: 14px` between the body's cards.
const CARD_GAP: f32 = 14.0;
/// A card's `padding: 11px 16px` heading and `padding: 13px 16px` rows -- one
/// horizontal padding, two vertical ones.
const CARD_PAD_X: i8 = 16;
const CARD_HEADING_PAD_Y: i8 = 11;
const ROW_PAD_Y: i8 = 13;
/// `font-size: 12px; font-weight: 700; letter-spacing: 0.06em` on a card's
/// heading, in points (0.06em x 12px).
const CARD_HEADING_SIZE: f32 = 12.0;
const CARD_HEADING_TRACKING: f32 = 0.72;
/// `gap: 16px` between a row's label column, its value and its controls.
const ROW_GAP: f32 = 16.0;
/// `gap: 8px` between two controls at one row's right-hand end.
const CONTROL_GAP: f32 = 8.0;
/// A row's content band -- the design's own `height: 28px` control, which is
/// the tallest thing an ordinary row contains. See [`row`] for why the band
/// has to be stated rather than left to grow.
const ROW_CONTENT_HEIGHT: f32 = 28.0;
/// The row label column's `width: 130px`, and its `font-size: 12px`.
const ROW_LABEL_WIDTH: f32 = 130.0;
const ROW_LABEL_SIZE: f32 = 12.0;
/// `font-size: 14px` on a row's value.
const ROW_VALUE_SIZE: f32 = 14.0;
/// A masked value: `font-size: 15px; letter-spacing: 0.08em` in monospace.
const MASKED_SIZE: f32 = 15.0;
const MASKED_TRACKING: f32 = 1.2;
/// How many bullets a masked value draws, **whatever its real length**.
///
/// It was `value.chars().count().max(8)` -- one bullet per character -- and
/// that is wrong twice over.
///
/// It **breaks the row**: an SSH private key is ~94 characters, so the value
/// column claimed the whole row and pushed the Copy and Reveal controls off
/// the pane. The pane then showed a masked private key with no way to reveal
/// or copy it, which is the entire point of an SSH key item.
/// `the_ssh_private_key_is_not_painted_by_default` catches it now; nothing
/// did before, because the only masked values that existed were a card
/// number and a security code, both short enough to fit.
///
/// It also **published the secret's exact character count** to anyone
/// glancing at the screen -- a password's length is not the password, but it
/// is not nothing either, and there is no reason to draw it.
const MASKED_BULLETS: usize = 10;
/// A live one-time code: `font-size: 17px; letter-spacing: 0.12em`, then a
/// `96x4` progress bar and the seconds remaining, `gap: 12px` apart.
const TOTP_CODE_SIZE: f32 = 17.0;
const TOTP_CODE_TRACKING: f32 = 2.04;
const TOTP_GAP: f32 = 12.0;
const TOTP_BAR_WIDTH: f32 = 96.0;
const TOTP_BAR_HEIGHT: f32 = 4.0;
/// The 11px runs the design uses for a row's secondary line (2b's `18s`).
const ROW_HINT_SIZE: f32 = 11.0;

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
    /// ~10s (the bridge's whole-request `READ_DEADLINE`) if a *different*
    /// item's poll is still outstanding and holding the one-poll-at-a-time
    /// gate. Distinct from
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
    /// One flag per PREVIOUS PASSWORDS row, indexed the same way
    /// [`crate::vault_bridge::password_history`] orders its entries.
    ///
    /// **An array rather than a single `bool`**, for the reason this struct
    /// exists at all: a shared flag would reveal every previous password at
    /// once, which is the same defect as two card rows sharing one bool and
    /// worse, because there can be five of them.
    ///
    /// It is a fixed-size array rather than a `Vec` so [`RevealState`] stays
    /// `Copy` -- `vault_window::mod` owns one by value and resets it by
    /// assignment, and making it non-`Copy` would change a file this work
    /// does not own. The length is [`MAX_HISTORY_ROWS`], which is above the
    /// server's own cap; a history longer than that is truncated *visibly*
    /// rather than silently -- see [`history_rows`].
    pub password_history: [bool; MAX_HISTORY_ROWS],
    /// An SSH key's private-key row.
    ///
    /// The fourth flag, and the one the struct's "adding a fourth masked row
    /// cannot quietly reuse another row's flag" paragraph above was written
    /// in anticipation of. It costs `vault_window::mod` nothing: `run`
    /// constructs this through `RevealState::default()` and resets it the
    /// same way, so the field is added and cleared without that file
    /// changing.
    pub ssh_private_key: bool,
}

/// How many PREVIOUS PASSWORDS rows the pane will draw.
///
/// **Eight, against a server-side cap of five.** Bitwarden's own
/// `CipherService.adjustPasswordHistoryLength` slices every save down to the
/// last five entries, so five is what an item can actually hold; eight leaves
/// room without pretending this array can hold an unbounded history. When
/// more than this arrive the pane says how many it is not showing rather than
/// dropping them quietly -- an omitted previous password looks exactly like a
/// password the user never had.
pub const MAX_HISTORY_ROWS: usize = 8;

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
    /// An SSH key's private key was copied. Named rather than carrying the
    /// value, for exactly the reason [`Self::CopyCardNumber`] is: the private
    /// key is `Zeroizing<String>` on the item, and [`Self::CopyValue`] is
    /// documented as carrying only values that are *not* `Zeroizing` in the
    /// model. Routing it through that variant would give the plaintext a
    /// second, non-zeroizing home inside this enum.
    CopySshPrivateKey,
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
    /// The header's favourite control was clicked, carrying **the state the
    /// item should end up in**, not "it was clicked".
    ///
    /// Carrying the target rather than a bare toggle is what keeps the pane
    /// and the caller from disagreeing: the pane already read
    /// `item.favorite` to decide which label to draw, so it is the one that
    /// knows what the other state is. A bare `ToggleFavorite` would have
    /// `vault_window::mod` re-derive `!item.favorite` from its own copy of
    /// the item, and the two copies are not guaranteed equal -- that is the
    /// same re-derivation `move_item_to_folder`'s doc rejects one field over.
    ///
    /// It is deliberately **not** routed through the edit draft:
    /// `detail_edit::EditDraft::apply_to` clones the item and overwrites only
    /// the fields the form owns, and `favorite` is not one of them, so a
    /// favourite that went through a draft would be silently dropped on save.
    /// See `vault_bridge::with_favorite`.
    ToggleFavorite(bool),
    /// One PREVIOUS PASSWORDS row's Copy was clicked, identified by its
    /// **index** into `vault_bridge::password_history(item)`.
    ///
    /// The index and not the value, for exactly the reason
    /// [`Self::CopyCardNumber`] carries neither: the caller already holds the
    /// item and can read the `Zeroizing<String>` back out of it, so a
    /// previous password -- which is a password -- never gets a second,
    /// non-zeroizing home inside this enum. [`Self::CopyValue`]'s door is for
    /// non-secrets and this must not use it.
    CopyPasswordHistory(usize),
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
    /// The SSH KEY card: public key, fingerprint, and the private key behind
    /// a mask.
    SshKey,
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
/// it would render whatever sat next to it.
///
/// That arm used to be an [`UnsupportedPane`], because `VaultItem` carried no
/// `ssh_key` field: type 5's wire shape was the one this repo could not
/// verify, and modelling it from memory is how a modelled field and its
/// `other` copy start disagreeing. The shape is now captured and modelled
/// (see [`SshKeyData`] and `.superpowers/sdd/item-shapes-capture.md`), so
/// `Unsupported` would now be the dishonest answer -- the data is there.
/// `Unknown` keeps its unsupported pane, and the two are no longer the same
/// situation at all.
pub fn detail_body_for(kind: ItemKind) -> DetailBody {
    match kind {
        ItemKind::Login => DetailBody::LoginCredentials,
        ItemKind::SecureNote => DetailBody::NotesOnly,
        ItemKind::Card => DetailBody::Card,
        ItemKind::Identity => DetailBody::Identity,
        ItemKind::SshKey => DetailBody::SshKey,
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

/// Every value the SSH key pane shows, emptiness-suppressed once.
///
/// The same shape as [`CardFields`], and it exists for the same two findings.
/// It is the *only* producer of an SSH key's displayed text, so what
/// `vault_window::mod`'s Copy handler puts on the clipboard is character for
/// character what the pane painted -- the trimming divergence that was found
/// on the card's number. And [`Self::is_empty`] is the one emptiness rule,
/// destructured so a fourth field is a compile error here rather than a row
/// that renders underneath a "No SSH key details on this item." note.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SshKeyFields {
    pub public_key: Option<String>,
    pub fingerprint: Option<String>,
    /// Masked until revealed. Plain `String` rather than `Zeroizing`: this is
    /// the same already-formatted copy the pane paints, and the wrapped field
    /// on the item is untouched -- exactly the position [`CardFields::number`]
    /// records, and the module's recorded "zeroize is leaky beyond the wrapped
    /// fields" deferral covers.
    pub private_key: Option<String>,
}

impl SshKeyFields {
    /// True when this SSH key has nothing at all to draw. See
    /// [`CardFields::is_empty`], which this mirrors deliberately.
    pub fn is_empty(&self) -> bool {
        let Self {
            public_key,
            fingerprint,
            private_key,
        } = self;
        public_key.is_none() && fingerprint.is_none() && private_key.is_none()
    }
}

/// See [`SshKeyFields`].
pub fn ssh_key_fields(data: &SshKeyData) -> SshKeyFields {
    SshKeyFields {
        public_key: non_empty(data.public_key.as_deref()).map(str::to_string),
        fingerprint: non_empty(data.key_fingerprint.as_deref()).map(str::to_string),
        private_key: non_empty(data.private_key.as_deref().map(|k| k.as_str())).map(str::to_string),
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

/// What the detail pane says about an item that is in the Trash or the
/// Archive: the heading, and the sentence explaining what can be done with
/// it.
///
/// A pure function returning the strings, so the wording is testable without
/// a frame and so the pane below cannot say one thing while a test asserts
/// another. Takes [`OutOfVault`], which has no "live" variant, so there is no
/// case here to get wrong for an ordinary item.
pub fn out_of_vault_text(out: OutOfVault) -> (&'static str, &'static str) {
    match out {
        OutOfVault::Trash => (
            "This item is in the Trash.",
            "Deskwarden does not edit, fill or copy from a trashed item -- the vault would \
             reject the write and the fill would have nothing to type. Right-click its row to \
             Restore it, or to delete it forever.",
        ),
        OutOfVault::Archive => (
            "This item is archived.",
            "Archived items are kept out of the vault list, out of app matching and out of \
             autofill -- which is what archiving them is for. Right-click its row to Unarchive \
             it, and everything it can normally do comes back.",
        ),
    }
}

/// The detail pane for an item that is not in the live vault.
///
/// **Deliberately not [`draw_detail_read`] with its buttons hidden.** Every
/// action that pane offers reads or writes through the live item list, which
/// by definition does not hold this item, so each one would be a control that
/// quietly did nothing -- the failure this window keeps having to un-write.
/// This pane has no controls at all: it states which of the two places the
/// item is in and where the action that works lives.
///
/// It paints the same surface and the same header strip as the read pane, so
/// the column does not change shape as the user moves between rows.
pub fn draw_out_of_vault_read(ui: &mut egui::Ui, item: &VaultItem, out: OutOfVault) {
    let (heading, body) = out_of_vault_text(out);
    let pane = ui.clip_rect();
    ui.painter()
        .rect_filled(pane, CornerRadius::ZERO, theme::WINDOW_BG);
    let mut pane_ui = ui.new_child(egui::UiBuilder::new().max_rect(pane));
    let ui = &mut pane_ui;
    ui.spacing_mut().item_spacing = egui::Vec2::ZERO;

    egui::Frame::new()
        .fill(theme::CARD)
        .inner_margin(Margin::symmetric(HEADER_PAD_X, HEADER_PAD_Y))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(&item.name)
                    .size(TITLE_SIZE)
                    .family(egui::FontFamily::Name(theme::BOLD.into()))
                    .color(theme::INK),
            );
            ui.add_space(6.0);
            ui.label(RichText::new(heading).size(12.0).color(theme::TEXT_MUTED));
        });
    ui.add_space(18.0);
    ui.label(RichText::new(body).size(13.0).color(theme::TEXT_FAINT));
}

/// The three values this pane offers a keyboard copy for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyShortcut {
    Username,
    Password,
    Totp,
}

/// The bindings, their keys and the hint each row paints, in ONE table.
///
/// One table because the third of those is a promise about the other two: a
/// row advertising `CTRL+B` beside a handler wired to something else is
/// worse than no hint at all, and the only way to make that impossible is
/// for the hint and the key to be the same tuple.
///
/// The chords are KeePass's, which password-manager users already have in
/// their fingers. All three are free in this app, checked rather than
/// assumed: `vault_window::mod` takes CTRL+K, CTRL+L and CTRL+N, the fill
/// button takes CTRL+SHIFT+F, the login window takes CTRL+H, and the global
/// fill hotkey is CTRL+ALT+B.
///
/// **CTRL+ALT+B not colliding with CTRL+B is a fact about Windows, not about
/// egui.** An earlier version of this comment claimed the two were "a
/// different chord ... owned by the OS rather than by egui" as if egui would
/// tell them apart. It does not: `InputState::consume_key` matches with
/// `Modifiers::matches_logically`, which rejects an event only when the
/// *pattern* wants a modifier the event lacks, so extra alt and shift are
/// ignored and CTRL+ALT+B, CTRL+SHIFT+B and CTRL+B were all one chord as far
/// as this pane was concerned. What actually kept the global hotkey out of
/// here is `global-hotkey` registering it through Win32 `RegisterHotKey`,
/// which makes Windows swallow the keystroke instead of delivering it to the
/// focused window -- a guarantee that covers exactly that one chord and
/// nothing else. CTRL+SHIFT+B put a password on the clipboard. See
/// [`consume_ctrl_key`], which now gates these on exact modifiers, and
/// `an_extra_modifier_does_not_fire_a_copy`.
const COPY_SHORTCUTS: [(CopyShortcut, egui::Key, &str); 3] = [
    (CopyShortcut::Password, egui::Key::B, "CTRL+B"),
    (CopyShortcut::Username, egui::Key::U, "CTRL+U"),
    (CopyShortcut::Totp, egui::Key::T, "CTRL+T"),
];

/// The hint text for one binding, read out of [`COPY_SHORTCUTS`] and never
/// written out a second time.
fn copy_shortcut_hint(which: CopyShortcut) -> &'static str {
    COPY_SHORTCUTS
        .iter()
        .find(|(candidate, _, _)| *candidate == which)
        .map(|(_, _, hint)| *hint)
        .expect("COPY_SHORTCUTS covers every CopyShortcut variant")
}

/// Which copy a chord asks for, given what the selected item actually
/// carries -- or `None`, meaning the chord does nothing at all.
///
/// **`None` rather than a fallback is the whole point.** The clipboard is a
/// global the user is about to paste somewhere, and both of the obvious
/// wrong answers are silent: an empty string looks like a failed paste, and
/// "copy the password because there is no username" hands over a secret
/// nobody asked for. So each binding copies its own field or nothing.
///
/// Pure, and separate from the closure that calls it, for this codebase's
/// standing reason: logic reachable only through an eframe closure is logic
/// that will not be tested.
fn copy_shortcut_action(
    which: CopyShortcut,
    username: &str,
    password: &str,
    totp: &TotpState,
) -> Option<DetailAction> {
    match which {
        CopyShortcut::Username => (!username.is_empty()).then_some(DetailAction::CopyUsername),
        CopyShortcut::Password => (!password.is_empty()).then_some(DetailAction::CopyPassword),
        // Only when a code is really on screen. `vault_window::mod` resolves
        // `CopyTotp` out of this same state, so every other variant --
        // `NoSecret`, `Fetching`, `Unavailable`, `NoCodeReported` -- would
        // have it copy nothing, or an empty string, without this gate.
        CopyShortcut::Totp => {
            matches!(totp, TotpState::Code { .. }).then_some(DetailAction::CopyTotp)
        }
    }
}

/// How the header strip arranges itself at a given width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HeaderLayout {
    /// The controls have moved off the title's line onto their own, right
    /// aligned underneath it. The strip gets taller; nothing is dropped.
    stacked: bool,
    /// "Fill in app" still carries its `CTRL+SHIFT+F` hint.
    hint: bool,
}

/// **What the header gives up, in order, as the pane narrows -- and it is
/// never the title.**
///
/// The commit that made the controls claim their width first fixed a real
/// bug (a "Fill in app" measured painting at x = -34.5, entirely off a 298pt
/// pane) and introduced its mirror image: with the controls served first the
/// title got the remainder, the remainder at 298pt was 21.6pt, and egui
/// elided a 26-character name down to a lone "…" painted *inside the strip's
/// left padding, on top of the avatar*. A layout that fits by annihilating
/// its own subject is not a layout that fits, and the test of the day could
/// not see it: `Galley::text()` returns the job's SOURCE text, so every
/// painted-text assertion in this file reads the full name off a galley that
/// drew one ellipsis.
///
/// So the title is given a floor ([`TITLE_MIN`]) and everything else is
/// ranked by how much it costs to lose:
///
/// 1. **The shortcut hint.** Pure redundancy -- twelve monospace characters
///    annotating a chord that works whether or not they are painted, and
///    wider than the label they annotate. First to go, and it does not come
///    back on the way down: a strip that regained ornament as the window
///    shrank would be harder to reason about than one that only ever sheds.
/// 2. **The single line.** Below that the controls move to their own row
///    under the title. The strip gets taller, which the design does not draw
///    -- but the design also does not draw a 298pt pane, and a taller strip
///    costs some body space while the alternative costs the item's name.
///
/// Nothing is ever dropped, and no control is ever shrunk below its 34px hit
/// target. The arithmetic says why there is no third option: at 298pt the
/// strip has 250pt inside its padding, the avatar and its gap take 58, and
/// the star, the kebab and a hintless "Fill in app" take about 188 of the
/// 192 left. Even reduced to icons the three controls plus their gaps leave
/// the title 0-ish points on one line. One row at that width means dropping
/// a control; stacking keeps all three, worded.
///
/// Pure, and taking measured widths rather than measuring them, for this
/// file's standing reason: a decision reachable only from inside an eframe
/// closure is a decision that will not be tested.
fn header_layout(content_width: f32, controls_with_hint: f32, controls_bare: f32) -> HeaderLayout {
    // What is left of the strip's content box once the avatar and the gap
    // after it are taken -- the band the controls and the title share.
    let beside_avatar = content_width - HEADER_AVATAR - HEADER_GAP;
    let fits_on_one_line =
        |controls: f32| controls + HEADER_GAP + TITLE_MIN <= beside_avatar;
    if fits_on_one_line(controls_with_hint) {
        HeaderLayout { stacked: false, hint: true }
    } else if fits_on_one_line(controls_bare) {
        HeaderLayout { stacked: false, hint: false }
    } else {
        // Stacked, and hintless: the ladder only descends. The controls' own
        // row is the full content width rather than `beside_avatar`, so it
        // has 58pt more to work with than the branch above just rejected.
        HeaderLayout { stacked: true, hint: false }
    }
}

/// `CTRL+key` and **nothing else held**, taken out of the event queue.
///
/// egui's own [`egui::InputState::consume_key`] is the obvious call here and
/// is wrong for this one: it matches with `Modifiers::matches_logically`,
/// which only rejects an event for lacking a modifier the pattern asked for.
/// Extra modifiers are ignored, so CTRL+ALT+B and CTRL+SHIFT+B both fired
/// CTRL+B -- and what these three chords do is put a secret on the clipboard.
/// Today nothing else in this app binds `CTRL+<any>+B|U|T`, so the only
/// visible symptom was a stray modifier copying a password the user did not
/// ask for; the moment something does bind one, the same keystroke would run
/// two commands, one of them silent.
///
/// `matches_exact` instead, and the retain-and-consume written out here
/// because egui exposes no exact-matching consumer. Consuming is still the
/// point (see the call site): a chord that means "copy" here must not also
/// reach whatever is underneath.
fn consume_ctrl_key(input: &mut egui::InputState, key: egui::Key) -> bool {
    let mut found = false;
    input.events.retain(|event| {
        let is_match = matches!(
            event,
            egui::Event::Key {
                key: event_key,
                modifiers,
                pressed: true,
                ..
            } if *event_key == key && modifiers.matches_exact(egui::Modifiers::CTRL)
        );
        found |= is_match;
        !is_match
    });
    found
}

pub fn draw_detail_read(
    ui: &mut egui::Ui,
    item: &VaultItem,
    fill_count: u32,
    totp: &TotpState,
    // Whether *this* item currently has a delete armed (its first click
    // already happened and the confirm window hasn't expired) -- purely for
    // what the kebab and its Delete entry show; `vault_window::mod`'s
    // `confirm_click` is what actually decides whether a click here is
    // arming or confirming.
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

    // **Consumed before anything below is drawn.** `consume_key` takes the
    // event out of the queue, so a chord that means "copy" here cannot also
    // reach a text field or a button underneath. What it resolves to is
    // `copy_shortcut_action`'s decision and not this closure's; applied at
    // the very end of this function so a click in the same frame -- which is
    // a deliberate act on a specific row -- wins over a keystroke.
    let shortcut = ui.input_mut(|i| {
        COPY_SHORTCUTS
            .iter()
            .find(|(_, key, _)| consume_ctrl_key(i, *key))
            .map(|(which, _, _)| *which)
    });

    // Whatever stacking the container handed us, kept for the body below --
    // the cards there still lay themselves out with it.
    let card_spacing = ui.spacing().item_spacing;

    // **The pane owns its own surface.** Design 2b fills the detail column
    // with `#f7f6f5` and puts exactly one white element on it: the header
    // strip across the top, edge to edge. `ui.clip_rect()` is that column --
    // `vault_window::mod`'s `CentralPanel` hands this function a `max_rect`
    // already inset by the panel's own margin, and a strip drawn inside that
    // inset reads as a card floating on grey rather than as the pane's own
    // header. Laying out in a child over the clip rect also makes every
    // number below independent of whatever padding the container supplies,
    // which is what lets the tests pin absolute geometry.
    let pane = ui.clip_rect();
    ui.painter()
        .rect_filled(pane, CornerRadius::ZERO, theme::WINDOW_BG);
    let mut pane_ui = ui.new_child(egui::UiBuilder::new().max_rect(pane));
    let ui = &mut pane_ui;
    // Every gap in the strip and the body below is stated, not inherited.
    ui.spacing_mut().item_spacing = egui::Vec2::ZERO;

    egui::Frame::new()
        .fill(theme::CARD)
        .inner_margin(Margin::symmetric(HEADER_PAD_X, HEADER_PAD_Y))
        .show(ui, |ui| {
            let content_width = ui.available_width();
            ui.set_width(content_width);

            // **Measured before anything is drawn, and drawn the way the
            // measurement said.** `header_primary_button_width` lays the very
            // galleys the button will paint, so the room reserved here and
            // the room taken below cannot drift apart -- which is the failure
            // mode that put this control off the edge of the pane once
            // already.
            //
            // A kind that offers no fill contributes neither the button nor
            // the gap before it, so the strip does not reserve space for a
            // control it will not draw.
            let controls_width = |hint: Option<&str>| {
                let fill = if kind_offers_fill(kind) {
                    HEADER_GAP + theme::header_primary_button_width(ui, FILL_LABEL, hint)
                } else {
                    0.0
                };
                // The star and the kebab, square at the strip's own control
                // height, plus the one gap between them.
                theme::HEADER_BUTTON_HEIGHT * 2.0 + HEADER_GAP + fill
            };
            let layout = header_layout(
                content_width,
                controls_width(Some(FILL_HINT)),
                controls_width(None),
            );

            // The three pieces of the strip, as closures, because the two
            // arrangements below draw exactly the same controls in exactly
            // the same order -- only on a different number of lines. Written
            // out twice they would be two headers that have to be kept
            // agreeing by hand.
            let draw_avatar = |ui: &mut egui::Ui| match icon {
                Some(tex) => {
                    // Rounded to match `theme::avatar`'s initials-tile
                    // treatment (same `size * 0.25` formula) -- see
                    // `item_list.rs`'s matching fix for why an unrounded
                    // favicon in an identical box reads as visually
                    // heavier than the monogram fallback.
                    ui.add(
                        egui::Image::new((tex.id(), tex.size_vec2()))
                            .fit_to_exact_size(egui::Vec2::splat(HEADER_AVATAR))
                            .corner_radius(CornerRadius::same((HEADER_AVATAR * 0.25) as u8)),
                    );
                }
                None => theme::avatar(ui, &theme::initials(&item.name), HEADER_AVATAR, true),
            };
            // The title column, in whatever it has been left. It TRUNCATES
            // rather than wrapping: the design draws one line, and a name
            // long enough to wrap would push the 44px avatar off its own
            // 20px top padding and make the strip's height a function of the
            // item's name. `header_layout` is what guarantees the width it
            // truncates at is at least `TITLE_MIN` -- truncation with no
            // floor under it is how this ended up painting a lone ellipsis.
            let draw_title = |ui: &mut egui::Ui| {
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = TITLE_GAP;
                        let mut title = theme::pane_title(&item.name, TITLE_SIZE, theme::INK);
                        title.wrap =
                            egui::text::TextWrapping::truncate_at_width(ui.available_width());
                        ui.label(title);
                        ui.label(RichText::new(kind.label()).size(12.0).color(theme::TEXT_FAINT));
                    });
                });
            };
            // **Right-to-left, so what reads left-to-right is ★, "Fill in
            // app", ⋮.** The caller supplies a `Ui` already in that layout
            // and already carrying `HEADER_GAP` spacing; this only fills it.
            let mut draw_controls = |ui: &mut egui::Ui| {
                // **The overflow menu, rightmost.** Edit and Delete both
                // live in here, at the user's explicit direction: four
                // worded buttons across this strip did not fit the app's
                // own minimum window size.
                //
                // Drawn ALWAYS, not gated: Delete means the same thing for
                // every kind (see `every_kind_can_still_be_deleted`), so
                // there is no kind whose menu would be empty. Edit inside
                // it is still `kind_offers_edit`'s decision.
                let kebab = theme::kebab_button(ui, delete_pending)
                    .on_hover_text("More actions for this item");
                egui::Popup::menu(&kebab).show(|ui| {
                    if kind_offers_edit(kind) && ui.button("Edit").clicked() {
                        action = DetailAction::Edit;
                        ui.close();
                    }
                    // **Still two clicks, and the menu stays open between
                    // them.** Burying Delete does not remove the reason
                    // `vault_window::mod`'s `confirm_click` gates it: one
                    // misclick permanently deletes. So the armed state is
                    // expressed exactly as the header button expressed it
                    // -- the same two labels, the same `ERROR` red -- and
                    // no `ui.close()` on the arming click, because a menu
                    // that shut itself would hide the state it just
                    // entered. The kebab itself also turns red (see
                    // `kebab_button`) so an armed delete is visible after
                    // a click elsewhere closes the menu.
                    let (delete_label, delete_hover) = if delete_pending {
                        (
                            "Delete? Click to confirm",
                            "Click again to delete this item. It may still be recoverable \
                             from bitwarden.com or another Bitwarden client afterward.",
                        )
                    } else {
                        ("Delete", "Delete this item")
                    };
                    let delete = ui.add(
                        egui::Button::new(RichText::new(delete_label).color(theme::ERROR))
                            .fill(theme::CARD),
                    );
                    if delete.on_hover_text(delete_hover).clicked() {
                        action = DetailAction::Delete;
                    }
                });
                // **Stays in the strip, and stays worded.** It is this
                // app's primary action, not one of the items the user
                // asked to have relocated into the kebab. What it gives up
                // when the pane is narrow is its shortcut hint and then
                // its line, never its label -- see `header_layout`.
                //
                // Not drawn for a kind that cannot be filled: the fill
                // path resolves exactly a username and a password, so this
                // button on a card would type two empty strings into
                // whatever window happens to be focused. See
                // `kind_offers_fill`.
                if kind_offers_fill(kind)
                    && theme::header_primary_button(ui, FILL_LABEL, layout.hint.then_some(FILL_HINT))
                        .clicked()
                {
                    action = DetailAction::Fill;
                }
                // **In the header, and gated on no kind at all.** Every
                // other control in this strip is per-kind because it acts
                // on the item's *contents* -- Fill needs a username and a
                // password, Edit needs a form that can honestly save the
                // type. A favourite is not a content field: it is a
                // property of the item as a row in a list, like its name
                // and its folder, and the sidebar's Favorites filter
                // applies it to every kind (`SidebarFilter::Favorites` is
                // `item.favorite` and nothing else). So a card can be a
                // favourite, and gating this the way Fill is gated would
                // make a filter the sidebar offers unreachable for four
                // of the five kinds.
                //
                // The header rather than a body row for the same reason:
                // the body is `detail_body_for`'s per-kind dispatch and a
                // row there would have to be repeated into every arm, and
                // would read as a field of the login/card/identity rather
                // than of the item.
                //
                // A STAR, and drawn rather than typed. The earlier version
                // of this control was two words ("Favourite"/"Favourited")
                // on the argument that a missing glyph renders as tofu --
                // correct about the risk, and `theme.rs`'s
                // `the_icon_codepoints_are_not_carried_by_this_apps_own
                // _typeface` now measures the actual answer: ★ resolves,
                // but only out of egui's fallback icon face, and ★/☆ are
                // two unrelated marks rather than one shape in two
                // weights. `theme::star_toggle` strokes it instead, so
                // both states are the same silhouette and the on state is
                // the palette's own BLUE.
                let favourite_hover = if item.favorite {
                    "Remove this item from Favorites"
                } else {
                    "Add this item to Favorites"
                };
                if theme::star_toggle(ui, item.favorite)
                    .on_hover_text(favourite_hover)
                    .clicked()
                {
                    // The TARGET state, computed here from the item this
                    // pane actually drew -- see `DetailAction::ToggleFavorite`.
                    action = DetailAction::ToggleFavorite(!item.favorite);
                }
            };

            if layout.stacked {
                // Two lines: the item on top, what can be done to it
                // underneath. The controls' row is the full content width,
                // right-aligned, which is where the room comes from that the
                // one-line arrangement did not have.
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    draw_avatar(ui);
                    ui.add_space(HEADER_GAP);
                    draw_title(ui);
                });
                ui.add_space(HEADER_ROW_GAP);
                // A band of exactly one control height, allocated rather than
                // left to `with_layout`: a right-to-left layout takes the
                // whole *available* rect, and the available rect here runs to
                // the bottom of the pane -- which centred the controls
                // halfway down the window and made the white strip the full
                // height of it.
                let band = egui::vec2(ui.available_width(), theme::HEADER_BUTTON_HEIGHT);
                ui.allocate_ui_with_layout(
                    band,
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = HEADER_GAP;
                        draw_controls(ui);
                    },
                );
            } else {
                // One line, and **the controls claim their width first.**
                // Laid out the other way round -- title, then a
                // right-aligned group in the remainder -- the title takes as
                // much room as its text wants and the strip overflows: "Fill
                // in app" was measured painting at x = -34.5, entirely off
                // the pane, under a title that ran straight through the
                // buttons. So the whole rest of the row is one right-to-left
                // group and the title column is a left-to-right child of it,
                // which egui hands exactly the rect the controls did not
                // take. That the rect is big enough to hold a name is
                // `header_layout`'s job, not this layout's.
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    draw_avatar(ui);
                    ui.add_space(HEADER_GAP);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = HEADER_GAP;
                        draw_controls(ui);
                        draw_title(ui);
                    });
                });
            }
        });
    // The strip's `border-bottom: 1px solid #eae7e7`.
    theme::hairline(ui);

    // The body's `padding: 18px 24px`, applied by placing the rest of the pane
    // in a child over the padded remainder rather than by a `Frame`, so
    // everything below keeps laying itself out exactly where it did.
    let body = egui::Rect::from_min_max(
        egui::pos2(
            pane.left() + f32::from(BODY_PAD_X),
            ui.cursor().top() + f32::from(BODY_PAD_Y),
        ),
        egui::pos2(
            pane.right() - f32::from(BODY_PAD_X),
            pane.bottom() - f32::from(BODY_PAD_Y),
        ),
    );
    let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
    let ui = &mut body_ui;
    ui.spacing_mut().item_spacing = card_spacing;

    // Which body this item gets is decided by `detail_body_for` and nowhere
    // else, so "what does a type-5 item render" is a question a unit test
    // asks directly. Exhaustive on purpose -- no catch-all arm -- so a new
    // `DetailBody` variant fails to compile here instead of silently
    // inheriting whatever the last arm happened to draw.
    match detail_body_for(kind) {
        DetailBody::LoginCredentials => {
            card(ui, "LOGIN CREDENTIALS", |ui| {
                credential_row(
                    ui,
                    "Username",
                    username,
                    Some(CopyShortcut::Username),
                    &mut action,
                    DetailAction::CopyUsername,
                );
                theme::row_rule(ui);
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
                    theme::row_rule(ui);
                    match row {
                        TotpRow::Fetching => totp_fetching_row(ui),
                        TotpRow::Code { code, seconds_left } => totp_code_row(ui, code, seconds_left, &mut action),
                        TotpRow::Unavailable => totp_unavailable_row(ui),
                        TotpRow::NoCode => totp_no_code_row(ui),
                    }
                }
            });
            ui.add_space(CARD_GAP);
        }
        // The body is the NOTES card below, and that card is shared with
        // every other kind rather than duplicated here.
        DetailBody::NotesOnly => {}
        DetailBody::Card => {
            card(ui, "CARD DETAILS", |ui| {
                card_rows(ui, item.card.as_ref().map(card_fields), reveal, &mut action);
            });
            ui.add_space(CARD_GAP);
        }
        DetailBody::Identity => {
            card(ui, "IDENTITY", |ui| {
                identity_rows(ui, item.identity.as_ref().map(identity_groups), &mut action);
            });
            ui.add_space(CARD_GAP);
        }
        DetailBody::SshKey => {
            card(ui, "SSH KEY", |ui| {
                ssh_key_rows(ui, item.ssh_key.as_ref().map(ssh_key_fields), reveal, &mut action);
            });
            ui.add_space(10.0);
        }
        DetailBody::Unsupported(pane) => {
            unsupported_card(ui, &pane);
            ui.add_space(CARD_GAP);
        }
    }

    // Directly under the body card, where a login's current password is, and
    // above NOTES. Drawn for ANY kind that has the array rather than gated on
    // `ItemKind::Login`: `passwordHistory` is an item-level key (see the
    // capture) that `bw` puts on every item it sends, so what decides whether
    // the card appears is whether there is anything in it -- the same rule
    // `notes_text` uses. An empty history draws nothing at all; a heading
    // over no rows would read as previous passwords that failed to load.
    let history = password_history(item);
    if !history.is_empty() {
        card(ui, "PREVIOUS PASSWORDS", |ui| {
            history_rows(ui, &history, reveal, &mut action);
        });
        ui.add_space(CARD_GAP);
    }

    if let Some(notes) = notes_text(item) {
        card(ui, "NOTES", |ui| {
            card_text(ui, RichText::new(notes).size(ROW_VALUE_SIZE).color(theme::INK));
        });
        ui.add_space(CARD_GAP);
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
            row(
                ui,
                "Website",
                |ui| {
                    ui.label(RichText::new(website).size(ROW_VALUE_SIZE).color(theme::INK));
                },
                |ui| {
                    if theme::row_button(ui, "Open").clicked() {
                        action = DetailAction::OpenWebsite(website.to_string());
                    }
                },
            );
        });
        ui.add_space(CARD_GAP);
    }

    let updated_days_ago = item
        .other
        .get("revisionDate")
        .and_then(|v| v.as_str())
        .and_then(days_since);
    // The design's last tile: a card like the others, `padding: 13px 16px;
    // font-size: 12px; color: #7d7979`. It used to be a bare line of ghost text
    // on the pane's grey -- the one part of the body that sat on no surface at
    // all.
    egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0, theme::HAIRLINE))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            card_text(
                ui,
                RichText::new(metadata_line_for(kind, updated_days_ago, fill_count, password))
                    .size(ROW_LABEL_SIZE)
                    .color(theme::TEXT_FAINT),
            );
        });

    if matches!(action, DetailAction::None) {
        if let Some(which) = shortcut {
            if let Some(copy) = copy_shortcut_action(which, username, password, totp) {
                action = copy;
            }
        }
    }
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
        card_text(
            ui,
            RichText::new(pane.message.as_str())
                .size(ROW_LABEL_SIZE)
                .color(theme::TEXT_FAINT),
        );
    });
}

/// One of the body's tiles (design 2b: `background: #ffffff; border: 1px solid
/// #eae7e7; border-radius: 10px; overflow: hidden`), with its heading rule
/// (`padding: 11px 16px; border-bottom: 1px solid #eae7e7; 12px/700
/// uppercase`) already drawn.
///
/// The card itself carries **no inner margin**: the design's row separators
/// run edge to edge across the tile, so every padding is the heading's or a
/// row's own and not the card's. `contents` therefore draws [`row`]s (or
/// [`card_text`]), never bare labels.
fn card(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0, theme::HAIRLINE))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            egui::Frame::new()
                .inner_margin(Margin::symmetric(CARD_PAD_X, CARD_HEADING_PAD_Y))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(theme::letterspaced(
                        title,
                        CARD_HEADING_SIZE,
                        theme::BOLD,
                        CARD_HEADING_TRACKING,
                        theme::TEXT_MUTED,
                    ));
                });
            // The heading's `border-bottom`, which is the card's own `#eae7e7`
            // and NOT the lighter rule that goes between rows.
            theme::hairline(ui);
            contents(ui);
        });
}

/// One row of a card (design 2b: `display: flex; align-items: center; gap:
/// 16px; padding: 13px 16px`) -- a fixed `width: 130px` label column, the
/// value taking whatever is left, and the row's controls right-aligned.
///
/// The two-column grid is the visible half of this task: every row on this
/// pane used to stack its label *above* its value, which put nothing in
/// register down the pane and left the values at four different left edges.
fn row(
    ui: &mut egui::Ui,
    label: &str,
    value: impl FnOnce(&mut egui::Ui),
    controls: impl FnOnce(&mut egui::Ui),
) {
    row_impl(ui, label, value, controls, egui::Sense::hover());
}

/// [`row`], plus: **clicking anywhere in the tile copies its value.**
///
/// The user's own words -- "all those username, pass, code, username etc
/// should copy the value on click anywhere within the tile". Only rows that
/// have something to copy get this; a row that reacted to a click and copied
/// nothing would be worse than an inert one, because there is no way to tell
/// from the outside that it did nothing.
///
/// **The eye keeps its own click, and it is the layout that guarantees it,
/// not a rect exclusion.** `row_impl` senses the tile on the *background* of
/// a `Ui`, and egui registers that widget when the `Ui` is created -- before
/// any of its children. The controls inside are therefore registered later,
/// which puts them on top, and egui hands a click to exactly one widget: the
/// topmost under the pointer. So a click on the eye reveals and does not
/// copy, and a click anywhere else in the tile copies. Pinned by
/// `clicking_the_eye_reveals_without_copying`, which asserts BOTH halves --
/// the flag flipped, and no copy reported -- because the negative alone
/// passes against a click that missed everything.
fn copy_row(
    ui: &mut egui::Ui,
    label: &str,
    value: impl FnOnce(&mut egui::Ui),
    controls: impl FnOnce(&mut egui::Ui),
    on_copy: DetailAction,
    action: &mut DetailAction,
) {
    if row_impl(ui, label, value, controls, egui::Sense::click()).clicked() {
        *action = on_copy;
    }
}

fn row_impl(
    ui: &mut egui::Ui,
    label: &str,
    value: impl FnOnce(&mut egui::Ui),
    controls: impl FnOnce(&mut egui::Ui),
    sense: egui::Sense,
) -> egui::Response {
    let clickable = sense == egui::Sense::click();
    let scope = ui.scope_builder(egui::UiBuilder::new().sense(sense), |ui| {
        // The hover tint's slot, reserved BEFORE anything paints into this
        // row: the response that decides whether to fill it only exists once
        // the row has been laid out, and a fill added then would cover the
        // row's own text.
        let tint = ui.painter().add(egui::Shape::Noop);
        row_body(ui, label, value, controls);
        tint
    });
    let response = scope.response;
    if clickable && response.hovered() {
        // The affordance. Design 2b has no hovered-row style of its own --
        // it draws no hover states at all -- so this borrows the tint the
        // design already uses for a raised surface (`CARD_TINT`, the same
        // one `toolbar_button_with_shortcut` and egui's `faint_bg_color`
        // use) rather than inventing a colour. The pointing hand is the
        // other half: this app already gives every hand-painted clickable
        // one, and a tile that copies on click must not look like the text
        // beside it.
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        ui.painter().set(
            scope.inner,
            egui::Shape::rect_filled(response.rect, CornerRadius::ZERO, theme::CARD_TINT),
        );
    }
    response
}

fn row_body(
    ui: &mut egui::Ui,
    label: &str,
    value: impl FnOnce(&mut egui::Ui),
    controls: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::new()
        .inner_margin(Margin::symmetric(CARD_PAD_X, ROW_PAD_Y))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // **`align-items: center` needs a band with a definite height.**
            // In a plain `ui.horizontal`, egui places each child as the row
            // grows, so the first child (a 13pt label) lands at the top and a
            // later 28pt button sits 5pt lower -- two centre lines in one row.
            // Allocating the row's own [`ROW_CONTENT_HEIGHT`] up front gives
            // every child the same band to be centred in. A taller value (the
            // two-line TOTP status rows) still overflows it and the frame
            // grows to fit, exactly as the design's `align-items: center` row
            // does.
            let band = egui::vec2(ui.available_width(), ROW_CONTENT_HEIGHT);
            ui.allocate_ui_with_layout(
                band,
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    label_cell(ui, label);
                    ui.add_space(ROW_GAP);
                    value(ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // `gap: 8px`, within the control group only.
                        ui.spacing_mut().item_spacing.x = CONTROL_GAP;
                        controls(ui);
                    });
                },
            );
        });
}

/// A row's label column: exactly [`ROW_LABEL_WIDTH`] wide whatever the label
/// says, so the values beside it line up down the whole pane. Painted rather
/// than `ui.label`ed because a label allocates its own text width.
fn label_cell(ui: &mut egui::Ui, label: &str) {
    let galley = ui.painter().layout(
        label.to_string(),
        egui::FontId::new(ROW_LABEL_SIZE, egui::FontFamily::Proportional),
        theme::TEXT_FAINT,
        ROW_LABEL_WIDTH,
    );
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ROW_LABEL_WIDTH, galley.size().y),
        egui::Sense::hover(),
    );
    let pos = egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, theme::TEXT_FAINT);
}

/// A card whose body is a paragraph rather than rows -- the notes card, the
/// two unsupported panes and the "nothing here" notes. Same `padding: 13px
/// 16px` a row has, so its text sits on the same left edge as every label.
fn card_text(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) {
    egui::Frame::new()
        .inner_margin(Margin::symmetric(CARD_PAD_X, ROW_PAD_Y))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(text);
        });
}

/// A plain value row. The whole tile copies; `hint`, when there is one, is
/// the keyboard chord that copies the same value without the mouse.
fn credential_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &str,
    hint: Option<CopyShortcut>,
    action: &mut DetailAction,
    on_copy: DetailAction,
) {
    copy_row(
        ui,
        label,
        |ui| {
            ui.label(RichText::new(value).size(ROW_VALUE_SIZE).color(theme::INK));
        },
        |ui| shortcut_hint(ui, hint),
        on_copy,
        action,
    );
}

/// A row's keyboard-shortcut hint, right-aligned with its controls.
///
/// The design's own idiom for this (`Deskwarden.dc.html` 2b: the search
/// field's `CTRL+K`, the Lock pill's `CTRL+L`) is bare 10px monospace in
/// ghost grey -- not [`theme::kbd_chip`]'s boxed treatment, which the design
/// reserves for the chips inside filled buttons and selected rows.
///
/// It exists because the explicit `Copy` buttons are gone: without a visible
/// hint the two ways left to copy are both invisible.
fn shortcut_hint(ui: &mut egui::Ui, hint: Option<CopyShortcut>) {
    if let Some(which) = hint {
        ui.label(
            RichText::new(copy_shortcut_hint(which))
                .size(10.0)
                .family(egui::FontFamily::Monospace)
                .color(theme::TEXT_GHOST),
        );
    }
}

fn password_row(ui: &mut egui::Ui, password: &str, revealed: &mut bool, action: &mut DetailAction) {
    masked_row(
        ui,
        "Password",
        password,
        revealed,
        action,
        DetailAction::CopyPassword,
        Some(CopyShortcut::Password),
    );
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
    hint: Option<CopyShortcut>,
) {
    let shown = if *revealed {
        value.to_string()
    } else {
        "•".repeat(MASKED_BULLETS)
    };
    copy_row(
        ui,
        label,
        |ui| {
            // `font-family: ui-monospace; font-size: 15px; letter-spacing:
            // 0.08em` -- the tracking is what stops a bullet run reading as
            // one solid blob.
            ui.label(theme::letterspaced_mono(
                &shown,
                MASKED_SIZE,
                MASKED_TRACKING,
                theme::INK,
            ));
        },
        |ui| {
            // AN EYE, not the words "Reveal"/"Hide". The state it shows is
            // the ACTION, the way every password manager spells it: an open
            // eye while the value is masked, struck through while it is
            // showing. Drawn rather than typed -- see `theme::eye_toggle` and
            // the font measurement its module header cites.
            //
            // The row around it copies on click; this does not. Registered
            // after the tile's own background widget, so egui gives it the
            // click instead -- see `copy_row`.
            if theme::eye_toggle(ui, *revealed)
                .on_hover_text(if *revealed { "Hide" } else { "Reveal" })
                .clicked()
            {
                *revealed = !*revealed;
            }
            shortcut_hint(ui, hint);
        },
        // **Copies the real value even while masked.** The mask is a display
        // concern; it was never what the old Copy button honoured either.
        on_copy,
        action,
    );
}

/// A short line of body text for a pane that has a heading and no rows.
///
/// The alternative is an empty box under a heading, which `notes_text`'s own
/// doc argues reads as contents that failed to load rather than as an item
/// with nothing in it. Reachable for real: the spec's rule is that a `type: 3`
/// carrying no `card` object is an *empty card*, not an unsupported item.
fn empty_pane_note(ui: &mut egui::Ui, text: &str) {
    card_text(
        ui,
        RichText::new(text)
            .size(ROW_LABEL_SIZE)
            .color(theme::TEXT_FAINT),
    );
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
            theme::row_rule(ui);
        }
    };
    if let Some(v) = &cardholder {
        separate(ui, &mut first);
        credential_row(ui, "Cardholder name", v, None, action, DetailAction::CopyValue(v.clone()));
    }
    if let Some(v) = &brand {
        separate(ui, &mut first);
        credential_row(ui, "Brand", v, None, action, DetailAction::CopyValue(v.clone()));
    }
    if let Some(v) = &number {
        separate(ui, &mut first);
        masked_row(ui, "Number", v, &mut reveal.card_number, action, DetailAction::CopyCardNumber, None);
    }
    if let Some(v) = &expiry {
        separate(ui, &mut first);
        credential_row(ui, "Expiry", v, None, action, DetailAction::CopyValue(v.clone()));
    }
    if let Some(v) = &code {
        separate(ui, &mut first);
        masked_row(ui, "Security code", v, &mut reveal.card_code, action, DetailAction::CopyCardCode, None);
    }
}

/// A previous password's row label: when it stopped being the current one.
///
/// The date goes in the LABEL column rather than beside the value because
/// every row on this pane puts its identifying text there, and what
/// identifies one previous password among five is when it was replaced.
///
/// Unparseable and absent both fall back to "Earlier" rather than to a
/// fabricated number -- the same rule [`updated_text`] follows for a
/// missing `revisionDate`, and the same reason `password_history` keeps an
/// entry whose date is missing: the secret is still real.
fn history_label(last_used_date: Option<&str>) -> String {
    match last_used_date.and_then(days_since) {
        Some(0) => "Today".to_string(),
        Some(1) => "1 day ago".to_string(),
        Some(n) => format!("{n} days ago"),
        None => "Earlier".to_string(),
    }
}

/// The PREVIOUS PASSWORDS rows: one masked row per entry, each driven by its
/// **own** flag in [`RevealState::password_history`].
///
/// The indexing is the load-bearing part. `masked_row` takes a `&mut bool`,
/// and passing the wrong one is a single-token slip that renders perfectly --
/// it was made for real on the card pane and caught only by a test that
/// revealed one row and asserted the other stayed masked. Here the same slip
/// would reveal a previous password the user did not ask to see, so the flag
/// is taken by index from the same enumeration that produces the row and
/// `each_history_row_is_revealed_only_by_its_own_flag` pins it.
///
/// The copy action carries the row's INDEX, not its value -- see
/// [`DetailAction::CopyPasswordHistory`].
fn history_rows(
    ui: &mut egui::Ui,
    history: &[PasswordHistoryEntry],
    reveal: &mut RevealState,
    action: &mut DetailAction,
) {
    for (index, entry) in history.iter().take(MAX_HISTORY_ROWS).enumerate() {
        if index > 0 {
            theme::row_rule(ui);
        }
        masked_row(
            ui,
            &history_label(entry.last_used_date.as_deref()),
            entry.password.as_str(),
            &mut reveal.password_history[index],
            action,
            DetailAction::CopyPasswordHistory(index),
            None,
        );
    }
    // Truncation is STATED, never silent. A previous password the pane simply
    // omitted is indistinguishable from one the user never had, and this pane
    // is the only place in the app they are visible at all. Unreachable
    // against today's backend -- Bitwarden's own `adjustPasswordHistoryLength`
    // slices every save to five -- which is exactly why it would rot unnoticed
    // if it were left to a comment.
    let hidden = history.len().saturating_sub(MAX_HISTORY_ROWS);
    if hidden > 0 {
        theme::row_rule(ui);
        empty_pane_note(
            ui,
            &format!(
                "{hidden} older {} not shown here -- open this item in the Bitwarden web \
                 vault or app to see all of them.",
                if hidden == 1 { "password is" } else { "passwords are" }
            ),
        );
    }
}

/// The SSH KEY rows: public key, fingerprint, and the private key behind a
/// mask.
///
/// **The private key is the only masked row, and it is the point of the
/// item.** The public key and the fingerprint are public by construction --
/// masking either would be theatre that makes the pane harder to use without
/// protecting anything.
///
/// Ordered public-first so the row a user reaches for most often is at the
/// top and the destructive-to-leak one is last.
///
/// It takes [`SshKeyFields`], never an [`SshKeyData`], for the structural
/// reason [`card_rows`] and [`identity_rows`] both record: with the raw data
/// in scope, a fourth row drawn straight off it compiles, renders, and is
/// invisible to [`SshKeyFields::is_empty`] -- so the pane draws a row *and*
/// says it has no SSH key details. The conversion happens at the call site,
/// so there is no `data` here to reach for.
fn ssh_key_rows(
    ui: &mut egui::Ui,
    fields: Option<SshKeyFields>,
    reveal: &mut RevealState,
    action: &mut DetailAction,
) {
    let Some(fields) = fields else {
        empty_pane_note(ui, "No SSH key details on this item.");
        return;
    };
    if fields.is_empty() {
        empty_pane_note(ui, "No SSH key details on this item.");
        return;
    }
    let SshKeyFields {
        public_key,
        fingerprint,
        private_key,
    } = fields;

    // `first` tracks whether a hairline is owed, so suppressing a row never
    // leaves a separator with nothing on one side of it -- same as
    // `card_rows`.
    let mut first = true;
    let separate = |ui: &mut egui::Ui, first: &mut bool| {
        if *first {
            *first = false;
        } else {
            theme::hairline(ui);
        }
    };
    if let Some(v) = &public_key {
        separate(ui, &mut first);
        credential_row(ui, "Public key", v, None, action, DetailAction::CopyValue(v.clone()));
    }
    if let Some(v) = &fingerprint {
        separate(ui, &mut first);
        credential_row(ui, "Fingerprint", v, None, action, DetailAction::CopyValue(v.clone()));
    }
    if let Some(v) = &private_key {
        separate(ui, &mut first);
        masked_row(ui, "Private key", v, &mut reveal.ssh_private_key, action, DetailAction::CopySshPrivateKey, None);
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
    for (group_name, rows) in groups.iter() {
        // A group boundary is the heavier rule -- the same one that sits under
        // the card's own heading, because a group heading is what follows it.
        // The lighter rule goes between the rows within a group.
        theme::hairline(ui);
        egui::Frame::new()
            .inner_margin(Margin::symmetric(CARD_PAD_X, CARD_HEADING_PAD_Y))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(theme::semibold(*group_name, ROW_LABEL_SIZE).color(theme::TEXT_SECONDARY));
            });
        for (row_index, (label, value)) in rows.iter().enumerate() {
            if row_index > 0 {
                theme::row_rule(ui);
            }
            credential_row(ui, label, value, None, action, DetailAction::CopyValue(value.clone()));
        }
    }
}

fn totp_code_row(ui: &mut egui::Ui, code: &str, seconds_left: u8, action: &mut DetailAction) {
    copy_row(
        ui,
        "One-time code",
        |ui| {
            // The design lays these three out along one centred line, `gap:
            // 12px` apart: the code, a 96x4 track, then the seconds left.
            ui.label(theme::letterspaced_mono(
                code,
                TOTP_CODE_SIZE,
                TOTP_CODE_TRACKING,
                theme::INK,
            ));
            ui.add_space(TOTP_GAP);
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(TOTP_BAR_WIDTH, TOTP_BAR_HEIGHT),
                egui::Sense::hover(),
            );
            ui.painter()
                .rect_filled(rect, CornerRadius::same(2), theme::HAIRLINE);
            let fraction = (seconds_left as f32 / 30.0).clamp(0.0, 1.0);
            let filled = egui::Rect::from_min_size(
                rect.min,
                egui::vec2(rect.width() * fraction, rect.height()),
            );
            ui.painter()
                .rect_filled(filled, CornerRadius::same(2), theme::BLUE);
            ui.add_space(TOTP_GAP);
            ui.label(
                RichText::new(format!("{seconds_left}s left"))
                    .size(ROW_HINT_SIZE)
                    .color(theme::TEXT_FAINT),
            );
        },
        |ui| shortcut_hint(ui, Some(CopyShortcut::Totp)),
        DetailAction::CopyTotp,
        action,
    );
}

/// The shape the three *non-code* One-time code rows share: the row's label
/// stays in its column, and the value column carries a plain status line with
/// an optional second line of explanation under it.
///
/// Shared *rendering* only. Which [`TotpState`] reaches which of the three is
/// unchanged and still lives in `totp_row_for` plus the one exhaustive match
/// in `draw_detail_read`; each row keeps its own function, its own wording and
/// its own doc comment, because the reasons they are three rows and not one
/// are the whole point of them.
fn totp_status_row(ui: &mut egui::Ui, status: &str, hint: Option<&str>) {
    row(
        ui,
        "One-time code",
        |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(
                    RichText::new(status)
                        .size(ROW_VALUE_SIZE)
                        .color(theme::TEXT_SECONDARY),
                );
                if let Some(hint) = hint {
                    ui.label(
                        RichText::new(hint)
                            .size(ROW_HINT_SIZE)
                            .color(theme::TEXT_GHOST),
                    );
                }
            });
        },
        |_ui| {},
    );
}

/// The One-time code row for `TotpState::Fetching`: this item has a TOTP
/// secret and a poll for its current code is already on its way, just not
/// back yet. Keeps the row's label in place, the same shape `Unavailable`'s
/// row does, but reads as an ordinary in-progress state rather than a
/// problem -- this is the everyday, usually sub-second case right after
/// selecting an item, not a backend issue.
fn totp_fetching_row(ui: &mut egui::Ui) {
    totp_status_row(ui, "Fetching\u{2026}", None);
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
    totp_status_row(
        ui,
        "Unavailable right now",
        Some("Couldn't reach the vault to get the current code."),
    );
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
    totp_status_row(
        ui,
        "No code available for this item",
        Some(
            "The vault has no current code for it. If its authenticator key was changed \
             on another device, Sync to pick that up.",
        ),
    );
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
            ssh_key: None,
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

    /// The same, for the pane an out-of-vault item gets.
    /// Everything the out-of-vault pane painted: its strings, and -- as a
    /// [`Frame`] -- the drawn icons, which paint no string at all.
    fn painted_out_of_vault(item: &VaultItem, out: OutOfVault) -> (Vec<String>, Frame) {
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

        let output = ctx.run_ui(input(), |ui| draw_out_of_vault_read(ui, item, out));
        let mut texts = Vec::new();
        let mut frame = Frame {
            action: DetailAction::None,
            texts: Vec::new(),
            rendered: Vec::new(),
            rects: Vec::new(),
            cursor: output.platform_output.cursor_icon,
            stars: Vec::new(),
            eyes: Vec::new(),
            kebab_dots: Vec::new(),
            segments: Vec::new(),
        };
        // One tree -- see `Pane::frame`, which explains why per-clipped-shape
        // probing cannot see a filled star.
        let all = egui::Shape::Vec(output.shapes.iter().map(|c| c.shape.clone()).collect());
        collect_text(&all, &mut texts);
        collect_text_rects(&all, &mut frame.texts);
        collect_rendered_text(&all, &mut frame.rendered);
        collect_rects(&all, &mut frame.rects);
        frame.stars = theme::icon_probe::stars(&all);
        frame.eyes = theme::icon_probe::eyes(&all);
        frame.kebab_dots = theme::icon_probe::kebab_dots(&all);
        frame.segments = theme::icon_probe::line_segments(&all);
        (texts, frame)
    }

    /// The out-of-vault pane names the item, says which of the two places it
    /// is in, and offers NO control.
    ///
    /// The negative half is the point and it is paired with a positive
    /// control, because "the pane has no Edit button" is also true of a pane
    /// that painted nothing at all: the item's own name and the state
    /// sentence are asserted present in the same test.
    #[test]
    fn the_out_of_vault_pane_states_where_the_item_is_and_offers_no_controls() {
        let item = VaultItem {
            id: "t1".into(),
            name: "Ledgerline".into(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: Some(1),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        };

        for (out, expected) in [
            (OutOfVault::Trash, "This item is in the Trash."),
            (OutOfVault::Archive, "This item is archived."),
        ] {
            let (painted, frame) = painted_out_of_vault(&item, out);
            assert!(painted.iter().any(|t| t == "Ledgerline"), "the pane did not name the item");
            assert!(
                painted.iter().any(|t| t == expected),
                "the pane never said {expected:?}; it painted {painted:?}"
            );
            // "Fill in app" is still a word. The read pane's other controls
            // are DRAWN now -- the favourite star, the kebab that carries
            // Edit and Delete, the reveal eye -- so their absence has to be
            // asserted against the shapes. Asserting the old strings here
            // would be a test that cannot fail: no pane in this app paints
            // the word "Delete" outside an open menu any more.
            assert!(
                !painted.iter().any(|t| t == "Fill in app"),
                "the out-of-vault pane offers Fill, which acts through the live item list \
                 and would do nothing for this item"
            );
            assert!(
                frame.stars.is_empty(),
                "the out-of-vault pane draws a favourite star, which writes through the \
                 live item list"
            );
            assert!(
                frame.kebab_dots.is_empty(),
                "the out-of-vault pane draws the kebab, which carries Edit and Delete"
            );
            assert!(
                frame.eyes.is_empty(),
                "the out-of-vault pane draws a reveal eye, so it is rendering rows it \
                 cannot act on"
            );
        }
    }

    /// The two states say different things. Without this, one message reused
    /// for both would pass every "the pane said something" assertion above.
    #[test]
    fn trash_and_archive_are_explained_differently() {
        let trash = out_of_vault_text(OutOfVault::Trash);
        let archive = out_of_vault_text(OutOfVault::Archive);
        assert_ne!(trash.0, archive.0);
        assert_ne!(trash.1, archive.1);
        // Each names the action that actually works for it, which is the
        // whole job of the sentence.
        assert!(trash.1.contains("Restore"), "the trash pane does not name Restore: {trash:?}");
        assert!(
            archive.1.contains("Unarchive"),
            "the archive pane does not name Unarchive: {archive:?}"
        );
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

    /// Same walk again, keeping the SOURCE string, the characters egui
    /// actually placed glyphs for, and the rect -- see [`Frame::rendered`]
    /// for why the first two are not the same thing.
    ///
    /// The rendered run is read out of the galley's rows rather than its
    /// `text()`: `Galley::rows[..].glyphs[..].chr` is one entry per glyph
    /// that was really laid out, so an elided run reports the prefix it drew
    /// and the ellipsis it drew instead of the name it was handed.
    fn collect_rendered_text(shape: &egui::Shape, out: &mut Vec<(String, String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => {
                let rendered: String = text
                    .galley
                    .rows
                    .iter()
                    .flat_map(|row| row.glyphs.iter().map(|glyph| glyph.chr))
                    .collect();
                out.push((
                    text.galley.text().to_string(),
                    rendered,
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                ));
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_rendered_text(shape, out);
                }
            }
            _ => {}
        }
    }

    /// The width and height `painted_*` lays the pane out at. Every geometry
    /// assertion below is an ABSOLUTE number measured against this pane, never
    /// one re-derived from the constant under test -- a review found four
    /// tests in this codebase that could not fail because they computed their
    /// expectation from the very value they were checking.
    const PANE: f32 = 900.0;

    /// One frame of `draw_detail_read`, as the filled rectangles it painted.
    ///
    /// The counterpart to [`painted_text`]: the pane surface, the header
    /// strip, the cards, the avatar tile and every button body are *fills*,
    /// and the report this work came from ("top is white") is a statement
    /// about exactly those. Nothing that walks galleys can see them.
    fn painted_rects(item: &VaultItem, totp: &TotpState) -> Vec<(egui::Rect, egui::Color32)> {
        let mut rects = Vec::new();
        for clipped in &frame_shapes(item, totp, RevealState::default()) {
            collect_rects(&clipped.shape, &mut rects);
        }
        rects
    }

    fn collect_rects(shape: &egui::Shape, out: &mut Vec<(egui::Rect, egui::Color32)>) {
        match shape {
            egui::Shape::Rect(rect) => out.push((rect.rect, rect.fill)),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_rects(shape, out);
                }
            }
            _ => {}
        }
    }

    /// Every painted string with the rectangle it landed in *and the font it
    /// was laid out with* -- the only way to assert a size or a weight, which
    /// [`collect_text_rects`] throws away.
    fn painted_type(
        item: &VaultItem,
        totp: &TotpState,
        reveal: RevealState,
    ) -> Vec<(String, egui::Rect, egui::FontId)> {
        let mut out = Vec::new();
        for clipped in &frame_shapes(item, totp, reveal) {
            collect_type(&clipped.shape, &mut out);
        }
        out
    }

    fn collect_type(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect, egui::FontId)>) {
        match shape {
            egui::Shape::Text(text) => {
                let font = text
                    .galley
                    .job
                    .sections
                    .first()
                    .map(|s| s.format.font_id.clone())
                    .unwrap_or_default();
                out.push((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                    font,
                ));
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_type(shape, out);
                }
            }
            _ => {}
        }
    }

    /// The one painted run with this exact text, with its rect and its font.
    /// Panics rather than returning an `Option` so a row that stopped being
    /// drawn names itself, and rejects a second match so no assertion here can
    /// quietly be about a different run than the one it names.
    fn only(
        painted: &[(String, egui::Rect, egui::FontId)],
        text: &str,
    ) -> (egui::Rect, egui::FontId) {
        let mut hits = painted.iter().filter(|(t, _, _)| t == text);
        let hit = hits
            .next()
            .unwrap_or_else(|| panic!("nothing painted {text:?}; painted: {painted:?}"));
        assert!(
            hits.next().is_none(),
            "{text:?} was painted more than once, so this assertion is ambiguous"
        );
        (hit.1, hit.2.clone())
    }

    fn frame_shapes(
        item: &VaultItem,
        totp: &TotpState,
        reveal: RevealState,
    ) -> Vec<egui::epaint::ClippedShape> {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(PANE, PANE),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});

        let mut reveal = reveal;
        ctx.run_ui(input(), |ui| {
            draw_detail_read(ui, item, 3, totp, false, &mut reveal, None);
        })
        .shapes
    }

    /// The detail column's width at this app's own MINIMUM window size --
    /// the width the header strip actually has to survive, and the one no
    /// geometry test here used to try.
    ///
    /// Derived from the three constants that produce it (900 - 212 - 390 =
    /// 298pt) rather than written out, because a hardcoded 298 would stop
    /// being the minimum the moment any of them moved and would then be
    /// checking a width the app can no longer be resized to.
    const MIN_PANE: f32 = crate::settings::MIN_VAULT_WINDOW_SIZE.0 as f32
        - crate::vault_window::SIDEBAR_WIDTH
        - crate::vault_window::LIST_WIDTH;

    /// A full press AND release, which is what egui needs before it reports
    /// `Response::clicked` -- a press alone is not a click.
    fn click_at(pos: egui::Pos2) -> Vec<egui::Event> {
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
    }

    fn ctrl(key: egui::Key) -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        }]
    }

    /// **The click harness.** Everything below that has to press a control
    /// rather than read a string goes through this.
    ///
    /// Modelled on `detail_edit.rs`'s `generator_row_tests` (commit
    /// `6020489`), including its two hard-won details: a press *and* a
    /// release is what egui counts as a click, and a popup only PAINTS on
    /// the frame after the click that opened it -- so the frame that finds
    /// a menu entry and the frame that opened the menu can never be the
    /// same one.
    ///
    /// It carries the `RevealState` and the `delete_pending` flag across
    /// frames the way `vault_window::mod`'s `run` does, which is the only
    /// way a toggle can be observed outliving the frame it happened in.
    struct Pane {
        ctx: egui::Context,
        width: f32,
        reveal: RevealState,
        delete_pending: bool,
    }

    /// One frame's output: what it returned, every string it painted with
    /// the rect it landed in, and the three drawn icons -- which paint no
    /// string at all and can therefore only be found by their geometry (see
    /// `theme::icon_probe`, which owns that lookup).
    struct Frame {
        action: DetailAction,
        texts: Vec<(String, egui::Rect)>,
        /// Every string the frame painted with **the characters that were
        /// actually laid out**, which is not the same list as [`texts`].
        ///
        /// `Galley::text()` returns the layout job's SOURCE string, so a run
        /// egui elided down to one "…" still reports the full name it was
        /// asked to draw. Every painted-text assertion in this file is blind
        /// to truncation in exactly that way, and a header that annihilated
        /// its own title passed a test that looked only at `texts`. The
        /// glyphs are not blind to it.
        rendered: Vec<(String, String, egui::Rect)>,
        /// Every filled rectangle, so the tests can see the surfaces --
        /// the strip, the avatar tile, a row's hover tint -- that paint no
        /// string at all.
        rects: Vec<(egui::Rect, egui::Color32)>,
        /// The cursor this frame asked for. Half of a click affordance; the
        /// other half is a fill in [`rects`].
        cursor: egui::CursorIcon,
        stars: Vec<theme::icon_probe::Star>,
        eyes: Vec<egui::Rect>,
        kebab_dots: Vec<(egui::Rect, egui::Color32)>,
        segments: Vec<egui::Rect>,
    }

    impl Frame {
        /// How many of this frame's eyes are struck through -- the only
        /// visible difference between a revealed row and a masked one.
        fn struck_eyes(&self) -> usize {
            self.eyes
                .iter()
                .filter(|eye| {
                    self.segments
                        .iter()
                        .any(|seg| eye.expand(4.0).contains_rect(*seg))
                })
                .count()
        }

        fn strings(&self) -> Vec<&str> {
            self.texts.iter().map(|(t, _)| t.as_str()).collect()
        }

        fn painted(&self, label: &str) -> bool {
            self.texts.iter().any(|(t, _)| t == label)
        }

        /// The one rect painting `label`, or a failure naming everything
        /// that *was* painted -- which turns "the control is gone" into a
        /// readable message instead of a click that silently hits nothing.
        fn rect_of(&self, label: &str) -> egui::Rect {
            let found: Vec<egui::Rect> = self
                .texts
                .iter()
                .filter(|(t, _)| t == label)
                .map(|(_, r)| *r)
                .collect();
            assert_eq!(
                found.len(),
                1,
                "expected exactly one {label:?} in the pane, found {}; painted: {:?}",
                found.len(),
                self.strings()
            );
            found[0]
        }

        /// The header's favourite star. Exactly one, in either state.
        fn star(&self) -> theme::icon_probe::Star {
            assert_eq!(
                self.stars.len(),
                1,
                "expected exactly one star in the header, found {}; the pane painted: {:?}",
                self.stars.len(),
                self.strings()
            );
            self.stars[0]
        }

        /// The header's kebab, as the union of its three dots. Three is the
        /// assertion: two would be an ellipsis and four a different control.
        fn kebab(&self) -> egui::Rect {
            assert_eq!(
                self.kebab_dots.len(),
                3,
                "expected exactly three kebab dots in the header, found {}; the pane \
                 painted: {:?}",
                self.kebab_dots.len(),
                self.strings()
            );
            self.kebab_dots
                .iter()
                .skip(1)
                .fold(self.kebab_dots[0].0, |a, (b, _)| a.union(*b))
        }

        /// The colour the kebab's three dots were filled in -- one colour,
        /// or a failure, so "the kebab is red" cannot be satisfied by one
        /// red dot out of three.
        fn kebab_colour(&self) -> egui::Color32 {
            let _ = self.kebab();
            let first = self.kebab_dots[0].1;
            assert!(
                self.kebab_dots.iter().all(|(_, c)| *c == first),
                "the kebab's dots are not all one colour: {:?}",
                self.kebab_dots
            );
            first
        }

        /// What was actually DRAWN for the run whose source text is `label`
        /// -- glyphs, not the string the layout job was handed. See
        /// [`Frame::rendered`].
        fn rendered_glyphs(&self, label: &str) -> String {
            let found: Vec<&String> = self
                .rendered
                .iter()
                .filter(|(source, _, _)| source == label)
                .map(|(_, rendered, _)| rendered)
                .collect();
            assert_eq!(
                found.len(),
                1,
                "expected exactly one run laid out from {label:?}, found {}; painted: {:?}",
                found.len(),
                self.strings()
            );
            found[0].clone()
        }

        /// The header's white strip: every [`theme::CARD`] fill that starts
        /// at the very top of the pane, unioned.
        ///
        /// The union rather than one rect because the strip is a `Frame`
        /// background and the tests care about its OUTER bounds. A body card
        /// is also CARD-filled, which is why this is anchored to the pane's
        /// top edge; the first card starts well below it.
        fn header_strip(&self) -> egui::Rect {
            let strip = self
                .rects
                .iter()
                .filter(|(rect, fill)| *fill == theme::CARD && rect.top() <= 0.5)
                .map(|(rect, _)| *rect)
                .reduce(egui::Rect::union);
            strip.expect("the pane painted no white header strip at its top edge")
        }

        /// The header's avatar tile: the [`HEADER_AVATAR`]-square box, found
        /// by its size because it paints no string of its own.
        ///
        /// Its fill and its border are two shapes at the same rect, so this
        /// unions rather than insisting on exactly one.
        fn avatar_tile(&self) -> egui::Rect {
            let tile = self
                .rects
                .iter()
                .filter(|(rect, _)| {
                    (rect.width() - HEADER_AVATAR).abs() < 0.5
                        && (rect.height() - HEADER_AVATAR).abs() < 0.5
                })
                .map(|(rect, _)| *rect)
                .reduce(egui::Rect::union);
            tile.expect("the header painted no 44px avatar tile")
        }

        /// Every reveal eye, top-down -- the order the rows are drawn in, so
        /// `eyes()[0]` is the first masked row on the pane.
        fn eyes(&self) -> Vec<egui::Rect> {
            let mut eyes = self.eyes.clone();
            eyes.sort_by(|a, b| a.top().total_cmp(&b.top()));
            eyes
        }
    }

    impl Pane {
        fn new() -> Self {
            Self::wide(PANE)
        }

        fn wide(width: f32) -> Self {
            let ctx = egui::Context::default();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, PANE),
                )),
                ..Default::default()
            };
            // The same two throwaway frames every harness in this crate
            // runs: a font set registered during a frame is only usable
            // from the start of the next one.
            let _ = ctx.run_ui(input.clone(), |_ui| {});
            theme::apply(&ctx);
            let _ = ctx.run_ui(input, |_ui| {});
            Self {
                ctx,
                width,
                reveal: RevealState::default(),
                delete_pending: false,
            }
        }

        fn frame(&mut self, item: &VaultItem, totp: &TotpState, events: Vec<egui::Event>) -> Frame {
            let mut action = DetailAction::None;
            let output = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(self.width, PANE),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    action = draw_detail_read(
                        ui,
                        item,
                        3,
                        totp,
                        self.delete_pending,
                        &mut self.reveal,
                        None,
                    );
                },
            );
            let mut frame = Frame {
                action,
                texts: Vec::new(),
                rendered: Vec::new(),
                rects: Vec::new(),
                cursor: output.platform_output.cursor_icon,
                stars: Vec::new(),
                eyes: Vec::new(),
                kebab_dots: Vec::new(),
                segments: Vec::new(),
            };
            // ONE tree, not one call per clipped shape: `paint_star` adds
            // the star's outline and its ten fill triangles as separate
            // top-level shapes, so a probe run per clipped shape could never
            // see both and would report every filled star as an outline.
            let all = egui::Shape::Vec(output.shapes.iter().map(|c| c.shape.clone()).collect());
            collect_text_rects(&all, &mut frame.texts);
            collect_rendered_text(&all, &mut frame.rendered);
            collect_rects(&all, &mut frame.rects);
            frame.stars = theme::icon_probe::stars(&all);
            frame.eyes = theme::icon_probe::eyes(&all);
            frame.kebab_dots = theme::icon_probe::kebab_dots(&all);
            frame.segments = theme::icon_probe::line_segments(&all);
            frame
        }

        fn idle(&mut self, item: &VaultItem, totp: &TotpState) -> Frame {
            self.frame(item, totp, Vec::new())
        }

        fn click(&mut self, item: &VaultItem, totp: &TotpState, pos: egui::Pos2) -> Frame {
            self.frame(item, totp, click_at(pos))
        }

        /// The pointer resting on `pos`, and the frame it produced -- which
        /// is where a hover *affordance* (a tint, a cursor) lives. A click
        /// test cannot see either: both are gone by the time the click has
        /// been reported.
        fn hover(&mut self, item: &VaultItem, totp: &TotpState, pos: egui::Pos2) -> Frame {
            self.frame(item, totp, vec![egui::Event::PointerMoved(pos)])
        }

        /// Lays the pane out, clicks the kebab, and returns the frame AFTER
        /// that click -- the first one on which the menu paints.
        fn open_kebab(&mut self, item: &VaultItem, totp: &TotpState) -> Frame {
            let closed = self.idle(item, totp);
            assert!(
                closed.stars.len() == 1,
                "the pane did not lay out at all, so opening its menu proves nothing"
            );
            let kebab = closed.kebab().center();
            let _ = self.click(item, totp, kebab);
            self.idle(item, totp)
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
    /// gating fill must not take it with it -- and moving it into the kebab
    /// must not either.
    ///
    /// Behavioural, because it has to be: Delete is no longer a word painted
    /// on the strip. The menu is really opened and the entry really clicked,
    /// so what is pinned is that the action still *reaches the caller* for
    /// every kind rather than that a string is somewhere on screen.
    #[test]
    fn every_kind_can_still_be_deleted() {
        for kind in EVERY_KIND {
            let item = an_item(item_type_for(kind));
            let mut pane = Pane::new();
            let open = pane.open_kebab(&item, &TotpState::NoSecret);
            assert!(
                open.painted("Delete"),
                "{kind:?}'s kebab menu carries no Delete; it painted: {:?}",
                open.strings()
            );
            let entry = open.rect_of("Delete");
            let clicked = pane.click(&item, &TotpState::NoSecret, entry.center());
            assert_eq!(
                clicked.action,
                DetailAction::Delete,
                "{kind:?}'s Delete entry is decoration -- clicking it reported {:?}",
                clicked.action
            );
        }
    }

    /// The positive control for the test above, and for
    /// `the_rendered_chrome_matches_the_chrome_decision_for_every_kind`'s
    /// menu half: the entries are only reachable once the kebab is clicked.
    /// Without this, "Edit is in the menu" would pass just as well against a
    /// menu that is permanently open, and the decluttering the user asked
    /// for would be undone without a single test noticing.
    #[test]
    fn the_kebab_menu_is_closed_until_the_kebab_is_clicked() {
        let item = a_login();
        let mut pane = Pane::new();
        let closed = pane.idle(&item, &TotpState::NoSecret);
        assert_eq!(
            closed.kebab_dots.len(),
            3,
            "no kebab painted at all, so this proves nothing: {:?}",
            closed.strings()
        );
        for entry in ["Edit", "Delete"] {
            assert!(
                !closed.painted(entry),
                "{entry:?} is on the header strip with the menu shut: {:?}",
                closed.strings()
            );
        }
        let open = pane.open_kebab(&item, &TotpState::NoSecret);
        for entry in ["Edit", "Delete"] {
            assert!(
                open.painted(entry),
                "the opened kebab menu does not carry {entry:?}: {:?}",
                open.strings()
            );
        }
    }

    /// Clicking Edit in the menu is what asks the caller to edit. Nothing
    /// else in the crate notices if that binding is dropped:
    /// `DetailAction::Edit` is `pub`, so its producers falling to zero is
    /// not even a warning, and every test of `kind_offers_edit` keeps
    /// passing while the feature is inert.
    #[test]
    fn clicking_edit_in_the_kebab_menu_asks_the_caller_to_edit() {
        let item = a_login();
        let mut pane = Pane::new();
        let open = pane.open_kebab(&item, &TotpState::NoSecret);
        let entry = open.rect_of("Edit");
        let clicked = pane.click(&item, &TotpState::NoSecret, entry.center());
        assert_eq!(
            clicked.action,
            DetailAction::Edit,
            "clicking the menu's Edit reported {:?}",
            clicked.action
        );
    }

    /// **Delete still takes two clicks, and the menu still says so.**
    /// `vault_window::mod`'s `confirm_click` is what actually gates the
    /// deletion; what this pane owes it is an armed state the user can see.
    /// Burying the control in a menu is exactly the change that could have
    /// dropped it -- and a menu that closed itself on the arming click would
    /// hide the state it had just entered.
    ///
    /// Both directions, so this cannot pass against an entry hardcoded to
    /// either label.
    #[test]
    fn an_armed_delete_says_so_in_the_menu_and_on_the_kebab() {
        let item = a_login();

        let mut idle = Pane::new();
        let unarmed = idle.open_kebab(&item, &TotpState::NoSecret);
        assert!(
            unarmed.painted("Delete") && !unarmed.painted("Delete? Click to confirm"),
            "an unarmed delete already asks for confirmation: {:?}",
            unarmed.strings()
        );

        let mut armed = Pane::new();
        armed.delete_pending = true;
        let open = armed.open_kebab(&item, &TotpState::NoSecret);
        assert!(
            open.painted("Delete? Click to confirm"),
            "an armed delete reads exactly like an unarmed one, so the confirmation \
             step is invisible: {:?}",
            open.strings()
        );
        // And it is still reported, so the SECOND click can confirm.
        let entry = open.rect_of("Delete? Click to confirm");
        let clicked = armed.click(&item, &TotpState::NoSecret, entry.center());
        assert_eq!(
            clicked.action,
            DetailAction::Delete,
            "the armed Delete entry is inert, so a delete can be armed but never confirmed"
        );
    }

    /// The armed state has to be legible with the menu SHUT, because a click
    /// anywhere else closes it while `confirm_click`'s window is still open.
    /// The kebab's own dots turn `ERROR` red for that.
    #[test]
    fn the_kebab_itself_shows_that_a_delete_is_armed() {
        let item = a_login();
        let colour = |delete_pending: bool| {
            let mut pane = Pane::new();
            pane.delete_pending = delete_pending;
            pane.idle(&item, &TotpState::NoSecret).kebab_colour()
        };
        assert_eq!(
            colour(true),
            theme::ERROR,
            "an armed delete leaves the kebab looking exactly like an unarmed one, so \
             closing the menu hides the confirmation entirely"
        );
        // Both directions, so this cannot pass against a kebab that is
        // always red -- which would be a permanent alarm instead of a state.
        assert_ne!(
            colour(false),
            theme::ERROR,
            "the kebab is red with nothing armed"
        );
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

    /// **`Unknown` is the only kind left with an unsupported pane.** SSH keys
    /// shared it while their wire shape was unverified; now that
    /// `SshKeyData` exists, an SSH item saying "open it in the web vault"
    /// would be a false claim about data the pane is holding.
    #[test]
    fn only_an_unknown_type_still_gets_the_unsupported_pane() {
        for kind in EVERY_KIND {
            let texts = painted(&an_item(item_type_for(kind)), &TotpState::NoSecret);
            assert_eq!(
                contains(&texts, "web vault"),
                matches!(kind, ItemKind::Unknown(_)),
                "{kind:?}: the unsupported pane is drawn for the wrong kinds: {texts:?}"
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
        // Was an `Unsupported` pane until `SshKeyData` existed.
        assert_eq!(detail_body_for(ItemKind::SshKey), DetailBody::SshKey);

        match detail_body_for(ItemKind::Unknown(9)) {
            DetailBody::Unsupported(pane) => {
                assert!(
                    pane.message.contains("web vault"),
                    "the message must point at where the data can still be seen: {:?}",
                    pane.message
                );
                assert!(
                    pane.message.contains("unchanged"),
                    "the message must say the item's own data is intact: {:?}",
                    pane.message
                );
            }
            other => panic!("Unknown(9) dispatched to {other:?}, not an unsupported pane"),
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
    /// type-5 item is one Deskwarden recognises *and can now show*, so it
    /// must not dispatch anywhere near the unsupported pane -- and an
    /// `Unknown(5)`, which is unreachable via `ItemKind::of` but
    /// constructible here, must not pick up the SSH body by number.
    #[test]
    fn the_ssh_pane_and_the_unknown_pane_are_different_bodies() {
        let (ssh, unknown) = (
            detail_body_for(ItemKind::SshKey),
            detail_body_for(ItemKind::Unknown(5)),
        );
        assert_ne!(ssh, unknown);
        assert_eq!(ssh, DetailBody::SshKey);
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
                contains(&texts, "Strength"),
                kind_offers_fill(kind),
                "{kind:?}: the metadata strip disagrees with kind_offers_fill"
            );

            // Edit lives in the kebab now, so the drift guard has to open it.
            // Same predicate, same drift caught -- a fix correct in
            // `kind_offers_edit` and inert in `draw_detail_read` still fails
            // here -- but the menu really is opened, which the old string
            // check could not distinguish from a permanently-visible button.
            let mut pane = Pane::new();
            let open = pane.open_kebab(&item, &TotpState::NoSecret);
            assert_eq!(
                open.painted("Edit"),
                kind_offers_edit(kind),
                "{kind:?}: the menu's Edit entry disagrees with kind_offers_edit; the \
                 menu painted: {:?}",
                open.strings()
            );
            // The positive control: the menu really did open, so a `false`
            // above is Edit being gated and not the menu failing to appear.
            assert!(
                open.painted("Delete"),
                "{kind:?}: the kebab menu did not open at all, so the Edit assertion \
                 above proves nothing: {:?}",
                open.strings()
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
                DetailBody::SshKey => assert!(heading_present("SSH KEY"), "{kind:?}: {texts:?}"),
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
        // The positive control, and the one thing "Reveal" used to say: the
        // pane really does offer a way to unmask what it hid. Two eyes,
        // because both masked rows must have one -- an assertion the old
        // `contains(.., "Reveal")` could not make, since one string is
        // enough to satisfy `contains`.
        let mut pane = Pane::new();
        assert_eq!(
            pane.idle(&a_full_card(), &TotpState::NoSecret).eyes.len(),
            2,
            "the card pane offers no way to reveal what it masked"
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
            password_history: [false; MAX_HISTORY_ROWS],
            ssh_private_key: false,
        };
        let texts = painted_with_reveal(&a_full_card(), &TotpState::NoSecret, reveal);
        assert!(
            contains(&texts, "4242424242424242"),
            "a revealed card number did not paint, so the pane ignores the caller's \
             reveal state: {texts:?}"
        );
        assert!(contains(&texts, "123"), "a revealed security code did not paint: {texts:?}");
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
        let mut pane = Pane::new();

        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        // The topmost eye is the Number row's: the security code's row is
        // drawn below it, and both are inside the same card.
        let eyes = laid_out.eyes();
        assert_eq!(
            eyes.len(),
            2,
            "the card pane painted {} reveal controls, not the two masked rows it has; \
             it painted: {:?}",
            eyes.len(),
            laid_out.strings()
        );
        let _ = pane.click(&item, &TotpState::NoSecret, eyes[0].center());

        // The click reached the caller's struct, and reached the field that
        // belongs to the row it landed on.
        assert_eq!(
            pane.reveal,
            RevealState {
                password: false,
                card_number: true,
                card_code: false,
                password_history: [false; MAX_HISTORY_ROWS],
                ssh_private_key: false,
            },
            "clicking the card number's eye did not write through to the caller's \
             RevealState, or wrote through to the wrong field"
        );

        // And a frame with no input at all still paints the digits.
        let after = pane.idle(&item, &TotpState::NoSecret);
        let texts: Vec<String> = after.texts.iter().map(|(t, _)| t.clone()).collect();
        assert!(
            contains(&texts, "4242424242424242"),
            "the frame after the reveal click painted the number masked again -- the \
             toggle did not outlive the frame it happened in: {texts:?}"
        );
        assert!(
            !contains(&texts, "123"),
            "the security code was revealed by a click on the number's eye: {texts:?}"
        );
        assert_eq!(
            after.struck_eyes(),
            1,
            "exactly one of the two eyes should now be struck through"
        );
    }

    /// **Clicking the eye reveals, and copies NOTHING.**
    ///
    /// The eye sits inside a tile that copies on click, and the value behind
    /// it is a secret. "Clicking the eye also put the password on the
    /// clipboard" is silent, user-visible and would be caught by no other
    /// test here -- so both halves are asserted in one frame: the flag
    /// flipped, and no action was reported. The positive half is what stops
    /// the negative one passing against a click that missed everything.
    #[test]
    fn clicking_the_eye_reveals_without_copying() {
        let item = a_login();
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        let eye = laid_out.eyes();
        assert_eq!(eye.len(), 1, "a login has exactly one masked row");

        let clicked = pane.click(&item, &TotpState::NoSecret, eye[0].center());
        assert!(
            pane.reveal.password,
            "clicking the eye did not reveal the password, so the assertion below is \
             about a click that hit nothing"
        );
        assert_eq!(
            clicked.action,
            DetailAction::None,
            "clicking the eye ALSO copied the password to the clipboard -- a secret the \
             user never asked for"
        );
    }

    /// The tile copies when the click lands anywhere else in it. Over the
    /// LABEL column, which is as far from the eye as the row goes.
    #[test]
    fn clicking_a_password_tile_copies_it_without_revealing_it() {
        let item = a_login();
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        let label = laid_out.rect_of("Password");

        let clicked = pane.click(&item, &TotpState::NoSecret, label.center());
        assert_eq!(
            clicked.action,
            DetailAction::CopyPassword,
            "clicking the password tile reported {:?}, so the tile is not the copy \
             target the user asked for",
            clicked.action
        );
        assert!(
            !pane.reveal.password,
            "copying by tile click also unmasked the password on screen"
        );
    }

    /// The username tile too -- a second row, so a `copy_row` wired to one
    /// fixed action cannot pass both this and the test above.
    #[test]
    fn clicking_a_username_tile_copies_the_username() {
        let item = a_login();
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        let value = laid_out.rect_of("a.novak@ledgerline.com");

        let clicked = pane.click(&item, &TotpState::NoSecret, value.center());
        assert_eq!(
            clicked.action,
            DetailAction::CopyUsername,
            "clicking the username tile reported {:?}",
            clicked.action
        );
    }

    /// **A row with nothing to copy stays inert.** The user's rule was
    /// "those username, pass, code" tiles, not the whole pane: a tile that
    /// reacted and copied nothing is worse than one that does not react,
    /// because there is no way to tell from outside that it did nothing.
    ///
    /// The TOTP status rows are the real case -- same `row` shape, same
    /// place, no value.
    #[test]
    fn a_row_with_nothing_to_copy_reports_nothing_when_clicked() {
        let mut item = a_login();
        item.login.as_mut().expect("a_login has login data").totp =
            Some("seed".to_string().into());
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::Unavailable);
        let row = laid_out.rect_of("Unavailable right now");

        let clicked = pane.click(&item, &TotpState::Unavailable, row.center());
        assert_eq!(
            clicked.action,
            DetailAction::None,
            "the TOTP row with no code copied {:?} when clicked",
            clicked.action
        );

        // The positive control: the same click, one row up, on a tile that
        // DOES have something to copy. Without it this test passes against a
        // harness whose clicks never land anywhere.
        let password = laid_out.rect_of("Password");
        let clicked = pane.click(&item, &TotpState::Unavailable, password.center());
        assert_eq!(
            clicked.action,
            DetailAction::CopyPassword,
            "no tile on this pane copies, so the inert assertion above proves nothing"
        );
    }

    /// **An inert row must not LOOK clickable either.**
    ///
    /// `copy_row`'s doc argues that "a row that reacted to a click and copied
    /// nothing would be worse than an inert one, because there is no way to
    /// tell from the outside that it did nothing" -- and that is an argument
    /// about the affordance, not about the returned action. Changing `row`'s
    /// `Sense::hover()` to `Sense::click()` switches on the `CARD_TINT` hover
    /// fill and the `PointingHand` cursor for exactly these rows, promising a
    /// copy that will never happen; the test above sees none of it, because
    /// a `Sense::click()` row with no `on_copy` still returns
    /// `DetailAction::None`. That mutation left 848/848 green.
    ///
    /// So: hover the inert row and assert BOTH halves of the affordance are
    /// absent, against a positive control on a row that has one.
    #[test]
    fn a_row_with_nothing_to_copy_offers_no_click_affordance() {
        let mut item = a_login();
        item.login.as_mut().expect("a_login has login data").totp =
            Some("seed".to_string().into());
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::Unavailable);
        let inert = laid_out.rect_of("Unavailable right now");
        let live = laid_out.rect_of("Password");

        let hovered = pane.hover(&item, &TotpState::Unavailable, inert.center());
        assert_ne!(
            hovered.cursor,
            egui::CursorIcon::PointingHand,
            "hovering the TOTP row that has nothing to copy asks for the pointing hand -- \
             it advertises a copy it cannot perform"
        );
        assert!(
            !hovered
                .rects
                .iter()
                .any(|(rect, fill)| *fill == theme::CARD_TINT && rect.contains(inert.center())),
            "hovering the TOTP row that has nothing to copy paints the CARD_TINT hover fill \
             every copyable tile uses: {:?}",
            hovered
                .rects
                .iter()
                .filter(|(_, fill)| *fill == theme::CARD_TINT)
                .collect::<Vec<_>>()
        );

        // The positive control, on the row one up that DOES copy. Without it
        // this passes against a harness whose pointer never lands anywhere,
        // or a build in which no row has ever had a hover state.
        let hovered = pane.hover(&item, &TotpState::Unavailable, live.center());
        assert_eq!(
            hovered.cursor,
            egui::CursorIcon::PointingHand,
            "no row on this pane offers the pointing hand, so its absence above proves nothing"
        );
        assert!(
            hovered
                .rects
                .iter()
                .any(|(rect, fill)| *fill == theme::CARD_TINT && rect.contains(live.center())),
            "no row on this pane paints the hover tint, so its absence above proves nothing"
        );
    }

    /// The live code's row DOES copy -- the `Unavailable` row above is inert
    /// because it has no code, not because TOTP rows are inert.
    #[test]
    fn clicking_the_one_time_code_tile_copies_the_code() {
        let mut item = a_login();
        item.login.as_mut().expect("a_login has login data").totp =
            Some("seed".to_string().into());
        let totp = TotpState::Code {
            code: "123456".to_string(),
            seconds_left: 21,
        };
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &totp);
        let row = laid_out.rect_of("One-time code");

        let clicked = pane.click(&item, &totp, row.center());
        assert_eq!(
            clicked.action,
            DetailAction::CopyTotp,
            "clicking the one-time code tile reported {:?}",
            clicked.action
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
                password_history: [false; MAX_HISTORY_ROWS],
                ssh_private_key: false,
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
                password_history: [false; MAX_HISTORY_ROWS],
                ssh_private_key: false,
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
                password_history: [false; MAX_HISTORY_ROWS],
                ssh_private_key: false,
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
        for absent in ["Cardholder name", "Number", "Security code"] {
            assert!(
                !contains(&texts, absent),
                "an empty card drew a {absent:?} row it has no data for: {texts:?}"
            );
        }
        // "Reveal" used to be in that list; the control paints no string
        // now, so the shape has to be asserted instead -- leaving the word
        // there would be an assertion that can no longer fail. Paired with
        // `the_card_number_and_security_code_are_masked_by_default`, which
        // shows a populated card really does paint two.
        let mut pane = Pane::new();
        assert!(
            pane.idle(&an_item(Some(3)), &TotpState::NoSecret).eyes.is_empty(),
            "an empty card drew a masked row it has no data for"
        );
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

    /// A private key body no other fixture string contains, so
    /// `contains(&texts, PRIVATE_KEY_BODY)` is a statement about the private
    /// key row and nothing else. The public key deliberately carries a
    /// *different* base64 run for the same reason.
    const PRIVATE_KEY_BODY: &str = "b3BlbnNzaC1rZXktdjEAAAAA";
    const PRIVATE_KEY: &str = concat!(
        "-----BEGIN OPENSSH PRIVATE KEY-----\n",
        "b3BlbnNzaC1rZXktdjEAAAAA\n",
        "-----END OPENSSH PRIVATE KEY-----"
    );
    const PUBLIC_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5 deskwarden-ssh-test";
    const FINGERPRINT: &str = "SHA256:8QhVn0pR";

    fn an_ssh_key_item() -> VaultItem {
        let mut item = an_item(Some(5));
        item.ssh_key = Some(crate::vault_bridge::SshKeyData {
            private_key: Some(PRIVATE_KEY.to_string().into()),
            public_key: Some(PUBLIC_KEY.to_string()),
            key_fingerprint: Some(FINGERPRINT.to_string()),
            other: serde_json::Map::new(),
        });
        item
    }

    #[test]
    fn the_ssh_key_pane_paints_every_populated_row() {
        let texts = painted(&an_ssh_key_item(), &TotpState::NoSecret);
        for label in ["SSH KEY", "Public key", PUBLIC_KEY, "Fingerprint", FINGERPRINT, "Private key"] {
            assert!(contains(&texts, label), "the SSH pane painted no {label:?}: {texts:?}");
        }
        // The pane it replaced said this build could not show SSH keys.
        assert!(
            !contains(&texts, "can't show SSH keys"),
            "the SSH pane is still the unsupported placeholder: {texts:?}"
        );
    }

    /// **The negative half.** The private key is the secret the item exists
    /// to hold, so it is masked until asked for, exactly as the card's number
    /// and security code are.
    ///
    /// Worthless on its own -- a pane that rendered *nothing at all* would
    /// pass it too. See the positive control below; that exact pair was
    /// required for the card secrets and is required here for the same
    /// reason.
    /// **A masked row draws the same number of bullets whatever it hides.**
    ///
    /// Two things ride on this, and neither had a test. A per-character run
    /// let a ~94-character SSH private key claim the whole row and push the
    /// Copy and Reveal controls off the pane -- the pane masked the key and
    /// then offered no way to see or copy it. It also drew the secret's exact
    /// length on screen for anyone looking.
    ///
    /// Asserted across a 16-digit card number and a ~94-character private
    /// key, i.e. the shortest and longest masked values the app has, and
    /// against the constant rather than against each other -- two runs that
    /// moved together would satisfy an equality check while both being wrong.
    #[test]
    fn a_masked_row_draws_a_fixed_bullet_run_whatever_it_hides() {
        let expected = "•".repeat(MASKED_BULLETS);
        for (label, item) in [("card", a_full_card()), ("ssh key", an_ssh_key_item())] {
            let texts = painted(&item, &TotpState::NoSecret);
            let runs: Vec<&String> = texts.iter().filter(|t| t.starts_with('•')).collect();
            assert!(
                !runs.is_empty(),
                "the {label} pane painted no masked row at all, so this proves nothing: {texts:?}"
            );
            for run in runs {
                assert_eq!(
                    *run, expected,
                    "a {label} masked row drew {} bullets instead of {MASKED_BULLETS}, so the \
                     mask still tracks the secret's length",
                    run.chars().count()
                );
            }
        }
    }

    #[test]
    fn the_ssh_private_key_is_not_painted_by_default() {
        let texts = painted(&an_ssh_key_item(), &TotpState::NoSecret);
        assert!(
            !contains(&texts, PRIVATE_KEY_BODY),
            "the SSH private key was painted in the clear by default: {texts:?}"
        );
        // The positive control, and what "Reveal" used to stand for: the
        // masked key still offers a way to see it. Exactly one eye, and
        // unstruck -- an open eye is what says "click to reveal".
        let mut pane = Pane::new();
        let frame = pane.idle(&an_ssh_key_item(), &TotpState::NoSecret);
        assert_eq!(
            frame.eyes.len(),
            1,
            "the SSH pane offers no way to reveal what it masked -- this test's whole \
             point is that a masked private key still has one"
        );
        assert_eq!(
            frame.struck_eyes(),
            0,
            "the masked key's eye is already struck through, so it is showing the wrong \
             state and 'click to hide' is what the user reads"
        );
    }

    /// **The positive control** for the test above: with the flag set, the
    /// very substring asserted absent there is painted. Without this, "the
    /// key is absent" would also pass against a pane that draws no rows.
    #[test]
    fn a_revealed_ssh_private_key_is_painted() {
        let texts = painted_with_reveal(
            &an_ssh_key_item(),
            &TotpState::NoSecret,
            RevealState {
                password: false,
                card_number: false,
                card_code: false,
                password_history: [false; MAX_HISTORY_ROWS],
                ssh_private_key: true,
            },
        );
        assert!(
            contains(&texts, PRIVATE_KEY_BODY),
            "a revealed SSH private key did not paint, so the pane ignores the caller's \
             reveal state: {texts:?}"
        );
        // The other direction of the same state: revealed, the eye is struck
        // through. Without this pair, `eye_toggle` could ignore its argument
        // and both states would look identical.
        let mut pane = Pane::new();
        pane.reveal.ssh_private_key = true;
        let frame = pane.idle(&an_ssh_key_item(), &TotpState::NoSecret);
        assert_eq!(
            frame.struck_eyes(),
            1,
            "a revealed private key's eye is not struck through, so the row still reads \
             as 'click to reveal' with the key on screen"
        );
    }

    /// Nothing but the private key is masked: a fingerprint behind bullets
    /// would be as wrong as a private key in front of them.
    #[test]
    fn nothing_but_the_private_key_is_masked_on_an_ssh_key() {
        let texts = painted(&an_ssh_key_item(), &TotpState::NoSecret);
        for visible in [PUBLIC_KEY, FINGERPRINT] {
            assert!(
                contains(&texts, visible),
                "{visible:?} was masked; only the private key may be: {texts:?}"
            );
        }
    }

    /// **Which flag feeds which row, across the two panes that have masked
    /// rows.** `each_card_secret_is_revealed_only_by_its_own_flag` pins the
    /// card's two against each other; this pins the fourth flag against them,
    /// which is the slip a fourth `&mut reveal.<field>` invites: passing
    /// `&mut reveal.card_number` to the private-key row compiles, and every
    /// other test in this file sets one item's flags at a time.
    ///
    /// Both directions. A card item and an SSH item are different kinds, so
    /// they cannot share a frame -- each `RevealState` is therefore painted
    /// onto both fixtures.
    #[test]
    fn the_ssh_private_key_and_the_card_secrets_do_not_share_a_flag() {
        let ssh_only = RevealState {
            password: false,
            card_number: false,
            card_code: false,
            password_history: [false; MAX_HISTORY_ROWS],
            ssh_private_key: true,
        };
        let texts = painted_with_reveal(&an_ssh_key_item(), &TotpState::NoSecret, ssh_only);
        assert!(
            contains(&texts, PRIVATE_KEY_BODY),
            "ssh_private_key: true did not reveal the private key: {texts:?}"
        );
        let texts = painted_with_reveal(&a_full_card(), &TotpState::NoSecret, ssh_only);
        assert!(
            !contains(&texts, "4242424242424242"),
            "revealing the SSH PRIVATE KEY also unmasked the card number -- the rows are \
             reading the same flag: {texts:?}"
        );
        assert!(
            !contains(&texts, "123"),
            "revealing the SSH PRIVATE KEY also unmasked the security code: {texts:?}"
        );

        let card_only = RevealState {
            password: false,
            card_number: true,
            card_code: true,
            password_history: [false; MAX_HISTORY_ROWS],
            ssh_private_key: false,
        };
        let texts = painted_with_reveal(&a_full_card(), &TotpState::NoSecret, card_only);
        assert!(
            contains(&texts, "4242424242424242"),
            "card_number: true did not reveal the number: {texts:?}"
        );
        let texts = painted_with_reveal(&an_ssh_key_item(), &TotpState::NoSecret, card_only);
        assert!(
            !contains(&texts, PRIVATE_KEY_BODY),
            "revealing the CARD's secrets also unmasked the SSH private key -- the rows \
             are reading the same flag: {texts:?}"
        );
    }

    /// A `type: 5` with no `sshKey` object is an *empty SSH key*, not an
    /// unsupported item -- the same rule the card pane follows, and for the
    /// same reason: an empty box under a heading reads as contents that
    /// failed to load.
    #[test]
    fn an_ssh_key_with_no_ssh_key_object_says_so_rather_than_drawing_blank_rows() {
        let texts = painted(&an_item(Some(5)), &TotpState::NoSecret);
        assert!(contains(&texts, "SSH KEY"), "{texts:?}");
        assert!(
            contains(&texts, "No SSH key details"),
            "an empty SSH key drew a heading over nothing: {texts:?}"
        );
        for absent in ["Public key", "Fingerprint", "Private key"] {
            assert!(
                !contains(&texts, absent),
                "an empty SSH key drew a {absent:?} row it has no data for: {texts:?}"
            );
        }
        // See `a_card_with_no_card_object_...`: the reveal control paints no
        // string, so its absence is asserted against the shape. Paired with
        // `the_ssh_private_key_is_not_painted_by_default`, where a populated
        // SSH key really does paint one.
        let mut pane = Pane::new();
        assert!(
            pane.idle(&an_item(Some(5)), &TotpState::NoSecret).eyes.is_empty(),
            "an empty SSH key drew a masked row it has no data for"
        );
    }

    /// The emptiness rule, expressed once and tested per field -- the same
    /// guard `a_card_is_empty_exactly_when_every_field_it_renders_is` gives
    /// the card pane. A field added to the rows and not to `is_empty` yields
    /// a pane that draws a row *and* says "No SSH key details on this item.".
    #[test]
    fn an_ssh_key_is_empty_exactly_when_every_field_it_renders_is() {
        use crate::vault_bridge::SshKeyData;
        assert!(ssh_key_fields(&SshKeyData::default()).is_empty());
        let one_at_a_time: [(&str, Box<dyn Fn(&mut SshKeyData)>); 3] = [
            ("public key", Box::new(|s: &mut SshKeyData| s.public_key = Some(PUBLIC_KEY.into()))),
            ("fingerprint", Box::new(|s: &mut SshKeyData| s.key_fingerprint = Some(FINGERPRINT.into()))),
            ("private key", Box::new(|s: &mut SshKeyData| s.private_key = Some(PRIVATE_KEY.to_string().into()))),
        ];
        for (name, populate) in one_at_a_time {
            let mut data = SshKeyData::default();
            populate(&mut data);
            assert!(
                !ssh_key_fields(&data).is_empty(),
                "an SSH key carrying only its {name} was called empty, so that row renders \
                 under a \"No SSH key details on this item.\" note"
            );
        }
    }

    /// Whitespace-only is absent, for the secret as much as the plain rows --
    /// and the trim is what makes the copied value character-for-character
    /// the painted one, the finding `card_fields` already carries.
    #[test]
    fn ssh_key_fields_trims_and_treats_blanks_as_absent() {
        use crate::vault_bridge::SshKeyData;
        let fields = ssh_key_fields(&SshKeyData {
            private_key: Some("  PRIV  ".to_string().into()),
            public_key: Some("   ".to_string()),
            key_fingerprint: Some(String::new()),
            other: serde_json::Map::new(),
        });
        assert_eq!(fields.private_key.as_deref(), Some("PRIV"));
        assert_eq!(fields.public_key, None);
        assert_eq!(fields.fingerprint, None);
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

    // -----------------------------------------------------------------
    // Design 2b, the header strip.
    // -----------------------------------------------------------------

    /// The report this work came from, as an assertion. Design 2b's detail
    /// column is `background: #f7f6f5` and the *only* white element on it is
    /// the header strip, which spans the pane's full width from its very top.
    #[test]
    fn the_pane_is_the_warm_grey_and_the_strip_across_its_top_is_the_white() {
        assert_ne!(
            theme::CARD,
            theme::WINDOW_BG,
            "this whole test is vacuous if the two surfaces are the same colour"
        );
        let rects = painted_rects(&an_item(Some(1)), &TotpState::NoSecret);
        let pane = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(PANE, PANE));
        assert!(
            rects
                .iter()
                .any(|(r, fill)| *fill == theme::WINDOW_BG && r.contains_rect(pane)),
            "the pane is not filled with the design's #f7f6f5: {rects:?}"
        );
        let strip = rects
            .iter()
            .find(|(r, fill)| {
                *fill == theme::CARD && r.top() == 0.0 && r.left() == 0.0 && r.right() == PANE
            })
            .unwrap_or_else(|| panic!("no full-width white header strip at the top: {rects:?}"));
        assert!(
            strip.0.height() >= 84.0,
            "the strip is shorter than the design's 20px padding + 44px avatar + 20px \
             padding: {:?}",
            strip.0
        );
    }

    /// The strip's own grid: `padding: 20px 24px`, a 44px avatar, `gap: 14px`,
    /// then a 22px/800 title over a 12px subtitle.
    #[test]
    fn the_header_strip_lays_its_avatar_and_title_out_on_the_designs_grid() {
        let item = an_item(Some(1));
        let rects = painted_rects(&item, &TotpState::NoSecret);
        let avatar = rects
            .iter()
            .find(|(r, fill)| *fill == theme::BLUE_WASH && r.width() == 44.0 && r.height() == 44.0)
            .unwrap_or_else(|| panic!("no 44px avatar tile in the header: {rects:?}"));
        assert_eq!(
            avatar.0.left(),
            24.0,
            "the avatar is not at the strip's 24px left padding"
        );
        assert_eq!(
            avatar.0.top(),
            20.0,
            "the avatar is not at the strip's 20px top padding -- if the title column \
             now stands taller than 44px, the centred row has pushed it down"
        );

        let painted = painted_type(&item, &TotpState::NoSecret, RevealState::default());
        let (title, title_font) = only(&painted, "Sample");
        assert_eq!(
            title.left(),
            82.0,
            "the title is not 24 + 44 + 14 from the pane's left edge"
        );
        assert_eq!(title_font.size, 22.0, "the title is not the design's 22px");
        assert_eq!(
            title_font.family,
            egui::FontFamily::Name(theme::EXTRABOLD.into()),
            "the title is not the design's 800 weight"
        );

        let (subtitle, subtitle_font) = only(&painted, "Login");
        assert_eq!(
            subtitle.left(),
            82.0,
            "the subtitle does not share the title's column"
        );
        assert_eq!(
            subtitle_font.size, 12.0,
            "the subtitle is not the design's 12px"
        );
    }

    /// The strip's one remaining button is the design's `height: 34px`
    /// filled primary, with its shortcut hint at 10px monospace beside its
    /// 13px label rather than appended to it at the label's own size.
    ///
    /// The outlined half of the old pair is gone with Edit; what stands
    /// beside the primary now is the star and the kebab, and
    /// `the_star_and_the_kebab_share_the_strips_34px_hit_target` pins those
    /// to the same height so the strip still sits on one line.
    #[test]
    fn the_header_primary_button_is_the_designs_34px_filled_control() {
        let item = an_item(Some(1));
        let rects = painted_rects(&item, &TotpState::NoSecret);
        assert!(
            rects
                .iter()
                .any(|(r, fill)| r.height() == 34.0 && *fill == theme::BLUE),
            "no 34px blue-filled \"Fill in app\" button: {rects:?}"
        );

        let painted = painted_type(&item, &TotpState::NoSecret, RevealState::default());
        assert_eq!(only(&painted, "Fill in app").1.size, 13.0);
        let (_, hint) = only(&painted, "CTRL+SHIFT+F");
        assert_eq!(hint.size, 10.0, "the shortcut hint is not the design's 10px");
        assert_eq!(hint.family, egui::FontFamily::Monospace);
    }

    /// The two drawn controls are square at the strip's own 34px control
    /// height, so their HIT TARGETS match the button between them rather
    /// than being only as big as the marks they paint.
    ///
    /// A star drawn at its own 18px would look identical in a screenshot and
    /// be half as easy to hit, which is exactly the kind of regression a
    /// shape-drawn control invites: nothing about the painted geometry says
    /// how big the clickable area is.
    #[test]
    fn the_star_and_the_kebab_share_the_strips_34px_hit_target() {
        let item = a_login();
        let mut pane = Pane::new();
        let frame = pane.idle(&item, &TotpState::NoSecret);
        let star = frame.star().rect;
        let kebab = frame.kebab();
        let primary = painted_rects(&item, &TotpState::NoSecret)
            .into_iter()
            .find(|(r, fill)| r.height() == 34.0 && *fill == theme::BLUE)
            .map(|(r, _)| r)
            .expect("no 34px primary button to measure the icons against");

        // The marks are smaller than the band they sit in, so the painted
        // geometry alone cannot say how big the hit target is. Two things
        // can: that both sit on the primary's own centre line, and that a
        // click near the TOP EDGE of the primary's 34px band -- well outside
        // the marks themselves -- still activates each of them.
        for (name, mark) in [("star", star), ("kebab", kebab)] {
            assert!(
                (mark.center().y - primary.center().y).abs() <= 0.5,
                "the {name} is not on the primary button's centre line"
            );
            assert!(
                mark.height() < primary.height(),
                "the {name}'s painted mark already fills the whole 34px band, so the \
                 edge click below proves nothing about its hit target"
            );
        }

        let corner = |mark: egui::Rect| egui::pos2(mark.center().x, primary.top() + 2.0);

        let mut star_pane = Pane::new();
        let _ = star_pane.idle(&item, &TotpState::NoSecret);
        let poked = star_pane.click(&item, &TotpState::NoSecret, corner(star));
        assert_eq!(
            poked.action,
            DetailAction::ToggleFavorite(true),
            "a click 2pt below the strip's top edge, over the star's column, missed the \
             star -- its target is smaller than the 34px control beside it"
        );

        let mut kebab_pane = Pane::new();
        let _ = kebab_pane.idle(&item, &TotpState::NoSecret);
        let _ = kebab_pane.click(&item, &TotpState::NoSecret, corner(kebab));
        assert!(
            kebab_pane.idle(&item, &TotpState::NoSecret).painted("Delete"),
            "a click 2pt below the strip's top edge, over the kebab's column, did not \
             open the menu -- its target is smaller than the 34px control beside it"
        );
    }

    /// **The strip fits at the width the app can actually be shrunk to, and
    /// this is the defect that motivated the whole change.**
    ///
    /// Every other geometry test in this file lays the pane out at
    /// [`PANE`] -- 900pt, which is a ~1500px window. The app's own minimum
    /// is 900px WIDE IN TOTAL, and at that size the detail column is
    /// [`MIN_PANE`]: 298pt. With four worded buttons in this strip,
    /// "Fill in app" was measured painting at x = -34.5..21.9 -- entirely
    /// off the pane -- and "Favourite" overlapping the item's own title.
    /// Nothing caught it, because nothing tried that width.
    ///
    /// The first attempt at this test asserted two things -- controls inside
    /// the pane, and `control.left() >= title.right()` -- and both held while
    /// the strip was, in fact, broken in three separate ways. The star was
    /// painted at x = 27.3..45.9, on top of an avatar tile occupying
    /// 24.0..68.0; the title was elided to a single "…" at x = 5.6..27.2, in
    /// the strip's own left padding; and the white strip itself started at
    /// x = -18.4, off the left edge of the pane. The non-overlap assertion
    /// passed by 0.1pt, and only because the title had collapsed to nothing.
    ///
    /// So this checks the four things that state actually violated:
    ///
    /// * every control inside the pane -- what the original test meant;
    /// * every control clear of the AVATAR, which nothing looked at;
    /// * the strip's own rect inside the pane, which is a layout overflow
    ///   independent of anything in it;
    /// * the title's RENDERED glyphs -- not `Galley::text()`, which returns
    ///   the source string and reported the full 26-character name off a
    ///   galley that drew one ellipsis.
    ///
    /// Rect intersection rather than a left/right comparison, because the
    /// controls are not necessarily beside the title any more: below about
    /// 420pt they move to their own line under it (see `header_layout`), and
    /// a one-axis test would then be trivially satisfied.
    #[test]
    fn every_header_control_fits_inside_the_minimum_width_pane() {
        let mut item = a_login();
        // A name long enough to want the room, since a short one leaves
        // slack that would hide exactly the overlap this pins.
        item.name = "Ledgerline Treasury Portal".to_string();

        let mut pane = Pane::wide(MIN_PANE);
        let frame = pane.idle(&item, &TotpState::NoSecret);
        let bounds = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(MIN_PANE, PANE));

        let title = frame.rect_of("Ledgerline Treasury Portal");
        let avatar = frame.avatar_tile();
        let controls = [
            ("the favourite star", frame.star().rect),
            ("the Fill in app button", frame.rect_of("Fill in app")),
            ("the kebab", frame.kebab()),
        ];

        for (name, rect) in controls {
            assert!(
                bounds.contains_rect(rect),
                "{name} is painted at x = {}..{} on a {MIN_PANE}pt pane -- outside it",
                rect.left(),
                rect.right()
            );
            assert!(
                !rect.intersects(avatar),
                "{name} is painted ON TOP of the item's avatar at the app's minimum window \
                 width: {name} {rect:?}, avatar {avatar:?}"
            );
            assert!(
                !rect.intersects(title),
                "{name} is painted ON TOP of the item's title at the app's minimum window \
                 width: {name} {rect:?}, title {title:?}"
            );
        }

        assert!(
            bounds.contains_rect(frame.header_strip()),
            "the white header strip itself runs to {:?} on a {MIN_PANE}pt pane -- it is \
             painting outside the column it belongs to",
            frame.header_strip()
        );

        // The positive control. Without it, a pane that painted no header at
        // all would satisfy every assertion above by vacuity -- and
        // `rect_of` would have caught that, but only for the two strings;
        // the star and the kebab are found by shape, and "no star" is not
        // distinguishable from "a star that fits" without this.
        assert_eq!(frame.stars.len(), 1, "no star painted at the minimum width");
        assert_eq!(frame.kebab_dots.len(), 3, "no kebab painted at the minimum width");
    }

    /// **A band, not a width.** The defect the test above pins was not a
    /// property of 298pt: the title was a bare "…" everywhere from 298 up to
    /// about 478, and the star sat on the avatar from 298 to about 338. A
    /// test at one width inside a broken band passes while the band is
    /// broken, and a fix tuned to one width leaves the rest of it.
    ///
    /// So: every pane width from the app's minimum up through the point where
    /// the strip has room to spare, checked for the one property the whole
    /// rearrangement exists to protect -- the item's name is still readable
    /// as a name.
    ///
    /// `rendered_glyphs`, not `rect_of`. The elided galley reports its full
    /// source text, so an assertion phrased over `texts` cannot fail here no
    /// matter what is on screen.
    #[test]
    fn the_title_is_never_reduced_to_an_ellipsis_at_any_width() {
        let mut item = a_login();
        item.name = "Ledgerline Treasury Portal".to_string();

        // 4pt steps through the two rearrangement thresholds (~420pt, where
        // the controls come back onto the title's line, and ~497pt, where the
        // shortcut hint returns) and out the far side of both.
        let mut width = MIN_PANE;
        while width <= 560.0 {
            let mut pane = Pane::wide(width);
            let frame = pane.idle(&item, &TotpState::NoSecret);
            let rendered = frame.rendered_glyphs("Ledgerline Treasury Portal");
            // The ellipsis and the spaces do not count: "…" alone, " …", and
            // "L…" are all the same failure.
            let readable = rendered
                .chars()
                .filter(|c| *c != '\u{2026}' && !c.is_whitespace())
                .count();
            assert!(
                readable >= 6,
                "at a {width}pt pane the header drew {rendered:?} for a title of {:?} -- \
                 the name has been truncated past the point of being a name",
                item.name
            );
            let bounds = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(width, PANE));
            let strip = frame.header_strip();
            assert!(
                bounds.contains_rect(strip),
                "at a {width}pt pane the white header strip is painted at {strip:?} -- \
                 outside the column"
            );
            let avatar = frame.avatar_tile();
            for (name, rect) in [
                ("the favourite star", frame.star().rect),
                ("the Fill in app button", frame.rect_of("Fill in app")),
                ("the kebab", frame.kebab()),
            ] {
                assert!(
                    bounds.contains_rect(rect),
                    "at a {width}pt pane {name} is painted at {rect:?} -- outside the column"
                );
                assert!(
                    !rect.intersects(avatar),
                    "at a {width}pt pane {name} is painted ON TOP of the item's avatar: \
                     {name} {rect:?}, avatar {avatar:?}"
                );
            }
            width += 4.0;
        }
    }

    /// The minimum width is the app's, not this test's. If
    /// `MIN_VAULT_WINDOW_SIZE` or either side panel moves, the assertion
    /// above follows it -- and this is what says so out loud, so a future
    /// reader does not have to re-derive 298 to know what is being checked.
    #[test]
    fn the_minimum_detail_pane_is_the_window_minimum_less_both_side_panels() {
        assert_eq!(MIN_PANE, 298.0);
        assert!(
            MIN_PANE < PANE,
            "the minimum-width test is laying out a WIDER pane than the ordinary one"
        );
    }

    // -----------------------------------------------------------------
    // The keyboard copies (CTRL+B / CTRL+U / CTRL+T)
    // -----------------------------------------------------------------

    /// The three chords, and the three fields, as a table -- so a
    /// `copy_shortcut_action` that returned one fixed answer cannot pass.
    #[test]
    fn each_binding_copies_its_own_field() {
        let code = TotpState::Code {
            code: "123456".to_string(),
            seconds_left: 9,
        };
        assert_eq!(
            copy_shortcut_action(CopyShortcut::Password, "u", "p", &code),
            Some(DetailAction::CopyPassword)
        );
        assert_eq!(
            copy_shortcut_action(CopyShortcut::Username, "u", "p", &code),
            Some(DetailAction::CopyUsername)
        );
        assert_eq!(
            copy_shortcut_action(CopyShortcut::Totp, "u", "p", &code),
            Some(DetailAction::CopyTotp)
        );
    }

    /// **A binding whose field is missing copies NOTHING.** Not an empty
    /// string, and not some other field: the clipboard is a global the user
    /// is about to paste, and both wrong answers are silent.
    ///
    /// Each case leaves the *other* two fields populated, so "returns None
    /// for everything" is not what makes it pass.
    #[test]
    fn a_binding_whose_field_is_absent_copies_nothing() {
        let code = TotpState::Code {
            code: "123456".to_string(),
            seconds_left: 9,
        };
        assert_eq!(
            copy_shortcut_action(CopyShortcut::Username, "", "p", &code),
            None,
            "CTRL+U on an item with no username put something on the clipboard"
        );
        assert_eq!(
            copy_shortcut_action(CopyShortcut::Password, "u", "", &code),
            None,
            "CTRL+B on an item with no password put something on the clipboard"
        );
        for empty in [
            TotpState::NoSecret,
            TotpState::Fetching,
            TotpState::Unavailable,
            TotpState::NoCodeReported,
        ] {
            assert_eq!(
                copy_shortcut_action(CopyShortcut::Totp, "u", "p", &empty),
                None,
                "CTRL+T copied something while the TOTP state was {empty:?} -- there is \
                 no code to copy in it"
            );
        }
    }

    /// The hints say what the code binds, because they are the same table.
    /// A row advertising `CTRL+B` beside a handler wired to something else
    /// is worse than no hint at all.
    #[test]
    fn every_binding_has_a_hint_that_names_its_own_key() {
        for (which, key, hint) in COPY_SHORTCUTS {
            assert_eq!(copy_shortcut_hint(which), hint);
            assert_eq!(
                hint,
                format!("CTRL+{}", key.name()),
                "{which:?}'s hint does not spell the key it is bound to"
            );
        }
    }

    /// And the hints really paint, on the rows they belong to.
    #[test]
    fn the_copy_hints_paint_on_the_rows_they_belong_to() {
        let mut item = a_login();
        item.login.as_mut().expect("a_login has login data").totp =
            Some("seed".to_string().into());
        let totp = TotpState::Code {
            code: "123456".to_string(),
            seconds_left: 9,
        };
        let mut pane = Pane::new();
        let frame = pane.idle(&item, &totp);

        for (label, hint) in [
            ("Username", "CTRL+U"),
            ("Password", "CTRL+B"),
            ("One-time code", "CTRL+T"),
        ] {
            let row = frame.rect_of(label);
            let tag = frame.rect_of(hint);
            assert!(
                (tag.center().y - row.center().y).abs() <= 2.0,
                "the {hint} hint is not on the {label:?} row's own line; the pane \
                 painted: {:?}",
                frame.strings()
            );
        }
    }

    /// **The chords are wired.** The decision above is pure and tested
    /// directly; this is the other half -- that `draw_detail_read` actually
    /// consults it, with a real key event rather than a source-text guard.
    #[test]
    fn pressing_each_chord_asks_for_that_chords_copy() {
        let mut item = a_login();
        item.login.as_mut().expect("a_login has login data").totp =
            Some("seed".to_string().into());
        let totp = TotpState::Code {
            code: "123456".to_string(),
            seconds_left: 9,
        };
        for (key, want) in [
            (egui::Key::B, DetailAction::CopyPassword),
            (egui::Key::U, DetailAction::CopyUsername),
            (egui::Key::T, DetailAction::CopyTotp),
        ] {
            let mut pane = Pane::new();
            let idle = pane.idle(&item, &totp);
            assert_eq!(
                idle.action,
                DetailAction::None,
                "the pane reported an action on a frame with no input at all"
            );
            let pressed = pane.frame(&item, &totp, ctrl(key));
            assert_eq!(
                pressed.action, want,
                "CTRL+{} reported {:?}",
                key.name(),
                pressed.action
            );
        }
    }

    /// The same chord on an item that has nothing for it stays silent all
    /// the way through the closure, not just in the pure function.
    #[test]
    fn a_chord_with_no_field_behind_it_reports_nothing_through_the_pane() {
        // A card: no username, no password, no TOTP.
        let item = a_full_card();
        for key in [egui::Key::B, egui::Key::U, egui::Key::T] {
            let mut pane = Pane::new();
            let _ = pane.idle(&item, &TotpState::NoSecret);
            let pressed = pane.frame(&item, &TotpState::NoSecret, ctrl(key));
            assert_eq!(
                pressed.action,
                DetailAction::None,
                "CTRL+{} on a card copied {:?}",
                key.name(),
                pressed.action
            );
        }
        // The positive control: the same harness, the same chords, on an
        // item that DOES carry the fields. Without it this test passes
        // against a harness whose key events never arrive.
        let login = a_login();
        let mut pane = Pane::new();
        let _ = pane.idle(&login, &TotpState::NoSecret);
        assert_eq!(
            pane.frame(&login, &TotpState::NoSecret, ctrl(egui::Key::B))
                .action,
            DetailAction::CopyPassword,
            "no chord reaches this pane at all, so the silence above proves nothing"
        );
    }

    /// **CTRL and nothing else.** Against `consume_key` -- egui's obvious
    /// call, and what this pane used -- every one of these put a secret on
    /// the clipboard: `Modifiers::matches_logically` rejects an event only
    /// for *lacking* a modifier the pattern wants, so CTRL+ALT+B and
    /// CTRL+SHIFT+B were both CTRL+B. Measured against the code as it stood:
    /// `CTRL+ALT+B -> CopyPassword`, `CTRL+SHIFT+B -> CopyPassword`,
    /// `CTRL+ALT+U -> CopyUsername`.
    ///
    /// The chord that made this urgent is CTRL+ALT+B, the app's global fill
    /// hotkey -- which never reaches egui only because `global-hotkey` takes
    /// it through Win32 `RegisterHotKey`, a guarantee about that one chord
    /// and no other. CTRL+SHIFT+B has no such cover.
    ///
    /// The plain-key row is the other half: a copy that fired on a bare `B`
    /// would pass an extra-modifier test and be far worse.
    #[test]
    fn an_extra_modifier_does_not_fire_a_copy() {
        let item = a_login();
        let modified = |modifiers: egui::Modifiers, key: egui::Key| {
            vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }]
        };
        let alt_ctrl = egui::Modifiers::CTRL | egui::Modifiers::ALT;
        let shift_ctrl = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
        for (name, modifiers, key) in [
            ("CTRL+ALT+B", alt_ctrl, egui::Key::B),
            ("CTRL+SHIFT+B", shift_ctrl, egui::Key::B),
            ("CTRL+ALT+U", alt_ctrl, egui::Key::U),
            ("CTRL+SHIFT+U", shift_ctrl, egui::Key::U),
            ("ALT+B", egui::Modifiers::ALT, egui::Key::B),
            ("plain B", egui::Modifiers::NONE, egui::Key::B),
        ] {
            let mut pane = Pane::new();
            let _ = pane.idle(&item, &TotpState::NoSecret);
            let pressed = pane.frame(&item, &TotpState::NoSecret, modified(modifiers, key));
            assert_eq!(
                pressed.action,
                DetailAction::None,
                "{name} copied {:?} -- a chord this pane does not bind put a secret on the \
                 clipboard",
                pressed.action
            );
        }

        // The positive control: the exact chord, through the same harness.
        let mut pane = Pane::new();
        let _ = pane.idle(&item, &TotpState::NoSecret);
        assert_eq!(
            pane.frame(&item, &TotpState::NoSecret, ctrl(egui::Key::B))
                .action,
            DetailAction::CopyPassword,
            "no chord reaches this pane at all, so the silence above proves nothing"
        );
    }

    // -----------------------------------------------------------------
    // Design 2b, the cards and their rows.
    // -----------------------------------------------------------------

    /// A login carrying the design's own sample values.
    fn a_login() -> VaultItem {
        let mut item = an_item(Some(1));
        item.login = Some(crate::vault_bridge::LoginData {
            username: Some("a.novak@ledgerline.com".to_string()),
            password: Some("hunter2".to_string().into()),
            totp: None,
            uris: vec![crate::vault_bridge::UriEntry {
                uri: Some("app.ledgerline.com".to_string()),
                other: serde_json::Map::new(),
            }],
            other: serde_json::Map::new(),
        });
        item
    }

    /// A row is `display: flex; align-items: center; gap: 16px; padding: 13px
    /// 16px` over a `width: 130px` label column. On this pane that puts the
    /// label at 24 (body padding) + 1 (the card's own border, which the design
    /// draws inside the box and egui likewise takes out of the content rect)
    /// + 16 (row padding) = 41, and the value at 41 + 130 (label column) + 16
    /// (gap) = 187. Both absolute: nothing here is computed from the constants
    /// being checked.
    #[test]
    fn a_card_row_puts_its_label_and_value_on_the_designs_two_columns() {
        let painted = painted_type(&a_login(), &TotpState::NoSecret, RevealState::default());

        let (label, label_font) = only(&painted, "Username");
        assert_eq!(
            label.left(),
            41.0,
            "the label column does not start at 24 + 1 + 16"
        );
        assert_eq!(label_font.size, 12.0, "the label is not the design's 12px");

        let (value, value_font) = only(&painted, "a.novak@ledgerline.com");
        assert_eq!(
            value.left(),
            187.0,
            "the value does not start at 41 + a 130px label column + a 16px gap"
        );
        assert_eq!(value_font.size, 14.0, "the value is not the design's 14px");
        assert!(
            (value.center().y - label.center().y).abs() <= 1.0,
            "the label and the value are not on one centred line: {label:?} vs {value:?}"
        );
    }

    /// The card's own heading rule: `padding: 11px 16px; font-size: 12px;
    /// font-weight: 700; text-transform: uppercase`, over a card that is
    /// white with a `#eae7e7` border.
    #[test]
    fn a_cards_heading_is_the_designs_tracked_uppercase_over_a_bordered_tile() {
        let item = a_login();
        let painted = painted_type(&item, &TotpState::NoSecret, RevealState::default());
        let (heading, font) = only(&painted, "LOGIN CREDENTIALS");
        assert_eq!(
            heading.left(),
            41.0,
            "the heading is not at the card's 16px padding"
        );
        assert_eq!(font.size, 12.0, "the heading is not the design's 12px");
        assert_eq!(
            font.family,
            egui::FontFamily::Name(theme::BOLD.into()),
            "the heading is not the design's 700 weight"
        );

        let rects = painted_rects(&item, &TotpState::NoSecret);
        assert!(
            rects
                .iter()
                .any(|(r, fill)| *fill == theme::CARD && r.left() == 24.0 && r.right() == 876.0),
            "no white card spanning the body's 24px padding on both sides: {rects:?}"
        );
    }

    /// **Every copyable row copies its OWN value.**
    ///
    /// The previous version of this test asserted that each row painted a
    /// "Copy" button on its own centred line -- one per row rather than one
    /// per card. The buttons are gone (the user asked for keys and a tile
    /// click instead), so the same property is asserted where it actually
    /// matters: a click on each row reports that row's copy and not its
    /// neighbour's. That is strictly stronger. The old test passed against
    /// five buttons all wired to `CopyValue(cardholder)`; this one does not.
    #[test]
    fn every_copyable_row_copies_its_own_value() {
        let item = a_full_card();
        let expected = [
            ("Cardholder name", DetailAction::CopyValue("John Doe".to_string())),
            ("Brand", DetailAction::CopyValue("visa".to_string())),
            ("Number", DetailAction::CopyCardNumber),
            ("Expiry", DetailAction::CopyValue("04/2023".to_string())),
            ("Security code", DetailAction::CopyCardCode),
        ];
        for (label, want) in expected {
            let mut pane = Pane::new();
            let laid_out = pane.idle(&item, &TotpState::NoSecret);
            let row = laid_out.rect_of(label);
            let clicked = pane.click(&item, &TotpState::NoSecret, row.center());
            assert_eq!(
                clicked.action, want,
                "clicking the {label:?} row reported {:?}",
                clicked.action
            );
        }
    }

    /// The password row goes through the same `masked_row` the two card
    /// secrets do, so restyling that function restyles all three at once --
    /// and nothing pinned the password half of it. Both directions, so this
    /// cannot pass on a row that is always masked or always clear.
    #[test]
    fn a_logins_password_is_masked_by_default_and_revealed_only_by_its_own_flag() {
        let item = a_login();
        let masked = painted(&item, &TotpState::NoSecret);
        assert!(
            !contains(&masked, "hunter2"),
            "the password was painted in the clear by default: {masked:?}"
        );
        let mut pane = Pane::new();
        assert_eq!(
            pane.idle(&item, &TotpState::NoSecret).eyes.len(),
            1,
            "the password row offers no way to reveal what it masked"
        );

        let revealed = painted_with_reveal(
            &item,
            &TotpState::NoSecret,
            RevealState {
                password: true,
                card_number: false,
                card_code: false,
                password_history: [false; MAX_HISTORY_ROWS],
                ssh_private_key: false,
            },
        );
        assert!(
            contains(&revealed, "hunter2"),
            "password: true did not reveal the password: {revealed:?}"
        );
    }

    /// The design's last card is the metadata strip -- a white tile like the
    /// others, `padding: 13px 16px; font-size: 12px`, not a bare line of 11px
    /// ghost text sitting on the pane's grey.
    #[test]
    fn the_metadata_strip_is_a_card_of_its_own_rather_than_bare_text_on_the_pane() {
        let item = a_login();
        let painted = painted_type(&item, &TotpState::NoSecret, RevealState::default());
        let (strip, font) = painted
            .iter()
            .find(|(t, _, _)| t.contains("Filled 3 times"))
            .map(|(_, r, f)| (*r, f.clone()))
            .unwrap_or_else(|| panic!("no metadata strip painted: {painted:?}"));
        assert_eq!(font.size, 12.0, "the metadata strip is not the design's 12px");
        assert_eq!(strip.left(), 41.0, "the strip's text is not at 24 + 1 + 16");

        let rects = painted_rects(&item, &TotpState::NoSecret);
        assert!(
            rects
                .iter()
                .any(|(r, fill)| *fill == theme::CARD && r.contains_rect(strip) && r.left() == 24.0),
            "the metadata strip is not inside a card of its own: {rects:?}"
        );
    }

    // -----------------------------------------------------------------
    // Favourites
    // -----------------------------------------------------------------

    /// **The star states the current state, and both states are drawn.**
    ///
    /// This used to read the words "Favourite"/"Favourited" off the header.
    /// The control paints no words now, so what is asserted is the only
    /// thing left that distinguishes the two: a favourited item's star is
    /// FILLED, in the palette's own "on" blue, and an un-favourited one's is
    /// an outline that is not blue.
    ///
    /// Both directions, exactly as before -- one alone would pass against a
    /// star hardcoded to whichever state the test happened to expect, which
    /// is the same defect the two labels were guarding against.
    #[test]
    fn the_header_star_is_filled_exactly_when_the_item_is_a_favourite() {
        let mut item = a_login();

        item.favorite = false;
        let off = Pane::new().idle(&item, &TotpState::NoSecret).star();
        assert!(
            !off.filled,
            "an un-favourited item's star is filled, so it claims the item already is one"
        );
        assert_ne!(
            off.stroke,
            theme::BLUE,
            "an un-favourited item's star is drawn in the palette's ON colour"
        );

        item.favorite = true;
        let on = Pane::new().idle(&item, &TotpState::NoSecret).star();
        assert!(on.filled, "a favourited item's star is not filled, so it does not say so");
        assert_eq!(
            on.stroke,
            theme::BLUE,
            "a favourited item's star is not the design's primary blue -- ERROR red is \
             reserved for failures and cannot be borrowed for an ON state"
        );
    }

    #[test]
    fn every_kind_gets_the_favourite_control_even_the_ones_it_cannot_fill_or_edit() {
        // The gating decision, asserted as behaviour rather than read off
        // `kind_offers_fill`. A favourite is a property of the item as a ROW,
        // not of its contents, and `SidebarFilter::Favorites` applies to every
        // kind -- so gating this the way Fill is gated would make a filter the
        // sidebar offers unreachable for four of the five kinds.
        for kind in EVERY_KIND {
            let item = an_item(item_type_for(kind));
            let frame = Pane::new().idle(&item, &TotpState::NoSecret);
            assert_eq!(
                frame.stars.len(),
                1,
                "{kind:?} has no favourite control; the pane painted: {:?}",
                frame.strings()
            );
        }
    }

    #[test]
    fn clicking_the_favourite_control_asks_for_the_opposite_state_not_a_bare_toggle() {
        // A real pointer press on the control, twice: once from each starting
        // state. What is pinned is the PAYLOAD -- the action carries the state
        // the item should end up in, computed from the item this pane drew,
        // so `vault_window::mod` never re-derives `!favorite` from a copy that
        // could differ. A bare `ToggleFavorite` would pass a weaker version of
        // this test and reintroduce that gap.
        //
        // The star is found by its geometry rather than by a label now (see
        // `theme::icon_probe`), which is the only change: a drawn control is
        // exactly as clickable as a worded one, and this test is what proves
        // it stayed so.
        for starting_state in [false, true] {
            let mut item = a_login();
            item.favorite = starting_state;

            let mut pane = Pane::new();
            let laid_out = pane.idle(&item, &TotpState::NoSecret);
            let control = laid_out.star().rect.center();

            let clicked = pane.click(&item, &TotpState::NoSecret, control);
            assert_eq!(
                clicked.action,
                DetailAction::ToggleFavorite(!starting_state),
                "clicking the favourite star on an item whose favorite is \
                 {starting_state} did not ask for {}",
                !starting_state
            );
        }
    }

    // -----------------------------------------------------------------
    // Previous passwords
    // -----------------------------------------------------------------

    /// A login carrying two previous passwords, in the wire shape the CLI's
    /// own `PasswordHistoryResponse` builds.
    fn a_login_with_history(entries: usize) -> VaultItem {
        let mut item = a_login();
        let history: Vec<serde_json::Value> = (0..entries)
            .map(|i| {
                serde_json::json!({
                    "lastUsedDate": "2026-07-30T09:15:00.000Z",
                    "password": format!("old-secret-{i}"),
                })
            })
            .collect();
        item.other
            .insert("passwordHistory".to_string(), serde_json::Value::Array(history));
        item
    }

    #[test]
    fn an_item_with_no_password_history_gets_no_card_at_all() {
        // A heading over no rows reads as previous passwords that failed to
        // load -- the same argument `notes_text` makes for the notes card.
        let texts = painted(&a_login(), &TotpState::NoSecret);
        assert!(
            !contains(&texts, "PREVIOUS PASSWORDS"),
            "an item with no history still drew the card: {texts:?}"
        );
    }

    /// **Masked by default, negative assertion, WITH A POSITIVE CONTROL.**
    /// The absence half alone is not evidence: a pane that rendered the card
    /// not at all, or rendered nothing whatever, satisfies it. So the same
    /// frame must also show the card's heading and a Reveal control, and the
    /// test below must show the value painting when the flag is set.
    #[test]
    fn previous_passwords_are_masked_by_default() {
        let texts = painted(&a_login_with_history(2), &TotpState::NoSecret);
        assert!(
            contains(&texts, "PREVIOUS PASSWORDS"),
            "the history card did not render, so the masking assertion below would \
             pass against a pane drawing nothing: {texts:?}"
        );
        assert!(
            !contains(&texts, "old-secret-0"),
            "a previous password was painted in the clear by default: {texts:?}"
        );
        assert!(
            !contains(&texts, "old-secret-1"),
            "a previous password was painted in the clear by default: {texts:?}"
        );
        // One eye per history row, plus the login's own password row. Two
        // history entries here, so three -- a count, not a `contains`, so a
        // card that grew a heading and lost its rows fails.
        let mut pane = Pane::new();
        assert_eq!(
            pane.idle(&a_login_with_history(2), &TotpState::NoSecret)
                .eyes
                .len(),
            3,
            "the history card offers no way to reveal what it masked"
        );
    }

    #[test]
    fn a_revealed_previous_password_paints_in_the_clear() {
        // The positive control for the test above.
        let mut reveal = RevealState::default();
        reveal.password_history[0] = true;
        reveal.password_history[1] = true;
        let texts = painted_with_reveal(&a_login_with_history(2), &TotpState::NoSecret, reveal);
        assert!(
            contains(&texts, "old-secret-0"),
            "password_history[0] did not reveal the first entry: {texts:?}"
        );
        assert!(
            contains(&texts, "old-secret-1"),
            "password_history[1] did not reveal the second entry: {texts:?}"
        );
    }

    /// **Which flag feeds which row**, the same property
    /// `each_card_secret_is_revealed_only_by_its_own_flag` pins one card over
    /// -- and the reason it is pinned again here is that the slip it catches
    /// was made for real: a `&mut reveal.card_number` passed to the wrong row
    /// renders perfectly and unmasks a secret the user did not ask to see.
    /// With five history rows the index is written once and reused, so an
    /// off-by-one or a constant index would look identical on screen for the
    /// first row.
    #[test]
    fn each_history_row_is_revealed_only_by_its_own_flag() {
        let item = a_login_with_history(3);

        let mut first_only = RevealState::default();
        first_only.password_history[0] = true;
        let texts = painted_with_reveal(&item, &TotpState::NoSecret, first_only);
        assert!(contains(&texts, "old-secret-0"), "index 0 did not reveal row 0: {texts:?}");
        assert!(
            !contains(&texts, "old-secret-1"),
            "revealing row 0 also unmasked row 1: {texts:?}"
        );
        assert!(
            !contains(&texts, "old-secret-2"),
            "revealing row 0 also unmasked row 2: {texts:?}"
        );

        let mut middle_only = RevealState::default();
        middle_only.password_history[1] = true;
        let texts = painted_with_reveal(&item, &TotpState::NoSecret, middle_only);
        assert!(contains(&texts, "old-secret-1"), "index 1 did not reveal row 1: {texts:?}");
        assert!(
            !contains(&texts, "old-secret-0"),
            "revealing row 1 also unmasked row 0 -- the rows share a flag, or the \
             index is constant: {texts:?}"
        );
        assert!(
            !contains(&texts, "old-secret-2"),
            "revealing row 1 also unmasked row 2: {texts:?}"
        );
    }

    #[test]
    fn a_history_longer_than_the_pane_can_reveal_says_how_much_it_is_hiding() {
        // Unreachable against today's backend -- Bitwarden slices every save
        // to five entries -- which is exactly why it needs a test: an omitted
        // previous password is indistinguishable from one the user never had,
        // and this pane is the only place in the app they are visible.
        let texts = painted(&a_login_with_history(MAX_HISTORY_ROWS + 3), &TotpState::NoSecret);
        assert!(
            contains(&texts, "3 older passwords are not shown"),
            "a truncated history did not say how many rows it dropped: {texts:?}"
        );
    }

    #[test]
    fn a_history_that_fits_says_nothing_about_hidden_rows() {
        // The mirror: the notice must not appear when nothing is hidden.
        let texts = painted(&a_login_with_history(MAX_HISTORY_ROWS), &TotpState::NoSecret);
        assert!(
            !contains(&texts, "not shown"),
            "a history that fits claimed rows were hidden: {texts:?}"
        );
    }

    #[test]
    fn a_history_row_is_labelled_by_when_that_password_stopped_being_current() {
        // The label column is what distinguishes one masked row from another,
        // so it carries the date. Absent and unparseable both fall back to a
        // word rather than a fabricated number, the same rule `updated_text`
        // follows.
        assert_eq!(history_label(None), "Earlier");
        assert_eq!(history_label(Some("not-a-date")), "Earlier");
        assert_eq!(history_label(Some("1970-01-01T00:00:00.000Z")).ends_with("days ago"), true);

        let mut item = a_login();
        item.other.insert(
            "passwordHistory".to_string(),
            serde_json::json!([{ "password": "dateless" }]),
        );
        let texts = painted(&item, &TotpState::NoSecret);
        assert!(
            contains(&texts, "Earlier"),
            "a dated-less history row lost its label entirely: {texts:?}"
        );
    }
}

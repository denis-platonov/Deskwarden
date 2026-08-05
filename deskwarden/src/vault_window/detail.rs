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
use crate::app_match::{AppMatch, TriggerMode};
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
    /// The `MATCHED APP` card's trigger control was set to a **new** mode,
    /// carrying the mode the match should end up in.
    ///
    /// Carrying the target rather than "the control was clicked", for exactly
    /// [`Self::ToggleFavorite`]'s reason: the pane already read the match's
    /// current trigger to decide which pill is filled, so it is the one that
    /// knows what the other state is.
    ///
    /// It carries **only the trigger**, not a whole [`AppMatch`]: the rest of
    /// the match (`process`, `title`, `hosted`, `path`) is the picker's
    /// capture off a live window, and a card that handed a rebuilt copy back
    /// would be a second producer of those four fields. The caller reads the
    /// current match off the item and changes one field --
    /// [`app_match_with_trigger`] is that change, so the pane and the caller
    /// cannot disagree about what "only the trigger" means.
    ///
    /// Reported only when the mode actually differs; see
    /// [`app_trigger_click`], which is where that gate lives so a click on
    /// the already-selected pill cannot cost a vault write.
    SetAppTrigger(TriggerMode),
    /// The `MATCHED APP` card's Remove was clicked: the item should stop
    /// being bound to any app at all.
    ///
    /// **No value, because there is nothing to choose.** The caller resolves
    /// this through `vault_bridge::without_app_match`, which removes the
    /// custom field rather than blanking it.
    RemoveAppMatch,
    /// The `MATCHED APP` card's Open was clicked for the APP: start the bound
    /// program.
    ///
    /// **It carries a [`LaunchPlan`] and not the [`AppMatch`] it came from,
    /// and that is the whole point.** Every question about whether this may
    /// run at all -- is the binding dead, is it a Store app, does
    /// [`AppMatch::launchable_path`] accept the recorded path, is the item's
    /// URI an `http(s)` one worth appending -- is answered ONCE, in
    /// [`app_launch_plan`], which is a pure function a test can call. A
    /// variant carrying the match would put those five questions in the
    /// caller as well, and the caller is inside `vault_window::mod`'s event
    /// loop where nothing can call them.
    ///
    /// So: **a `LaunchPlan` existing is the permission.** `vault_window::mod`
    /// does not re-check and must never re-derive -- see `launch_app`, whose
    /// doc says the same thing from the other end.
    ///
    /// The website half of the same control is NOT a second variant: it is
    /// [`Self::OpenWebsite`], which already exists and already goes through
    /// `is_safe_web_url` and `ShellExecuteW`.
    OpenApp(LaunchPlan),
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

/// The values this pane offers a keyboard copy for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyShortcut {
    Username,
    Password,
    Totp,
    /// The AUTOFILL TARGETS card's website. The one entry here that is not a
    /// secret, and the one whose row also does something else on click --
    /// see the Website row in [`draw_detail_read`].
    Url,
}

/// The bindings, their keys and the chord each row's tooltip names, in ONE
/// table.
///
/// One table because the third of those is a promise about the other two: a
/// row advertising `CTRL+B` beside a handler wired to something else is
/// worse than no hint at all, and the only way to make that impossible is
/// for the spelled chord and the key to be the same tuple. The chord used to
/// be *painted* beside each row and is now carried by the row's hover
/// tooltip instead ([`copy_row_tooltip`]) -- a change of surface, not of
/// source: it is still this table's third field and nothing else.
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
/// CTRL+SHIFT+U, the website copy, is KeePass's again: it uses CTRL+U for
/// *open* URL and CTRL+SHIFT+U for *copy* URL. CTRL+U is already this app's
/// copy-username, so the shifted form is the one that stays consistent with
/// both. Checked free rather than assumed, over the whole crate: the only
/// SHIFT chord anywhere in it is `vault_window::mod`'s CTRL+SHIFT+F, and the
/// only other binding on `U` is the CTRL+U above.
///
/// **A shifted chord is only safe here because of `matches_exact`.** Under
/// the `consume_key` this pane used to call, CTRL+SHIFT+U *was* CTRL+U and a
/// website copy could not have been told apart from a username copy at all --
/// which is also why the two live in the same table with their modifiers in
/// it, rather than one of them being special-cased somewhere else.
const COPY_SHORTCUTS: [(CopyShortcut, egui::Modifiers, egui::Key, &str); 4] = [
    (CopyShortcut::Password, egui::Modifiers::CTRL, egui::Key::B, "CTRL+B"),
    (CopyShortcut::Username, egui::Modifiers::CTRL, egui::Key::U, "CTRL+U"),
    (CopyShortcut::Totp, egui::Modifiers::CTRL, egui::Key::T, "CTRL+T"),
    (
        CopyShortcut::Url,
        egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT),
        egui::Key::U,
        "CTRL+SHIFT+U",
    ),
];

/// How one binding is spelled to the user, read out of [`COPY_SHORTCUTS`] and
/// never written out a second time.
fn copy_shortcut_chord(which: CopyShortcut) -> &'static str {
    COPY_SHORTCUTS
        .iter()
        .find(|(candidate, _, _, _)| *candidate == which)
        .map(|(_, _, _, chord)| *chord)
        .expect("COPY_SHORTCUTS covers every CopyShortcut variant")
}

/// **The one place a chord-bound field's NAME is written.**
///
/// The row paints this as its label and the copy confirmation names the same
/// string, so the two cannot drift: there is no second list of field names to
/// fall out of step with the first. That is the whole reason this exists
/// rather than the toast carrying its own words -- this crate has been bitten
/// repeatedly by parallel tables, and a confirmation reading "Password
/// copied" beside a row labelled something else is exactly that bug wearing a
/// new hat.
///
/// It is deliberately keyed on [`CopyShortcut`] rather than on
/// [`DetailAction`]: a `DetailAction`-keyed table would have to name
/// [`DetailAction::CopyValue`], which is one variant shared by the website,
/// every identity field and three card fields, and could not tell them apart
/// without -- again -- a second list. Rows with no chord name themselves the
/// same way they always have, with the literal they paint, which
/// [`copy_row`] reads straight off the row it just drew.
fn copy_shortcut_label(which: CopyShortcut) -> &'static str {
    match which {
        CopyShortcut::Username => "Username",
        CopyShortcut::Password => "Password",
        CopyShortcut::Totp => "One-time code",
        CopyShortcut::Url => "Website",
    }
}

/// How long a copy confirmation stays up, in seconds. The user asked for
/// "5 seconds tooltip".
const COPY_TOAST_SECONDS: f64 = 5.0;

/// How far the confirmation sits in from the window's bottom-right corner.
///
/// Clear of `login_ui`'s resize handles, which live in their own foreground
/// layer along the very edge, and clear of the detail pane's own controls --
/// every one of which (Edit, Fill, the kebab, the star) is in the header
/// strip at the TOP of the pane. It also cannot cover the row that was just
/// clicked: the rows are laid out from the top of the body downwards and this
/// is anchored to the opposite corner.
const COPY_TOAST_INSET: f32 = 20.0;

/// The confirmation's type size -- the pane's own body size (`ROW_VALUE_SIZE`
/// is 15, a row label 12); 13 is the design's plain-text size, used here so
/// the toast reads as this app rather than as a system notification.
const COPY_TOAST_TEXT_SIZE: f32 = 13.0;

/// What was copied, and when -- on egui's own frame clock (`InputState::time`,
/// seconds), not `Instant`, so a test can drive the whole lifetime by running
/// frames or by handing `RawInput::time` a number.
///
/// Kept in the context's temporary data rather than threaded through
/// [`draw_detail_read`]'s signature because the two writers are a widget deep
/// inside the pane ([`copy_row`]) and the chord resolution at the end of the
/// pane, and neither can reach a local of the other without a new out-param
/// on five row helpers. The DECISION does not live in there: see
/// [`copy_toast_now`], which is a pure function of this value and the time,
/// and is what the tests call.
#[derive(Clone, Debug, PartialEq)]
struct CopyToast {
    /// The row's label. **Never the value.** See [`copy_toast_text`].
    label: String,
    shown_at: f64,
}

/// Where [`CopyToast`] lives in `egui`'s temporary data.
fn copy_toast_id() -> egui::Id {
    egui::Id::new("detail-copy-toast")
}

/// Where the id of the item the pane drew LAST lives.
///
/// Companion to [`copy_toast_id`], and deliberately a second entry rather than
/// a field on [`CopyToast`]: the toast is written by [`copy_row`], a widget
/// several helpers deep inside the body, and the item id is not reachable from
/// there without the new out-param on every row helper that `CopyToast` exists
/// to avoid (see its doc). The pane's one entry point knows the item; this is
/// where it says so.
fn copy_toast_item_id() -> egui::Id {
    egui::Id::new("detail-copy-toast-item")
}

/// Drops any live confirmation when the pane starts drawing a **different**
/// item, and records the item now being drawn.
///
/// The confirmation is context-global, so without this it followed the pane:
/// copy the password on a login, click any other item inside the five seconds,
/// and the new item painted "Password copied" -- on an item that may have no
/// Password row at all. That is this feature's own central claim (the toast
/// names the row it belongs to) failing across a selection change.
///
/// **Cleared, not merely filtered.** Hiding a mismatched toast would leave it
/// in the map, so copying on A, glancing at B and coming back to A inside the
/// five seconds would RESURRECT a confirmation for a copy the user has since
/// looked away from -- almost as wrong as the followed one. Removing it makes
/// "you left the item" and "it expired" the same end state.
///
/// **Keyed on the item, not on the redraw.** A vault refresh, a write landing
/// or any other reason the pane redraws the SAME item leaves the id equal and
/// the toast alone: the confirmation belongs to the item, not to the frame.
fn forget_copy_toast_on_item_change(ctx: &egui::Context, item: &str) {
    ctx.data_mut(|data| {
        if data.get_temp::<String>(copy_toast_item_id()).as_deref() != Some(item) {
            data.remove::<CopyToast>(copy_toast_id());
            data.insert_temp(copy_toast_item_id(), item.to_string());
        }
    });
}


/// The sentence a confirmation shows for a row labelled `label`.
///
/// **The label and nothing else.** The whole point of the confirmation is to
/// say that *something happened*, and the one thing it must never say is the
/// thing that just went on the clipboard: this pane's rows are passwords,
/// card numbers, security codes and private keys, and a 5-second banner in
/// the corner of the window is precisely the surface a shoulder-surfer reads.
/// The value is not a parameter here, so it cannot be interpolated by
/// accident.
fn copy_toast_text(label: &str) -> String {
    format!("{label} copied")
}

/// What the confirmation should say **now**, and how many seconds are left --
/// or `None` once it has expired.
///
/// The remainder is returned rather than kept private because the caller owes
/// egui a `request_repaint_after` for exactly that long. egui only redraws on
/// input, so a toast whose deadline nobody schedules stays on screen until
/// the next mouse move; that is this feature's most likely bug and the reason
/// the deadline is part of this function's answer instead of a separate
/// calculation somewhere else.
///
/// A second copy overwrites the whole [`CopyToast`], label and timestamp
/// together, so it replaces the message and restarts the clock -- nothing
/// queues and nothing stacks.
///
/// Pure, and separate from the closure that draws it, for this file's
/// standing reason: a decision reachable only from inside an eframe closure
/// is a decision that will not be tested.
fn copy_toast_now(toast: Option<&CopyToast>, now: f64) -> Option<(String, f64)> {
    let toast = toast?;
    let left = COPY_TOAST_SECONDS - (now - toast.shown_at);
    (left > 0.0).then(|| (copy_toast_text(&toast.label), left))
}

/// Records that `label`'s row was just copied, starting the confirmation.
///
/// Called from BOTH copy paths -- [`copy_row`]'s click and
/// [`draw_detail_read`]'s chord -- so a keyboard copy confirms exactly as a
/// clicked one does. That is not a nicety: a chord is the case where the user
/// has least evidence anything happened at all, since there is no row under
/// the pointer to have reacted.
fn note_copied(ctx: &egui::Context, label: &str) {
    let shown_at = ctx.input(|i| i.time);
    ctx.data_mut(|data| {
        data.insert_temp(
            copy_toast_id(),
            CopyToast {
                label: label.to_string(),
                shown_at,
            },
        )
    });
}

/// Paints the copy confirmation, if one is live, and schedules the repaint
/// that retires it.
///
/// **A floating toast, not an `on_hover_text` tooltip**, despite the word the
/// user used. An egui tooltip is bound to a widget and to the pointer: it
/// appears only while the pointer rests on the row, and it vanishes the
/// instant the pointer moves. Neither half survives what was actually asked
/// for. "Five seconds" is a duration the pointer will not sit still for, and
/// a chord copy has no pointer on the row at all -- CTRL+B with the mouse
/// parked over the item list would have shown nothing whatsoever. The
/// behaviour described ("you don't know what happened") is what is built
/// here; the surface named is the one thing that cannot deliver it.
///
/// Its own `Order::Foreground` [`egui::Area`], and non-interactable, so it
/// floats over the pane's cards without stealing a click from the row
/// underneath -- the same treatment `login_ui`'s resize handles and
/// `folder_modal`'s scrim already use.
/// The id under which the read body's "did it overflow last frame?" reading
/// is kept.
fn body_overflow_id() -> egui::Id {
    egui::Id::new("detail-read-body-overflow")
}

/// Whether the read body's content was taller than its viewport the last time
/// it was drawn -- i.e. whether the scroll bar has anything to say.
///
/// Absent (the first frame this pane is ever drawn, or the first after a
/// context is rebuilt) answers TRUE: a bar shown on a body that turns out to
/// fit disappears on the next frame, whereas a bar hidden on a body that
/// really does scroll would tell the reader there is nothing below when there
/// is. Ties go to showing it.
fn body_overflowed(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<bool>(body_overflow_id()))
        .unwrap_or(true)
}

/// Records this frame's reading for [`body_overflowed`] to use on the next.
fn note_body_overflow(ctx: &egui::Context, overflowed: bool) {
    ctx.data_mut(|data| data.insert_temp(body_overflow_id(), overflowed));
}

fn draw_copy_toast(ui: &mut egui::Ui, pane: egui::Rect) {
    let now = ui.input(|i| i.time);
    let toast = ui.ctx().data(|data| data.get_temp::<CopyToast>(copy_toast_id()));
    let Some((text, left)) = copy_toast_now(toast.as_ref(), now) else {
        // **Expired means gone, not merely unpainted.** Left in place, a dead
        // `CopyToast` sat in the context's temp map for the rest of the
        // session. Nothing sensitive is in it (the label is one of a fixed
        // set of literals -- see `copy_toast_text`) and the next copy
        // overwrites it, so this is tidiness rather than a leak; it is here
        // because it leaves the map in the same state
        // `forget_copy_toast_on_item_change` does, so "a toast is recorded"
        // and "a toast is live" cannot become two different answers.
        if toast.is_some() {
            ui.ctx()
                .data_mut(|data| data.remove::<CopyToast>(copy_toast_id()));
        }
        return;
    };
    // The deadline, handed to egui. Without this the toast expires only when
    // something else happens to cause a frame.
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_secs_f64(left));

    // **Measured here and placed with `fixed_pos`, not handed to `anchor`.**
    // An `Area` whose size egui does not yet know runs an INVISIBLE sizing
    // pass on its first frame when it has to align itself -- so an anchored
    // toast paints nothing on the very frame the copy happened and only
    // appears on the next one. The copy and its confirmation are one gesture;
    // a frame of silence in between is the bug this whole feature exists to
    // remove. Laying the text out first makes the size known, which makes the
    // placement arithmetic this function's own and the paint immediate.
    let galley = ui.painter().layout_no_wrap(
        text,
        egui::FontId::proportional(COPY_TOAST_TEXT_SIZE),
        theme::CARD,
    );
    let pad = egui::vec2(14.0, 10.0);
    let size = galley.size() + pad * 2.0;
    // `max` on the near edges so a pane narrower than the message keeps the
    // START of the sentence on screen rather than the end of it.
    //
    // **Unreachable today, and kept anyway.** The widest message boxes at
    // about 127pt, and `MIN_VAULT_WINDOW_SIZE` puts the narrowest pane this
    // window can be resized to at `MIN_PANE` (298), so the clamp needs a pane
    // under roughly 167pt that the app cannot produce -- deleting it changes
    // nothing visible, which is exactly why it would be deleted by mistake.
    // It is not dead code but a bound on arithmetic that would otherwise
    // place the box off the left of the pane the moment any of those three
    // numbers moves, so its unreachability is written down here rather than
    // rediscovered.
    let pos = egui::pos2(
        (pane.right() - COPY_TOAST_INSET - size.x).max(pane.left() + COPY_TOAST_INSET),
        (pane.bottom() - COPY_TOAST_INSET - size.y).max(pane.top() + COPY_TOAST_INSET),
    );
    // **A painter on a foreground layer, not an `egui::Area`.** An `Area`
    // that has never been laid out before runs its first pass invisibly to
    // learn its own size, so an `Area`-based toast painted NOTHING on the
    // frame the copy happened and only appeared on the next one -- measured,
    // not assumed: `clicking_a_row_confirms_the_copy_by_name` failed against
    // exactly that version while the placement test, which reads the frame
    // after, passed. A painter has no layout and no state to warm up, and a
    // confirmation is only ever painted: it takes no input, so it wants none
    // of what an `Area` is for, and cannot swallow a click meant for the row
    // beneath it.
    let painter = ui
        .ctx()
        .layer_painter(egui::LayerId::new(egui::Order::Foreground, copy_toast_id()));
    let rect = egui::Rect::from_min_size(pos, size);
    painter.rect_filled(rect, CornerRadius::same(8), theme::INK);
    painter.galley(rect.min + pad, galley, theme::CARD);
}

/// What a copy-on-click tile says when the pointer rests on it.
///
/// **Two invisible things, one sentence.** Neither of them is discoverable
/// from the pane: the tile copies on click but is not drawn as a button, and
/// the chord that copies the same value without the mouse is a chord. The
/// chord used to be painted beside the row instead -- twelve monospace
/// characters on the control line of every credential row -- and the user
/// asked for that text gone from the Password row in favour of the eye. It
/// is moved here rather than deleted, and moved for EVERY row that had one
/// rather than only that row: a Password row with no chord beside a Username
/// row that still had one is worse than either answer applied uniformly.
///
/// The wording is `theme::gear_button`'s idiom -- a plain `on_hover_text` on
/// the response -- and the chord comes from [`COPY_SHORTCUTS`], so a row
/// cannot name a key it is not bound to.
fn copy_row_tooltip(hint: Option<CopyShortcut>) -> String {
    match hint {
        Some(which) => format!("Click to copy · {}", copy_shortcut_chord(which)),
        None => "Click to copy".to_string(),
    }
}

/// Whether a row that would copy `value` has anything to copy.
///
/// **The click path's half of the rule [`copy_shortcut_action`] already
/// states.** That function refuses a chord over an empty field rather than
/// falling back, because "an empty string looks like a failed paste"; the
/// click path had no such gate, so a Password or Username row -- both of
/// which are drawn whether or not the item carries a value -- took the hover
/// tint, the pointing hand and the "Click to copy" tooltip, reported a copy,
/// and raised a "Password copied" toast with nothing on the clipboard.
///
/// **An empty row is made INERT, not merely quiet.** Suppressing only the
/// toast was the smaller change and was rejected: the tint, the cursor and
/// the tooltip are three separate promises made *before* the click, and a row
/// that keeps all three and then does nothing is the case `copy_row`'s own
/// doc already calls "worse than an inert one, because there is no way to
/// tell from the outside that it did nothing". So an empty row senses hover
/// only -- no tint, no hand, no tooltip, no action, no toast.
///
/// Pure and trivial on purpose: what is worth pinning is not the expression
/// but that both paths agree, which
/// `an_empty_field_is_refused_by_the_click_path_and_the_chord_path_alike`
/// asserts against [`copy_shortcut_action`] directly.
fn row_offers_copy(value: &str) -> bool {
    !value.is_empty()
}

/// A row's keyboard-shortcut hint, on the control line.
///
/// Bare 10px monospace in ghost grey, matching the design's other shortcut
/// hints (the search field's `CTRL+K`, the Lock pill's `CTRL+L`) rather than
/// `theme::kbd_chip`'s boxed treatment, which the design reserves for the
/// menu. The text comes from [`COPY_SHORTCUTS`], so a row cannot paint a key
/// it is not bound to.
fn shortcut_hint(ui: &mut egui::Ui, hint: Option<CopyShortcut>) {
    if let Some(which) = hint {
        ui.label(
            RichText::new(copy_shortcut_chord(which))
                .size(10.0)
                .family(egui::FontFamily::Monospace)
                .color(theme::TEXT_GHOST),
        );
    }
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
    // The website exactly as the AUTOFILL TARGETS card has it -- empty
    // whenever that card is not on screen, so the chord and the row it
    // belongs to are gated by one expression (see `draw_detail_read`).
    website: &str,
) -> Option<DetailAction> {
    match which {
        CopyShortcut::Username => (!username.is_empty()).then_some(DetailAction::CopyUsername),
        CopyShortcut::Password => (!password.is_empty()).then_some(DetailAction::CopyPassword),
        // Through `CopyValue`, whose doc reserves it for values that are not
        // `Zeroizing` in the model -- a URI is not, and this is the only
        // binding here that copies something the item does not hide.
        CopyShortcut::Url => {
            (!website.is_empty()).then(|| DetailAction::CopyValue(website.to_string()))
        }
        // Only when a code is really on screen. `vault_window::mod` resolves
        // `CopyTotp` out of this same state, so every other variant --
        // `NoSecret`, `Fetching`, `Unavailable`, `NoCodeReported` -- would
        // have it copy nothing, or an empty string, without this gate.
        //
        // **And `row_offers_copy` on the code itself**, not `Code { .. }`
        // alone (review 20's Minor 4). `Code { code: String::new() }` is a
        // shape this pane cannot rule out -- `totp_code_row` passes
        // `row_offers_copy(code)` for exactly that reason, and the note on
        // `TotpRow` refuses to assume otherwise -- and the bare variant test
        // left CTRL+T copying an empty string and raising "One-time code
        // copied" over it, which is the defect `bc161b2` fixed on the click
        // path and only on the click path. Asking `row_offers_copy` is what
        // makes the two paths one rule rather than two that agree today.
        CopyShortcut::Totp => match totp {
            TotpState::Code { code, .. } => {
                row_offers_copy(code).then_some(DetailAction::CopyTotp)
            }
            _ => None,
        },
    }
}

/// The header's second line, under the item's name.
///
/// Design 2b, line 803: `Login · Engineering` -- **one line carrying the kind
/// and the folder**, not the kind with a folder line added under it. The pane
/// already painted the kind here; the folder joins it.
///
/// The user asked for the folder "under the title like design has it - if no
/// folder just don't print anything". "Nothing" is the separator and the name:
/// the line still reads `Login`, because the kind was never the part that was
/// missing. A subtitle that vanished with the folder would take the one fact
/// this line has always carried with it.
///
/// `folder` is a NAME the caller has already resolved (`sidebar::folder_name`,
/// which is where a folder id becomes a name and where every reason there
/// might be no name is decided). This function never sees an id, so it cannot
/// paint one.
///
/// Pure, and separate from the closure that draws it, for this file's standing
/// reason: a decision reachable only from inside an eframe closure is a
/// decision that will not be tested.
fn header_subtitle(kind: ItemKind, folder: Option<&str>) -> String {
    match folder {
        Some(name) => format!("{} · {name}", kind.label()),
        None => kind.label(),
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

/// `modifiers+key` and **nothing else held**, taken out of the event queue.
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
///
/// Exactness is now load-bearing rather than merely careful: CTRL+U and
/// CTRL+SHIFT+U are two different copies on the same key, and telling them
/// apart is exactly what `matches_logically` cannot do.
fn consume_chord(
    input: &mut egui::InputState,
    modifiers: egui::Modifiers,
    key: egui::Key,
) -> bool {
    let mut found = false;
    input.events.retain(|event| {
        let is_match = matches!(
            event,
            egui::Event::Key {
                key: event_key,
                modifiers: held,
                pressed: true,
                ..
            } if *event_key == key && held.matches_exact(modifiers)
        );
        found |= is_match;
        !is_match
    });
    found
}

pub fn draw_detail_read(
    ui: &mut egui::Ui,
    item: &VaultItem,
    // The name of the folder this item is in, for the header's subtitle --
    // already resolved by `sidebar::folder_name`, which owns that lookup and
    // every reason there might not be one. A NAME, never an id: see
    // [`header_subtitle`].
    folder: Option<&str>,
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
    // **First, before anything is drawn or any chord is resolved.** A copy
    // made later in THIS frame belongs to the item being drawn now, so it
    // must outlive this clear; a copy made on the item that was on screen
    // before it must not. See `forget_copy_toast_on_item_change`.
    forget_copy_toast_on_item_change(ui.ctx(), &item.id);
    // Derived once, here, and passed to the pure decisions below -- not
    // re-derived per widget, so the header, the chrome, the body and the
    // metadata strip cannot disagree about what this item is.
    let kind = ItemKind::of(item);
    // **Derived once, beside `kind`, for the same reason `kind` is.** The
    // header measures the room its controls need and then draws them, and
    // those two must not be able to disagree -- drift between them is what
    // put a control off the edge of the pane once already (see
    // `controls_width` below). It is also what `fill_hotkey_applies` asks, so
    // the button and Ctrl+Shift+F cannot end up offering different things.
    let offers_fill = item_offers_fill(item, kind);
    let login = item.login.as_ref();
    let username = login.and_then(|l| l.username.as_deref()).unwrap_or("");
    let password = login
        .and_then(|l| l.password.as_deref())
        .map(|p| p.as_str())
        .unwrap_or("");
    // **One expression for the AUTOFILL TARGETS card and for CTRL+SHIFT+U.**
    // Derived up here with the other two fields rather than beside the card
    // it draws, because the chord is resolved before anything is drawn and
    // the two must not be able to disagree: a chord that copied a URL the
    // pane is not showing would be copying from a card the user cannot see.
    //
    // Gated on the kind as well as on there being a URI: this card is the
    // autofill *targets* card, and advertising targets for an item the fill
    // path will not fill is the same false promise the Fill button was.
    let website = login
        .and_then(|l| l.uris.first())
        .and_then(|u| u.uri.as_deref())
        .filter(|_| kind_offers_fill(kind))
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
            .find(|(_, modifiers, key, _)| consume_chord(i, *modifiers, *key))
            .map(|(which, _, _, _)| *which)
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
                let fill = if offers_fill {
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
                        ui.label(
                            RichText::new(header_subtitle(kind, folder))
                                .size(12.0)
                                .color(theme::TEXT_FAINT),
                        );
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
                //
                // Not drawn for a DEAD binding either -- see
                // `item_offers_fill`. The card below is printing "Deskwarden
                // is ignoring this match, so it never fires" in this same
                // frame; a live Fill above it would be an offer to act
                // through it anyway.
                if offers_fill
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
    //
    // The RIGHT padding is 0 here because the scroll bar below is given that
    // lane instead -- `theme::scrollbar_in_gutter` reserves exactly
    // `BODY_PAD_X` for itself, so the cards still end at
    // `pane.right() - BODY_PAD_X` as they always did, and the bar is centred
    // in the padding rather than drawn hard against them. Same arrangement,
    // and the same reason, as `item_list.rs`'s list.
    let body = egui::Rect::from_min_max(
        egui::pos2(
            pane.left() + f32::from(BODY_PAD_X),
            ui.cursor().top() + f32::from(BODY_PAD_Y),
        ),
        egui::pos2(pane.right(), pane.bottom() - f32::from(BODY_PAD_Y)),
    );
    let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
    // **The fix.** This body was one plain `Ui` with no scroll area at any
    // level, so on a pane shorter than the item -- a full identity with notes
    // and previous passwords paints to y = 1967 on a pane the app can be
    // resized down to 600 -- egui laid the lower cards out past the bottom
    // and culled them: not painted, not scrollable to, not reachable by
    // anything. The whole `MATCHED APP` card, including the Autofill row and
    // the Open button, was among them. This is the read-side half of the
    // defect commit `68f86cb` fixed on the edit form.
    //
    // What is pinned and what scrolls: the HEADER STRIP stays outside this
    // area, and only it. It carries the item's name, the folder subtitle and
    // the star / kebab / Fill controls -- a title that scrolls away leaves
    // the reader with no answer to "which item is this?", and the controls
    // there act on the item as a whole rather than on any row below. The edit
    // pane additionally pinned an action strip to the BOTTOM; there is no
    // counterpart here, because the read pane has no Save or Cancel and no
    // other control that must be reachable without reading what it applies
    // to. Nothing else is held back: pinning, say, the metadata line as well
    // would spend a second slice of a 600pt pane on chrome.
    //
    // Horizontal scrolling is not offered, deliberately -- the rows already
    // elide long values (see `value_text`), and a horizontal bar would be the
    // regression rather than the fix.
    theme::scrollbar_in_gutter(&mut body_ui, f32::from(BODY_PAD_X));
    // ... and the bar is hidden outright when there is nothing to scroll.
    // The lane stays reserved either way -- that is what `AlwaysVisible`
    // below is for, and why the cards keep ONE width whether or not the bar
    // is showing, which is the 10pt jump `092da70` measured on the item list.
    //
    // Read back from the last frame rather than predicted: unlike the item
    // list, whose content height is a row count times a pitch, this body's
    // height is the sum of however many cards this kind of item draws, each
    // with wrapped text of its own. The one frame of lag can only show a bar
    // for a frame on an item that does not need one -- never hide one that
    // is needed for longer than that -- and a first-ever frame, which has no
    // reading, shows it. That is the safe direction.
    if !body_overflowed(ui.ctx()) {
        theme::hide_scrollbar(&mut body_ui);
    }
    let scrolled = egui::ScrollArea::vertical()
        // The area takes the full width and height of the body rect: the
        // cards inside set their own width from `available_width`, which the
        // reserved lane has already narrowed back to what it was.
        .auto_shrink([false; 2])
        // Required by `scrollbar_in_gutter`: the lane is only reserved for a
        // bar egui is actually showing, so anything conditional here puts the
        // width jump back.
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .show(&mut body_ui, |ui| {
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
                    // The label is read out of `copy_shortcut_label` rather
                    // than typed here, so this row and its copy confirmation
                    // are literally the same string.
                    copy_shortcut_label(CopyShortcut::Username),
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

    if !website.is_empty() {
        card(ui, "AUTOFILL TARGETS", |ui| {
            // **Two things one click can mean, split by where it lands.**
            // The URL text is the link -- clicking it opens the browser --
            // and the rest of the tile copies, like every other row on this
            // pane. That split is safe for exactly the reason the eye's is
            // (see `copy_row`): `row_impl` senses the tile on a `UiBuilder`
            // background, which egui registers when the `Ui` is created and
            // therefore *before* its children, and a click goes to the
            // topmost widget under the pointer and to nothing else. The link
            // is a child, so it wins its own click and does not also copy.
            // Pinned by `clicking_the_website_link_opens_it_without_copying`,
            // which asserts both halves in one frame.
            let mut opened = false;
            copy_row(
                ui,
                // See `copy_shortcut_label`: one string for the row and its
                // toast.
                copy_shortcut_label(CopyShortcut::Url),
                |ui| {
                    opened = theme::link_label(ui, website, ROW_VALUE_SIZE)
                        .on_hover_text("Open in browser")
                        .clicked();
                },
                |_ui| {},
                DetailAction::CopyValue(website.to_string()),
                Some(CopyShortcut::Url),
                // Not a constant `true`: this card is already gated on
                // `!website.is_empty()` above, and stating the rule through
                // the same predicate every other row uses means the two
                // cannot drift if that gate ever changes.
                row_offers_copy(website),
                &mut action,
            );
            if opened {
                action = DetailAction::OpenWebsite(website.to_string());
            }
        });
        ui.add_space(CARD_GAP);
    }

    // **Directly under AUTOFILL TARGETS, and NOT inside it** -- see
    // `APP_CARD_HEADING`. Last of the body cards, above the metadata strip:
    // the cards above are the item's own contents, and this one is about what
    // Deskwarden does with them.
    let app_match = crate::vault_bridge::extract_app_match(item);
    // **The FIELD, not the parsed match.** A field that will not parse is a
    // binding the user can see in every other Bitwarden client and must be
    // able to clear from here; asking `app_match.is_some()` filed it as "no
    // field at all" and hid the card outright on a non-fillable kind. See
    // `app_card_body`.
    let app_field_present = crate::vault_bridge::has_app_match_field(item);
    if app_card_visible(app_field_present, kind) {
        card(ui, APP_CARD_HEADING, |ui| {
            app_match_card(ui, app_match.as_ref(), app_field_present, website, &mut action);
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
        });
    note_body_overflow(
        body_ui.ctx(),
        scrolled.content_size.y > scrolled.inner_rect.height(),
    );
    let ui = &mut body_ui;

    if matches!(action, DetailAction::None) {
        if let Some(which) = shortcut {
            if let Some(copy) = copy_shortcut_action(which, username, password, totp, website) {
                action = copy;
                // The same confirmation a click gets, named out of
                // `copy_shortcut_label` -- which is also what the row for
                // this chord painted as its label, so the two agree by
                // construction rather than by inspection.
                note_copied(ui.ctx(), copy_shortcut_label(which));
            }
        }
    }
    // Last, so a copy reported anywhere above -- click or chord -- is already
    // on the clock by the time this reads it and shows it in the same frame.
    draw_copy_toast(ui, pane);
    action
}

// ---------------------------------------------------------------------------
// The MATCHED APP card.
//
// `AppMatch::path` has been written into real vault items since the picker
// learned to capture it and NOTHING has ever read it back; an app match could
// only be created, never seen, corrected or undone -- not even to find out
// which app an item is bound to. This card is that reader.
//
// Every decision it makes is one of the pure functions below, and
// `app_match_card` does nothing but obey them, for this file's standing
// reason: a decision reachable only through an `egui` closure is a decision
// no test can call.
// ---------------------------------------------------------------------------

/// **Its own card, beside `AUTOFILL TARGETS` rather than inside it.**
///
/// An app match *is* an autofill target, and folding these rows into that card
/// was the first thing considered and the first thing rejected: that card is
/// drawn only `if !website.is_empty()`, so an item bound to an app and carrying
/// no URI would have had its match hidden by a gate about a website -- which is
/// the exact invisibility this card exists to end. Two cards also keep the
/// heading honest about what the rows underneath are: a web address and a
/// Windows executable are matched by two different engines against two
/// different things.
const APP_CARD_HEADING: &str = "MATCHED APP";

/// What the card says when the item is bound to nothing.
///
/// **It names the door rather than offering one.** There IS a picker
/// (`picker_ui::run_picker`), and this card deliberately does not duplicate or
/// route to it: the picker is opened by the tray's "Add app..." on `main`'s
/// own thread, and the vault window is a *blocking* call on that same thread
/// -- so there is no way for this pane to raise it without restructuring
/// `main.rs`. Saying nothing at all was the alternative, and it leaves a user
/// looking at an empty card with no idea that the feature exists.
const APP_MATCH_EMPTY_NOTICE: &str =
    "No app is matched to this item yet. Use \"Add app...\" in the Deskwarden tray menu to \
     pick a window, and it will show up here.";

/// What the card says under the rows when the match was captured off a
/// Microsoft Store / UWP frame.
///
/// **The word `hosted` never reaches the screen**, and neither does
/// `ApplicationFrameHost.exe`: they are the mechanism, not the fact. What the
/// user needs to know is that this one match is keyed on a window title rather
/// than on an executable name, because that is what makes it behave
/// differently -- it is the only match that keeps working while the app is
/// suspended, and the only one a renamed window can break.
const APP_HOSTED_NOTE: &str =
    "Matched by its window title, because this is a Microsoft Store app.";

/// What the card says under the rows when the match exists but the engine
/// will never act on it -- see [`app_match_is_dead`].
///
/// **It replaces [`trigger_caption`], it does not join it.** The caption is a
/// promise about what happens when the app is focused, and on a dead binding
/// every one of the three is false; printing "Show the overlay when this app
/// is focused." next to "this never fires" is the same lie with a disclaimer
/// stapled on.
///
/// The wording is `picker_ui::existing_host_match_notice`'s, said about the
/// binding rather than about the picker's target, and it names the same two
/// ways out the picker names: re-add through the tray, or clear it. Unlike
/// every other note on this card, the process name it is about is already in
/// the App row directly above, so this sentence does not repeat it.
const APP_MATCH_DEAD_NOTICE: &str =
    "Deskwarden is ignoring this match, so it never fires: that process owns the window for \
     every Microsoft Store app, and no window title was recorded to tell those apps apart. \
     Nothing in your vault has been changed. Use \u{201c}Add app\u{2026}\u{201d} in the \
     Deskwarden tray menu to pick the app again, or Remove to clear it.";

/// What the card says when the item carries a `deskwarden:app-match` field
/// whose value this build cannot read -- see
/// [`crate::vault_bridge::has_app_match_field`].
///
/// **It says the field is there and unreadable, not that nothing is bound.**
/// The field is visible and hand-editable in every other Bitwarden client, so
/// a user who broke it there is looking at a row this pane used to claim did
/// not exist. It names Remove because Remove is the only thing this pane can
/// honestly do with it: the value cannot be repaired from a shape nothing can
/// parse, and `without_app_match` clears it on the field's NAME and so works
/// perfectly on exactly this case.
const APP_MATCH_UNREADABLE_NOTICE: &str =
    "This item has a Deskwarden app match that cannot be read \u{2014} the \
     \u{201c}deskwarden:app-match\u{201d} custom field is there, but its contents are not \
     something this version understands, which usually means it was edited by hand in \
     another Bitwarden client. Autofill ignores it. Remove clears the field, and \
     \u{201c}Add app\u{2026}\u{201d} in the Deskwarden tray menu can bind this item again.";

/// Whether [`MatchEngine`](crate::match_engine::MatchEngine) has dropped this
/// match -- i.e. whether the binding the card is about **can never fire**.
///
/// **Derived from the engine's own gate, not restated.** `rebuild` keeps a
/// match out of `by_process` when `is_host_process(&m.process)`, and admits it
/// to `by_title` only when `m.hosted && !m.title.is_empty()`; a match in
/// neither table is unreachable from any foreground window at all. That is one
/// call to [`crate::window_watch::is_host_process`] off the same field, which
/// is what `picker_ui`'s `host_process_refusal` already does for the same
/// purpose -- so this is not a second copy of the rule, it is the same
/// predicate asked from a second place.
///
/// `match_engine`'s own doc says telling the user "needs a channel out of
/// `main`'s loop". That is true of telling them *at the moment it goes quiet*.
/// It is not true here: the card is holding the match in its hand.
///
/// Pinned against the engine's real behaviour by
/// `a_card_calls_a_match_dead_exactly_when_the_engine_can_never_look_it_up`,
/// which builds a `MatchEngine` from the match and asks it, rather than
/// re-spelling the condition.
pub fn app_match_is_dead(m: &AppMatch) -> bool {
    crate::window_watch::is_host_process(&m.process) && !(m.hosted && !m.title.is_empty())
}

/// Whether *this item's* binding is one [`app_match_is_dead`] calls dead.
///
/// The item-level spelling of that predicate, so the two places that must act
/// on it -- the header's Fill button here, and `vault_window::mod`'s
/// `fill_item_into_app` -- ask one question rather than each unpacking the
/// field for themselves. An item with no field, and an item whose field will
/// not parse, are both `false`: there is no binding to be dead, and the fill
/// path's own "no app is matched to this item yet" is the honest report for
/// them.
pub fn item_binding_is_dead(item: &VaultItem) -> bool {
    crate::vault_bridge::extract_app_match(item).is_some_and(|m| app_match_is_dead(&m))
}

/// Whether the pane offers "Fill in app" for this item at all -- the ONE
/// predicate behind the header button, the room the header strip reserves for
/// it, and `vault_window::mod`'s `fill_hotkey_applies`.
///
/// [`kind_offers_fill`] is the first half and answers "could a fill of this
/// item mean anything" (see its doc: a card has no username and password to
/// type). [`item_binding_is_dead`] is the second and answers "is there an app
/// this fill could go to". A dead binding fails the second: the card in the
/// same frame is printing [`APP_MATCH_DEAD_NOTICE`] -- *Deskwarden is ignoring
/// this match, so it never fires* -- and a live blue Fill beside that sentence
/// is the pane offering to act through a binding it has just said it never
/// acts through. Worse than the trigger pills that commit `8db47a0` removed
/// for the same reason: the pills only changed which of three things did not
/// happen, and this types the user's password into whatever the resolution
/// picks.
///
/// **Not the only gate, and deliberately not the load-bearing one.** The
/// refusal that actually protects the credential is in
/// `app::find_window_for_process`, which cannot be bypassed by any caller.
/// This one is so that the pane does not *offer* what that one will refuse.
pub fn item_offers_fill(item: &VaultItem, kind: ItemKind) -> bool {
    kind_offers_fill(kind) && !item_binding_is_dead(item)
}

/// Which of the card's three bodies an item asks for.
///
/// The pair of answers -- "does the field exist" and "did it parse" -- is
/// three states, and the card used to collapse them into two by asking only
/// the second (review 20's Important 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppCardBody<'a> {
    /// A match this build understands. It may still be dead: see
    /// [`app_match_is_dead`].
    Bound(&'a AppMatch),
    /// The field is there and its value will not parse. Nothing to draw rows
    /// from, and Remove is the only honest offer.
    Unreadable,
    /// No `deskwarden:app-match` field on the item at all.
    Unbound,
}

fn app_card_body(app_match: Option<&AppMatch>, field_present: bool) -> AppCardBody<'_> {
    match app_match {
        Some(m) => AppCardBody::Bound(m),
        None if field_present => AppCardBody::Unreadable,
        None => AppCardBody::Unbound,
    }
}

/// Whether the pane draws a [`APP_CARD_HEADING`] card at all.
///
/// **Two reasons, and the first outranks the second.** An item that CARRIES
/// the field always gets the card, whatever kind it is: a binding the pane
/// refuses to draw is the defect being fixed, and an app match sitting on a
/// secure note is exactly the case a user would most need to see in order to
/// remove it. An item with no field gets the card only where a match would do
/// something -- `kind_offers_fill`, the same predicate that gates the Fill
/// button and the `AUTOFILL TARGETS` card, so a card, a note and a button
/// cannot end up disagreeing about which kinds autofill.
///
/// **`has_field`, not "has a match this build could parse".** A secure note
/// whose app-match field was corrupted by hand elsewhere had the whole card
/// suppressed, which is the same invisibility one paragraph up, reached by a
/// different route.
fn app_card_visible(has_field: bool, kind: ItemKind) -> bool {
    has_field || kind_offers_fill(kind)
}

/// One row of the card: its label, the text in its value column, and whether
/// that text is the match's own value (so the row copies) or a placeholder
/// standing in for a value that was never recorded (so it does not).
#[derive(Debug, Clone, PartialEq, Eq)]
struct AppRow {
    label: &'static str,
    value: String,
    /// `false` for a placeholder. It reaches [`copy_row`]'s `copyable`, so a
    /// row saying "Not recorded" is inert -- no tint, no hand, no tooltip, no
    /// toast -- for the same reason an empty Password row is (see
    /// [`row_offers_copy`]).
    real: bool,
}

/// The placeholder for a `path` this match never captured -- every match saved
/// before the field existed, which is a shape still sitting in real vaults.
const APP_PATH_UNRECORDED: &str = "Not recorded";

/// The card's rows, in order, for a match that exists.
///
/// **The user asked for "name, path + keys", and this is that mapping made
/// explicit.** `process` is the name, `path` is the path, `trigger` is the
/// keys -- but the trigger is a control rather than a row of text, so it is
/// not here; it is [`trigger_label`]'s three pills, drawn after these.
///
///  * **App** -- `process`, the executable's file name. This is the thing the
///    match engine actually compares, so it is first.
///  * **Window title** -- `title`, and ONLY when `hosted`. An unhosted title
///    is inert by design (see [`AppMatch::hosted`]): every one saved during
///    the one commit that recorded titles for every row is deliberately never
///    matched on, and drawing it here would tell the user it does something.
///  * **Program file** -- `path`, or [`APP_PATH_UNRECORDED`]. Shown as the
///    match stores it, NOT through `AppMatch::launchable_path`: that function
///    answers "is this safe to execute", this row answers "what did the picker
///    record", and showing nothing for a path that fails the launch check
///    would hide the very corruption a user needs to see in order to fix it.
fn app_match_rows(m: &AppMatch) -> Vec<AppRow> {
    let mut rows = vec![AppRow {
        label: "App",
        value: m.process.clone(),
        real: row_offers_copy(&m.process),
    }];
    if m.hosted && !m.title.is_empty() {
        rows.push(AppRow {
            label: "Window title",
            value: m.title.clone(),
            real: true,
        });
    }
    let recorded = row_offers_copy(&m.path);
    rows.push(AppRow {
        label: "Program file",
        value: if recorded {
            m.path.clone()
        } else {
            APP_PATH_UNRECORDED.to_string()
        },
        real: recorded,
    });
    rows
}


// ---------------------------------------------------------------------------
// Open: starting the app this item is bound to.
//
// The user's case, in their words: "I have two browsers open - personal and
// work... if I click Open in MS365 of work account - it launches Chrome with
// certain profile and logins there if personal - it is another Chrome." Two
// vault items name the same `chrome.exe` at the same `path` and differ only in
// `AppMatch::args` -- which is what that field was added for, and this is its
// first reader.
//
// Every decision below is a free function returning a value. Nothing here
// draws, and nothing here spawns: the control reports a `DetailAction` and
// `vault_window::mod` starts the process, for this file's standing reason --
// a decision reachable only through an `egui` closure is a decision no test
// can call, and a `Command::spawn` reachable from a test is a test that
// launches a browser.
// ---------------------------------------------------------------------------

/// A program to start, in the two pieces Windows actually needs.
///
/// **`raw_tail` is a command line, not an argument list, and that is a
/// decision.** Windows does not pass programs a vector of arguments; it passes
/// one string, and each program splits it itself. `AppMatch::args` is already
/// one such string, stored exactly as the user typed it (see that field's
/// doc, which promises never to re-quote or split it). The obvious
/// implementation -- split `args` into a `Vec<String>` and hand it to
/// `Command::args` -- therefore does a round trip: split by one convention,
/// then let `std` re-quote by another. For the motivating value,
/// `--profile-directory="Profile 2"`, that round trip is *lossy in a way the
/// user can see*: `CommandLineToArgvW` yields the single token
/// `--profile-directory=Profile 2` (the quotes are consumed, they were never
/// argument delimiters here), and `std`'s re-quoting turns that back into
/// `"--profile-directory=Profile 2"` -- quotes around the WHOLE thing. Chrome
/// reads that as a flag literally named `--profile-directory=Profile 2` only
/// because it re-splits with the same convention; anything that does not
/// (and plenty of Windows programs roll their own parser) gets a different
/// flag than the user typed.
///
/// **What was rejected, and why.**
///
///  * *Hand-written tokenisation.* Matching `CommandLineToArgvW`'s real rules
///    -- `2n` backslashes then a quote, `2n+1` backslashes then a quote, `""`
///    inside a quoted run -- is notoriously error-prone, and getting it wrong
///    is silent.
///  * *`CommandLineToArgvW` itself*, which is in the `windows` crate already
///    pinned here. It parses correctly, and it is still the wrong tool: it is
///    the lossy half of the round trip above, its argv[0] rules differ from
///    its argv[n] rules, and it would make this crate's behaviour depend on
///    a Win32 call in a function that otherwise needs no OS at all.
///  * Both share the same defect: they change the user's string. This field's
///    doc promises not to.
///
/// **What is done instead**: `std::os::windows::process::CommandExt::raw_arg`,
/// which appends a string to the command line *verbatim*. `args` is never
/// split, never re-quoted, and never parsed by this crate -- it arrives at the
/// target program byte-for-byte as the user wrote it, and the target program's
/// own parser is the only one that ever looks at it. `args` is never split,
/// never re-quoted, and never parsed by this crate.
///
/// **What `args` can do, stated accurately.** An earlier version of this doc
/// claimed the worst a corrupted `args` could do was "pass extra flags to a
/// program whose identity `launchable_path` has already pinned -- which is
/// exactly what an honest `args` does too". The first half is right and the
/// reassurance is wrong, because for the program this feature was built for,
/// extra flags ARE arbitrary code execution:
///
/// ```text
/// args = --gpu-launcher="cmd /c calc.exe"
/// argv = ["...chrome.exe", "--gpu-launcher=cmd /c calc.exe", "https://..."]
/// ```
///
/// Chrome executes that string. `--gpu-launcher`, `--renderer-cmd-prefix`,
/// `--utility-cmd-prefix` and `--browser-subprocess-path` are all "run this
/// command as my child process" flags, and every Chromium browser has them.
/// So `launchable_path` pinning the *image* pins nothing about what the image
/// is made to do.
///
/// **And `args` is not necessarily the user's own text.** It is unvalidated
/// vault data, and an item can be shared into a vault by another member of an
/// organisation. The threat model is therefore not "the user corrupted their
/// own field" but "somebody else wrote the field, and the user clicks Open".
///
/// **What is genuinely inert, so this is not overstated.** Shell
/// metacharacters do nothing: no shell is involved (`Command` does not go
/// through `cmd.exe`), so `&`, `|`, `^` and `%` come back from
/// `CommandLineToArgvW` as ordinary argv tokens, a newline stays inside one
/// token, and no *second* program can be injected. The surface is exactly
/// "arbitrary flags to the one pinned program" -- which happens to be the
/// whole machine when that program is a browser.
///
/// **Nothing here validates `args`, and that is a standing decision, not an
/// oversight.** A denylist of dangerous flags is unmaintainable (every
/// Chromium release adds some) and gives false assurance; a confirmation
/// prompt on every Open trains the user to click through it. The mitigation
/// this crate actually ships is [`command_line`], which puts the exact command
/// line in the Open control's tooltip so it can be read before clicking --
/// weak, because it requires hovering, and with two menu entries it requires
/// opening the menu first. Anything stronger is a product decision about what
/// Deskwarden will refuse to run, and belongs with the person making it.
///
/// The one string this crate *does* have to compose is the item's URL, which
/// is appended after `args`; that one goes through [`quote_arg`], because it
/// is being joined onto a command line rather than passed through one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    /// The image to run. **Always a value [`AppMatch::launchable_path`]
    /// returned**, never `AppMatch::path` itself -- see [`app_launch_plan`],
    /// which is the only constructor.
    pub program: String,
    /// Everything after the program, already in Windows command-line form,
    /// to be handed to `raw_arg` as one piece. Empty when there is nothing to
    /// add, in which case no `raw_arg` call is made at all.
    pub raw_tail: String,
}

/// One argument, quoted the way `CommandLineToArgvW` will read it back.
///
/// Used for exactly one thing -- appending the item's URL onto a command line
/// (see [`launch_tail`]) -- and deliberately NOT used on `AppMatch::args`,
/// which is passed through untouched.
///
/// The rules, which are the documented MSVC/`CommandLineToArgvW` ones:
///
///  * a run of `n` backslashes followed by a `"` becomes `2n` backslashes and
///    `\"`; the same run followed by anything else stays `n`;
///  * a run of `n` backslashes at the very end of the argument, inside the
///    quotes, becomes `2n`, so the closing quote is not escaped by it;
///  * an argument with no space, tab or quote in it is returned unchanged --
///    the overwhelmingly common case for a URL, so the tooltip that shows the
///    command line shows something a user recognises;
///  * the empty string becomes `""`, which is how an empty argument is spelled
///    and is not the same as no argument at all.
pub(crate) fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let chars: Vec<char> = arg.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let mut backslashes = 0;
        while i < chars.len() && chars[i] == '\\' {
            backslashes += 1;
            i += 1;
        }
        if i == chars.len() {
            // At the end: double them, so the closing quote below is not
            // escaped by the last one.
            for _ in 0..backslashes * 2 {
                out.push('\\');
            }
        } else if chars[i] == '"' {
            for _ in 0..backslashes * 2 + 1 {
                out.push('\\');
            }
            out.push('"');
            i += 1;
        } else {
            for _ in 0..backslashes {
                out.push('\\');
            }
            out.push(chars[i]);
            i += 1;
        }
    }
    out.push('"');
    out
}

/// The command line after the program: the match's `args` verbatim, then the
/// item's URL if there is one.
///
/// **The URL goes last**, which is where every browser this is for expects it:
/// `chrome.exe --profile-directory="Profile 2" https://...` opens that URL in
/// that profile, and the reverse order makes the URL a positional argument to
/// nothing. It is the ONLY part composed by this crate, so it is the only part
/// [`quote_arg`] touches.
///
/// Both ends are trimmed because leading and trailing whitespace on a command
/// line means nothing to any parser, and a stored `args` of `"   "` would
/// otherwise produce a tail that is nothing but a space -- which
/// `LaunchPlan::raw_tail`'s emptiness test is supposed to catch.
pub(crate) fn launch_tail(args: &str, url: &str) -> String {
    let args = args.trim();
    let url = url.trim();
    match (args.is_empty(), url.is_empty()) {
        (true, true) => String::new(),
        (false, true) => args.to_string(),
        (true, false) => quote_arg(url),
        (false, false) => format!("{args} {}", quote_arg(url)),
    }
}

/// The whole command line a [`LaunchPlan`] will produce, program included --
/// what the Open control puts in its tooltip.
///
/// **Shown to the user on purpose.** This runs a path that came out of vault
/// data; the one thing that makes that reviewable is being able to read what
/// will run before clicking. It is also what a test can assert on, which is
/// why it is a function rather than a `format!` at the call site.
pub(crate) fn command_line(plan: &LaunchPlan) -> String {
    if plan.raw_tail.is_empty() {
        quote_arg(&plan.program)
    } else {
        format!("{} {}", quote_arg(&plan.program), plan.raw_tail)
    }
}

/// The item's website, but only when it is something worth handing to a
/// program or to the shell.
///
/// `super::is_safe_web_url` and not a second copy of the scheme test:
/// `webbrowser_open` refuses anything else anyway, so a `javascript:` URI that
/// got as far as an Open entry would draw a control that silently does
/// nothing. Asked here, the entry is simply not offered.
fn openable_url(website: &str) -> Option<&str> {
    super::is_safe_web_url(website).then_some(website)
}

/// The program this match may start, or `None` if it may not be started at
/// all. **The only constructor of a [`LaunchPlan`], and the only gate.**
///
/// Three refusals, each of which is also a sentence on the card (see
/// [`app_open_refusal`], which is held to this function by
/// `a_refusal_is_shown_exactly_when_there_is_no_plan`):
///
///  * a **dead** binding ([`app_match_is_dead`]) -- the card already says
///    Deskwarden ignores it, and an Open beside that sentence would be this
///    pane offering to act on a binding it has just said it never acts on;
///  * a **hosted** (Microsoft Store / UWP) match -- there is no exe to run.
///    A packaged app is started through the app model, not by `CreateProcess`
///    on its image, and the image under `WindowsApps` is not launchable by
///    path even when one was recorded. Refused explicitly rather than left to
///    `launchable_path`, so the reason the user is shown is the true one;
///  * anything [`AppMatch::launchable_path`] refuses -- an unrecorded path, a
///    relative or UNC or device path, a `..`, an alternate data stream, or a
///    file name that is not this match's own `process`. **There is no fallback
///    branch**: the path that is run is the `&str` that function returned, not
///    `m.path`, so there is no expression in this crate that reaches
///    `Command::new` with a path it refused.
fn app_launch_plan(m: &AppMatch, website: &str) -> Option<LaunchPlan> {
    if app_match_is_dead(m) || m.hosted {
        return None;
    }
    let program = m.launchable_path()?;
    Some(LaunchPlan {
        program: program.to_string(),
        raw_tail: launch_tail(&m.args, openable_url(website).unwrap_or("")),
    })
}

/// What the card says when there is a readable, live binding that still cannot
/// be started -- the failure the user must be told about *before* they click,
/// because there will be no click.
///
/// `None` for a dead binding: [`APP_MATCH_DEAD_NOTICE`] already says the whole
/// truth about it, and a second sentence adding "also, Open is missing" is
/// noise about a consequence.
fn app_open_refusal(m: &AppMatch) -> Option<&'static str> {
    if app_match_is_dead(m) {
        return None;
    }
    if m.hosted {
        return Some(APP_OPEN_HOSTED_NOTE);
    }
    if m.path.is_empty() {
        return Some(APP_OPEN_NO_PATH_NOTE);
    }
    if m.launchable_path().is_none() {
        return Some(APP_OPEN_REFUSED_NOTE);
    }
    None
}

/// Why a Microsoft Store app gets no Open. The word `hosted` stays off the
/// screen, exactly as it does in [`APP_HOSTED_NOTE`].
const APP_OPEN_HOSTED_NOTE: &str =
    "Deskwarden can\u{2019}t start this one for you: Microsoft Store apps are opened through \
     Windows rather than by running a program file. Use the Start menu.";

/// Why a match saved before `path` existed gets no Open.
const APP_OPEN_NO_PATH_NOTE: &str =
    "Deskwarden can\u{2019}t start this app: no program file was recorded when it was matched. \
     Use \u{201c}Add app\u{2026}\u{201d} in the Deskwarden tray menu to pick it again, and the \
     program file will be recorded this time.";

/// Why a recorded path this build will not execute gets no Open. It does not
/// repeat the path -- the Program file row directly above is showing it, which
/// is the whole reason that row shows `path` raw rather than through
/// `launchable_path`.
const APP_OPEN_REFUSED_NOTE: &str =
    "Deskwarden won\u{2019}t start the program file above: it isn\u{2019}t a plain drive path \
     ending in this app\u{2019}s own executable name, so it can\u{2019}t be trusted to be the \
     program that was matched. Re-add the app from the Deskwarden tray menu to record it \
     again.";

/// One thing the card's Open control can do.
///
/// **Not `Option<LaunchPlan>` plus `Option<String>`**: the control's shape --
/// a plain button or a menu -- is "how many of these are there", and a list
/// makes that a `len()` instead of a two-`bool` match at the draw site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OpenChoice {
    /// Start the bound program. `name` is the match's `process`, which is what
    /// the entry is labelled with.
    App { name: String, plan: LaunchPlan },
    /// Open the item's first URI in the default browser.
    Website(String),
}

/// What Open offers for this match and this website, in the order the entries
/// are drawn.
///
/// **The website entry exists only alongside an app entry**, which is the
/// user's own spec ("show dropdown exe or web if both present") and is also
/// the honest scope: an item with a website and no launchable app already has
/// a control that opens it -- the blue URL in the `AUTOFILL TARGETS` card,
/// which reports the very same [`DetailAction::OpenWebsite`] this entry does.
/// A second button for it would be a second way to do one thing, and
/// `the_website_row_has_no_open_button` exists because that was already
/// rejected once.
///
/// So the shapes are: nothing (no launchable app), one plain button (a
/// launchable app and no web URL), or a menu of two.
fn app_open_choices(m: &AppMatch, website: &str) -> Vec<OpenChoice> {
    let Some(plan) = app_launch_plan(m, website) else {
        return Vec::new();
    };
    let mut choices = vec![OpenChoice::App {
        name: m.process.clone(),
        plan,
    }];
    if let Some(url) = openable_url(website) {
        choices.push(OpenChoice::Website(url.to_string()));
    }
    choices
}

/// The word on a menu entry -- and, when there is only one choice, on the
/// button itself. The user asked for "Open {key}", and the key is the
/// executable's name.
fn open_choice_label(choice: &OpenChoice) -> String {
    match choice {
        OpenChoice::App { name, .. } => format!("Open {name}"),
        OpenChoice::Website(_) => OPEN_WEBSITE_LABEL.to_string(),
    }
}

/// The tooltip: for the app, **the exact command line that will run**; for the
/// website, the URL and where it goes. See [`command_line`].
fn open_choice_hover(choice: &OpenChoice) -> String {
    match choice {
        OpenChoice::App { plan, .. } => format!("Runs {}", command_line(plan)),
        OpenChoice::Website(url) => format!("Opens {url} in your default browser"),
    }
}

fn open_choice_action(choice: &OpenChoice) -> DetailAction {
    match choice {
        OpenChoice::App { plan, .. } => DetailAction::OpenApp(plan.clone()),
        OpenChoice::Website(url) => DetailAction::OpenWebsite(url.clone()),
    }
}

/// The website entry's label. A constant so the source pin and the draw site
/// cannot drift.
const OPEN_WEBSITE_LABEL: &str = "Open website";
/// The menu button's own label, when there are two choices behind it.
const OPEN_MENU_LABEL: &str = "Open";
const OPEN_MENU_HOVER: &str = "Open this item\u{2019}s app or its website";

/// The card's footer lines, under the rows: what the selected trigger means,
/// and -- for a Store app -- why this match is keyed on a title.
///
/// **A dead match gets [`APP_MATCH_DEAD_NOTICE`] and NOTHING else.** See that
/// constant: the trigger caption is a claim about what focusing the app does,
/// and there is no trigger on this binding that does anything at all. The
/// hosted note goes with it, because "matched by its window title" is exactly
/// what a dead match failed to be.
fn app_card_notes(m: &AppMatch) -> Vec<&'static str> {
    if app_match_is_dead(m) {
        return vec![APP_MATCH_DEAD_NOTICE];
    }
    let mut notes = vec![trigger_caption(m.trigger)];
    if m.hosted {
        notes.push(APP_HOSTED_NOTE);
    }
    // Last, under the caption that says what the match does, because it is
    // about a control the user is looking for and cannot see. See
    // [`app_open_refusal`], which is `None` exactly when there IS an Open.
    if let Some(refusal) = app_open_refusal(m) {
        notes.push(refusal);
    }
    notes
}

/// Whether the three trigger pills are drawn for `m`.
///
/// **Not on a dead binding.** A pill is a control whose entire meaning is
/// "when this match fires, do THIS"; offering three of them on a match that
/// cannot fire invites a vault write (`SetAppTrigger` PUTs the item and
/// supersedes its `revisionDate`) whose only possible effect is to change
/// which of three things does not happen. Remove stays -- clearing the field
/// is the one action on this binding that does what it says.
fn app_card_offers_triggers(m: &AppMatch) -> bool {
    !app_match_is_dead(m)
}

/// The three trigger pills, in the order they are drawn.
///
/// The same three the picker offers, in the same order, so the control a user
/// met when they created the match is the control they meet when they change
/// it. (The picker's own `TRIGGER_CHOICES` is private to `picker_ui`, and this
/// pass does not own that file; the wording below is held to it by
/// `the_trigger_pills_say_what_the_picker_says`.)
pub(crate) const TRIGGER_ORDER: [TriggerMode; 3] = [TriggerMode::Prompt, TriggerMode::Hotkey, TriggerMode::Auto];

/// A trigger mode's pill label. Exhaustive with no catch-all: a fourth
/// [`TriggerMode`] must be a compile error here rather than silently
/// inheriting a neighbour's name.
pub(crate) fn trigger_label(mode: TriggerMode) -> &'static str {
    match mode {
        TriggerMode::Prompt => "Prompt",
        TriggerMode::Hotkey => "Hotkey",
        TriggerMode::Auto => "Auto",
    }
}

/// The sentence under the pills, saying what the selected mode does.
/// Exhaustive for [`trigger_label`]'s reason.
pub(crate) fn trigger_caption(mode: TriggerMode) -> &'static str {
    match mode {
        TriggerMode::Prompt => "Show the overlay when this app is focused.",
        TriggerMode::Hotkey => "Fill only when the fill hotkey is pressed.",
        TriggerMode::Auto => "Fill immediately when this app is focused.",
    }
}

/// What a click on the `clicked` pill should report, given the mode the match
/// is already on.
///
/// **`None` for the pill that is already selected**, and that is the whole of
/// this function. A segmented control's selected segment still reports clicks,
/// and every one of them would otherwise be a PUT to the user's vault that
/// changes nothing -- each of which supersedes the item's `revisionDate` and
/// so is a chance for the write to fail for no reason at all.
fn app_trigger_click(current: TriggerMode, clicked: TriggerMode) -> Option<DetailAction> {
    (current != clicked).then_some(DetailAction::SetAppTrigger(clicked))
}

/// `m` with its trigger set to `to` and **every other field untouched**.
///
/// Public because the write lives in `vault_window::mod` -- the pane reports
/// a [`DetailAction`] and never writes -- and both must agree that changing
/// the trigger changes exactly one field. `process`, `title`, `hosted` and
/// `path` are the picker's capture off a live window; a write arm that
/// rebuilt them would be a second producer of four fields whose whole value
/// is that they came off a real window once.
pub fn app_match_with_trigger(m: &AppMatch, to: TriggerMode) -> AppMatch {
    AppMatch { trigger: to, ..m.clone() }
}

/// The card's body: the rows, the trigger pills, the notes and Remove -- or,
/// for an item bound to nothing, one sentence saying so.
fn app_match_card(
    ui: &mut egui::Ui,
    app_match: Option<&AppMatch>,
    field_present: bool,
    // The item's website exactly as the `AUTOFILL TARGETS` card has it --
    // empty when there is none, or when this kind does not autofill. Passed
    // in rather than re-read off the item so that the URL this card would
    // open and the URL that card is showing are one expression (see
    // `draw_detail_read`, which derives it once).
    website: &str,
    action: &mut DetailAction,
) {
    let m = match app_card_body(app_match, field_present) {
        AppCardBody::Bound(m) => m,
        AppCardBody::Unbound => {
            card_text(
                ui,
                RichText::new(APP_MATCH_EMPTY_NOTICE)
                    .size(ROW_LABEL_SIZE)
                    .color(theme::TEXT_FAINT),
            );
            return;
        }
        // No rows: there is no parsed match to draw any from, and inventing
        // an "App: (unreadable)" row would be this pane fabricating a field
        // value. The notice says what is wrong, and Remove -- the same
        // control, in the same column, as the bound card's -- clears it.
        AppCardBody::Unreadable => {
            app_notice_with_remove(ui, APP_MATCH_UNREADABLE_NOTICE, action);
            return;
        }
    };
    for (index, app_row) in app_match_rows(m).iter().enumerate() {
        if index > 0 {
            theme::row_rule(ui);
        }
        if app_row.real {
            app_value_row(ui, app_row.label, &app_row.value, action);
        } else {
            row(
                ui,
                app_row.label,
                |ui| {
                    ui.label(
                        RichText::new(&app_row.value)
                            .size(ROW_VALUE_SIZE)
                            .color(theme::TEXT_FAINT),
                    );
                },
                |_ui| {},
            );
        }
    }

    // Skipped entirely on a dead binding -- see `app_card_offers_triggers`.
    // The row is not merely disabled: a greyed control still says "this is
    // the setting for this binding", and the footer note says the binding has
    // no settings because it has no behaviour.
    if app_card_offers_triggers(m) {
        let pill_width = app_card_value_width(ui);
        theme::row_rule(ui);
        // The trigger lives in the VALUE column, not the control group: it is
        // this row's value -- what the match's `trigger` currently is -- and
        // not an action performed on a value shown elsewhere.
        row(
            ui,
            "Autofill",
            |ui| {
                // **Wrapped, and inside the column the rows above measure.**
                // Three pills need about 200pt and the value column is 71pt
                // at the app's minimum window size, so laid out in a plain
                // horizontal row the third one was drawn past the pane's
                // right edge -- the same way `Remove` was, and just as
                // unclickable. See `app_card_value_width`.
                ui.set_max_width(pill_width);
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for mode in TRIGGER_ORDER {
                    let selected = mode == m.trigger;
                    let button =
                        egui::Button::new(theme::semibold(trigger_label(mode), 12.0).color(
                            if selected { egui::Color32::WHITE } else { theme::INK },
                        ))
                        .fill(if selected { theme::BLUE } else { theme::CARD })
                        .stroke(if selected {
                            Stroke::NONE
                        } else {
                            Stroke::new(1.0, theme::BORDER_STRONG)
                        })
                        .corner_radius(CornerRadius::same(7));
                    if ui.add(button).clicked() {
                        if let Some(chosen) = app_trigger_click(m.trigger, mode) {
                            *action = chosen;
                        }
                    }
                }
                });
            },
            |_ui| {},
        );
    }

    theme::row_rule(ui);
    app_card_footer(ui, &app_card_notes(m), &app_open_choices(m, website), action);
}

/// The word on the card's one destructive control, in one place: the button
/// draws it and [`app_footer_controls_width`] measures it, and a footer that
/// reserved room for a different string than it drew would be exactly the
/// drift that put the wide layout's controls off the pane.
const APP_REMOVE_LABEL: &str = "Remove";

/// How much room [`app_card_footer`]'s controls need to sit on the notes'
/// line, with the gap that really separates them.
///
/// Measured through [`theme::row_button_width`] -- the same galley the button
/// will lay -- rather than estimated, so the decision here and the drawing
/// below cannot disagree about whether they fit.
fn app_footer_controls_width(ui: &egui::Ui, choices: &[OpenChoice]) -> f32 {
    let remove = theme::row_button_width(ui, APP_REMOVE_LABEL);
    let open = match choices {
        // No Open at all -- see `app_open_choices`. Remove is the whole
        // control group, and it is never absent: every body this footer draws
        // offers it.
        [] => return remove,
        [only] => theme::row_button_width(ui, &open_choice_label(only)),
        _ => theme::row_button_width(ui, OPEN_MENU_LABEL),
    };
    remove + CONTROL_GAP + open
}

/// One of the card's real rows: [`credential_row`], except that the value
/// **wraps**.
///
/// A plain non-secret value, copied through `CopyValue` -- the door
/// `DetailAction::CopyValue` reserves for values that are not `Zeroizing` in
/// the model, which an exe name and a path are not.
///
/// **The wrap is load-bearing, not cosmetic.** `Program file` is the only
/// value on this pane that is a Windows path: one long token with no break
/// egui will take, and `row_body` lays its band out in a horizontal layout,
/// whose default wrap mode is `Extend`. So the path laid itself out at its
/// natural ~300pt whatever the pane was; the `ScrollArea` grows its content
/// `Ui` to fit its widest child, so the whole MATCHED APP card was then laid
/// out 467.8pt wide inside a 298pt pane -- and the footer's controls went
/// with it. `Remove` was culled past the clip rect and never painted at all,
/// and `Open Ledgerline.exe` began at x = 283.7 with 14 of its 110pt on
/// screen. Wrapping is what keeps the card inside the pane, which is what
/// makes [`app_card_footer`]'s "do the controls fit on this line" question
/// answerable from `ui.available_width()` at all.
///
/// **Scoped to this card rather than moved into [`credential_row`].** Every
/// other value on the pane is prose, an e-mail, a user name or a masked run
/// -- shapes that already break or already fit -- so widening the change
/// would put every row's geometry on this pane in the blast radius of a
/// defect reported about one card.
fn app_value_row(ui: &mut egui::Ui, label: &str, value: &str, action: &mut DetailAction) {
    // **Measured on the card's own `Ui`, before `copy_row` builds the row's
    // band.** Inside the band `ui.available_width()` is derived from a
    // `max_rect` the `ScrollArea` grew to fit LAST frame's widest child --
    // which is this very label. Wrapping to that is a fixed point at the
    // width the text wanted in the first place: it held the card at 467.8pt
    // and changed nothing at all. See [`app_card_content_width`].
    let wrap_width = app_card_value_width(ui);
    copy_row(
        ui,
        label,
        |ui| {
            // **`break_anywhere`, not merely `wrap()`.** egui breaks a
            // wrapped run at word boundaries, and a Windows path offers it
            // exactly one -- its single space -- which still left a 204pt
            // second line and a card 390.9pt wide in a 298pt pane. A path has
            // no word boundaries worth honouring, and breaking it at the
            // character is what makes the column's width the card's width.
            let mut job = egui::text::LayoutJob::simple(
                value.to_string(),
                egui::FontId::new(ROW_VALUE_SIZE, egui::FontFamily::Proportional),
                theme::INK,
                wrap_width,
            );
            job.wrap.break_anywhere = true;
            // **Laid here and handed over as a `Galley`.** Given a
            // `LayoutJob`, `Label` re-lays it and overwrites `wrap.max_width`
            // with its own -- `f32::INFINITY` in a horizontal layout, whose
            // wrap mode is `Extend`. So the job's width was ignored and the
            // path drew its full 285pt anyway. A `Galley` is already laid;
            // `Label` paints it as it is.
            let galley = ui.painter().layout_job(job);
            ui.label(galley);
        },
        |_ui| {},
        DetailAction::CopyValue(value.to_string()),
        None,
        row_offers_copy(value),
        action,
    );
}

/// How wide the MATCHED APP card really is **on screen**: from its own left
/// edge to the right edge of whatever is being clipped to.
///
/// **Not `ui.available_width()`, and the difference is the whole of this
/// defect.** The pane's body is a `ScrollArea`, and a `ScrollArea` grows its
/// content `Ui` to fit the widest thing drawn in it -- so one unwrapped
/// Windows path in the `Program file` row laid the card out 467.8pt wide
/// inside a 298pt pane, and every later question asked of `available_width`
/// got 467.8 back and concluded there was room to spare. The clip rect is the
/// viewport. Nothing drawn inside the card can widen it, so it is the one
/// honest answer to "how much of this can the user see, and reach".
fn app_card_content_width(ui: &egui::Ui) -> f32 {
    (ui.clip_rect().right() - ui.max_rect().left()).max(0.0)
}

/// The width a card row's VALUE column really gets: [`app_card_content_width`]
/// less the card's own padding, the fixed [`ROW_LABEL_WIDTH`] label column and
/// the gap after it.
///
/// One expression, because [`app_card_footer`] asks the same question about
/// the same column when it decides whether its controls fit beside the notes,
/// and a footer measuring a different column than the rows above it is how
/// the two would drift apart again.
fn app_card_value_width(ui: &egui::Ui) -> f32 {
    (app_card_content_width(ui) - f32::from(CARD_PAD_X) * 2.0 - ROW_LABEL_WIDTH - ROW_GAP).max(0.0)
}

/// The card's footer: the notes on the left where a value goes, Remove in the
/// control group where every other row's control goes. An empty label keeps it
/// on the same two columns as the rows above it.
///
/// Shared by the bound card and by [`app_notice_with_remove`], so an
/// unreadable field's Remove is the same control in the same place -- not a
/// second button that happens to say the same word.
///
/// **It stacks when those two columns will not hold it, and that is not a
/// nicety.** Every other row on this pane has a short value and one small
/// control; this one carries a paragraph and up to two buttons, and the label
/// column is a fixed [`ROW_LABEL_WIDTH`] whatever the pane's width is. At the
/// app's minimum window size the detail pane is 298pt, which left the value
/// and the controls about 70pt between them: `Open Ledgerline.exe` was drawn
/// starting at x = 283.7 on a pane 298 wide -- 14 of its 110pt on screen --
/// and `Remove` was not painted at all, egui having culled it past the clip
/// rect. Remove is the ONLY way to undo an app binding from this pane, and
/// this pane refuses horizontal scrolling on purpose, so neither control
/// could be reached by any sequence of clicks until the window was about
/// 1200pt wide.
///
/// **Stacked rather than elided or abbreviated.** The labels are the only
/// thing that says *which* app would be started and *which* binding removed;
/// `Open Le\u{2026}` answers neither. Room was what was missing, so room is
/// what the second line supplies.
///
/// Pinned by `the_matched_app_card_is_reachable_on_the_shortest_window`,
/// whose `assert_visible` measures both axes and the glyphs really laid.
fn app_card_footer(
    ui: &mut egui::Ui,
    notes: &[&str],
    // What Open offers, from [`app_open_choices`]. Empty draws no Open at
    // all, which is every case in which there is nothing this pane may
    // honestly start.
    choices: &[OpenChoice],
    action: &mut DetailAction,
) {
    // The same column the rows above wrap into -- a note left to
    // `available_width` runs off the card exactly as the `Program file` path
    // did (`Show the overlay when this app is focused.` reached x = 394.3 on
    // a 298pt pane).
    let notes_width = app_card_value_width(ui);
    let draw_notes = |ui: &mut egui::Ui| {
        ui.vertical(|ui| {
            ui.set_max_width(notes_width);
            ui.spacing_mut().item_spacing.y = 2.0;
            for note in notes {
                ui.label(
                    RichText::new(*note)
                        .size(ROW_HINT_SIZE)
                        .color(theme::TEXT_GHOST),
                );
            }
        });
    };
    // What is left of the row for the controls once the card's own padding
    // and the fixed label column are taken out -- 162pt, which at the app's
    // minimum window size is most of the pane. Through `app_card_value_width`
    // and NOT `ui.available_width()`: see `app_card_content_width` for why
    // the latter answers with a width this card grew for itself.
    let room = app_card_value_width(ui);
    if room < app_footer_controls_width(ui, choices) {
        app_card_footer_stacked(ui, notes, draw_notes, choices, action);
        return;
    }
    row(
        ui,
        "",
        draw_notes,
        |ui| {
            // **One click, no arming.** `confirm_click`'s two-click gate is
            // reserved for the item Delete, which trashes the whole item;
            // this removes one custom field, the card says so immediately by
            // flipping to `APP_MATCH_EMPTY_NOTICE`, and that notice names the
            // way to put it back. Making this the third armed control on the
            // pane would have cost `draw_detail_read` another parameter and
            // `vault_window::mod` another piece of per-item pending state,
            // for a click whose undo is four clicks in the tray.
            //
            // Hand-editing `process` and `path` is deliberately NOT offered
            // here -- see the module's own note on the card.
            app_card_remove_control(ui, action);
            // **After Remove, so it reads BEFORE it.** This control group is
            // laid out right-to-left (see `row_body`), and Open is the
            // ordinary action while Remove is the destructive one.
            app_card_open_control(ui, choices, action);
        },
    );
}

/// [`app_card_footer`] on a pane too narrow to hold the controls beside the
/// notes: the notes take the row's whole content width, and the controls get
/// a line of their own beneath them.
///
/// The controls line starts at the card's own [`CARD_PAD_X`] -- the left edge
/// every label above it sits on -- and reads Open, then Remove, which is what
/// the wide layout's right-to-left group paints. Added in the opposite source
/// order there for that reason; here the layout is left-to-right, so they are
/// added in the order they are read. Two layouts, one control set.
///
/// `horizontal_wrapped` rather than a plain `horizontal`, so a longer program
/// name or a future third control takes a second line instead of the fate
/// this whole function exists to undo.
fn app_card_footer_stacked(
    ui: &mut egui::Ui,
    notes: &[&str],
    draw_notes: impl FnOnce(&mut egui::Ui),
    choices: &[OpenChoice],
    action: &mut DetailAction,
) {
    // Only when there is something to say: an empty notes row would be a
    // band of padding above the controls.
    if !notes.is_empty() {
        row(ui, "", draw_notes, |_ui| {});
    }
    egui::Frame::new()
        .inner_margin(Margin::symmetric(CARD_PAD_X, ROW_PAD_Y))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = CONTROL_GAP;
                app_card_open_control(ui, choices, action);
                app_card_remove_control(ui, action);
            });
        });
}

/// The card's one destructive control, in one place because two layouts draw
/// it -- see [`app_card_footer_stacked`].
fn app_card_remove_control(ui: &mut egui::Ui, action: &mut DetailAction) {
    if theme::row_button(ui, APP_REMOVE_LABEL)
        .on_hover_text("Stop autofilling this item into that app")
        .clicked()
    {
        *action = DetailAction::RemoveAppMatch;
    }
}

/// The Open half of the footer's controls, in one place for the same reason
/// [`app_card_remove_control`] is.
///
/// Three shapes, and `choices.len()` is the whole decision -- see
/// [`app_open_choices`], which owns it.
fn app_card_open_control(ui: &mut egui::Ui, choices: &[OpenChoice], action: &mut DetailAction) {
    match choices {
        // Nothing this pane may start. The reason is a sentence in the notes
        // beside it, never a disabled button: a greyed control says "not
        // now", and every one of these cases is "not until you change
        // something".
        [] => {}
        // One thing to do, so no menu to open first. The button says which
        // thing, because "Open" alone beside a Program file row and a website
        // is a question.
        [only] => {
            if theme::row_button(ui, &open_choice_label(only))
                .on_hover_text(open_choice_hover(only))
                .clicked()
            {
                *action = open_choice_action(only);
            }
        }
        // Both. The user's own words: "show dropdown exe or web if both
        // present".
        many => {
            let open = theme::row_button(ui, OPEN_MENU_LABEL).on_hover_text(OPEN_MENU_HOVER);
            egui::Popup::menu(&open).show(|ui| {
                for choice in many {
                    if ui
                        .button(open_choice_label(choice))
                        .on_hover_text(open_choice_hover(choice))
                        .clicked()
                    {
                        *action = open_choice_action(choice);
                        ui.close();
                    }
                }
            });
        }
    }
}

/// A card body that is one sentence and a Remove: the shape an unreadable
/// `deskwarden:app-match` field gets (see [`APP_MATCH_UNREADABLE_NOTICE`]).
///
/// It goes through [`app_card_footer`] rather than drawing its own button so
/// that the control reports the same [`DetailAction::RemoveAppMatch`] the
/// bound card's does -- and so the write arm in `vault_window::mod`, which
/// clears the field by NAME through `without_app_match`, is reached by both.
fn app_notice_with_remove(ui: &mut egui::Ui, notice: &'static str, action: &mut DetailAction) {
    // No Open, and not because none was computed: there is no parsed match to
    // compute one from. Remove is the only honest offer on an unreadable
    // field, and that is the whole of this body.
    app_card_footer(ui, &[notice], &[], action);
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
///
/// **The tooltip does not get that for free**, and assuming it did was a
/// bug the user reported: over the eye and over the website link the tile
/// still offered to copy. See the `hovered()` gate below for what egui does
/// instead and why the gate has to be on the call.
fn copy_row(
    ui: &mut egui::Ui,
    label: &str,
    value: impl FnOnce(&mut egui::Ui),
    controls: impl FnOnce(&mut egui::Ui),
    on_copy: DetailAction,
    // Where this row's chord is told to the user, if it has one. Every copy
    // row gets a tooltip; only some have a chord, and only some of those
    // paint it on the row; the chord is drawn to the right of them.
    hint: Option<CopyShortcut>,
    // **Whether there is anything to copy at all** -- the caller's answer,
    // because this function cannot work it out. The secret variants
    // (`CopyPassword`, `CopyUsername`, `CopyTotp`, `CopyCardNumber`,
    // `CopyCardCode`, `CopySshPrivateKey`) deliberately do NOT carry their
    // value (see `DetailAction`'s docs), so `on_copy` says which field and
    // never whether it is empty. Every caller derives this from the value it
    // is about to paint, through `row_offers_copy`.
    copyable: bool,
    action: &mut DetailAction,
) {
    // **The chord is added FIRST, which puts it at the far RIGHT.** The
    // control group packs right-to-left, so the earliest widget is the
    // rightmost one. The user asked for the keys to be "always in the end",
    // and for the Password row specifically to read eye-then-chord: adding
    // the chord first and the row's own controls after it gives exactly
    // that, on every row, without any row needing to know the rule.
    //
    // The previous arrangement (controls first, chord after) put the chord
    // to the LEFT of the eye, so the key drifted inwards on rows that had a
    // control and sat at the edge on rows that did not -- the ragged result
    // this ordering exists to avoid.
    let controls = |ui: &mut egui::Ui| {
        shortcut_hint(ui, hint);
        controls(ui);
    };
    // **`Sense::hover()` when there is nothing to copy**, which is what
    // withdraws the tint and the pointing hand -- `row_impl` gates both on
    // the sense it was handed. See [`row_offers_copy`] for why the row is
    // made inert rather than merely silent.
    if !copyable {
        row_impl(ui, label, value, controls, egui::Sense::hover());
        return;
    }
    let response = row_impl(ui, label, value, controls, egui::Sense::click());
    // **Asked for only while the tile itself is what the pointer is on.**
    // `Response::hovered` is egui's answer to "which one widget would a click
    // go to", so over the eye or the website link it is the CHILD that is
    // hovered and this tile is not -- the same layering the click already
    // relies on, applied to the tooltip, which does not get it for free.
    //
    // Gating the CALL rather than trusting `on_hover_text` to gate itself is
    // the whole fix. `Tooltip::should_show_tooltip` returns true, before it
    // ever looks at `hovered`, for a tooltip that is ALREADY OPEN and whose
    // widget rect still contains the pointer -- and the tile's rect contains
    // the eye. So the real gesture (rest on the row, read "Click to copy",
    // slide across to the eye) kept this tooltip up all the way onto the eye,
    // and egui shows one tooltip per layer, so the eye's own "Reveal" was
    // then refused. Hovering the eye offered to copy, which is not what a
    // click there does.
    let response = if response.hovered() {
        response.on_hover_text(copy_row_tooltip(hint))
    } else {
        response
    };
    if response.clicked() {
        *action = on_copy;
        // The row names its own confirmation, off the very label it just
        // painted -- so the toast cannot say a field the row does not.
        note_copied(ui.ctx(), label);
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
/// the keyboard chord that copies the same value without the mouse, named in
/// the tile's tooltip (see [`copy_row_tooltip`]).
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
        |_ui| {},
        on_copy,
        hint,
        row_offers_copy(value),
        action,
    );
}

fn password_row(ui: &mut egui::Ui, password: &str, revealed: &mut bool, action: &mut DetailAction) {
    masked_row(
        ui,
        // See `copy_shortcut_label`: one string for the row and its toast.
        copy_shortcut_label(CopyShortcut::Password),
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
        },
        // **Copies the real value even while masked.** The mask is a display
        // concern; it was never what the old Copy button honoured either.
        on_copy,
        hint,
        // **`value`, never `shown`.** `shown` is the bullet run while the row
        // is masked, which is never empty -- deriving the offer from it would
        // make an empty Password row copyable again, and mask the bug behind
        // the mask.
        row_offers_copy(value),
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
        // See `copy_shortcut_label`: one string for the row and its toast.
        // The three NON-code One-time code rows keep their own literal --
        // they copy nothing, so they have no toast to disagree with.
        copy_shortcut_label(CopyShortcut::Totp),
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
        |_ui| {},
        DetailAction::CopyTotp,
        Some(CopyShortcut::Totp),
        // Stated rather than assumed true. `TotpRow::Code` is only reached
        // from `TotpState::Code`, whose code has always been non-empty in
        // practice -- but "in practice" is what the Password row had too.
        row_offers_copy(code),
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
            // No folder: this harness reads the pane's BODY, and every one of
            // its callers would have to gain a folder to keep saying what it
            // says. The header's own subtitle has `Pane`, which carries one.
            draw_detail_read(ui, item, None, 3, totp, delete_pending, &mut reveal, None);
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
            // The out-of-vault pane copies nothing, so it has no deadline of
            // its own; no test here reads this.
            repaint_delay: std::time::Duration::MAX,
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
            draw_detail_read(ui, item, None, 3, totp, false, &mut reveal, None);
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
        /// The folder name the window would have resolved for this item --
        /// `None` unless a test sets it, which is the vault's usual case.
        folder: Option<String>,
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
        /// The soonest this frame asked egui to come back on its own.
        ///
        /// egui redraws on input; anything with a DEADLINE has to say so, or
        /// it stays on screen until something else happens to cause a frame.
        /// The copy confirmation is the one thing on this pane with a
        /// deadline, so this is how a test can see that it scheduled its own
        /// disappearance rather than hoping for a mouse move.
        repaint_delay: std::time::Duration,
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

        /// The one filled `colour` rectangle that encloses `inner` -- the
        /// surface a run of text is painted ON, which paints no string of its
        /// own and so cannot be found by name.
        ///
        /// The enclosure is what makes this specific: the pane is full of
        /// filled rects, and only the confirmation's own box contains the
        /// confirmation's own glyphs.
        fn filled_box_around(&self, inner: egui::Rect, colour: egui::Color32) -> egui::Rect {
            let found: Vec<egui::Rect> = self
                .rects
                .iter()
                .filter(|(rect, fill)| *fill == colour && rect.contains_rect(inner))
                .map(|(rect, _)| *rect)
                .collect();
            assert_eq!(
                found.len(),
                1,
                "expected exactly one {colour:?} box around {inner:?}, found {found:?}"
            );
            found[0]
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
                folder: None,
            }
        }

        /// The same pane, drawing an item the window has resolved a folder
        /// name for.
        fn in_folder(mut self, name: &str) -> Self {
            self.folder = Some(name.to_string());
            self
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
                        self.folder.as_deref(),
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
                repaint_delay: output
                    .viewport_output
                    .values()
                    .map(|viewport| viewport.repaint_delay)
                    .min()
                    .unwrap_or(std::time::Duration::MAX),
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

        /// The pointer resting on `pos` **long enough for a tooltip**, and
        /// the frame that produced.
        ///
        /// A tooltip is not a hover affordance: egui holds it back for
        /// `Style::interaction::tooltip_delay` (0.5s) after the pointer
        /// *last moved*, and only shows it while the pointer is still. So
        /// [`Pane::hover`]'s single frame paints none, and -- this is the
        /// part that cost an hour -- neither do forty of them: every
        /// `PointerMoved` event resets egui's "last movement" clock, so a
        /// pointer re-announced at the same position each pass is a pointer
        /// that never settles.
        ///
        /// One move, then frames with no input at all. egui keeps the
        /// pointer where it was and advances its own clock by one predicted
        /// frame (1/60s) per pass, so forty of them rest it for 0.66s --
        /// past the delay without being derived from it.
        fn hover_settled(&mut self, item: &VaultItem, totp: &TotpState, pos: egui::Pos2) -> Frame {
            let mut frame = self.hover(item, totp, pos);
            for _ in 0..40 {
                frame = self.idle(item, totp);
            }
            frame
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

    /// **The folder joins that line; it does not get one of its own.**
    ///
    /// Design 2b line 803 is `Login · Engineering` in a single 12px run. The
    /// user asked for the folder "under the title like design has it - if no
    /// folder just don't print anything", and "nothing" is the separator and
    /// the name: the line still reads `Login`. It is asserted for every kind,
    /// so a subtitle wired to a fixed "Login" cannot pass.
    ///
    /// The no-folder half is here as well as in
    /// `the_header_subtitle_is_the_items_own_kind` because that test reads the
    /// rendered pane and this one reads the decision; between them the two
    /// mutations that produce a bare kind line -- dropping the folder, and
    /// dropping the subtitle -- are both caught.
    #[test]
    fn the_header_subtitle_carries_the_folder_after_the_kind_and_nothing_when_there_is_none() {
        for kind in EVERY_KIND {
            assert_eq!(
                header_subtitle(kind, Some("Engineering")),
                format!("{} · Engineering", kind.label()),
                "{kind:?} does not read as the design's `<kind> · <folder>`"
            );
            assert_eq!(
                header_subtitle(kind, None),
                kind.label(),
                "{kind:?} with no folder does not still name its kind"
            );
        }
    }

    /// **And the header really paints it, on the title's own line-under.**
    ///
    /// The decision above is pure; this is the other half. It also holds the
    /// shape: ONE run, so a folder painted as a second line under the kind --
    /// which is what "show Folder under the title" could equally have meant --
    /// fails here rather than shipping.
    ///
    /// An item whose `folder_id` names a folder that is not in the list never
    /// reaches this function with a name at all: `sidebar::folder_name`
    /// answers `None` for it (see
    /// `a_folder_is_named_only_when_the_list_really_has_it`) and the pane then
    /// draws the no-folder case, which is the line the kind test above pins.
    #[test]
    fn the_header_paints_the_folder_beside_the_kind_under_the_item_name() {
        let mut pane = Pane::new().in_folder("Engineering");
        let frame = pane.idle(&a_login(), &TotpState::NoSecret);

        assert!(
            frame.painted("Login · Engineering"),
            "the header painted no `<kind> · <folder>` subtitle; it painted: {:?}",
            frame.strings()
        );
        // Neither half on its own: a bare "Login" beside the combined line
        // would be the old subtitle still there, and a bare "Engineering"
        // would be the second line the design does not draw.
        for stray in ["Login", "Engineering"] {
            assert!(
                !frame.strings().iter().any(|t| *t == stray),
                "the header painted {stray:?} as a run of its own, so the subtitle is \
                 not the design's single line; it painted: {:?}",
                frame.strings()
            );
        }

        let title = frame.rect_of("Sample");
        let subtitle = frame.rect_of("Login · Engineering");
        assert!(
            (subtitle.left() - title.left()).abs() < 0.5,
            "the subtitle at {subtitle:?} does not share the title's column ({title:?})"
        );
        assert!(
            subtitle.top() > title.top(),
            "the subtitle at {subtitle:?} is not UNDER the title at {title:?}"
        );
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

    // -----------------------------------------------------------------
    // The MATCHED APP card.
    // -----------------------------------------------------------------

    /// The match the picker builds for an ordinary desktop app: an exe name,
    /// a full image path, no title, not hosted.
    fn a_desktop_match() -> AppMatch {
        AppMatch {
            process: "Ledgerline.exe".to_string(),
            title: String::new(),
            hosted: false,
            path: r"C:\Apps\Ledgerline\Ledgerline.exe".to_string(),
            args: String::new(),
            trigger: TriggerMode::Prompt,
        }
    }

    /// The match the picker builds for a Microsoft Store row: the only shape
    /// that records a title, and the only one whose title is ever matched on.
    fn a_store_match() -> AppMatch {
        AppMatch {
            process: "Speedtest.exe".to_string(),
            title: "Speedtest".to_string(),
            hosted: true,
            path: r"C:\Program Files\WindowsApps\Speedtest\Speedtest.exe".to_string(),
            args: String::new(),
            trigger: TriggerMode::Hotkey,
        }
    }

    /// `item` carrying `m` in the `deskwarden:app-match` custom field --
    /// through `vault_bridge::with_app_match`, the one producer of that
    /// field, so these tests read back exactly what a real save writes.
    fn bound_to(item: &VaultItem, m: &AppMatch) -> VaultItem {
        crate::vault_bridge::with_app_match(item, m)
    }

    #[test]
    fn the_card_is_offered_wherever_a_match_would_do_something_and_shown_wherever_one_exists() {
        for kind in EVERY_KIND {
            // A binding is NEVER hidden, whatever the item is: an app match
            // on a secure note is precisely the one a user needs to find in
            // order to remove it.
            assert!(
                app_card_visible(true, kind),
                "a {kind:?} that IS bound to an app does not show that binding"
            );
            // With no match, the card follows the same predicate the Fill
            // button and AUTOFILL TARGETS follow, so the three cannot
            // disagree about which kinds autofill.
            assert_eq!(
                app_card_visible(false, kind),
                kind_offers_fill(kind),
                "an unbound {kind:?} offers the card on different terms from Fill"
            );
        }
        // The control: the two answers really are different for some kind,
        // so the assertions above are not both satisfied by a constant.
        assert!(!app_card_visible(false, ItemKind::SecureNote));
        assert!(app_card_visible(false, ItemKind::Login));
    }

    #[test]
    fn the_cards_rows_name_the_app_and_the_program_file() {
        let rows = app_match_rows(&a_desktop_match());
        assert_eq!(
            rows,
            vec![
                AppRow { label: "App", value: "Ledgerline.exe".to_string(), real: true },
                AppRow {
                    label: "Program file",
                    value: r"C:\Apps\Ledgerline\Ledgerline.exe".to_string(),
                    real: true,
                },
            ],
            "the rows are not the user's \"name, path\""
        );
    }

    /// Every match saved before `path` existed -- a shape still sitting in
    /// real vaults. The row must say so, and must not offer to copy the words
    /// "Not recorded" onto the clipboard.
    #[test]
    fn a_match_that_recorded_no_program_file_says_so_and_that_row_is_inert() {
        let m = AppMatch::for_process("Ledgerline.exe", TriggerMode::Auto);
        let rows = app_match_rows(&m);
        let path = rows
            .iter()
            .find(|r| r.label == "Program file")
            .expect("the card dropped the Program file row entirely");
        assert_eq!(path.value, "Not recorded");
        assert!(!path.real, "the placeholder would be copied to the clipboard");
        // Control: a match that DID record one is copyable, so `real` is not
        // simply always false.
        let recorded = app_match_rows(&a_desktop_match());
        assert!(recorded.iter().find(|r| r.label == "Program file").unwrap().real);
    }

    /// **An unhosted title is inert by design** (see `AppMatch::hosted`):
    /// every one saved during the commit that recorded titles for every row
    /// is deliberately never matched on, so drawing it would tell the user it
    /// does something. The four-key shape below is the literal JSON that
    /// commit wrote.
    #[test]
    fn a_title_that_is_never_matched_on_is_never_drawn() {
        let stored = AppMatch::from_field_value(
            r#"{"process":"Ledgerline.exe","title":"Ledgerline - Invoices","path":"C:\\Apps\\Ledgerline.exe","trigger":"prompt"}"#,
        )
        .expect("a shipped field value must parse");
        assert_eq!(stored.title, "Ledgerline - Invoices", "the premise: it HAS a title");
        assert!(!stored.hosted);
        assert!(
            !app_match_rows(&stored).iter().any(|r| r.label == "Window title"),
            "an inert title is drawn as if it matched something"
        );
        // Control: the row exists at all, for the match that really is keyed
        // on its title.
        assert!(app_match_rows(&a_store_match())
            .iter()
            .any(|r| r.label == "Window title" && r.value == "Speedtest"));
    }

    #[test]
    fn a_store_apps_card_explains_why_it_is_matched_by_a_title() {
        let notes = app_card_notes(&a_store_match());
        assert!(
            notes.iter().any(|n| n.contains("Microsoft Store")),
            "a Store match does not say why it behaves differently: {notes:?}"
        );
        assert!(
            !notes.iter().any(|n| n.contains("hosted") || n.contains("FrameHost")),
            "the mechanism reached the screen: {notes:?}"
        );
        // The caption for the SELECTED mode is there too, and it is the
        // selected one -- Hotkey, not the first in the list.
        assert!(notes.contains(&trigger_caption(TriggerMode::Hotkey)), "{notes:?}");
        assert!(!notes.contains(&trigger_caption(TriggerMode::Prompt)), "{notes:?}");
        // Control: an ordinary app gets the caption and NOT the Store note.
        let ordinary = app_card_notes(&a_desktop_match());
        assert_eq!(ordinary, vec![trigger_caption(TriggerMode::Prompt)]);
    }

    #[test]
    fn every_trigger_mode_is_a_pill_with_its_own_name_and_its_own_sentence() {
        // A fourth `TriggerMode` left out of `TRIGGER_ORDER` fails here; one
        // left out of `trigger_label`/`trigger_caption` fails to compile.
        for mode in [TriggerMode::Prompt, TriggerMode::Hotkey, TriggerMode::Auto] {
            assert!(TRIGGER_ORDER.contains(&mode), "{mode:?} has no pill to click");
        }
        let labels: std::collections::BTreeSet<&str> =
            TRIGGER_ORDER.iter().map(|m| trigger_label(*m)).collect();
        let captions: std::collections::BTreeSet<&str> =
            TRIGGER_ORDER.iter().map(|m| trigger_caption(*m)).collect();
        assert_eq!(labels.len(), 3, "two pills share a name: {labels:?}");
        assert_eq!(captions.len(), 3, "two modes share a sentence: {captions:?}");
    }

    #[test]
    fn clicking_the_pill_that_is_already_selected_costs_no_vault_write() {
        for mode in TRIGGER_ORDER {
            assert_eq!(
                app_trigger_click(mode, mode),
                None,
                "re-clicking {mode:?} would PUT the item for no change"
            );
            for other in TRIGGER_ORDER.iter().filter(|m| **m != mode) {
                assert_eq!(
                    app_trigger_click(mode, *other),
                    Some(DetailAction::SetAppTrigger(*other)),
                    "moving from {mode:?} to {other:?} reports the wrong thing"
                );
            }
        }
    }

    #[test]
    fn changing_the_trigger_changes_the_trigger_and_nothing_else() {
        let before = a_store_match();
        let after = app_match_with_trigger(&before, TriggerMode::Auto);
        assert_eq!(after.trigger, TriggerMode::Auto);
        // Each field named, not `..before`: the whole risk is that the write
        // path rebuilds one of the four the picker captured off a live
        // window.
        assert_eq!(after.process, before.process);
        assert_eq!(after.title, before.title);
        assert_eq!(after.hosted, before.hosted);
        assert_eq!(after.path, before.path);
        assert_ne!(after.trigger, before.trigger, "the premise: it really did change");
    }

    // --- the wiring: does the pane actually draw and report any of this ---

    /// Deleting the `card(ui, APP_CARD_HEADING, ...)` call in
    /// `draw_detail_read` fails this.
    #[test]
    fn the_pane_draws_the_matched_app_card_for_a_bound_item() {
        let item = bound_to(&a_login(), &a_store_match());
        let mut pane = Pane::new();
        let frame = pane.idle(&item, &TotpState::NoSecret);
        for needle in [
            "MATCHED APP",
            "App",
            "Speedtest.exe",
            "Window title",
            "Speedtest",
            "Program file",
            r"C:\Program Files\WindowsApps\Speedtest\Speedtest.exe",
            "Autofill",
            "Prompt",
            "Hotkey",
            "Auto",
            "Remove",
            APP_HOSTED_NOTE,
        ] {
            assert!(
                frame.painted(needle),
                "the pane painted no {needle:?}; it painted: {:?}",
                frame.strings()
            );
        }
        // The path is read back with the GLYPHS that were laid out, not the
        // source string: a value column too narrow to hold it would elide to
        // "…" and still report the full path in `texts`.
        assert!(
            frame
                .rendered_glyphs(r"C:\Program Files\WindowsApps\Speedtest\Speedtest.exe")
                .contains("Speedtest.exe"),
            "the program file was drawn but not legibly"
        );
        // The control: the same pane, on the same item with no match, paints
        // none of the values -- so the assertions above are about this card
        // and not about something else on the pane that happens to say
        // "App".
        let unbound = a_login();
        let bare = pane.idle(&unbound, &TotpState::NoSecret);
        assert!(!bare.painted("Speedtest.exe"), "{:?}", bare.strings());
        assert!(!bare.painted("Window title"), "{:?}", bare.strings());
    }

    #[test]
    fn the_pane_says_so_when_an_item_is_bound_to_nothing() {
        let mut pane = Pane::new();
        let frame = pane.idle(&a_login(), &TotpState::NoSecret);
        assert!(frame.painted("MATCHED APP"), "{:?}", frame.strings());
        assert!(
            frame.strings().iter().any(|t| t.contains("Add app...")),
            "the empty card does not name the way to create a match: {:?}",
            frame.strings()
        );
        assert!(
            !frame.painted("Remove"),
            "an item bound to nothing offers to unbind it"
        );
        // Control: an item that IS bound does NOT show the empty notice.
        let bound = pane.idle(&bound_to(&a_login(), &a_desktop_match()), &TotpState::NoSecret);
        assert!(
            !bound.strings().iter().any(|t| t.contains("Add app...")),
            "a bound item is told it has no match: {:?}",
            bound.strings()
        );
    }

    /// A secure note is not filled into anything, so an unbound one gets no
    /// card -- but a bound one still shows what it is bound to.
    #[test]
    fn a_kind_that_never_autofills_is_offered_no_card_until_it_has_one() {
        let note = an_item(Some(2));
        let mut pane = Pane::new();
        let bare = pane.idle(&note, &TotpState::NoSecret);
        assert!(
            !bare.painted("MATCHED APP"),
            "a secure note is offered an autofill binding: {:?}",
            bare.strings()
        );
        let bound = pane.idle(&bound_to(&note, &a_desktop_match()), &TotpState::NoSecret);
        assert!(
            bound.painted("MATCHED APP") && bound.painted("Ledgerline.exe"),
            "a secure note that IS bound cannot see or remove that binding: {:?}",
            bound.strings()
        );
    }

    /// Deleting `*action = chosen;` in the pill loop fails this.
    #[test]
    fn clicking_a_trigger_pill_reports_the_mode_it_names() {
        // From Prompt, so both of the other two are a real change.
        let item = bound_to(&a_login(), &a_desktop_match());
        for mode in [TriggerMode::Hotkey, TriggerMode::Auto] {
            let mut pane = Pane::new();
            let laid_out = pane.idle(&item, &TotpState::NoSecret);
            let pill = laid_out.rect_of(trigger_label(mode));
            let clicked = pane.click(&item, &TotpState::NoSecret, pill.center());
            assert_eq!(
                clicked.action,
                DetailAction::SetAppTrigger(mode),
                "clicking the {mode:?} pill reported {:?}",
                clicked.action
            );
        }
        // The other half of `app_trigger_click`, at the pane: the pill that
        // is already selected reports nothing at all.
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        let pill = laid_out.rect_of(trigger_label(TriggerMode::Prompt));
        let clicked = pane.click(&item, &TotpState::NoSecret, pill.center());
        assert_eq!(
            clicked.action,
            DetailAction::None,
            "re-clicking the selected pill reported {:?}, which is a vault write for no change",
            clicked.action
        );
    }

    /// Deleting `*action = DetailAction::RemoveAppMatch;` fails this.
    #[test]
    fn clicking_remove_reports_that_the_binding_should_go() {
        let item = bound_to(&a_login(), &a_store_match());
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        let remove = laid_out.rect_of("Remove");
        let clicked = pane.click(&item, &TotpState::NoSecret, remove.center());
        assert_eq!(
            clicked.action,
            DetailAction::RemoveAppMatch,
            "clicking Remove reported {:?}",
            clicked.action
        );
    }

    // --- review 20's Important 1: a binding the engine has dropped ---

    /// A match in the shape the reported bug produced: the host's own name,
    /// no title, not hosted. `MatchEngine::rebuild` drops it from both
    /// tables, and the card used to draw it as `App: ApplicationFrameHost.exe`
    /// under "Show the overlay when this app is focused."
    fn a_dead_host_match() -> AppMatch {
        AppMatch {
            process: "ApplicationFrameHost.exe".to_string(),
            title: String::new(),
            hosted: false,
            path: String::new(),
            args: String::new(),
            trigger: TriggerMode::Prompt,
        }
    }

    /// **The predicate is checked against `MatchEngine` itself, not against a
    /// second spelling of its filter.** An engine is built from the one match
    /// and asked every foreground window that could plausibly reach it; the
    /// card is "right" only when it says dead exactly on the matches no such
    /// window can look up.
    ///
    /// The candidate windows are a cross product, not the one event the match
    /// was captured off: a match is dead only if NOTHING can find it, and
    /// probing a single event would call a title-keyed match dead merely
    /// because the process-keyed event misses it.
    #[test]
    fn a_card_calls_a_match_dead_exactly_when_the_engine_can_never_look_it_up() {
        use crate::match_engine::MatchEngine;
        use crate::window_watch::ForegroundEvent;

        let mut any_dead = false;
        let mut any_live = false;
        for process in ["Ledgerline.exe", "ApplicationFrameHost.exe"] {
            for title in ["", "Speedtest"] {
                for hosted in [false, true] {
                    let m = AppMatch {
                        process: process.to_string(),
                        title: title.to_string(),
                        hosted,
                        path: String::new(),
                        args: String::new(),
                        trigger: TriggerMode::Prompt,
                    };
                    let mut engine = MatchEngine::new();
                    engine.rebuild(&[("item-1".to_string(), m.clone())]);

                    // Every window that could conceivably reach this entry:
                    // its own process and its own title, the host frame, and
                    // a stranger's name and title as the control.
                    let mut reachable = false;
                    for exe_name in [process, "ApplicationFrameHost.exe", "Stranger.exe"] {
                        for window_title in [title, "", "Some Other Window"] {
                            let event = ForegroundEvent {
                                hwnd: 1,
                                pid: 2,
                                exe_name: exe_name.to_string(),
                                title: window_title.to_string(),
                            };
                            if engine.lookup(&event).is_some() {
                                reachable = true;
                            }
                        }
                    }

                    assert_eq!(
                        app_match_is_dead(&m),
                        !reachable,
                        "the card and the match engine disagree about {m:?}: the card says \
                         dead={}, the engine says it is reachable={reachable}",
                        app_match_is_dead(&m)
                    );
                    if reachable {
                        any_live = true;
                    } else {
                        any_dead = true;
                    }
                }
            }
        }
        // The control: the loop really exercised both answers, so the
        // assertion above is not satisfied by a constant on either side.
        assert!(any_dead, "no shape in the matrix was dead");
        assert!(any_live, "no shape in the matrix was live");
    }

    /// **The frame that said both things at once.** The reviewer's exhibit
    /// was one frame painting "...ignoring this match, so it never fires..."
    /// and a live blue "Fill in app" above it. Asserted on the PANE, not on
    /// the predicate: a gate correct in `item_offers_fill` and never reached
    /// by the header is this repository's signature defect.
    #[test]
    fn a_dead_binding_is_not_offered_a_fill_in_the_frame_that_says_it_never_fires() {
        let dead = bound_to(&a_login(), &a_dead_host_match());
        let mut pane = Pane::new();
        let frame = pane.idle(&dead, &TotpState::NoSecret);

        // The premise, read off the same frame: the card really is saying the
        // binding is ignored. Without it this test would pass on a pane that
        // drew no card at all.
        assert!(
            frame.strings().iter().any(|t| t.contains("ignoring this match")),
            "the card is not calling this binding dead, so there is nothing to contradict: \
             {:?}",
            frame.strings()
        );
        assert!(
            !frame.painted(FILL_LABEL),
            "the pane offers {FILL_LABEL:?} on a binding it has just said never fires: {:?}",
            frame.strings()
        );

        // **The positive control, and it is the whole test.** The same kind,
        // the same pane, differing only in the binding -- so a header that
        // had simply stopped drawing Fill altogether could not pass.
        let mut pane = Pane::new();
        let live = pane.idle(&bound_to(&a_login(), &a_desktop_match()), &TotpState::NoSecret);
        assert!(
            live.painted(FILL_LABEL),
            "a login bound to a LIVE match lost its Fill button: {:?}",
            live.strings()
        );
        let mut pane = Pane::new();
        let unbound = pane.idle(&a_login(), &TotpState::NoSecret);
        assert!(
            unbound.painted(FILL_LABEL),
            "an unbound login lost its Fill button: {:?}",
            unbound.strings()
        );
    }

    /// The predicate behind the frame above, over every kind and both
    /// bindings, so the pair the header and `fill_hotkey_applies` share is
    /// pinned in one place.
    #[test]
    fn only_a_fillable_kind_with_a_binding_that_can_fire_offers_a_fill() {
        for kind in EVERY_KIND {
            let bare = an_item(item_type_for(kind));
            assert_eq!(
                item_offers_fill(&bare, kind),
                kind_offers_fill(kind),
                "{kind:?}: an item bound to nothing should follow the kind alone"
            );
            let dead = bound_to(&bare, &a_dead_host_match());
            assert!(
                !item_offers_fill(&dead, kind),
                "{kind:?}: a dead binding is still offered a fill"
            );
            let live = bound_to(&bare, &a_desktop_match());
            assert_eq!(
                item_offers_fill(&live, kind),
                kind_offers_fill(kind),
                "{kind:?}: a LIVE binding changed the answer, which is the control"
            );
        }
        // The control on `item_binding_is_dead` itself: it is the binding
        // that decides, and the three inputs really do differ.
        assert!(item_binding_is_dead(&bound_to(&a_login(), &a_dead_host_match())));
        assert!(!item_binding_is_dead(&bound_to(&a_login(), &a_desktop_match())));
        assert!(!item_binding_is_dead(&a_login()));
    }

    /// The card's words for a dead binding: it must say it is ignored, and it
    /// must NOT keep making the promise the trigger caption makes.
    #[test]
    fn a_dead_matchs_notes_say_it_is_ignored_and_drop_the_trigger_promise() {
        let dead = a_dead_host_match();
        assert!(app_match_is_dead(&dead), "the premise: this match really is dead");
        let notes = app_card_notes(&dead);
        assert!(
            notes.iter().any(|n| n.contains("ignoring")),
            "a dead binding's card does not say it is ignored: {notes:?}"
        );
        assert!(
            notes.iter().any(|n| n.contains("Add app")),
            "a dead binding's card does not say how to replace it: {notes:?}"
        );
        for mode in TRIGGER_ORDER {
            assert!(
                !notes.contains(&trigger_caption(mode)),
                "a dead binding still promises {:?}: {notes:?}",
                trigger_caption(mode)
            );
        }
        assert!(
            !app_card_offers_triggers(&dead),
            "a binding that cannot fire still offers three settings for how it fires"
        );
        // The controls, on a LIVE match: the caption is there and the dead
        // notice is not, so neither assertion above is satisfied by a
        // constant.
        let live = app_card_notes(&a_store_match());
        assert!(live.contains(&trigger_caption(TriggerMode::Hotkey)), "{live:?}");
        assert!(!live.iter().any(|n| n.contains("ignoring")), "{live:?}");
        assert!(app_card_offers_triggers(&a_store_match()));
        assert!(app_card_offers_triggers(&a_desktop_match()));
    }

    /// **The wiring.** The reviewer's demonstration was that the pane painted
    /// the process name and the trigger caption in one frame while the engine
    /// answered `None` -- and that no painted string contained "ignor". This
    /// asserts the pane's real output on that same item.
    #[test]
    fn the_pane_tells_the_user_a_dead_binding_is_dead_and_offers_no_trigger_pills() {
        let item = bound_to(&a_login(), &a_dead_host_match());
        let mut pane = Pane::new();
        let frame = pane.idle(&item, &TotpState::NoSecret);

        assert!(
            frame.painted("MATCHED APP"),
            "the card is not on screen at all: {:?}",
            frame.strings()
        );
        assert!(
            frame.strings().iter().any(|t| t.contains("ignoring")),
            "the pane paints a dead binding with nothing saying it is ignored: {:?}",
            frame.strings()
        );
        for mode in TRIGGER_ORDER {
            assert!(
                !frame.painted(trigger_caption(mode)),
                "the pane still promises {:?} on a binding that cannot fire: {:?}",
                trigger_caption(mode),
                frame.strings()
            );
            assert!(
                !frame.painted(trigger_label(mode)),
                "the pane still offers the {mode:?} pill on a binding that cannot fire"
            );
        }
        // Remove stays: clearing the field is the one thing that works.
        assert!(frame.painted("Remove"), "{:?}", frame.strings());
        let remove = frame.rect_of("Remove");
        let clicked = pane.click(&item, &TotpState::NoSecret, remove.center());
        assert_eq!(clicked.action, DetailAction::RemoveAppMatch);

        // The control: the SAME pane on a LIVE match paints all three pills
        // and the selected caption, so the absences above are about this
        // binding and not about a card that stopped drawing anything.
        let live = bound_to(&a_login(), &a_store_match());
        let live_frame = pane.idle(&live, &TotpState::NoSecret);
        for mode in TRIGGER_ORDER {
            assert!(live_frame.painted(trigger_label(mode)), "{:?}", live_frame.strings());
        }
        assert!(live_frame.painted(trigger_caption(TriggerMode::Hotkey)));
        assert!(
            !live_frame.strings().iter().any(|t| t.contains("ignoring")),
            "a live binding is being called ignored: {:?}",
            live_frame.strings()
        );
    }

    // --- review 20's Important 2: a field that will not parse ---

    /// `item` carrying a `deskwarden:app-match` field whose value is `value`
    /// -- built at the field level on purpose, because `with_app_match` can
    /// only produce values that parse and the whole case is a value that does
    /// not. This is the shape a user reaches by editing the custom field in
    /// any other Bitwarden client.
    fn carrying_raw_app_match_field(item: &VaultItem, value: &str) -> VaultItem {
        let mut updated = item.clone();
        updated.fields.push(crate::vault_bridge::VaultField {
            name: Some(crate::app_match::APP_MATCH_FIELD_NAME.to_string()),
            value: Some(value.to_string()),
            other: serde_json::Map::new(),
        });
        updated
    }

    /// The three states the card must tell apart, decided by the PAIR of
    /// answers rather than by `extract_app_match` alone.
    #[test]
    fn a_field_that_will_not_parse_is_not_the_same_as_no_field_at_all() {
        let bound = bound_to(&a_login(), &a_desktop_match());
        let corrupt = carrying_raw_app_match_field(&a_login(), "{not json");
        let unknown_trigger = carrying_raw_app_match_field(
            &a_login(),
            r#"{"process":"Ledgerline.exe","trigger":"telepathy"}"#,
        );
        let unbound = a_login();

        // The premise: both broken shapes really do carry the field and
        // really do fail to parse.
        for broken in [&corrupt, &unknown_trigger] {
            assert!(crate::vault_bridge::has_app_match_field(broken));
            assert!(crate::vault_bridge::extract_app_match(broken).is_none());
            assert_eq!(
                app_card_body(None, crate::vault_bridge::has_app_match_field(broken)),
                AppCardBody::Unreadable
            );
        }
        assert!(!crate::vault_bridge::has_app_match_field(&unbound));
        assert_eq!(app_card_body(None, false), AppCardBody::Unbound);
        let parsed = crate::vault_bridge::extract_app_match(&bound).unwrap();
        assert_eq!(
            app_card_body(Some(&parsed), true),
            AppCardBody::Bound(&parsed)
        );

        // And the card is drawn for a broken field on a kind that offers no
        // fill at all -- the case that used to suppress it entirely and so
        // left the field unclearable from this pane.
        assert!(
            app_card_visible(true, ItemKind::SecureNote),
            "a secure note whose app-match field is corrupt cannot see it"
        );
        assert!(
            !app_card_visible(false, ItemKind::SecureNote),
            "the control: a secure note with NO field still gets no card"
        );
    }

    /// **The wiring, and the fix's whole point:** the field can be removed
    /// from this pane by a sequence of clicks.
    #[test]
    fn a_corrupted_app_match_field_says_what_is_wrong_and_can_be_removed_from_this_pane() {
        // **Both kinds, and the second one is the point.** Every pane-level
        // case here used to be built on a Login, which `kind_offers_fill`
        // makes the card visible for whatever its first argument says -- so
        // substituting `app_card_visible(app_match.is_some(), kind)` for
        // `app_card_visible(app_field_present, kind)` passed the whole suite
        // and restored the reported bug verbatim: a SECURE NOTE whose
        // app-match field was corrupted elsewhere got no card, and the field
        // could not be cleared from this pane by any sequence of clicks. The
        // non-fillable kind was asserted only as a bare unit call, which that
        // substitution leaves untouched.
        let subjects = [
            ("a login", a_login()),
            ("a secure note", an_item(item_type_for(ItemKind::SecureNote))),
        ];
        for (which, subject) in &subjects {
            // The premise: the two fixtures really are different kinds, and
            // the second really is one the pane offers no fill for.
            if *which == "a secure note" {
                assert_eq!(ItemKind::of(subject), ItemKind::SecureNote);
                assert!(!kind_offers_fill(ItemKind::of(subject)));
            }
        for value in ["{not json", r#"{"process":"a.exe","trigger":"telepathy"}"#] {
            let item = carrying_raw_app_match_field(subject, value);
            let mut pane = Pane::new();
            let frame = pane.idle(&item, &TotpState::NoSecret);

            assert!(
                frame.painted("MATCHED APP"),
                "{which} with a corrupt field gets no card at all: {:?}",
                frame.strings()
            );
            assert!(
                frame.strings().iter().any(|t| t.contains("cannot be read")),
                "the pane does not say the field is unreadable for {which} / {value:?}: {:?}",
                frame.strings()
            );
            assert!(
                !frame.strings().iter().any(|t| t.contains("No app is matched")),
                "the pane still claims nothing is bound for {which} / {value:?}: {:?}",
                frame.strings()
            );
            // **Clickable, not merely painted.** `rect_of` panics if it was
            // never drawn, and the click is aimed at the rect it really
            // occupies -- so a Remove pushed off the pane, or drawn under
            // something else, fails here rather than passing on its
            // existence.
            let remove = frame.rect_of("Remove");
            let clicked = pane.click(&item, &TotpState::NoSecret, remove.center());
            assert_eq!(
                clicked.action,
                DetailAction::RemoveAppMatch,
                "Remove on an unreadable field on {which} reported {:?}",
                clicked.action
            );
        }
        }

        // The control: an item with NO field paints the empty notice and
        // offers no Remove, so the assertions above are about the corrupted
        // field and not about a card that now always says both.
        //
        // And its own control, on the kind that the substitution above turned
        // invisible: a secure note with no field gets no card at all, so the
        // secure-note half of the loop cannot be passing on a card that is
        // simply always there.
        let mut pane = Pane::new();
        let bare_note = pane.idle(
            &an_item(item_type_for(ItemKind::SecureNote)),
            &TotpState::NoSecret,
        );
        assert!(
            !bare_note.painted("MATCHED APP"),
            "a secure note bound to nothing draws an app card: {:?}",
            bare_note.strings()
        );
        let mut pane = Pane::new();
        let bare = pane.idle(&a_login(), &TotpState::NoSecret);
        assert!(
            bare.strings().iter().any(|t| t.contains("No app is matched")),
            "{:?}",
            bare.strings()
        );
        assert!(!bare.painted("Remove"), "{:?}", bare.strings());
        assert!(
            !bare.strings().iter().any(|t| t.contains("cannot be read")),
            "{:?}",
            bare.strings()
        );
    }

    /// A corrupted field is cleared by the arm the card reports into --
    /// `without_app_match`, which filters on the field's NAME and so does not
    /// care that the value never parsed.
    #[test]
    fn the_remove_the_card_reports_really_does_clear_an_unparseable_field() {
        let item = carrying_raw_app_match_field(&a_login(), "{not json");
        assert!(crate::vault_bridge::has_app_match_field(&item), "the premise");
        let cleared = crate::vault_bridge::without_app_match(&item);
        assert!(
            !crate::vault_bridge::has_app_match_field(&cleared),
            "Remove leaves the unreadable field in place, so the card's offer is empty"
        );
    }

    /// The card's rows are the pane's ordinary copy rows, and the placeholder
    /// is not -- Task 1's rule, reaching this card.
    #[test]
    fn the_program_file_row_copies_its_path_and_the_placeholder_copies_nothing() {
        let bound = bound_to(&a_login(), &a_desktop_match());
        let mut pane = Pane::new();
        let laid_out = pane.idle(&bound, &TotpState::NoSecret);
        let row = laid_out.rect_of("Program file");
        let clicked = pane.click(&bound, &TotpState::NoSecret, row.center());
        assert_eq!(
            clicked.action,
            DetailAction::CopyValue(r"C:\Apps\Ledgerline\Ledgerline.exe".to_string()),
            "the Program file row reported {:?}",
            clicked.action
        );

        let unrecorded = bound_to(
            &a_login(),
            &AppMatch::for_process("Ledgerline.exe", TriggerMode::Prompt),
        );
        let mut pane = Pane::new();
        let laid_out = pane.idle(&unrecorded, &TotpState::NoSecret);
        assert!(laid_out.painted("Not recorded"), "{:?}", laid_out.strings());
        let row = laid_out.rect_of("Program file");
        let clicked = pane.click(&unrecorded, &TotpState::NoSecret, row.center());
        assert_eq!(
            clicked.action,
            DetailAction::None,
            "the \"Not recorded\" placeholder was copied to the clipboard"
        );
        let hovered = pane.hover(&unrecorded, &TotpState::NoSecret, row.center());
        assert_ne!(
            hovered.cursor,
            egui::CursorIcon::PointingHand,
            "the placeholder row still offers a click"
        );
    }

    // -----------------------------------------------------------------
    // An empty row promises nothing (see `row_offers_copy`).
    // -----------------------------------------------------------------

    /// A login whose username and password are both the empty string -- which
    /// is a real vault item (an item saved with only a URI and a note), and
    /// the one the toast used to lie about.
    fn a_login_with_no_credentials() -> VaultItem {
        let mut item = a_login();
        if let Some(login) = item.login.as_mut() {
            login.username = Some(String::new());
            login.password = Some(String::new().into());
        }
        item
    }

    /// **The two paths, asked the same question.** The chord path has always
    /// refused an empty field; this asserts the click path's predicate gives
    /// the identical answer, against `copy_shortcut_action` itself rather
    /// than against a second copy of its rule.
    #[test]
    fn an_empty_field_is_refused_by_the_click_path_and_the_chord_path_alike() {
        for value in ["", " ", "hunter2"] {
            let chord_takes_username =
                copy_shortcut_action(CopyShortcut::Username, value, "x", &TotpState::NoSecret, "")
                    .is_some();
            let chord_takes_password =
                copy_shortcut_action(CopyShortcut::Password, "x", value, &TotpState::NoSecret, "")
                    .is_some();
            assert_eq!(
                row_offers_copy(value),
                chord_takes_username,
                "the click path and CTRL+U disagree about {value:?}"
            );
            assert_eq!(
                row_offers_copy(value),
                chord_takes_password,
                "the click path and CTRL+B disagree about {value:?}"
            );
            // **TOTP, which this test used to leave out** (review 20's Minor
            // 4). `totp_code_row` passes `row_offers_copy(code)`, so the
            // click path already refused `Code { code: "" }`; the chord
            // gated on the VARIANT alone and never read the code, so CTRL+T
            // copied an empty string and raised "One-time code copied" over
            // it. Asked here through the same live-code state the pane
            // draws, so a variant test cannot satisfy it.
            let live = TotpState::Code { code: value.to_string(), seconds_left: 17 };
            let chord_takes_totp =
                copy_shortcut_action(CopyShortcut::Totp, "x", "x", &live, "").is_some();
            assert_eq!(
                row_offers_copy(value),
                chord_takes_totp,
                "the click path and CTRL+T disagree about {value:?}"
            );
            // The URL binding, for completeness: the fourth chord, and the
            // one whose row already states the rule through the same
            // predicate.
            let chord_takes_url =
                copy_shortcut_action(CopyShortcut::Url, "x", "x", &TotpState::NoSecret, value)
                    .is_some();
            assert_eq!(
                row_offers_copy(value),
                chord_takes_url,
                "the click path and CTRL+SHIFT+U disagree about {value:?}"
            );
        }
        // The control that keeps the loop above from passing against a
        // `row_offers_copy` that is always false AND a `copy_shortcut_action`
        // that is always `None`: the two agree, and they agree on YES for a
        // value and NO for the empty string.
        assert!(row_offers_copy("hunter2"));
        assert!(!row_offers_copy(""));
        // And TOTP's non-`Code` states are still refused whatever the loop
        // above proved -- the gate the variant test was right about, which
        // reading the code must not have thrown away.
        for state in [TotpState::NoSecret, TotpState::Fetching] {
            assert_eq!(
                copy_shortcut_action(CopyShortcut::Totp, "x", "x", &state, ""),
                None,
                "CTRL+T reported a copy in {state:?}"
            );
        }
        assert_eq!(
            copy_shortcut_action(
                CopyShortcut::Totp,
                "x",
                "x",
                &TotpState::Code { code: "123456".to_string(), seconds_left: 9 },
                ""
            ),
            Some(DetailAction::CopyTotp),
            "CTRL+T refuses a code that really is on screen"
        );
    }

    /// The bug, at the pane: a click on an empty Password row reported a copy
    /// and raised "Password copied" over an untouched clipboard.
    #[test]
    fn clicking_an_empty_credential_row_reports_nothing_and_raises_no_toast() {
        for (label, toast) in [("Password", "Password copied"), ("Username", "Username copied")] {
            let empty = a_login_with_no_credentials();
            let mut pane = Pane::new();
            let laid_out = pane.idle(&empty, &TotpState::NoSecret);
            let row = laid_out.rect_of(label);

            let clicked = pane.click(&empty, &TotpState::NoSecret, row.center());
            assert_eq!(
                clicked.action,
                DetailAction::None,
                "clicking the empty {label:?} row reported {:?}",
                clicked.action
            );
            // The frame AFTER the click is where a toast raised by that click
            // would be painted, exactly as the toast's own tests read it.
            let after = pane.idle(&empty, &TotpState::NoSecret);
            assert!(
                !after.painted(toast),
                "the empty {label:?} row confirmed a copy that never happened; the frame \
                 painted: {:?}",
                after.strings()
            );

            // POSITIVE CONTROL, same rect, same gesture, on an item that DOES
            // carry the value: without it a pane that had stopped drawing the
            // row at all -- or a click that missed -- would pass the above.
            let filled = a_login();
            let mut pane = Pane::new();
            let laid_out = pane.idle(&filled, &TotpState::NoSecret);
            let row = laid_out.rect_of(label);
            let clicked = pane.click(&filled, &TotpState::NoSecret, row.center());
            assert_ne!(
                clicked.action,
                DetailAction::None,
                "the filled {label:?} row reported no copy either, so the assertion above \
                 is about a click that hit nothing"
            );
            let after = pane.idle(&filled, &TotpState::NoSecret);
            assert!(
                after.painted(toast),
                "the filled {label:?} row raised no {toast:?} either; painted: {:?}",
                after.strings()
            );
        }
    }

    /// The other two promises: the hover tint and the pointing hand. Both are
    /// made *before* the click, which is why suppressing the toast alone was
    /// not enough.
    #[test]
    fn an_empty_credential_row_offers_no_hover_affordance() {
        for label in ["Password", "Username"] {
            let empty = a_login_with_no_credentials();
            let mut pane = Pane::new();
            let laid_out = pane.idle(&empty, &TotpState::NoSecret);
            let row = laid_out.rect_of(label);
            let hovered = pane.hover(&empty, &TotpState::NoSecret, row.center());
            assert_ne!(
                hovered.cursor,
                egui::CursorIcon::PointingHand,
                "the empty {label:?} row still shows the pointing hand"
            );
            assert!(
                !hovered
                    .rects
                    .iter()
                    .any(|(rect, fill)| *fill == theme::CARD_TINT && rect.contains(row.center())),
                "the empty {label:?} row still takes the hover tint"
            );

            // POSITIVE CONTROL: the same read, on a row that really does copy.
            let filled = a_login();
            let mut pane = Pane::new();
            let laid_out = pane.idle(&filled, &TotpState::NoSecret);
            let row = laid_out.rect_of(label);
            let hovered = pane.hover(&filled, &TotpState::NoSecret, row.center());
            assert_eq!(
                hovered.cursor,
                egui::CursorIcon::PointingHand,
                "the filled {label:?} row shows no hand either"
            );
            assert!(
                hovered
                    .rects
                    .iter()
                    .any(|(rect, fill)| *fill == theme::CARD_TINT && rect.contains(row.center())),
                "the filled {label:?} row takes no tint either"
            );
        }
    }

    /// The third promise: the tooltip. Not covered by the two above -- egui
    /// holds it back half a second, so only a settled hover can see it.
    #[test]
    fn an_empty_credential_row_offers_no_click_to_copy_tooltip() {
        for (label, tooltip) in [
            ("Password", "Click to copy · CTRL+B"),
            ("Username", "Click to copy · CTRL+U"),
        ] {
            let empty = a_login_with_no_credentials();
            let mut pane = Pane::new();
            let laid_out = pane.idle(&empty, &TotpState::NoSecret);
            let row = laid_out.rect_of(label);
            let settled = pane.hover_settled(&empty, &TotpState::NoSecret, row.center());
            assert!(
                !settled.painted(tooltip),
                "the empty {label:?} row still offers {tooltip:?}; painted: {:?}",
                settled.strings()
            );

            // POSITIVE CONTROL: the tooltip really is reachable by this
            // gesture, at this rect, on a row that copies.
            let filled = a_login();
            let mut pane = Pane::new();
            let laid_out = pane.idle(&filled, &TotpState::NoSecret);
            let row = laid_out.rect_of(label);
            let settled = pane.hover_settled(&filled, &TotpState::NoSecret, row.center());
            assert!(
                settled.painted(tooltip),
                "the filled {label:?} row painted no {tooltip:?} either, so the assertion \
                 above is about a gesture that shows nothing; painted: {:?}",
                settled.strings()
            );
        }
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

        // THE FOURTH THING THE DOC ABOVE CLAIMS, which for four commits this
        // body did not do. `rect_of` reads `texts`, which is the galley's
        // SOURCE string -- the exact channel the doc says this avoids -- so
        // the state this test was written for (the title elided to a lone
        // "…" at x = 5.6..27.2) reported the full 26-character name and
        // satisfied every rect assertion above. `header_layout`'s final
        // branch with `stacked: false` restores that state at 298pt;
        // `the_title_is_never_reduced_to_an_ellipsis_at_any_width` fails on
        // it and, until this, nothing here did.
        //
        // Same threshold as that sweep, so the two agree on what "still a
        // name" means: the ellipsis and the spaces do not count, and "…",
        // " …" and "L…" are all the same failure.
        let rendered = frame.rendered_glyphs("Ledgerline Treasury Portal");
        let readable = rendered
            .chars()
            .filter(|c| *c != '\u{2026}' && !c.is_whitespace())
            .count();
        assert!(
            readable >= 6,
            "at the {MIN_PANE}pt minimum pane the header DREW {rendered:?} for a title of \
             {:?} -- the name has been truncated past the point of being a name, and the \
             rect assertions above pass precisely because it collapsed",
            item.name
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
            copy_shortcut_action(CopyShortcut::Password, "u", "p", &code, "w"),
            Some(DetailAction::CopyPassword)
        );
        assert_eq!(
            copy_shortcut_action(CopyShortcut::Username, "u", "p", &code, "w"),
            Some(DetailAction::CopyUsername)
        );
        assert_eq!(
            copy_shortcut_action(CopyShortcut::Totp, "u", "p", &code, "w"),
            Some(DetailAction::CopyTotp)
        );
        assert_eq!(
            copy_shortcut_action(CopyShortcut::Url, "u", "p", &code, "w"),
            Some(DetailAction::CopyValue("w".to_string()))
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
            copy_shortcut_action(CopyShortcut::Username, "", "p", &code, "w"),
            None,
            "CTRL+U on an item with no username put something on the clipboard"
        );
        assert_eq!(
            copy_shortcut_action(CopyShortcut::Password, "u", "", &code, "w"),
            None,
            "CTRL+B on an item with no password put something on the clipboard"
        );
        assert_eq!(
            copy_shortcut_action(CopyShortcut::Url, "u", "p", &code, ""),
            None,
            "CTRL+SHIFT+U on an item with no website put something on the clipboard"
        );
        for empty in [
            TotpState::NoSecret,
            TotpState::Fetching,
            TotpState::Unavailable,
            TotpState::NoCodeReported,
        ] {
            assert_eq!(
                copy_shortcut_action(CopyShortcut::Totp, "u", "p", &empty, "w"),
                None,
                "CTRL+T copied something while the TOTP state was {empty:?} -- there is \
                 no code to copy in it"
            );
        }
    }

    /// The chords say what the code binds, because they are the same table.
    /// A row advertising `CTRL+B` beside a handler wired to something else
    /// is worse than no hint at all.
    #[test]
    fn every_binding_has_a_chord_that_names_its_own_key() {
        for (which, modifiers, key, chord) in COPY_SHORTCUTS {
            assert_eq!(copy_shortcut_chord(which), chord);
            // Spelled from the modifiers the binding really carries, so the
            // one shifted chord in the table cannot be spelled as if it were
            // not -- which is the drift that would put "CTRL+U" in two
            // tooltips for two different copies.
            let mut spelled = "CTRL".to_string();
            assert!(
                modifiers.ctrl && !modifiers.alt && !modifiers.mac_cmd,
                "{which:?} is not a plain-CTRL-based chord: {modifiers:?}"
            );
            if modifiers.shift {
                spelled.push_str("+SHIFT");
            }
            assert_eq!(
                chord,
                format!("{spelled}+{}", key.name()),
                "{which:?}'s chord does not spell the keys it is bound to"
            );
        }
    }

    /// **No two bindings are the same chord.** CTRL+U and CTRL+SHIFT+U are
    /// one key apart from one another and a `matches_exact` apart from being
    /// the same binding; a duplicate would resolve to whichever came first in
    /// the table and the other would silently never fire.
    #[test]
    fn no_two_bindings_share_a_chord() {
        for (i, (which, modifiers, key, _)) in COPY_SHORTCUTS.iter().enumerate() {
            for (other, other_modifiers, other_key, _) in COPY_SHORTCUTS.iter().skip(i + 1) {
                assert!(
                    !(modifiers == other_modifiers && key == other_key),
                    "{which:?} and {other:?} are both bound to {modifiers:?}+{key:?}"
                );
            }
        }
    }

    /// A login with a live one-time code -- the one fixture that has all
    /// three chorded rows on screen at once.
    fn a_login_with_a_code() -> (VaultItem, TotpState) {
        let mut item = a_login();
        item.login.as_mut().expect("a_login has login data").totp =
            Some("seed".to_string().into());
        (
            item,
            TotpState::Code {
                code: "123456".to_string(),
                seconds_left: 9,
            },
        )
    }

    /// **The chords are not painted beside the rows any more.** The user
    /// asked for the `CTRL+B` text on the Password row to go, leaving the
    /// eye as the row's only control, and the same was applied to the
    /// Username and One-time code rows so the card does not carry one row
    /// with a chord beside two without.
    ///
    /// The row labels are the positive control: "no CTRL+B anywhere" is also
    /// true of a pane that failed to draw the LOGIN CREDENTIALS card at all,
    /// and this fixture is the one that puts all three chorded rows on
    /// screen together.
    #[test]
    fn every_chord_is_painted_beside_its_row_and_last_on_the_line() {
        let (item, totp) = a_login_with_a_code();
        let mut pane = Pane::new();
        let frame = pane.idle(&item, &totp);

        for label in ["Username", "Password", "One-time code", "Website"] {
            assert!(
                frame.painted(label),
                "the {label:?} row is not on this pane at all, so finding no chord on \
                 it proves nothing; the pane painted: {:?}",
                frame.strings()
            );
        }
        // EVERY row with a chord paints it, the Password row included.
        for chord in ["CTRL+U", "CTRL+B", "CTRL+T", "CTRL+SHIFT+U"] {
            assert!(
                frame.painted(chord),
                "the {chord} hint is not painted beside its row; the pane painted: {:?}",
                frame.strings()
            );
        }

        // **And the chord is the last thing on the line.** The user asked
        // for the keys to be "always in the end", and for the Password row
        // to read eye-then-chord. The Password row is the one that can get
        // this wrong, because it is the only chord-bearing row that also has
        // a control -- so the eye is what the chord has to clear.
        // The eye is a stroked shape, not text, so it is found through
        // `icon_probe` rather than by string. Positive control first: an
        // empty eye list would make the comparison below vacuous.
        let eyes = &frame.eyes;
        assert!(
            !eyes.is_empty(),
            "no eye was painted at all, so 'the chord clears the eye' proves nothing"
        );
        let chord = frame.rect_of("CTRL+B");
        let eye = eyes
            .iter()
            .min_by(|a, b| a.center().y.total_cmp(&b.center().y))
            .expect("checked non-empty above");
        assert!(
            chord.left() >= eye.right(),
            "the Password row paints CTRL+B at {chord:?}, to the LEFT of the eye at \
             {eye:?} -- the chord is meant to be the last thing on the line"
        );
    }

    /// **And the tooltip carries it instead.** Each of the three rows names
    /// its own chord on hover, and it names the chord that row is bound to --
    /// asserted per row, so a tooltip wired to one fixed string cannot pass.
    #[test]
    fn hovering_a_row_names_the_chord_that_copies_it() {
        let (item, totp) = a_login_with_a_code();
        for (label, chord) in [
            ("Username", "CTRL+U"),
            ("Password", "CTRL+B"),
            ("One-time code", "CTRL+T"),
            ("Website", "CTRL+SHIFT+U"),
        ] {
            let mut pane = Pane::new();
            let laid_out = pane.idle(&item, &totp);
            let row = laid_out.rect_of(label);
            // The tooltip belongs to the row's own background widget, so the
            // pointer goes over the label column -- as far from the eye and
            // its own "Reveal" tooltip as the row goes.
            let hovered = pane.hover_settled(&item, &totp, row.center());
            let want = format!("Click to copy · {chord}");
            assert!(
                hovered.painted(&want),
                "hovering the {label:?} row painted no {want:?} tooltip; the frame \
                 painted: {:?}",
                hovered.strings()
            );
            // No OTHER row's tooltip may be in this frame: one tooltip
            // naming all four chords would satisfy the assertion above.
            //
            // Matched on the whole tooltip sentence, not the bare chord.
            // The chords are painted on their own rows again, so `CTRL+T` is
            // legitimately in this frame whichever row is hovered -- as a
            // row hint, not as this row's tooltip. Searching for the bare
            // chord confused the two and made this test fail on a pane that
            // was behaving correctly.
            for (_, other) in [
                ("Username", "CTRL+U"),
                ("Password", "CTRL+B"),
                ("One-time code", "CTRL+T"),
                ("Website", "CTRL+SHIFT+U"),
            ] {
                if other == chord {
                    continue;
                }
                let other_tooltip = format!("Click to copy · {other}");
                assert!(
                    !hovered.strings().iter().any(|t| t.contains(&other_tooltip)),
                    "hovering the {label:?} row also surfaced {other_tooltip:?}; the frame \
                     painted: {:?}",
                    hovered.strings()
                );
            }
        }
    }

    /// **A tooltip that is already up has to let go when the pointer reaches
    /// a child of the tile.**
    ///
    /// The user: "Click to copy should disapear when hovered over eye or link
    /// - not relevant". Clicking the eye reveals and clicking the link opens
    /// a browser; neither copies. The CLICK was already theirs by layout (see
    /// [`copy_row`]), and the tooltip did not inherit that -- it is a
    /// different mechanism with a different rule.
    ///
    /// **This test arrives at the child the way a pointer does**, and that is
    /// the entire difference between it and
    /// `hovering_the_url_offers_to_open_it_rather_than_to_copy_it`, which
    /// passed throughout: it settles on the tile FIRST, so the tile's tooltip
    /// is up, and only then slides onto the child.
    /// `Tooltip::should_show_tooltip` short-circuits to `true` for a tooltip
    /// that is already open and whose widget rect still contains the pointer
    /// -- before it consults `hovered` at all -- and the tile's rect contains
    /// both children. egui then shows one tooltip per layer, first one wins,
    /// so the child's own was refused on top of that. A test that lands on
    /// the child out of nowhere opens no tile tooltip and cannot see any of
    /// it.
    ///
    /// Both halves, for each child: the child's own tooltip present AND the
    /// tile's gone. Suppressing both would be worse than the bug, and the
    /// negative on its own passes against a pane that painted no tooltip at
    /// all -- which is what the settle on the tile above it controls for.
    #[test]
    fn the_tiles_copy_tooltip_lets_go_when_the_pointer_reaches_the_eye_or_the_link() {
        let item = a_login();
        let totp = TotpState::NoSecret;
        // The tile the pointer starts on, what that tile says, where the
        // child inside it is, and what the child says instead. The eye paints
        // no string of its own, so it is found by geometry; the link is its
        // own URL.
        let children: [(&str, &str, fn(&Frame) -> egui::Rect, &str); 2] = [
            (
                "Password",
                "Click to copy · CTRL+B",
                |frame| {
                    *frame
                        .eyes()
                        .first()
                        .expect("the Password row painted no eye to hover")
                },
                "Reveal",
            ),
            (
                "Website",
                "Click to copy · CTRL+SHIFT+U",
                |frame| frame.rect_of(WEBSITE),
                "Open in browser",
            ),
        ];
        for (tile_label, tile_tooltip, child_of, child_tooltip) in children {
            // A fresh pane per child: the gesture is stateful, and a tooltip
            // left open by the previous one would be this one's premise.
            let mut pane = Pane::new();
            let laid_out = pane.idle(&item, &totp);
            let tile = laid_out.rect_of(tile_label);
            let child = child_of(&laid_out);

            let on_tile = pane.hover_settled(&item, &totp, tile.center());
            assert!(
                on_tile.painted(tile_tooltip),
                "the {tile_label:?} tile painted no {tile_tooltip:?} to begin with, so \
                 it never gives way to anything; the frame painted: {:?}",
                on_tile.strings()
            );

            let on_child = pane.hover_settled(&item, &totp, child.center());
            assert!(
                on_child.painted(child_tooltip),
                "sliding from the {tile_label:?} tile onto its child painted no \
                 {child_tooltip:?}, so the child lost its own tooltip too; the frame \
                 painted: {:?}",
                on_child.strings()
            );
            assert!(
                !on_child
                    .strings()
                    .iter()
                    .any(|t| t.starts_with("Click to copy")),
                "the {tile_label:?} tile's copy tooltip followed the pointer onto its \
                 child, which does not copy; the frame painted: {:?}",
                on_child.strings()
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
        // CTRL+SHIFT+U is deliberately NOT in this list any more: it is the
        // website copy now, and `the_url_chord_and_the_username_chord_are_two
        // _different_copies` is where it belongs. CTRL+ALT+SHIFT+U replaces
        // it here so `U` still has an unbound-modifier case of its own.
        for (name, modifiers, key) in [
            ("CTRL+ALT+B", alt_ctrl, egui::Key::B),
            ("CTRL+SHIFT+B", shift_ctrl, egui::Key::B),
            ("CTRL+ALT+U", alt_ctrl, egui::Key::U),
            ("CTRL+ALT+SHIFT+U", alt_ctrl | egui::Modifiers::SHIFT, egui::Key::U),
            ("SHIFT+U", egui::Modifiers::SHIFT, egui::Key::U),
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
    // The Website row: a link, and a chord of its own.
    // -----------------------------------------------------------------

    /// The URL `a_login` carries, so no test below writes it out twice.
    const WEBSITE: &str = "app.ledgerline.com";

    /// **Clicking the URL opens it, and copies NOTHING.** The link sits
    /// inside a tile that copies on click, exactly as the eye does, and the
    /// two must not both fire. One `DetailAction` is returned per frame, so
    /// asserting it *is* `OpenWebsite` asserts both halves at once -- and is
    /// its own positive control: a click that missed everything reports
    /// `None` and fails here.
    #[test]
    fn clicking_the_website_link_opens_it_without_copying() {
        let item = a_login();
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        let url = laid_out.rect_of(WEBSITE);

        let clicked = pane.click(&item, &TotpState::NoSecret, url.center());
        assert_eq!(
            clicked.action,
            DetailAction::OpenWebsite(WEBSITE.to_string()),
            "clicking the URL reported {:?} -- the link must open the browser, and it \
             must not be the tile's copy that answered",
            clicked.action
        );
    }

    /// And a click anywhere else in the same tile copies it. Over the LABEL
    /// column, which is as far from the link as the row goes -- the other
    /// half of the split, and the one that says the tile did not simply stop
    /// being clickable when the link arrived.
    #[test]
    fn clicking_elsewhere_in_the_website_tile_copies_the_url() {
        let item = a_login();
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        let label = laid_out.rect_of("Website");

        let clicked = pane.click(&item, &TotpState::NoSecret, label.center());
        assert_eq!(
            clicked.action,
            DetailAction::CopyValue(WEBSITE.to_string()),
            "clicking the Website tile reported {:?}, so the tile is not the copy \
             target every other row on this pane is",
            clicked.action
        );
    }

    /// **The link is the control, so the `Open` button is gone.** The row's
    /// label and the URL itself are the positive control: "nothing painted
    /// Open" is also true of a pane that drew no AUTOFILL TARGETS card, and
    /// of one that drew no card at all.
    #[test]
    fn the_website_row_has_no_open_button() {
        let item = a_login();
        let mut pane = Pane::new();
        let frame = pane.idle(&item, &TotpState::NoSecret);

        assert!(
            frame.painted("Website") && frame.painted(WEBSITE),
            "the Website row is not on this pane at all, so finding no Open button \
             proves nothing; the pane painted: {:?}",
            frame.strings()
        );
        assert!(
            !frame.painted("Open"),
            "the Open button is still painted beside the URL that replaced it; the \
             pane painted: {:?}",
            frame.strings()
        );
    }

    /// Every painted run with the colour it was laid out in -- which
    /// [`collect_text_rects`] and [`collect_type`] both throw away, and which
    /// is the whole of "make the URL blue".
    fn painted_colours(item: &VaultItem, totp: &TotpState) -> Vec<(String, egui::Color32)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Color32)>) {
            match shape {
                egui::Shape::Text(text) => {
                    let colour = text.override_text_color.unwrap_or_else(|| {
                        text.galley
                            .job
                            .sections
                            .first()
                            .map(|s| s.format.color)
                            .unwrap_or_default()
                    });
                    out.push((text.galley.text().to_string(), colour));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &frame_shapes(item, totp, RevealState::default()) {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    /// **The URL paints in `BLUE`, not `INK`.** The username, painted in the
    /// same kind of row on the same pane, is the control: it pins that this
    /// walk reads real colours off the galleys rather than reporting one
    /// default for everything, so "the URL is blue" cannot pass by accident.
    #[test]
    fn the_url_is_painted_as_a_link_and_the_values_around_it_are_not() {
        let colours = painted_colours(&a_login(), &TotpState::NoSecret);
        let colour_of = |needle: &str| {
            let hits: Vec<egui::Color32> = colours
                .iter()
                .filter(|(text, _)| text == needle)
                .map(|(_, colour)| *colour)
                .collect();
            assert_eq!(
                hits.len(),
                1,
                "expected exactly one {needle:?} on the pane, found {}; painted: {:?}",
                hits.len(),
                colours
            );
            hits[0]
        };
        assert_eq!(
            colour_of(WEBSITE),
            theme::BLUE,
            "the URL is not painted as a link"
        );
        assert_eq!(
            colour_of("a.novak@ledgerline.com"),
            theme::INK,
            "the username is not INK either, so this test is not reading colours"
        );
    }

    /// **The link wins the hover as well as the click.** It says "Open in
    /// browser"; the tile under it says "Click to copy". Both are wired, and
    /// only the link's own can appear when the pointer is on the link --
    /// which is the same layering the click test relies on, observed through
    /// a second, independent channel.
    #[test]
    fn hovering_the_url_offers_to_open_it_rather_than_to_copy_it() {
        let item = a_login();
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        let url = laid_out.rect_of(WEBSITE);
        let label = laid_out.rect_of("Website");

        let on_link = pane.hover_settled(&item, &TotpState::NoSecret, url.center());
        assert!(
            on_link.painted("Open in browser"),
            "hovering the URL offered no way to open it; the frame painted: {:?}",
            on_link.strings()
        );
        assert!(
            !on_link
                .strings()
                .iter()
                .any(|t| t.starts_with("Click to copy")),
            "hovering the URL ALSO offered the tile's copy, so the two are not \
             layered; the frame painted: {:?}",
            on_link.strings()
        );

        // And the rest of the tile is the copy tooltip -- the control that
        // says the row really does carry one for the link to be winning.
        let mut pane = Pane::new();
        let _ = pane.idle(&item, &TotpState::NoSecret);
        let on_tile = pane.hover_settled(&item, &TotpState::NoSecret, label.center());
        assert!(
            on_tile.painted("Click to copy · CTRL+SHIFT+U"),
            "hovering the Website row's label painted no copy tooltip; the frame \
             painted: {:?}",
            on_tile.strings()
        );
    }

    /// **CTRL+SHIFT+U copies the URL and CTRL+U still copies the username.**
    /// One key, two chords, told apart only by `consume_chord`'s
    /// `matches_exact` -- under egui's own `consume_key` these would be the
    /// same binding and one of them would silently never fire.
    #[test]
    fn the_url_chord_and_the_username_chord_are_two_different_copies() {
        let item = a_login();
        let ctrl_shift = |key| {
            vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            }]
        };

        let mut pane = Pane::new();
        let _ = pane.idle(&item, &TotpState::NoSecret);
        assert_eq!(
            pane.frame(&item, &TotpState::NoSecret, ctrl_shift(egui::Key::U))
                .action,
            DetailAction::CopyValue(WEBSITE.to_string()),
            "CTRL+SHIFT+U did not copy the website"
        );

        let mut pane = Pane::new();
        let _ = pane.idle(&item, &TotpState::NoSecret);
        assert_eq!(
            pane.frame(&item, &TotpState::NoSecret, ctrl(egui::Key::U))
                .action,
            DetailAction::CopyUsername,
            "CTRL+U stopped copying the username when the shifted chord arrived"
        );
    }

    /// **Neither chord fires on an item that has nothing for it, and the
    /// clipboard is UNTOUCHED rather than emptied.** `DetailAction::None` is
    /// the assertion: a `CopyValue("")` would also leave nothing to paste,
    /// and would silently replace whatever the user had copied before.
    ///
    /// Two fixtures, because they fail differently. The login with its URI
    /// stripped still has a username, so it is what says CTRL+SHIFT+U did
    /// not simply fall through to the unshifted binding; the card has
    /// neither field.
    #[test]
    fn a_missing_field_leaves_the_clipboard_alone_for_both_u_chords() {
        let ctrl_shift_u = || {
            vec![egui::Event::Key {
                key: egui::Key::U,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            }]
        };

        let mut no_uri = a_login();
        no_uri
            .login
            .as_mut()
            .expect("a_login has login data")
            .uris
            .clear();
        let mut pane = Pane::new();
        let laid_out = pane.idle(&no_uri, &TotpState::NoSecret);
        assert!(
            !laid_out.painted("Website"),
            "the fixture still has a Website row, so it is not the no-URI case"
        );
        assert_eq!(
            pane.frame(&no_uri, &TotpState::NoSecret, ctrl_shift_u())
                .action,
            DetailAction::None,
            "CTRL+SHIFT+U on an item with no website put something on the clipboard"
        );
        // The positive control on the very same item: its username is still
        // there, so the silence above is about the missing URI and not about
        // a harness whose key events never arrive.
        let mut pane = Pane::new();
        let _ = pane.idle(&no_uri, &TotpState::NoSecret);
        assert_eq!(
            pane.frame(&no_uri, &TotpState::NoSecret, ctrl(egui::Key::U))
                .action,
            DetailAction::CopyUsername,
            "no chord reaches this pane at all, so the silence above proves nothing"
        );

        // A card has neither field, and no AUTOFILL TARGETS card either.
        let card = a_full_card();
        for (name, events) in [("CTRL+SHIFT+U", ctrl_shift_u()), ("CTRL+U", ctrl(egui::Key::U))] {
            let mut pane = Pane::new();
            let _ = pane.idle(&card, &TotpState::NoSecret);
            assert_eq!(
                pane.frame(&card, &TotpState::NoSecret, events).action,
                DetailAction::None,
                "{name} on a card copied something"
            );
        }
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

    /// ------------------------------------------------------------------
    /// The copy confirmation ("Password copied"), the user's request:
    /// "when clicked on password - it gets copied but you don't know what
    /// happened - should be like 5 seconds tooltip".
    /// ------------------------------------------------------------------

    /// **One name per copyable chord, and no two the same.**
    ///
    /// The twin of the `COPY_SHORTCUTS` coverage/collision pair one table
    /// over: the field NAME is now as load-bearing as the chord is, because
    /// the row paints it and the confirmation says it. Two fields sharing a
    /// name would produce a toast that cannot be told apart from another
    /// field's.
    #[test]
    fn every_copy_shortcut_names_one_field_and_no_two_name_the_same_one() {
        let all = [
            CopyShortcut::Username,
            CopyShortcut::Password,
            CopyShortcut::Totp,
            CopyShortcut::Url,
        ];
        for (index, first) in all.iter().enumerate() {
            let name = copy_shortcut_label(*first);
            assert!(
                !name.is_empty(),
                "{first:?} has no field name, so its toast would read \" copied\""
            );
            for second in &all[index + 1..] {
                assert_ne!(
                    name,
                    copy_shortcut_label(*second),
                    "{first:?} and {second:?} are both called {name:?}, so their \
                     confirmations are indistinguishable"
                );
            }
        }
        // Absolute, not re-derived: these are the four words the user reads.
        assert_eq!(copy_shortcut_label(CopyShortcut::Password), "Password");
        assert_eq!(copy_shortcut_label(CopyShortcut::Username), "Username");
        assert_eq!(copy_shortcut_label(CopyShortcut::Totp), "One-time code");
        assert_eq!(copy_shortcut_label(CopyShortcut::Url), "Website");
    }

    /// **The row paints the name the confirmation says.**
    ///
    /// This is the promise `copy_shortcut_label` exists to make, and it is
    /// only a promise if the rows really do read it. A row labelled anything
    /// else beside a toast naming this string is the parallel-table bug this
    /// crate keeps shipping.
    #[test]
    fn each_chord_bound_row_paints_the_name_its_confirmation_uses() {
        let mut item = a_login();
        item.login.as_mut().expect("a_login has login data").totp =
            Some("seed".to_string().into());
        let totp = TotpState::Code {
            code: "123456".to_string(),
            seconds_left: 9,
        };
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &totp);
        for which in [
            CopyShortcut::Username,
            CopyShortcut::Password,
            CopyShortcut::Totp,
            CopyShortcut::Url,
        ] {
            let name = copy_shortcut_label(which);
            assert!(
                laid_out.painted(name),
                "{which:?}'s confirmation would say {name:?}, but no row on the pane \
                 is labelled that; painted: {:?}",
                laid_out.strings()
            );
        }
    }

    /// **The confirmation names the field. It cannot carry the value.**
    ///
    /// This pane's copyable rows are passwords, card numbers, security codes
    /// and private keys, and the confirmation is a banner that sits in the
    /// corner of the window for five seconds -- exactly the surface a
    /// shoulder-surfer reads. `copy_toast_text` takes no value parameter at
    /// all, so the strongest form of this assertion is that the real secrets
    /// of the real fixtures cannot appear in it however it is called.
    #[test]
    fn the_confirmation_names_the_field_and_never_the_copied_value() {
        // The fixtures' actual secrets, spelled out here rather than read off
        // the items, so this test still fails if a fixture changes.
        let secrets = ["hunter2", "4242424242424242", "a.novak@ledgerline.com"];
        for label in ["Password", "Username", "One-time code", "Website", "Number"] {
            let text = copy_toast_text(label);
            assert_eq!(text, format!("{label} copied"));
            for secret in secrets {
                assert!(
                    !text.contains(secret),
                    "the {label:?} confirmation put {secret:?} on screen: {text:?}"
                );
            }
        }
        // The control: the needle-check above is only worth something if
        // `contains` would actually find these strings when they ARE present.
        for secret in secrets {
            assert!(
                format!("prefix {secret} suffix").contains(secret),
                "the secret-leak check cannot detect {secret:?} at all"
            );
        }
        // And the fixture's password really is "hunter2", so the needle above
        // is not a string with no bearing on anything this pane holds.
        assert_eq!(
            a_login()
                .login
                .and_then(|l| l.password)
                .map(|p| p.to_string()),
            Some("hunter2".to_string()),
            "the fixture's password is not the one this test guards against"
        );
    }

    /// **The clipboard's contents never reach the screen through a real
    /// copy either** -- the pure check above, driven end to end.
    #[test]
    fn a_real_copy_puts_the_field_name_on_screen_and_not_the_secret() {
        let item = a_login();
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        let row = laid_out.rect_of("Password");
        let copied = pane.click(&item, &TotpState::NoSecret, row.center());
        assert_eq!(
            copied.action,
            DetailAction::CopyPassword,
            "the click did not copy, so the assertions below are about nothing"
        );
        assert!(copied.painted("Password copied"));
        // The row itself is masked, so "hunter2" is not on the pane at all --
        // which is exactly what the confirmation must not change.
        assert!(
            !contains(
                &copied.strings().iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                "hunter2"
            ),
            "copying the password painted it on screen: {:?}",
            copied.strings()
        );
        // The control: revealing it DOES paint it, so the check above can
        // fail. Without this, a harness that saw no text would also pass.
        pane.reveal.password = true;
        let revealed = pane.idle(&item, &TotpState::NoSecret);
        assert!(
            contains(
                &revealed.strings().iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                "hunter2"
            ),
            "the harness cannot see the password even when the row reveals it, so \
             the negative above proves nothing: {:?}",
            revealed.strings()
        );
    }

    /// **Five seconds, and then nothing** -- the number the user asked for.
    ///
    /// Every time here is an absolute second count, never one computed from
    /// `COPY_TOAST_SECONDS`: a test that derives its expectation from the
    /// constant under test passes at any value of it.
    #[test]
    fn the_confirmation_lasts_five_seconds_and_reports_the_time_left() {
        let toast = CopyToast {
            label: "Password".to_string(),
            shown_at: 100.0,
        };
        assert_eq!(
            copy_toast_now(Some(&toast), 100.0),
            Some(("Password copied".to_string(), 5.0)),
            "the confirmation is not up on the very frame the copy happened"
        );
        let (text, left) = copy_toast_now(Some(&toast), 104.9)
            .expect("the confirmation is gone before five seconds are up");
        assert_eq!(text, "Password copied");
        assert!(
            (left - 0.1).abs() < 1e-6,
            "0.1s before the deadline the confirmation reported {left}s left, so a \
             repaint scheduled from it would come at the wrong time"
        );
        assert_eq!(
            copy_toast_now(Some(&toast), 105.0),
            None,
            "the confirmation outlived its five seconds"
        );
        assert_eq!(copy_toast_now(Some(&toast), 900.0), None);
        assert_eq!(
            copy_toast_now(None, 100.0),
            None,
            "a pane that has copied nothing is showing a confirmation"
        );
    }

    /// **A second copy replaces the first and restarts the clock** -- it does
    /// not queue behind it, and two do not stack.
    #[test]
    fn a_second_copy_replaces_the_message_and_restarts_the_five_seconds() {
        let first = CopyToast {
            label: "Password".to_string(),
            shown_at: 100.0,
        };
        // Four seconds in, one second left.
        let (text, left) = copy_toast_now(Some(&first), 104.0).expect("still up at 4s");
        assert_eq!(text, "Password copied");
        assert!((left - 1.0).abs() < 1e-6, "reported {left}s left at 4s in");

        // The username is copied at that moment. One value, overwritten
        // whole, so the label and the deadline move together.
        let second = CopyToast {
            label: "Username".to_string(),
            shown_at: 104.0,
        };
        assert_eq!(
            copy_toast_now(Some(&second), 104.0),
            Some(("Username copied".to_string(), 5.0)),
            "the second copy did not take over the confirmation"
        );
        // And the first one's original deadline no longer retires anything:
        // at 105.5 -- past the FIRST five seconds -- the second is still up.
        assert_eq!(
            copy_toast_now(Some(&second), 105.5),
            Some(("Username copied".to_string(), 3.5)),
            "the second copy inherited the first one's deadline instead of its own"
        );
    }

    /// The same replacement, through the pane: two copies in a row leave ONE
    /// message on screen, the second one's.
    #[test]
    fn a_second_copy_through_the_pane_leaves_only_the_newer_message() {
        let item = a_login();
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        let password = laid_out.rect_of("Password");
        let username = laid_out.rect_of("Username");
        let first = pane.click(&item, &TotpState::NoSecret, password.center());
        assert!(first.painted("Password copied"), "the first copy said nothing");
        let second = pane.click(&item, &TotpState::NoSecret, username.center());
        assert!(
            second.painted("Username copied"),
            "the second copy did not take over: {:?}",
            second.strings()
        );
        assert!(
            !second.painted("Password copied"),
            "both confirmations are on screen at once: {:?}",
            second.strings()
        );
    }

    /// **The click path is wired.** The decision above is pure; this is the
    /// other half, driven with a real click on a real row.
    ///
    /// Deliberately separate from the chord test below: they are two call
    /// sites, and one of them passing proves nothing about the other.
    #[test]
    fn clicking_a_row_confirms_the_copy_by_name() {
        for (row, want) in [
            ("Password", "Password copied"),
            ("Username", "Username copied"),
            ("Website", "Website copied"),
        ] {
            let item = a_login();
            let mut pane = Pane::new();
            let laid_out = pane.idle(&item, &TotpState::NoSecret);
            assert!(
                !laid_out.painted(want),
                "the pane showed {want:?} before anything was copied"
            );
            let target = laid_out.rect_of(row);
            let clicked = pane.click(&item, &TotpState::NoSecret, target.center());
            assert!(
                !matches!(clicked.action, DetailAction::None),
                "the click on the {row:?} row copied nothing, so the confirmation \
                 below is about a click that hit nothing"
            );
            assert!(
                clicked.painted(want),
                "clicking the {row:?} row copied silently -- the frame painted: {:?}",
                clicked.strings()
            );
        }
    }

    /// **A keyboard chord confirms exactly as a click does**, and this is the
    /// case that needs it most: there is no row under the pointer to have
    /// reacted, so without this the user has no evidence at all.
    ///
    /// Driven with key events and NO pointer anywhere on the pane -- which is
    /// also why an `on_hover_text` tooltip could not have been the surface.
    #[test]
    fn a_chord_confirms_the_copy_exactly_as_a_click_does() {
        let mut item = a_login();
        item.login.as_mut().expect("a_login has login data").totp =
            Some("seed".to_string().into());
        let totp = TotpState::Code {
            code: "123456".to_string(),
            seconds_left: 9,
        };
        let shift_ctrl = egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT);
        for (modifiers, key, want) in [
            (egui::Modifiers::CTRL, egui::Key::B, "Password copied"),
            (egui::Modifiers::CTRL, egui::Key::U, "Username copied"),
            (egui::Modifiers::CTRL, egui::Key::T, "One-time code copied"),
            (shift_ctrl, egui::Key::U, "Website copied"),
        ] {
            let mut pane = Pane::new();
            let idle = pane.idle(&item, &totp);
            assert!(
                !idle.painted(want),
                "the pane showed {want:?} with no chord pressed"
            );
            let pressed = pane.frame(
                &item,
                &totp,
                vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
            );
            assert!(
                !matches!(pressed.action, DetailAction::None),
                "the chord for {want:?} copied nothing, so the assertion below is \
                 about a chord that did not fire"
            );
            assert!(
                pressed.painted(want),
                "a chord copied silently; expected {want:?}, the frame painted: {:?}",
                pressed.strings()
            );
        }
    }

    /// **The confirmation retires itself.** egui only redraws on input, so a
    /// toast with a deadline that nobody schedules stays on screen until the
    /// user happens to move the mouse. This is the single most likely bug in
    /// the feature, so it is pinned twice: the frame asks for a repaint, and
    /// the message really is gone five seconds later.
    ///
    /// The repaint half asserts the request SHRINKS as the deadline nears --
    /// a constant delay (or egui's own idle default) cannot do that, and a
    /// bare "is it under five seconds" could be satisfied by an unrelated
    /// animation.
    #[test]
    fn the_confirmation_schedules_the_repaint_that_retires_it_and_then_goes_away() {
        let item = a_login();
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        let row = laid_out.rect_of("Password");
        let copied = pane.click(&item, &TotpState::NoSecret, row.center());
        assert!(copied.painted("Password copied"), "nothing was copied");
        // The frame the click landed on asks for an immediate repaint for
        // reasons of its own -- egui always does after a pointer event -- so
        // the countdown is read off the quiet frames that follow. egui
        // advances its own clock by one predicted frame (1/60s) per
        // input-less pass, the same trick `hover_settled` uses.
        //
        // 121 frames is 2.02s, so a five-second deadline started at the copy
        // has about 2.98s left. Every number below is ABSOLUTE: none is
        // computed from `COPY_TOAST_SECONDS`, and no constant delay and no
        // unrelated animation can land in this window AND in the next one a
        // second later.
        for _ in 0..120 {
            let _ = pane.idle(&item, &TotpState::NoSecret);
        }
        let two_seconds_in = pane.idle(&item, &TotpState::NoSecret);
        assert!(
            two_seconds_in.painted("Password copied"),
            "the confirmation was gone two seconds after the copy; the frame painted: {:?}",
            two_seconds_in.strings()
        );
        let left = two_seconds_in.repaint_delay.as_secs_f64();
        assert!(
            (2.7..3.2).contains(&left),
            "two seconds after the copy the frame asked egui back in {left}s; a \
             five-second deadline started at the copy has about 2.98s left"
        );

        // One more second of frames, and the request must have shrunk by
        // about exactly that second.
        for _ in 0..60 {
            let _ = pane.idle(&item, &TotpState::NoSecret);
        }
        let three_seconds_in = pane.idle(&item, &TotpState::NoSecret);
        let left = three_seconds_in.repaint_delay.as_secs_f64();
        assert!(
            (1.7..2.2).contains(&left),
            "a second later the frame asked egui back in {left}s, so the request is \
             not counting the confirmation down"
        );

        // Past five seconds. 200 more frames is 3.33s, taking the total to
        // 6.35s -- past the deadline without being computed from it.
        for _ in 0..200 {
            let _ = pane.idle(&item, &TotpState::NoSecret);
        }
        let expired = pane.idle(&item, &TotpState::NoSecret);
        assert!(
            !expired.painted("Password copied"),
            "the confirmation was still up more than five seconds after the copy: {:?}",
            expired.strings()
        );
    }

    /// **Where it sits: the window's bottom-right, clear of everything that
    /// matters.**
    ///
    /// It must not cover the row that was just clicked (the rows lay out from
    /// the top of the body down; this is anchored to the opposite corner), it
    /// must not sit under the pane's own controls (Edit, Fill, the kebab and
    /// the star are all in the header strip at the top), and it must still
    /// read at the smallest window the app can be resized to.
    ///
    /// Every number is absolute against the pane, never re-derived from
    /// `COPY_TOAST_INSET`.
    #[test]
    fn the_confirmation_sits_in_the_bottom_right_clear_of_the_row_and_the_header() {
        let item = a_login();
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        let header = laid_out.header_strip();
        let row = laid_out.rect_of("Password");
        let _ = pane.click(&item, &TotpState::NoSecret, row.center());
        // One more frame, on a QUIET pane. Not because the placement needs
        // to settle -- it does not, and that is the point: `draw_copy_toast`
        // measures the galley itself and paints through a layer painter
        // precisely so the toast is in its final position on the very frame
        // the copy happened. The `Area` that had to be laid out before it
        // knew its own size is the design this feature REJECTED, for that
        // exact reason. This frame is read instead of the click's so the
        // geometry is not taken off a frame that also carries a pointer
        // press, a hover tint and the row's own click response.
        let settled = pane.idle(&item, &TotpState::NoSecret);
        let toast = settled.rect_of("Password copied");
        assert_eq!(
            settled.rendered_glyphs("Password copied"),
            "Password copied",
            "the confirmation was elided; `Galley::text` would not have shown it"
        );

        assert!(
            toast.right() > 450.0 && toast.bottom() > 450.0,
            "the confirmation is not in the pane's bottom-right quadrant: {toast:?}"
        );
        assert!(
            toast.right() <= 900.0 && toast.bottom() <= 900.0,
            "the confirmation runs off the bottom-right of a 900x900 pane: {toast:?}"
        );

        // **The BOX, to the point, not the quadrant.** Everything above is
        // satisfied by a toast placed anywhere in a 450pt square, which is
        // how a 24pt/18pt shift (drawing it against the padded body instead
        // of the whole pane) passed the whole suite. The dark box is also the
        // thing the user sees meet the window edge -- the galley sits a
        // padding in from it, so the text's rect can never say where the
        // toast IS. 880 is 900 less the documented 20pt inset, written
        // absolutely rather than derived from `COPY_TOAST_INSET`.
        let box_rect = settled.filled_box_around(toast, theme::INK);
        assert!(
            (box_rect.right() - 880.0).abs() < 0.5,
            "the confirmation's box is at {box_rect:?}: its right edge is not 20pt \
             in from a 900pt pane"
        );
        assert!(
            (box_rect.bottom() - 880.0).abs() < 0.5,
            "the confirmation's box is at {box_rect:?}: its bottom edge is not 20pt \
             up from a 900pt pane"
        );
        assert!(
            !toast.intersects(row),
            "the confirmation covers the very row that was clicked: {toast:?} over {row:?}"
        );
        assert!(
            toast.top() > header.bottom(),
            "the confirmation sits over the header strip, where the pane's own \
             controls are: {toast:?} over {header:?}"
        );

        // The smallest the window can get. Same message, still whole, still
        // inside the pane.
        let mut narrow = Pane::wide(MIN_PANE);
        let narrow_laid_out = narrow.idle(&item, &TotpState::NoSecret);
        let narrow_row = narrow_laid_out.rect_of("Password");
        let _ = narrow.click(&item, &TotpState::NoSecret, narrow_row.center());
        let narrow_settled = narrow.idle(&item, &TotpState::NoSecret);
        assert_eq!(
            narrow_settled.rendered_glyphs("Password copied"),
            "Password copied",
            "at the minimum window size the confirmation was elided"
        );
        let narrow_toast = narrow_settled.rect_of("Password copied");
        assert!(
            narrow_toast.left() >= 0.0 && narrow_toast.right() <= MIN_PANE,
            "at the minimum window size the confirmation ({narrow_toast:?}) runs \
             outside the {MIN_PANE}pt pane"
        );
    }

    /// **A chord that copies nothing says nothing.** The one thing standing
    /// between the user and "One-time code copied" on an item with no TOTP.
    ///
    /// Live behaviour is already correct, and that is the problem this pins:
    /// hoisting `note_copied` OUT of the `if let Some(copy)` guard -- so
    /// every chord confirms whether or not it copied -- passed the entire
    /// suite. The same guard has already failed one variant over: CTRL+T on
    /// `Code { code: "" }` copied an empty string and confirmed it (fixed in
    /// `8db47a0`), which is what a silent-path assertion would have caught
    /// and no positive-path test could.
    ///
    /// Every chord, and every state that has nothing for it: an item with a
    /// login object whose fields are all empty, and one with no login object
    /// at all -- crossed with each TOTP state that carries no code, the
    /// empty-string `Code` included.
    #[test]
    fn a_chord_with_nothing_to_copy_raises_no_confirmation() {
        let shift_ctrl = egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT);
        let chords = [
            (egui::Modifiers::CTRL, egui::Key::U, "Username copied"),
            (egui::Modifiers::CTRL, egui::Key::B, "Password copied"),
            (egui::Modifiers::CTRL, egui::Key::T, "One-time code copied"),
            (shift_ctrl, egui::Key::U, "Website copied"),
        ];

        // **The positive control, and it is not the negative one rephrased.**
        // "No confirmation appeared" is also true of a harness that cannot
        // deliver a key event, of a pane that draws no toast at all, and of
        // four chord literals that match nothing painted. This drives the
        // SAME four chords through the SAME harness on an item that has all
        // four fields, and requires each to produce its own message.
        let mut stocked = a_login();
        stocked.login.as_mut().expect("a_login has login data").totp =
            Some("seed".to_string().into());
        let live = TotpState::Code {
            code: "123456".to_string(),
            seconds_left: 21,
        };
        for (modifiers, key, want) in chords {
            let mut pane = Pane::new();
            let _ = pane.idle(&stocked, &live);
            let pressed = pane.frame(
                &stocked,
                &live,
                vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
            );
            assert!(
                pressed.painted(want),
                "the control failed: this chord does not raise {want:?} even on an \
                 item that has the field, so finding it absent below proves nothing \
                 -- the frame painted: {:?}",
                pressed.strings()
            );
        }

        // A login object with every field empty, and an item with no login
        // object whatsoever (a secure note). Neither has a URI, so
        // CTRL+SHIFT+U has nothing either.
        let mut bare_login = a_login();
        {
            let login = bare_login.login.as_mut().expect("a_login has login data");
            login.username = Some(String::new());
            login.password = Some(String::new().into());
            login.totp = None;
            login.uris = Vec::new();
        }
        let note = an_item(Some(2));

        for item in [&bare_login, &note] {
            for totp in [
                TotpState::NoSecret,
                TotpState::Fetching,
                TotpState::Unavailable,
                TotpState::NoCodeReported,
                // The drift `8db47a0` fixed: a live-code variant carrying no
                // code. The chord must read the code, not the variant.
                TotpState::Code {
                    code: String::new(),
                    seconds_left: 9,
                },
            ] {
                for (modifiers, key, message) in chords {
                    let mut pane = Pane::new();
                    let _ = pane.idle(item, &totp);
                    let pressed = pane.frame(
                        item,
                        &totp,
                        vec![egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers,
                        }],
                    );
                    // The copy really did not happen -- so the silence below
                    // is a chord that confirmed nothing, not a chord that
                    // copied and confirmed correctly.
                    assert!(
                        matches!(pressed.action, DetailAction::None),
                        "{message:?}: the chord reported {:?} on an item with nothing \
                         to copy (TOTP {totp:?})",
                        pressed.action
                    );
                    assert!(
                        !pressed.painted(message),
                        "{message:?} was shown for a chord that copied NOTHING \
                         (TOTP {totp:?}); the frame painted: {:?}",
                        pressed.strings()
                    );
                    // And no OTHER confirmation either: a chord that named the
                    // wrong field would slip past the assertion above.
                    let claims: Vec<&str> = pressed
                        .strings()
                        .into_iter()
                        .filter(|painted| painted.ends_with(" copied"))
                        .collect();
                    assert!(
                        claims.is_empty(),
                        "a chord that copied nothing claimed {claims:?} (TOTP {totp:?})"
                    );
                }
            }
        }
    }

    /// **The confirmation belongs to the item it was copied on.**
    ///
    /// The toast lives in the context, which is shared by every item the pane
    /// ever draws: copy the password on a login, click any other item inside
    /// the five seconds, and the new item painted "Password copied" -- on a
    /// secure note, which has no Password row at all. That is this feature's
    /// own claim (the toast names the row it belongs to) failing the moment
    /// the selection changes.
    ///
    /// Three things in one test, because two of them are each other's
    /// control:
    ///   * a redraw of the SAME item keeps it -- a vault refresh or a write
    ///     landing is not the user walking away, and a "clear on every frame"
    ///     fix would pass the other two assertions while making the toast
    ///     invisible in the app;
    ///   * a different item does not get it;
    ///   * and coming BACK does not resurrect it, which is what separates
    ///     clearing the value from merely hiding it.
    #[test]
    fn the_confirmation_does_not_follow_the_pane_onto_another_item() {
        let item = a_login();
        let mut other = an_item(Some(2));
        other.id = "id-2".to_string();
        other.name = "Other".to_string();
        assert_ne!(item.id, other.id, "the two fixtures are the same item");

        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        let row = laid_out.rect_of("Password");
        let copied = pane.click(&item, &TotpState::NoSecret, row.center());
        assert!(
            copied.painted("Password copied"),
            "nothing was confirmed on the item that was copied, so the assertions \
             below are about a toast that never existed -- painted: {:?}",
            copied.strings()
        );

        // Redrawn for a reason that is not a selection change.
        let refreshed = pane.idle(&item, &TotpState::NoSecret);
        assert!(
            refreshed.painted("Password copied"),
            "a redraw of the SAME item lost the confirmation -- a vault refresh or a \
             write landing must not cancel it; painted: {:?}",
            refreshed.strings()
        );

        // The user clicks another item, well inside the five seconds.
        let switched = pane.idle(&other, &TotpState::NoSecret);
        assert!(
            !switched.painted("Password copied"),
            "the confirmation followed the pane onto another item, which has no \
             Password row at all; it painted: {:?}",
            switched.strings()
        );
        let claims: Vec<&str> = switched
            .strings()
            .into_iter()
            .filter(|painted| painted.ends_with(" copied"))
            .collect();
        assert!(
            claims.is_empty(),
            "the newly selected item claimed {claims:?} about a copy made on a \
             different item"
        );

        // And back again, still inside the five seconds: nothing returns.
        let back = pane.idle(&item, &TotpState::NoSecret);
        assert!(
            !back.painted("Password copied"),
            "the confirmation came back when the item was reselected -- it was \
             hidden rather than cleared; painted: {:?}",
            back.strings()
        );
    }

    // -----------------------------------------------------------------
    // Open: what the card offers, and exactly what would run.
    // -----------------------------------------------------------------

    /// The user's own case, spelled the way they spelled it: two vault items
    /// naming one `chrome.exe` at one path, told apart only by which profile
    /// it is started with.
    fn a_browser_match() -> AppMatch {
        AppMatch {
            process: "chrome.exe".to_string(),
            title: String::new(),
            hosted: false,
            path: r"C:\Program Files\Google\Chrome\Application\chrome.exe".to_string(),
            args: r#"--profile-directory="Profile 2""#.to_string(),
            trigger: TriggerMode::Prompt,
        }
    }

    /// A login whose first URI really is an `http(s)` URL. `a_login`'s is
    /// `app.ledgerline.com` -- schemeless, which `is_safe_web_url` refuses --
    /// so it could never have produced a website choice, and a test written
    /// against it would have passed for the wrong reason.
    fn a_login_on_the_web() -> VaultItem {
        let mut item = a_login();
        item.login.as_mut().unwrap().uris = vec![crate::vault_bridge::UriEntry {
            uri: Some("https://portal.ledgerline.com/sign-in".to_string()),
            other: serde_json::Map::new(),
        }];
        item
    }

    const WEB: &str = "https://portal.ledgerline.com/sign-in";

    /// **The oracle for [`quote_arg`], and it is not this crate.**
    ///
    /// Every other assertion about quoting in this file would be this
    /// function's own rules restated -- a control that re-derives its
    /// expectation from the thing under test, which this crate has shipped
    /// twice. So the expectation comes from Windows itself: build a command
    /// line the way [`command_line`] does, hand it to `CommandLineToArgvW`
    /// (the parser `std`, the CRT and every browser's own splitter are
    /// modelled on), and read back what the program would actually receive.
    ///
    /// No process is started. `CommandLineToArgvW` is a pure string function.
    fn argv_of(command_line: &str) -> Vec<String> {
        let wide: Vec<u16> = command_line
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut count = 0i32;
        // Safety: `wide` is a NUL-terminated UTF-16 buffer that outlives the
        // call; `count` is a live `i32`. The returned block is read for
        // `count` pointers and then freed with `LocalFree`, which is what
        // the API documents as its deallocator.
        unsafe {
            let argv = windows::Win32::UI::Shell::CommandLineToArgvW(
                windows::core::PCWSTR(wide.as_ptr()),
                &mut count,
            );
            assert!(!argv.is_null(), "CommandLineToArgvW refused a command line");
            let out = (0..count)
                .map(|i| (*argv.offset(i as isize)).to_string().unwrap())
                .collect();
            let _ = windows::Win32::Foundation::LocalFree(
                windows::Win32::Foundation::HLOCAL(argv as *mut core::ffi::c_void),
            );
            out
        }
    }

    /// [`quote_arg`] round-trips through Windows' own parser, for the shapes
    /// that break naive quoting.
    #[test]
    fn a_quoted_argument_comes_back_out_of_windows_own_parser_unchanged() {
        let awkward = [
            "https://portal.ledgerline.com/sign-in",
            "https://x.test/a b",
            "",
            "   ",
            "a\tb",
            r#"say "hi""#,
            r"C:\dir\",
            r"C:\dir with space\",
            r#"back\"slash"#,
            r"\\\\",
            r#"\\\""#,
            "--profile-directory=Profile 2",
        ];
        for arg in awkward {
            let line = format!("prog.exe {}", quote_arg(arg));
            assert_eq!(
                argv_of(&line),
                vec!["prog.exe".to_string(), arg.to_string()],
                "quote_arg({arg:?}) produced {line:?}, which Windows reads back as \
                 something else"
            );
        }
        // The control: the quoting is doing work. Passing these through RAW
        // does not round-trip, so the assertions above are not satisfied by
        // an identity function.
        for arg in [r#"say "hi""#, "https://x.test/a b"] {
            assert_ne!(
                argv_of(&format!("prog.exe {arg}")),
                vec!["prog.exe".to_string(), arg.to_string()],
                "control: {arg:?} round-trips unquoted, so it proves nothing about quoting"
            );
        }
    }

    /// An argument with nothing awkward in it is returned untouched -- so the
    /// command line in the tooltip is one a user recognises.
    #[test]
    fn a_plain_argument_is_not_wrapped_in_quotes_it_did_not_need() {
        assert_eq!(quote_arg("https://x.test/a"), "https://x.test/a");
        assert_eq!(quote_arg("--headless"), "--headless");
        // And the two that must be quoted, spelled out rather than merely
        // round-tripped, so a change of strategy is visible in a diff.
        assert_eq!(quote_arg(""), r#""""#);
        assert_eq!(quote_arg("a b"), r#""a b""#);
    }

    /// **The whole point of the feature, as one assertion.**
    ///
    /// `--profile-directory="Profile 2"` is passed through byte for byte, and
    /// Windows' own parser reads the flag Chrome expects back out of the line
    /// that would be run.
    #[test]
    fn the_profile_flag_reaches_the_browser_exactly_as_the_user_typed_it() {
        let m = a_browser_match();
        let plan = app_launch_plan(&m, "").expect("the browser match is launchable");
        assert_eq!(
            plan.raw_tail, r#"--profile-directory="Profile 2""#,
            "the stored arguments were re-quoted, split or trimmed on the way to the \
             command line"
        );
        assert_eq!(
            argv_of(&command_line(&plan)),
            vec![
                r"C:\Program Files\Google\Chrome\Application\chrome.exe".to_string(),
                "--profile-directory=Profile 2".to_string(),
            ],
            "the command line Deskwarden would run does not deliver the profile flag"
        );
        // With the website too: the URL is a SECOND argument after the flag,
        // not folded into it.
        let with_web = app_launch_plan(&m, WEB).expect("still launchable");
        assert_eq!(
            argv_of(&command_line(&with_web)),
            vec![
                r"C:\Program Files\Google\Chrome\Application\chrome.exe".to_string(),
                "--profile-directory=Profile 2".to_string(),
                WEB.to_string(),
            ],
            "the item's URL did not arrive as its own trailing argument"
        );
    }

    #[test]
    fn the_command_line_tail_is_the_arguments_verbatim_then_the_url() {
        assert_eq!(launch_tail("", ""), "");
        assert_eq!(launch_tail("   \t  ", ""), "", "whitespace-only arguments are no arguments");
        assert_eq!(launch_tail("-a   -b", ""), "-a   -b", "a run of spaces INSIDE the arguments is the user's");
        assert_eq!(launch_tail("  -a  ", ""), "-a");
        assert_eq!(launch_tail("", WEB), WEB, "a bare URL needs no quoting");
        assert_eq!(launch_tail("", "https://x.test/a b"), r#""https://x.test/a b""#);
        assert_eq!(launch_tail("-a", WEB), format!("-a {WEB}"));
        assert_eq!(
            launch_tail(r#"--k="v w""#, WEB),
            format!(r#"--k="v w" {WEB}"#),
            "the URL must come AFTER the arguments"
        );
        // Unbalanced quotes in the stored arguments are the user's string and
        // are passed on unchanged -- this crate does not repair them, and
        // must not silently drop them either.
        assert_eq!(launch_tail(r#"--k="v"#, ""), r#"--k="v"#);
    }

    #[test]
    fn the_command_line_quotes_a_program_path_with_a_space_in_it() {
        let plan = LaunchPlan {
            program: r"C:\Program Files\App\App.exe".to_string(),
            raw_tail: String::new(),
        };
        assert_eq!(command_line(&plan), r#""C:\Program Files\App\App.exe""#);
        assert_eq!(argv_of(&command_line(&plan)), vec![plan.program.clone()]);
        // No tail means no trailing space.
        assert!(!command_line(&plan).ends_with(' '));
    }

    /// **The gate.** Nothing `AppMatch::launchable_path` refuses ever becomes
    /// a plan, and the plan's `program` is that function's return value rather
    /// than the field it was derived from.
    #[test]
    fn no_plan_is_ever_built_from_a_path_the_launch_check_refuses() {
        let refused = [
            (r"C:\Apps\..\Windows\System32\chrome.exe", "a .. component"),
            (r"chrome.exe", "a relative path"),
            (r"\\host\share\chrome.exe", "a UNC path"),
            (r"\\?\C:\Apps\chrome.exe", "a device path"),
            (r"C:/Apps/chrome.exe", "forward slashes"),
            (r"C:\Apps\evil.exe:s\chrome.exe", "an alternate data stream"),
            (r"C:\Apps\notchrome.exe", "a different file name"),
            (r"C:\Apps\chrome.exe.", "a trailing dot"),
            ("", "nothing recorded"),
        ];
        for (path, why) in refused {
            let mut m = a_browser_match();
            m.path = path.to_string();
            assert!(
                m.launchable_path().is_none(),
                "the premise failed: launchable_path ACCEPTS {path:?} ({why})"
            );
            assert!(
                app_launch_plan(&m, WEB).is_none(),
                "a plan was built from {path:?}, which the launch check refuses ({why})"
            );
            assert!(
                app_open_choices(&m, WEB).is_empty(),
                "Open was offered for {path:?} ({why})"
            );
        }
        // The control: the same match with an accepted path DOES get a plan,
        // and the plan carries the checked value.
        let ok = a_browser_match();
        let plan = app_launch_plan(&ok, WEB).expect("control: this one is launchable");
        assert_eq!(plan.program, ok.launchable_path().unwrap());
    }

    /// A Microsoft Store app is not started by running its image, and a dead
    /// binding is not started at all.
    #[test]
    fn a_store_app_and_a_dead_binding_are_never_launched() {
        let store = a_store_match();
        assert!(
            store.launchable_path().is_some(),
            "the premise: this path passes the structural check, so the refusal below is \
             about being a Store app and not about the path"
        );
        assert!(app_launch_plan(&store, WEB).is_none(), "a Store app was offered as a program to run");

        let dead = AppMatch::for_process("ApplicationFrameHost.exe", TriggerMode::Prompt);
        assert!(app_match_is_dead(&dead), "the premise: this binding really is dead");
        assert!(app_launch_plan(&dead, WEB).is_none());
    }

    /// [`app_open_refusal`] says something exactly when there is nothing to
    /// click -- and says nothing when there is.
    #[test]
    fn a_refusal_is_shown_exactly_when_there_is_no_plan() {
        let mut no_path = a_browser_match();
        no_path.path = String::new();
        let mut bad_path = a_browser_match();
        bad_path.path = r"C:\Apps\..\chrome.exe".to_string();
        let dead = AppMatch::for_process("ApplicationFrameHost.exe", TriggerMode::Prompt);

        let cases = [
            (a_browser_match(), false),
            (a_desktop_match(), false),
            (a_store_match(), true),
            (no_path, true),
            (bad_path, true),
            // Dead: refused, but the DEAD notice is what says so, and a
            // second sentence would be noise.
            (dead, false),
        ];
        for (m, expects_refusal) in cases {
            assert_eq!(
                app_open_refusal(&m).is_some(),
                expects_refusal,
                "the refusal note is wrong for {:?}",
                m.process
            );
            if !app_match_is_dead(&m) {
                assert_eq!(
                    app_open_refusal(&m).is_some(),
                    app_launch_plan(&m, "").is_none(),
                    "a live binding either offers Open or explains why not, and this one \
                     does neither or both: {m:?}"
                );
            }
            // Whatever the card says, a refused match never gets a control.
            if app_open_refusal(&m).is_some() {
                assert!(app_open_choices(&m, WEB).is_empty());
            }
        }
    }

    /// *Which* refusal, not merely that there is one -- including the case
    /// where two reasons are both true at once.
    ///
    /// **[`a_refusal_is_shown_exactly_when_there_is_no_plan`] only asks
    /// `is_some()`**, so every arm of [`app_open_refusal`] could return every
    /// other arm's sentence and it would still pass. The ordering of those
    /// arms is a decision: a Store app with no recorded path is refused for
    /// BOTH reasons, and telling that user to pick it again with "Add app..."
    /// sends them round a loop that cannot help, because re-picking a Store
    /// app produces another hosted match that still gets no Open. Being hosted
    /// is the reason that stays true no matter what the user does, so it is
    /// the one that must be said.
    #[test]
    fn each_refusal_gives_its_own_reason_and_hosted_outranks_a_missing_path() {
        let mut no_path = a_browser_match();
        no_path.path = String::new();
        let mut bad_path = a_browser_match();
        bad_path.path = r"C:\Apps\..\chrome.exe".to_string();

        assert_eq!(app_open_refusal(&a_store_match()), Some(APP_OPEN_HOSTED_NOTE));
        assert_eq!(app_open_refusal(&no_path), Some(APP_OPEN_NO_PATH_NOTE));
        assert_eq!(app_open_refusal(&bad_path), Some(APP_OPEN_REFUSED_NOTE));

        // The overlap, which no fixture covered: hosted AND nothing recorded.
        let mut hosted_no_path = a_store_match();
        hosted_no_path.path = String::new();
        assert!(
            hosted_no_path.hosted && hosted_no_path.path.is_empty(),
            "the premise: this match trips both refusals at once"
        );
        assert_eq!(
            app_open_refusal(&hosted_no_path),
            Some(APP_OPEN_HOSTED_NOTE),
            "a Store app with no recorded path is told to re-pick it from the tray menu, \
             which cannot help: re-picking a Store app records another hosted match that \
             still gets no Open"
        );

        // Control: the three sentences really are distinct, so the assertions
        // above are not all satisfied by one string.
        let all = [APP_OPEN_HOSTED_NOTE, APP_OPEN_NO_PATH_NOTE, APP_OPEN_REFUSED_NOTE];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b, "two refusals are the same sentence");
            }
        }
    }

    /// The refusal reaches the card's notes, and the dead binding's does not.
    #[test]
    fn the_card_says_why_open_is_missing() {
        let mut no_path = a_browser_match();
        no_path.path = String::new();
        let notes = app_card_notes(&no_path);
        assert!(
            notes.iter().any(|n| n.contains("no program file was recorded")),
            "the card offers no Open and does not say why: {notes:?}"
        );
        // The trigger caption is still there: the binding still fires.
        assert!(notes.contains(&trigger_caption(no_path.trigger)), "{notes:?}");

        // A Store app gets its own reason, not the "no program file" one.
        let store = app_card_notes(&a_store_match());
        assert!(store.iter().any(|n| n.contains("Microsoft Store apps are opened through")), "{store:?}");

        // A dead binding gets the dead notice and NOTHING else -- adding a
        // second sentence about a missing button would be noise about a
        // consequence.
        let dead = app_card_notes(&AppMatch::for_process(
            "ApplicationFrameHost.exe",
            TriggerMode::Prompt,
        ));
        assert_eq!(dead, vec![APP_MATCH_DEAD_NOTICE], "{dead:?}");

        // The control: a launchable match's notes say none of this.
        let fine = app_card_notes(&a_browser_match());
        assert!(
            !fine.iter().any(|n| n.contains("can\u{2019}t start") || n.contains("won\u{2019}t start")),
            "a perfectly launchable app is told it cannot be launched: {fine:?}"
        );
    }

    /// The dropdown appears only when both are present -- the user's spec.
    #[test]
    fn open_offers_a_menu_only_when_there_is_an_app_and_a_website() {
        let m = a_browser_match();

        let both = app_open_choices(&m, WEB);
        assert_eq!(both.len(), 2, "an app and a website did not produce two choices: {both:?}");
        assert_eq!(open_choice_label(&both[0]), "Open chrome.exe");
        assert_eq!(open_choice_label(&both[1]), "Open website");

        let app_only = app_open_choices(&m, "");
        assert_eq!(app_only.len(), 1, "{app_only:?}");
        assert_eq!(open_choice_label(&app_only[0]), "Open chrome.exe");

        // A schemeless URI is not a website this pane can open, so it does
        // not add a choice -- and `a_login`'s own URI is exactly that shape.
        assert_eq!(app_open_choices(&m, "app.ledgerline.com").len(), 1);
        assert_eq!(app_open_choices(&m, "javascript:alert(1)").len(), 1);
        // And it is not smuggled onto the app's command line either.
        assert_eq!(
            app_launch_plan(&m, "javascript:alert(1)").unwrap().raw_tail,
            m.args,
            "a URL the pane refuses to open was appended to the program's command line"
        );

        // No app: no choices at all. The website already has a control -- the
        // blue link in AUTOFILL TARGETS.
        let store = a_store_match();
        assert!(app_open_choices(&store, WEB).is_empty(), "a Store app got an Open menu");
    }

    #[test]
    fn each_choice_reports_its_own_action_and_says_what_it_will_do() {
        let m = a_browser_match();
        let choices = app_open_choices(&m, WEB);
        let plan = app_launch_plan(&m, WEB).unwrap();

        assert_eq!(open_choice_action(&choices[0]), DetailAction::OpenApp(plan.clone()));
        assert_eq!(open_choice_action(&choices[1]), DetailAction::OpenWebsite(WEB.to_string()));
        // The two arms are genuinely different -- the fallback-chain failure
        // this crate has shipped is two branches returning one value.
        assert_ne!(open_choice_action(&choices[0]), open_choice_action(&choices[1]));

        // The app's tooltip is the command line that will run, in full.
        let hover = open_choice_hover(&choices[0]);
        assert!(hover.contains(&command_line(&plan)), "{hover:?}");
        assert!(hover.contains(r#"--profile-directory="Profile 2""#), "{hover:?}");
        assert!(hover.contains(WEB), "{hover:?}");
        // The website's names the URL and where it goes, and is NOT the
        // app's.
        let web_hover = open_choice_hover(&choices[1]);
        assert!(web_hover.contains(WEB) && web_hover.contains("default browser"), "{web_hover:?}");
        assert_ne!(web_hover, hover);
    }

    // -----------------------------------------------------------------
    // Open: the control on the pane.
    // -----------------------------------------------------------------

    /// One launchable app and no web URL: a plain button that names the app.
    #[test]
    fn a_launchable_app_with_no_website_draws_one_named_open_button() {
        let item = bound_to(&a_login(), &a_browser_match());
        let mut pane = Pane::new();
        let frame = pane.idle(&item, &TotpState::NoSecret);
        assert!(
            frame.painted("Open chrome.exe"),
            "the card drew no Open control at all; it painted: {:?}",
            frame.strings()
        );
        // Not a menu button: the bare word would be a question beside a
        // Program file row.
        assert!(!frame.painted(OPEN_MENU_LABEL), "{:?}", frame.strings());
        assert!(!frame.painted(OPEN_WEBSITE_LABEL), "{:?}", frame.strings());
        // And what it draws is the LABEL, not an elided stub of it.
        assert_eq!(frame.rendered_glyphs("Open chrome.exe"), "Open chrome.exe");
    }

    /// Clicking that button reports the plan -- the same plan the pure layer
    /// builds, so the control and the launcher cannot disagree.
    #[test]
    fn clicking_open_reports_the_plan_that_would_be_run() {
        let m = a_browser_match();
        let item = bound_to(&an_item(Some(1)), &m);
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        let button = laid_out.rect_of("Open chrome.exe").center();

        let clicked = pane.click(&item, &TotpState::NoSecret, button);
        assert_eq!(
            clicked.action,
            DetailAction::OpenApp(app_launch_plan(&m, "").unwrap()),
            "clicking Open reported {:?}",
            clicked.action
        );
        // The control: a click somewhere else in the same card reports
        // something else, so the assertion above is not satisfied by a pane
        // that returns OpenApp for every click.
        let elsewhere = pane.click(&item, &TotpState::NoSecret, laid_out.rect_of("App").center());
        assert_ne!(elsewhere.action, clicked.action, "every click on this card opens the app");
    }

    /// Both present: one button saying "Open", and the two entries behind it.
    #[test]
    fn an_app_and_a_website_draw_one_open_menu_with_both_entries() {
        let item = bound_to(&a_login_on_the_web(), &a_browser_match());
        let mut pane = Pane::new();

        let closed = pane.idle(&item, &TotpState::NoSecret);
        assert!(closed.painted(OPEN_MENU_LABEL), "{:?}", closed.strings());
        assert!(
            !closed.painted("Open chrome.exe"),
            "the menu's entries are painted with the menu shut: {:?}",
            closed.strings()
        );

        let open_button = closed.rect_of(OPEN_MENU_LABEL).center();
        let _ = pane.click(&item, &TotpState::NoSecret, open_button);
        // A popup only PAINTS on the frame after the click that opened it.
        let menu = pane.idle(&item, &TotpState::NoSecret);
        assert!(menu.painted("Open chrome.exe"), "{:?}", menu.strings());
        assert!(menu.painted(OPEN_WEBSITE_LABEL), "{:?}", menu.strings());
    }

    /// And each entry does its own thing.
    #[test]
    fn the_menus_two_entries_open_the_app_and_the_website() {
        let m = a_browser_match();
        let item = bound_to(&a_login_on_the_web(), &m);

        for (entry, expected) in [
            ("Open chrome.exe", DetailAction::OpenApp(app_launch_plan(&m, WEB).unwrap())),
            (OPEN_WEBSITE_LABEL, DetailAction::OpenWebsite(WEB.to_string())),
        ] {
            let mut pane = Pane::new();
            let closed = pane.idle(&item, &TotpState::NoSecret);
            let _ = pane.click(&item, &TotpState::NoSecret, closed.rect_of(OPEN_MENU_LABEL).center());
            let menu = pane.idle(&item, &TotpState::NoSecret);
            let row = menu.rect_of(entry).center();
            let clicked = pane.click(&item, &TotpState::NoSecret, row);
            assert_eq!(clicked.action, expected, "clicking {entry:?} reported the wrong action");
        }
    }

    /// A match that may not be launched draws no control, and the pane says
    /// why in words the user can read.
    #[test]
    fn a_match_that_cannot_be_launched_draws_no_open_and_explains_itself() {
        let mut no_path = a_browser_match();
        no_path.path = String::new();
        let item = bound_to(&a_login_on_the_web(), &no_path);
        let mut pane = Pane::new();
        let frame = pane.idle(&item, &TotpState::NoSecret);

        assert!(!frame.painted(OPEN_MENU_LABEL), "{:?}", frame.strings());
        assert!(!frame.painted("Open chrome.exe"), "{:?}", frame.strings());
        assert!(!frame.painted(OPEN_WEBSITE_LABEL), "{:?}", frame.strings());
        assert!(
            frame.painted(APP_OPEN_NO_PATH_NOTE),
            "the card silently dropped Open with no explanation: {:?}",
            frame.strings()
        );
        // Remove is still there -- this did not take the card's other
        // control with it.
        assert!(frame.painted("Remove"), "{:?}", frame.strings());
    }

    /// An unreadable `deskwarden:app-match` field offers Remove and nothing
    /// else: there is no parsed match to build a plan from.
    #[test]
    fn an_unreadable_field_is_never_offered_an_open() {
        let mut item = a_login_on_the_web();
        item.fields = vec![crate::vault_bridge::VaultField {
            name: Some(crate::app_match::APP_MATCH_FIELD_NAME.to_string()),
            value: Some("{not json".to_string()),
            other: serde_json::Map::new(),
        }];
        let mut pane = Pane::new();
        let frame = pane.idle(&item, &TotpState::NoSecret);
        assert!(
            frame.painted(APP_MATCH_UNREADABLE_NOTICE),
            "the premise: this item's field really is unreadable; painted {:?}",
            frame.strings()
        );
        assert!(frame.painted("Remove"), "{:?}", frame.strings());
        for label in [OPEN_MENU_LABEL, OPEN_WEBSITE_LABEL, "Open chrome.exe"] {
            assert!(!frame.painted(label), "an unreadable field offered {label:?}");
        }
    }
}

/// The read pane's SHAPE, on a pane too short to hold the item in it.
///
/// The bug these exist for: `draw_detail_read` drew the header strip and
/// every body card into one plain `Ui` with no scroll area at any level. On
/// the tallest realistic item -- a full identity with notes, five previous
/// passwords and a bound app -- the body painted down to y = 1967 on a pane
/// the app can be resized to 600. Everything past the fold was not merely
/// off-screen: egui culled it, so it was not painted, not clickable and not
/// scrollable to. The `MATCHED APP` card, its Autofill triggers and its Open
/// button -- the whole feature of commits `4b05adb` and `a33b75e` -- were
/// among the unreachable, which is very likely why the user went looking in
/// the Edit pane at all. This is the read-side half of `68f86cb`.
///
/// Everything here is a geometry assertion on rects egui really painted, and
/// the card assertions read the GLYPHS rather than `Galley::text()`, which
/// answers with the layout job's source string and is therefore blind to a
/// run that was elided to nothing. Each test carries the control that says
/// what it would look like to be blind.
#[cfg(test)]
mod read_pane_scroll_tests {
    use super::*;

    /// The narrowest the detail pane can be: 900 - 212 - 390 = 298pt,
    /// derived rather than written out for the same reason `MIN_PANE` above
    /// is.
    const NARROW: f32 = crate::settings::MIN_VAULT_WINDOW_SIZE.0 as f32
        - crate::vault_window::SIDEBAR_WIDTH
        - crate::vault_window::LIST_WIDTH;

    /// The app's minimum window HEIGHT. 600 is the whole window and the pane
    /// really gets less, so this over-states the room -- the safe direction:
    /// an item that will not fit here cannot fit in the app either.
    const SHORT: f32 = crate::settings::MIN_VAULT_WINDOW_SIZE.1 as f32;

    /// A pane with room to spare for the same item -- taller than the 1967pt
    /// the tallest body measures, so nothing scrolls and the bar has nothing
    /// to say. Not a value to guess at: `a_roomy_pane_really_does_fit_the_
    /// item` checks that this height really is roomy, because every test
    /// below that contrasts "scrolls" with "fits" is vacuous if it is not.
    const ROOMY: f32 = 2400.0;

    /// Every string the frame painted, as (source, glyphs, rect), plus every
    /// filled rectangle.
    #[derive(Default)]
    struct Shot {
        runs: Vec<(String, String, egui::Rect)>,
        rects: Vec<(egui::Rect, egui::Color32)>,
    }

    impl Shot {
        /// The rect of the one run laid out from `source`, or `None` if the
        /// pane painted nothing from it -- which is what a culled card looks
        /// like, and so is the answer this suite is mostly about.
        fn rect_of(&self, source: &str) -> Option<egui::Rect> {
            let hits: Vec<&(String, String, egui::Rect)> =
                self.runs.iter().filter(|(s, _, _)| s == source).collect();
            assert!(
                hits.len() <= 1,
                "{source:?} was painted {} times, so an assertion naming it is ambiguous",
                hits.len()
            );
            hits.first().map(|(_, _, r)| *r)
        }

        /// The glyphs actually laid out for `source`. `None` when it was not
        /// painted at all.
        fn glyphs_of(&self, source: &str) -> Option<String> {
            self.runs
                .iter()
                .find(|(s, _, _)| s == source)
                .map(|(_, g, _)| g.clone())
        }

        fn sources(&self) -> Vec<&str> {
            self.runs.iter().map(|(s, _, _)| s.as_str()).collect()
        }
    }

    fn walk(shape: &egui::Shape, shot: &mut Shot) {
        match shape {
            egui::Shape::Text(text) => {
                let glyphs: String = text
                    .galley
                    .rows
                    .iter()
                    .flat_map(|row| row.glyphs.iter().map(|g| g.chr))
                    .collect();
                shot.runs.push((
                    text.galley.text().to_string(),
                    glyphs,
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                ));
            }
            egui::Shape::Rect(rect) => shot.rects.push((rect.rect, rect.fill)),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, shot);
                }
            }
            _ => {}
        }
    }

    /// A pane of a chosen SIZE -- which the `Pane` harness above cannot do,
    /// being fixed at `PANE` tall, and the height is the entire subject here.
    struct ShortPane {
        ctx: egui::Context,
        size: egui::Vec2,
        reveal: RevealState,
    }

    impl ShortPane {
        fn new(size: egui::Vec2) -> Self {
            let ctx = egui::Context::default();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                ..Default::default()
            };
            // The crate's standing two throwaway frames: a font set
            // registered during a frame is only usable from the next one.
            let _ = ctx.run_ui(input.clone(), |_ui| {});
            theme::apply(&ctx);
            let _ = ctx.run_ui(input, |_ui| {});
            Self {
                ctx,
                size,
                reveal: RevealState::default(),
            }
        }

        fn bounds(&self) -> egui::Rect {
            egui::Rect::from_min_size(egui::Pos2::ZERO, self.size)
        }

        fn frame(&mut self, item: &VaultItem, events: Vec<egui::Event>) -> Shot {
            let output = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(self.bounds()),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let _ = draw_detail_read(
                        ui,
                        item,
                        None,
                        3,
                        &TotpState::NoSecret,
                        false,
                        &mut self.reveal,
                        None,
                    );
                },
            );
            let mut shot = Shot::default();
            let all = egui::Shape::Vec(output.shapes.iter().map(|c| c.shape.clone()).collect());
            walk(&all, &mut shot);
            shot
        }

        fn idle(&mut self, item: &VaultItem) -> Shot {
            self.frame(item, Vec::new())
        }

        /// Scrolls the body to its bottom: the pointer parked in the middle
        /// of the pane (a wheel goes to the area under the pointer, and with
        /// no pointer there is no area) and a wheel delta far larger than the
        /// content, which a `ScrollArea` clamps to its own end. Several
        /// frames because that clamp is against LAST frame's content size.
        fn scroll_to_bottom(&mut self, item: &VaultItem) -> Shot {
            let mut shot = self.idle(item);
            for _ in 0..4 {
                shot = self.frame(
                    item,
                    vec![
                        egui::Event::PointerMoved(self.bounds().center()),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: egui::vec2(0.0, -4000.0),
                            modifiers: egui::Modifiers::NONE,
                            phase: egui::TouchPhase::Move,
                        },
                    ],
                );
            }
            shot
        }
    }

    /// The tallest item this app can really be asked to draw: an identity
    /// with EVERY field filled (four groups, eighteen rows), a note, five
    /// previous passwords, and a bound desktop app -- which an identity does
    /// show (see `app_card_visible`: a binding is never hidden, whatever the
    /// kind). This is the item whose body measured 1967pt.
    fn the_tallest_item() -> VaultItem {
        let some = |s: &str| Some(s.to_string());
        let mut item = VaultItem {
            id: "id-tall".to_string(),
            name: "Ada Lovelace".to_string(),
            fields: Vec::new(),
            login: None,
            card: None,
            identity: Some(crate::vault_bridge::IdentityData {
                title: some("Ms"),
                first_name: some("Ada"),
                middle_name: some("Augusta"),
                last_name: some("Lovelace"),
                address1: some("12 Analytical Way"),
                address2: some("Flat 4"),
                address3: some("Difference Court"),
                city: some("London"),
                state: some("Greater London"),
                postal_code: some("EC1A 1BB"),
                country: some("United Kingdom"),
                company: some("Ledgerline"),
                email: some("ada@example.com"),
                phone: some("+44 20 7946 0000"),
                ssn: some("123-45-6789"),
                username: some("ada"),
                passport_number: some("P123456"),
                license_number: some("L987654"),
                other: serde_json::Map::new(),
            }),
            ssh_key: None,
            notes: Some("Recovery codes are in the safe.".to_string().into()),
            item_type: Some(4),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        };
        let history: Vec<serde_json::Value> = (0..5)
            .map(|i| {
                serde_json::json!({
                    "lastUsedDate": "2026-07-30T09:15:00.000Z",
                    "password": format!("old-secret-{i}"),
                })
            })
            .collect();
        item.other
            .insert("passwordHistory".to_string(), serde_json::Value::Array(history));
        crate::vault_bridge::with_app_match(
            &item,
            &AppMatch {
                process: "Ledgerline.exe".to_string(),
                title: String::new(),
                hosted: false,
                // A path that exists nowhere, so nothing here depends on
                // what happens to be installed on the machine running it.
                path: "C:\\Deskwarden Test\\Ledgerline\\Ledgerline.exe".to_string(),
                args: String::new(),
                trigger: TriggerMode::Prompt,
            },
        )
    }


    /// `source` is painted, wholly inside `pane` **on both axes**, and the
    /// glyphs really laid for it are the whole of `source`.
    ///
    /// **Both axes, and the vertical-only version this replaces is why.**
    /// That version passed while, on the same pane at 298pt,
    /// `Open Ledgerline.exe` was drawn at x = 283.7..393.9 -- 14 of its 110pt
    /// on a pane whose right edge is 298 -- and `Remove`, the only way to
    /// undo an app binding from this pane, was culled and never painted at
    /// all. Horizontal scrolling is refused here on purpose, so an x outside
    /// the pane is exactly as unreachable as a y outside it. The sibling
    /// `detail_edit.rs` suite has asserted `contains_rect` on both axes all
    /// along; this is the same assertion, spelled out so the message can name
    /// which axis failed.
    ///
    /// **And the glyphs must be the whole label, not merely non-empty.**
    /// `rect_of` reads the galley's box, which a run elided to nothing still
    /// has; `glyphs != "\u{2026}"` was the previous guard and it does not
    /// catch `"Open Le\u{2026}"`, which is a control fitted into the pane by
    /// destroying the only thing that says which app it would open. Comparing
    /// against `source` catches every elision, including that one.
    fn assert_visible(shot: &Shot, source: &str, pane: egui::Rect) {
        let rect = shot.rect_of(source).unwrap_or_else(|| {
            panic!(
                "{source:?} was not painted at all -- egui culled it, which is the bug. \
                 Painted: {:?}",
                shot.sources()
            )
        });
        assert!(
            pane.contains_rect(rect),
            "{source:?} is painted at x = {}..{}, y = {}..{} on a {}x{}pt pane -- it is off \
             the {} edge and this pane does not scroll horizontally",
            rect.left(),
            rect.right(),
            rect.top(),
            rect.bottom(),
            pane.width(),
            pane.height(),
            if rect.left() < pane.left() || rect.right() > pane.right() {
                "right or left"
            } else {
                "top or bottom"
            }
        );
        let glyphs = shot.glyphs_of(source).unwrap_or_default();
        assert_eq!(
            glyphs, source,
            "{source:?} occupies {rect:?} but rendered {glyphs:?} -- it was elided, so the \
             control is on the pane and its label is not"
        );
    }

    /// **The controls on the assertion itself.** Every test in this module
    /// leans on `assert_visible`, so a weakening of it -- back to one axis,
    /// or back to "the glyphs are not empty" -- would quietly re-green the
    /// very defects it exists to catch. These two feed it hand-built `Shot`s
    /// that no layout produced and demand that it refuses them.
    ///
    /// `catch_unwind` because the failure IS the assertion: a helper that
    /// accepted these would be caught by nothing else.
    #[test]
    fn assert_visible_refuses_a_control_that_is_off_the_right_edge() {
        let pane = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(NARROW, SHORT));
        // Vertically perfect, horizontally the exact geometry the reviewer
        // measured: 14 of 110pt on screen.
        let mut shot = Shot::default();
        shot.runs.push((
            "Open Ledgerline.exe".to_string(),
            "Open Ledgerline.exe".to_string(),
            egui::Rect::from_min_max(egui::pos2(283.7, 484.5), egui::pos2(393.9, 497.5)),
        ));
        assert!(
            std::panic::catch_unwind(|| assert_visible(&shot, "Open Ledgerline.exe", pane))
                .is_err(),
            "assert_visible accepted a control 95.9pt past the right edge of the pane"
        );

        // The control on the control: move the same run inside the pane and
        // it is accepted, so the panic above is about the x and not about the
        // hand-built `Shot`.
        let mut inside = Shot::default();
        inside.runs.push((
            "Open Ledgerline.exe".to_string(),
            "Open Ledgerline.exe".to_string(),
            egui::Rect::from_min_max(egui::pos2(51.0, 484.5), egui::pos2(161.2, 497.5)),
        ));
        assert_visible(&inside, "Open Ledgerline.exe", pane);
    }

    #[test]
    fn assert_visible_refuses_a_label_that_was_elided_to_fit() {
        let pane = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(NARROW, SHORT));
        // Every one of these is inside the pane and none is empty, so the
        // previous guard -- non-empty and not exactly "..." -- passed all
        // three.
        for elided in ["Open Le\u{2026}", "O\u{2026}", "\u{2026}"] {
            let mut shot = Shot::default();
            shot.runs.push((
                "Open Ledgerline.exe".to_string(),
                elided.to_string(),
                egui::Rect::from_min_max(egui::pos2(51.0, 484.5), egui::pos2(101.0, 497.5)),
            ));
            assert!(
                std::panic::catch_unwind(|| assert_visible(&shot, "Open Ledgerline.exe", pane))
                    .is_err(),
                "assert_visible accepted {elided:?} as a rendering of \"Open Ledgerline.exe\""
            );
        }
    }

    /// **The bug, and the fix, as one test.** Before scrolling the app card
    /// is not on the pane at all; after scrolling every part of it is.
    ///
    /// Positive control: the first half is it. On the pre-fix layout -- one
    /// plain `Ui`, no scroll area -- the wheel does nothing at all, so the
    /// second half fails with the card still unpainted. Verified by running
    /// this against that layout, not assumed.
    #[test]
    fn the_matched_app_card_is_reachable_on_the_shortest_window() {
        let mut pane = ShortPane::new(egui::vec2(NARROW, SHORT));
        let item = the_tallest_item();
        let bounds = pane.bounds();

        let before = pane.idle(&item);
        // The premise. Without it this test would pass on a pane the item
        // already fits in, where the scroll below is a no-op and the whole
        // thing is blind.
        assert!(
            before.rect_of(APP_CARD_HEADING).is_none(),
            "the tallest item already fits in a {NARROW}x{SHORT} pane, so this test is \
             not exercising scrolling at all"
        );
        // ... and the control on the control: the pane IS drawing something,
        // so "nothing is painted" cannot be what satisfies the line above.
        assert_visible(&before, "IDENTITY", bounds);

        let after = pane.scroll_to_bottom(&item);
        for source in [
            APP_CARD_HEADING,
            // The row the binding's behaviour is set on ...
            "Autofill",
            // ... and the control commit `a33b75e` added, which was the
            // single least reachable thing on the pane.
            "Open Ledgerline.exe",
            // **The only way to undo an app binding from this pane**, and the
            // one this list did not name: it was not painted at all below
            // about 600pt of pane, and the vertical-only `assert_visible`
            // could not have said so if it had been named.
            "Remove",
            // Every trigger pill, for the same reason: the third was drawn
            // past the pane's right edge.
            "Auto",
            "Hotkey",
            "Prompt",
            "App",
            "Program file",
            // **The VALUES, not only the labels.** Every label on this card
            // sits at x = 41 whatever the pane is, so a list of labels alone
            // is satisfied by a card laid out at any width at all: with the
            // `Program file` path drawn unwrapped the card measured 467.8pt
            // inside a 298pt pane and every label above still passed. The
            // path is the widest thing this pane can be asked to draw, and
            // it is the one that inflated the card.
            "Ledgerline.exe",
            r"C:\Deskwarden Test\Ledgerline\Ledgerline.exe",
        ] {
            assert_visible(&after, source, bounds);
        }
    }

    /// The header stays where it is. It is the only thing held out of the
    /// scroll area, and the reason is that a scrolled-away title leaves no
    /// answer to "which item am I looking at?" -- so it has to be pinned in
    /// fact and not just in intent.
    #[test]
    fn the_header_does_not_move_when_the_body_scrolls() {
        let mut pane = ShortPane::new(egui::vec2(NARROW, SHORT));
        let item = the_tallest_item();
        let bounds = pane.bounds();

        let before = pane.idle(&item);
        let title_before = before
            .rect_of(&item.name)
            .expect("the header painted no title");
        assert_visible(&before, &item.name, bounds);

        let after = pane.scroll_to_bottom(&item);
        let title_after = after.rect_of(&item.name).expect("the title scrolled away");
        assert_eq!(
            title_before, title_after,
            "the header title moved from {title_before:?} to {title_after:?} when the body \
             scrolled -- it is inside the scroll area"
        );
        // The control on that: the BODY did move, so an equality that held
        // because nothing scrolled at all would not pass here.
        assert!(
            after.rect_of(APP_CARD_HEADING).is_some(),
            "nothing scrolled, so the title standing still proves nothing"
        );
    }

    /// The copy confirmation is drawn on a foreground layer against the PANE,
    /// not against the scrolled content, so it keeps the corner
    /// `copy_toast_tests` pins it to -- 20pt in from the pane's bottom-right
    /// -- however far the body has been scrolled, and the scroll area does
    /// not clip it.
    #[test]
    fn the_copy_toast_keeps_its_corner_while_the_body_is_scrolled() {
        let mut pane = ShortPane::new(egui::vec2(NARROW, SHORT));
        let item = the_tallest_item();
        let bounds = pane.bounds();

        let _ = pane.scroll_to_bottom(&item);
        note_copied(&pane.ctx, "Password");
        let shot = pane.idle(&item);

        let text = copy_toast_text("Password");
        let glyphs = shot.rect_of(&text).unwrap_or_else(|| {
            panic!(
                "the confirmation was not painted; the pane painted {:?}",
                shot.sources()
            )
        });
        // The BOX, not the glyphs: the assertions in `copy_toast_tests` are
        // about the box's own edges, and a box is what a scroll area would
        // have clipped.
        let boxes: Vec<egui::Rect> = shot
            .rects
            .iter()
            .filter(|(rect, fill)| *fill == theme::INK && rect.contains_rect(glyphs))
            .map(|(rect, _)| *rect)
            .collect();
        assert_eq!(
            boxes.len(),
            1,
            "expected one confirmation box around {glyphs:?}, found {boxes:?}"
        );
        let toast = boxes[0];
        assert_eq!(toast.right(), bounds.right() - COPY_TOAST_INSET);
        assert_eq!(toast.bottom(), bounds.bottom() - COPY_TOAST_INSET);
    }

    /// No horizontal scrolling: the wheel moves the body up and down and
    /// NOTHING sideways. The rows already elide what they can, and a
    /// horizontal bar under them would be the regression rather than the fix.
    ///
    /// Stated as "every run that is painted both before and after the scroll
    /// keeps its x", which is what a horizontal offset would break and what a
    /// bar appearing mid-scroll would break too.
    #[test]
    fn the_body_never_scrolls_sideways() {
        let mut pane = ShortPane::new(egui::vec2(NARROW, SHORT));
        let item = the_tallest_item();
        let before = pane.idle(&item);
        let mut after = pane.scroll_to_bottom(&item);
        assert!(
            after.rect_of(APP_CARD_HEADING).is_some(),
            "nothing scrolled, so an x that did not move proves nothing"
        );
        // And then asked, in as many words, to scroll sideways. Without this
        // the sweep below only says a VERTICAL wheel has no sideways
        // component -- `ScrollArea::both` in place of `::vertical` would pass
        // it while giving the pane a horizontal bar and a horizontal offset.
        for _ in 0..4 {
            after = pane.frame(
                &item,
                vec![
                    egui::Event::PointerMoved(pane.bounds().center()),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(-4000.0, 0.0),
                        modifiers: egui::Modifiers::NONE,
                        phase: egui::TouchPhase::Move,
                    },
                ],
            );
        }

        let mut compared = 0;
        let once = |shot: &Shot, source: &str| {
            let hits: Vec<egui::Rect> = shot
                .runs
                .iter()
                .filter(|(s, _, _)| s == source)
                .map(|(_, _, r)| *r)
                .collect();
            // Exactly one, or this comparison cannot say WHICH run moved --
            // the five "6 days ago" stamps in the previous-passwords card
            // are five different rows with one string between them.
            (hits.len() == 1).then(|| hits[0])
        };
        for (source, _, rect) in &before.runs {
            let (Some(_), Some(moved)) = (once(&before, source), once(&after, source)) else {
                continue;
            };
            assert_eq!(
                (rect.left(), rect.right()),
                (moved.left(), moved.right()),
                "{source:?} moved sideways from x = {}..{} to {}..{} when the body was                  scrolled",
                rect.left(),
                rect.right(),
                moved.left(),
                moved.right()
            );
            compared += 1;
        }
        // The sweep has to have swept something: an empty loop asserts
        // nothing, and the identity rows above the fold are painted in both
        // frames, so there is no honest way for this to be zero.
        assert!(compared >= 5, "only {compared} runs were common to both frames");
    }

    /// The premise every "scrolls versus fits" contrast below rests on:
    /// [`ROOMY`] really is tall enough for the whole body, and [`SHORT`]
    /// really is not. Without this, two panes that both overflow would
    /// satisfy those tests while comparing nothing.
    #[test]
    fn a_roomy_pane_really_does_fit_the_item() {
        let mut item = the_tallest_item();
        item.fields.clear();
        // The last card of the body proper, and so the one that is only
        // painted when everything above it fitted.
        let last = "PREVIOUS PASSWORDS";

        let mut roomy = ShortPane::new(egui::vec2(NARROW, ROOMY));
        let _ = roomy.idle(&item);
        let shot = roomy.idle(&item);
        let rect = shot
            .rect_of(last)
            .unwrap_or_else(|| panic!("{last:?} is not on a {ROOMY}pt pane either"));
        assert!(
            rect.bottom() <= ROOMY,
            "{last:?} ends at y = {} on a {ROOMY}pt pane, so ROOMY is not roomy",
            rect.bottom()
        );

        let mut short = ShortPane::new(egui::vec2(NARROW, SHORT));
        let _ = short.idle(&item);
        let shot = short.idle(&item);
        assert!(
            shot.rect_of(last).is_none(),
            "{last:?} already fits a {SHORT}pt pane unscrolled, so SHORT does not overflow"
        );
    }

    /// The bar is PAINTED when there is something to scroll and not painted
    /// when there is not.
    ///
    /// Hiding it is not cosmetic tidying: `AlwaysVisible` above is what keeps
    /// the cards one width, and it also means egui would otherwise draw a
    /// full-height bar down a body that cannot move. That is exactly the
    /// report `092da70` fixed on the item list -- a bar in the margin reads
    /// as the padding having shrunk. Nothing about the LAYOUT changes either
    /// way, which is what `the_bar_does_not_move_the_cards` pins; this is
    /// only about whether ink lands in the lane.
    #[test]
    fn the_bar_is_painted_only_when_there_is_something_to_scroll() {
        let mut item = the_tallest_item();
        // The binding dropped for the same reason as in the test below: its
        // path row widens the body sideways and would paint into the lane on
        // its own account.
        item.fields.clear();

        let ink_in_the_lane = |height: f32| {
            let mut pane = ShortPane::new(egui::vec2(NARROW, height));
            // Two frames: the first is the reading the second decides on.
            // The pointer is IN the area on both, because egui's floating bar
            // is dormant -- fully transparent -- while the pointer is away,
            // and a test that read a dormant bar would certify the placement
            // of something not drawn. That is one of the vacuous tests this
            // crate has already shipped.
            let over = vec![egui::Event::PointerMoved(pane.bounds().center())];
            let _ = pane.frame(&item, over.clone());
            let shot = pane.frame(&item, over);
            let lane = NARROW - f32::from(BODY_PAD_X);
            shot.rects
                .iter()
                .filter(|(rect, fill)| {
                    // Anything with colour in it, in the reserved lane and
                    // no wider than the bar. The width is part of the filter
                    // because at 298pt the cards themselves spill sideways
                    // into the lane (the horizontal defect noted in
                    // `assert_visible`) -- a card is hundreds of points wide
                    // and the bar is `SCROLLBAR_WIDTH`.
                    fill.a() > 0
                        && rect.left() >= lane - 0.5
                        && rect.width() > 0.0
                        && rect.width() <= theme::SCROLLBAR_WIDTH + 0.5
                })
                .count()
        };

        // The FIRST frame a context ever draws has no reading to go on, and
        // `body_overflowed` answers TRUE there so the bar is shown rather
        // than missing. Ties go to showing it: a bar on a body that turns
        // out to fit is gone next frame, a missing bar on a body that really
        // scrolls says there is nothing below.
        let mut first = ShortPane::new(egui::vec2(NARROW, SHORT));
        let shot = first.frame(&item, vec![egui::Event::PointerMoved(egui::pos2(150.0, 300.0))]);
        let lane = NARROW - f32::from(BODY_PAD_X);
        assert!(
            shot.rects.iter().any(|(rect, fill)| fill.a() > 0
                && rect.left() >= lane - 0.5
                && rect.width() > 0.0
                && rect.width() <= theme::SCROLLBAR_WIDTH + 0.5),
            "the very first frame paints no bar at all"
        );

        assert!(
            ink_in_the_lane(SHORT) > 0,
            "a body that overflows a {SHORT}pt pane paints no scroll bar at all, so              nothing tells the reader there is more below"
        );
        assert_eq!(
            ink_in_the_lane(ROOMY),
            0,
            "a body with nothing to scroll still paints a bar down its right margin"
        );
    }

    /// The cards keep ONE width whether or not the scroll bar is showing.
    ///
    /// This is the trap `092da70` measured on the item list: under egui's
    /// default `VisibleWhenNeeded` the bar reserves its lane only while it is
    /// shown, so the content's right edge jumps by the lane's width as the
    /// content crosses the overflow threshold -- a 10pt jump the user
    /// noticed. Here `AlwaysVisible` plus `theme::scrollbar_in_gutter` makes
    /// the reservation unconditional and `theme::hide_scrollbar` merely stops
    /// painting the bar, so nothing moves.
    ///
    /// The same ITEM on two pane HEIGHTS is the comparison: one short enough
    /// to overflow and one tall enough not to. Comparing two different items
    /// instead would have measured the items, not the bar -- the app card is
    /// wider than a 298pt pane whatever the height, which is the separate
    /// horizontal defect noted in `assert_visible`, and this test's equality
    /// across heights is also what shows that defect is not this fix's doing.
    #[test]
    fn the_bar_does_not_move_the_cards() {
        let edges = |item: &VaultItem, height: f32| {
            let mut pane = ShortPane::new(egui::vec2(NARROW, height));
            // Two frames: the second is the one whose bar state was decided
            // by a real reading of the first, which is when a conditional
            // lane would appear or vanish.
            let _ = pane.idle(item);
            let shot = pane.idle(item);
            let heading = shot
                .rect_of("IDENTITY")
                .expect("the identity card lost its heading");
            let card = shot
                .rects
                .iter()
                .filter(|(rect, fill)| *fill == theme::CARD && rect.contains_rect(heading))
                .map(|(rect, _)| *rect)
                .reduce(egui::Rect::union)
                .expect("the identity card has no white surface");
            (card.left(), card.right())
        };

        // **The absolute half**, on an item with nothing in it long enough to
        // widen a card: the lane REPLACES the body's right padding, so a lane
        // that is never reserved leaves the cards running to the pane's very
        // edge. Measured here rather than on the tall item because at 298pt
        // ordinary identity values already overflow their card sideways --
        // the separate horizontal defect noted in `assert_visible`.
        let mut small = the_tallest_item();
        small.identity = Some(crate::vault_bridge::IdentityData {
            first_name: Some("Ada".to_string()),
            ..Default::default()
        });
        small.notes = None;
        small.other.remove("passwordHistory");
        small.fields.clear();
        assert_eq!(
            edges(&small, SHORT),
            (f32::from(BODY_PAD_X), NARROW - f32::from(BODY_PAD_X)),
            "a card does not span the body's own {BODY_PAD_X}pt padding on a {NARROW}pt pane"
        );

        // **The consistency half**: the same item on two heights, one that
        // overflows and one that does not, so the bar is really showing in
        // one and hidden in the other. The binding is dropped for the reason
        // above; without it the body is still ~1000pt on a 600pt pane, which
        // is all this needs.
        let mut tall = the_tallest_item();
        tall.fields.clear();
        let scrolls = edges(&tall, SHORT);
        let fits = edges(&tall, ROOMY);
        assert_ne!(
            scrolls,
            (0.0, 0.0),
            "the card was not found at all, so the equality below is vacuous"
        );
        assert_eq!(
            scrolls, fits,
            "the identity card spans {scrolls:?} on a pane that scrolls and {fits:?} on one              that does not -- the bar's lane is being reserved conditionally"
        );
    }
}

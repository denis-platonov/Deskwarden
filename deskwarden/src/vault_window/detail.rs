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
use crate::app_match::AppMatch;
/// The trigger vocabulary this file still spells is test-only -- see
/// [`TRIGGER_ORDER`] -- and so is the import it needs.
#[cfg(test)]
use crate::app_match::TriggerMode;
use crate::breach::{breach_phrase, BreachCache, BreachStatus};
use crate::password_strength;
use crate::theme;
use crate::vault_bridge::{
    password_history, CardData, IdentityData, ItemKind, PasswordHistoryEntry, SshKeyData, VaultItem,
};
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke};
/// [`totp_key_of`] has to allocate to hand back a decoded key, and the seed is
/// the one value in this file where that allocation must not be an ordinary
/// `String` -- see that function's doc and the crate's allocator probe.
use zeroize::Zeroizing;

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
/// The gap between the two lines of a [`RowShape::Stacked`] row. Smaller than
/// [`ROW_GAP`], which separates two COLUMNS: the label and the value below it
/// are one field, and a gap as wide as the one between fields would read as
/// two.
const STACKED_LABEL_GAP: f32 = 4.0;
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
/// The masked SECRET row's label -- the seed, distinct from the "One-time
/// code" row above it, which is the six digits derived from it.
///
/// Not read out of `copy_shortcut_label`: that helper's whole job is to keep a
/// row and its copy TOAST spelling the same string, and this row has no chord
/// and therefore no `CopyShortcut` variant. The word "secret" is the user's
/// own ("a field masked 'show secret'"), and it is what the preferences row
/// that turns this on calls it too (`prefs_ui::TOTP_SECRET_LABEL`).
const TOTP_SECRET_LABEL: &str = "One-time code secret";
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
    /// A login's TOTP SECRET row -- the fifth flag, and the one the paragraph
    /// above about a fifth masked row not reusing a fourth's was written for.
    ///
    /// **This is the whole of "the reveal is per-view".** The row itself
    /// remembers nothing: `masked_row` is handed this `&mut bool` and there
    /// is no `egui` memory entry, no id and no cache anywhere on that path,
    /// so a revealed seed can survive a change of item only if THIS field
    /// does. `vault_window::mod`'s `run` builds the struct with
    /// [`Default::default`] and re-assigns it wholesale on every selection
    /// change, so the field is added and cleared without that file changing
    /// -- which is why the guard here is a test that resets exactly as `run`
    /// does and then asserts the next item's seed comes up masked.
    ///
    /// It matters more here than for a password: a password left revealed can
    /// be rotated afterwards, and a TOTP seed cannot -- it is the long-lived
    /// shared secret every future code is derived from.
    pub totp_secret: bool,
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
    CopyUsername,
    CopyPassword,
    CopyTotp,
    /// A login's TOTP *secret* -- the seed, not the six-digit code -- was
    /// copied off the details screen's masked secret row.
    ///
    /// Named rather than carrying the value, for exactly the reason
    /// [`Self::CopyCardNumber`] and [`Self::CopySshPrivateKey`] are: the seed
    /// is `Option<Zeroizing<String>>` on the item, the caller already holds
    /// the item, and routing it through [`Self::CopyValue`] would give the
    /// plaintext a second, non-zeroizing home inside this enum.
    ///
    /// Distinct from [`Self::CopyTotp`], which copies the derived code out of
    /// the [`TotpState`]. One expires in thirty seconds; the other does not
    /// expire at all, so they are two actions and never one.
    CopyTotpSecret,
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

/// Whether this pane should ask the breach cache about this item at all.
///
/// Three conditions, all of them necessary: the preference is on, the kind
/// has a password worth asking about ([`kind_offers_fill`] -- the same
/// predicate the fill count and the strength are already gated on, so the
/// strip cannot claim one kind of item in its first half and another in its
/// second), and there is actually a password. An empty password has no
/// prefix worth a request, and every passwordless item in the vault would
/// share the same five hex characters -- one cache entry standing for a
/// question nobody asked.
///
/// **This is the whole gate, and it runs LAZILY, once per opened pane.**
/// There is no sweep over the vault: a 500-item vault would be 500 requests
/// in a burst at a free public endpoint and 500 verdicts held in memory that
/// the user never asked for. The list does not show a verdict for the same
/// reason -- it would have to have one for every row.
pub fn should_check(enabled: bool, kind: ItemKind, password: &str) -> bool {
    enabled && kind_offers_fill(kind) && !password.is_empty()
}

/// The metadata strip's second half, for a status the pane actually has.
///
/// `None` => the strip is byte-identical to today: nothing is appended and
/// the card paints the single `RichText` it always did. That is the answer
/// [`breach_segment`] gives whenever [`should_check`] is false, which is
/// every item in the vault until the user turns the preference on.
///
/// Reached only once a status exists, and every status there is says
/// something out loud -- which is why each arm here is `Some`. A status with
/// no segment would be a badge that silently disappears, and the one this
/// would happen to first is `Unavailable`.
///
/// **`Unavailable` carries no reassurance.** It does not say "safe", it does
/// not say "not in", and [`segment_color`] does not paint it as a verdict. A
/// soothing badge on a request that failed is the worst outcome this whole
/// feature has: it tells the user a password was checked and cleared when
/// nothing was checked at all.
pub fn strip_segment(status: BreachStatus) -> Option<String> {
    Some(match status {
        BreachStatus::Pending => "\u{b7} Breach check: checking\u{2026}".to_string(),
        BreachStatus::Safe => "\u{b7} Not in any known breach".to_string(),
        // `breach_phrase` owns this wording, including the rule that the
        // advice never varies by count -- "seen 3 times" and "seen 40,000
        // times" mean the same thing and get the same sentence. Reused, never
        // restated: a second copy here would be a second place for the advice
        // to soften.
        BreachStatus::Breached(count) => format!("\u{b7} {}", breach_phrase(count)),
        BreachStatus::Unavailable => "\u{b7} Breach check unavailable".to_string(),
    })
}

/// Whether the segment is the one that must be RED.
///
/// Exactly one status is: a password on a public list, which is the only
/// thing here the user has to act on. `Pending`, `Safe` and `Unavailable`
/// are all reports on the check itself and none of them is an alarm.
pub fn segment_is_urgent(status: BreachStatus) -> bool {
    matches!(status, BreachStatus::Breached(_))
}

/// The colour the segment is painted in -- the palette's red for the one
/// urgent status, and the same faint ink as the rest of the strip for the
/// other three.
///
/// A function rather than an `if` at the paint site so the colour is a
/// decision a test can reach; the test asserts the *painted* ink as well, so
/// this and the renderer cannot disagree.
pub fn segment_color(status: BreachStatus) -> egui::Color32 {
    if segment_is_urgent(status) {
        theme::ERROR
    } else {
        theme::TEXT_FAINT
    }
}

/// The badge as the pane needs it: the text and the colour, or `None` for
/// "this strip is what it always was".
///
/// The **only** place the cache is asked anything. Everything above it is
/// pure; `BreachCache::status` answers from the map or starts one worker and
/// says [`BreachStatus::Pending`], requesting a repaint so the answer lands
/// on a later frame without the user touching anything.
fn breach_segment(
    ctx: &egui::Context,
    enabled: bool,
    kind: ItemKind,
    password: &str,
    breaches: &mut BreachCache,
) -> Option<(String, egui::Color32)> {
    if !should_check(enabled, kind, password) {
        return None;
    }
    let status = breaches.status(ctx, password);
    strip_segment(status).map(|text| (text, segment_color(status)))
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
/// assumed: `vault_window::mod` takes CTRL+K, CTRL+L and CTRL+N, the login
/// window takes CTRL+H, and the global fill hotkey is CTRL+ALT+B. (It used to
/// take CTRL+SHIFT+F as well, for the header's "Fill in app" button; button
/// and chord are both gone, so that key is free again.)
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
/// other SHIFT chord anywhere in it is [`OPEN_APP_CHORD`]'s CTRL+SHIFT+O --
/// `vault_window::mod`'s CTRL+SHIFT+F is gone with the button it belonged to
/// -- and the only other binding on `U` is the CTRL+U above. Both surviving
/// SHIFT chords are in [`pane_chords`], which is what the collision guard
/// actually walks; this paragraph is the part of the survey no table can do.
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

/// The one chord on this pane that is not a copy: **open the matched app**.
///
/// The user asked for the app's name to "be clickable like link and have Ctrl
/// + shortcut". Spelled as `(modifiers, key, chord)` -- the same triple
/// [`COPY_SHORTCUTS`] carries, for the same reason: the chord a row PAINTS and
/// the keys the handler is wired to have to be one value or they drift.
///
/// **Standalone rather than a fifth row of [`COPY_SHORTCUTS`], and that is a
/// decision about honesty, not tidiness.** Every function hanging off that
/// table is about copying -- `copy_shortcut_action` returns a `Copy*`,
/// `copy_shortcut_label` names *a field whose value goes on the clipboard*,
/// and `copy_row`'s tooltip and its toast both say "copied". Adding "open the
/// app" to it would put a `CopyShortcut::OpenApp` variant through all four and
/// raise "Application copied" over a launched process. It is also not a
/// sibling *table*: there is one of it, and a one-row table is a shape
/// pretending to be a set.
///
/// **The collision guard still covers it**, which is the thing that mattered:
/// `no_two_bindings_share_a_chord` now walks [`pane_chords`], which is
/// `COPY_SHORTCUTS` **plus this**, so a future binding cannot silently shadow
/// it or be shadowed by it.
///
/// `O` for *open*, and free -- checked over the whole crate rather than
/// assumed: `Key::O` appears in no other expression in it. Shifted, because
/// plain CTRL+O is the universal "open a file" and this opens a program;
/// shifted chords are safe here only because [`consume_chord`] matches
/// modifiers exactly (see [`COPY_SHORTCUTS`]'s own note, where CTRL+SHIFT+U
/// versus CTRL+U turns on the same fact).
const OPEN_APP_CHORD: (egui::Modifiers, egui::Key, &str) = (
    egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT),
    egui::Key::O,
    "CTRL+SHIFT+O",
);

/// Every chord this pane consumes, named, so the collision guard sees all of
/// them and not only the copies. See [`OPEN_APP_CHORD`].
#[cfg(test)]
fn pane_chords() -> Vec<(&'static str, egui::Modifiers, egui::Key, &'static str)> {
    let mut all: Vec<(&'static str, egui::Modifiers, egui::Key, &'static str)> = COPY_SHORTCUTS
        .iter()
        .map(|(which, modifiers, key, chord)| match which {
            CopyShortcut::Username => ("copy Username", *modifiers, *key, *chord),
            CopyShortcut::Password => ("copy Password", *modifiers, *key, *chord),
            CopyShortcut::Totp => ("copy One-time code", *modifiers, *key, *chord),
            CopyShortcut::Url => ("copy Website", *modifiers, *key, *chord),
        })
        .collect();
    all.push(("open the matched app", OPEN_APP_CHORD.0, OPEN_APP_CHORD.1, OPEN_APP_CHORD.2));
    all
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
            forget_copy_toast_in(data);
            data.insert_temp(copy_toast_item_id(), item.to_string());
        }
    });
}

/// Drops any live confirmation **and** the record of which item it belonged
/// to, unconditionally.
///
/// The other route out of a confirmation. [`forget_copy_toast_on_item_change`]
/// covers the pane being handed a different item, which is the only look-away
/// the read pane itself can see -- and for a long time it was the only one
/// wired up at all, which left two routes that the same reasoning covers wide
/// open:
///
///  * copy a password, click **Edit**, cancel back inside the five seconds --
///    `draw_detail_edit` never called this, so the read pane resumed with the
///    recorded id still equal to the item's and the toast still in the map,
///    and the confirmation came back;
///  * deselect the item and reselect it, same window, same result.
///
/// Neither crosses items, so neither breaks the "the toast belongs to the
/// item" rule that [`forget_copy_toast_on_item_change`]'s doc states -- but
/// both are exactly the RESURRECTION that doc sets out to make impossible,
/// reached by a door it does not watch.
///
/// **Clear, rather than merely decline to resurrect.** "Don't resurrect" means
/// leaving the toast in the map and suppressing it, which is the "hidden
/// rather than cleared" state the cross-item test already refuses -- it makes
/// a live toast's visibility depend on where the user has been rather than on
/// what is on screen, and every new door then needs its own suppression. One
/// primitive that destroys the toast makes "you left the item", "you opened
/// the editor", "you deselected it" and "it expired" the same end state, and
/// that is the whole rule.
///
/// The item-id record goes too, and not just the toast: leaving a stale id
/// behind would make the next `forget_copy_toast_on_item_change` for that same
/// item a no-op, which is harmless today only because there is nothing left to
/// drop. Removing both leaves no state at all, which is the state a fresh
/// context is in.
pub(crate) fn forget_copy_toast(ctx: &egui::Context) {
    ctx.data_mut(forget_copy_toast_in);
}

/// The one place a confirmation is destroyed, so the two callers above cannot
/// drift into forgetting different halves of it.
fn forget_copy_toast_in(data: &mut egui::util::IdTypeMap) {
    data.remove::<CopyToast>(copy_toast_id());
    data.remove::<String>(copy_toast_item_id());
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

/// **Whether a [`masked_row`] draws AT ALL.**
///
/// The user's words: "if password is empty - show no mask (and no field at
/// all)". A mask is a claim that there is a secret behind it, and ten bullets
/// over `""` made every credential-less login look like it held a password
/// this build had merely declined to show. [`row_offers_copy`] already
/// withdrew the *copy* from such a row; it kept the label, the bullets and the
/// reveal eye, which is an eye offering to reveal nothing.
///
/// **The row is SKIPPED, not painted invisibly.** A transparent or zero-height
/// row is still a row: it still takes vertical space, it still sits between
/// two hairlines, and it still answers `rect_of` in a test. So this predicate
/// is consulted by the CALLERS too -- to decide whether the separator above
/// the row is owed at all -- and `masked_row` returns before it lays anything
/// out. Pinned by `an_empty_password_draws_no_row_at_all` (the label is absent
/// from the painted frame and the card is shorter by the row's height) and by
/// its positive control `a_non_empty_password_still_draws_its_whole_row`.
///
/// **Where the skip actually happens, stated exactly, because a commit
/// message once overstated it.** Every one of the four production calls to
/// [`masked_row`] is already gated on emptiness BEFORE the call:
///
/// * the login password and each previous password, on this predicate itself
///   (`password_row`'s caller and `previous_passwords`);
/// * a card's `Number` and `Security code`, and an SSH key's `Private key`,
///   because those three arrive as `Option`s their field structs have already
///   trimmed and filtered, so an empty one is `None` and the call is not made.
///
/// So the early return inside `masked_row` is **defence in depth and not a
/// repair of a live path**: nothing in production reaches it, and the test
/// that covers it calls `masked_row` directly. It is kept because the
/// function is public to this module and a fifth caller must not have to
/// rediscover the rule -- not because a row was ever observed getting past
/// it. `64192c7`'s subject reads as a repair and `235e1f1`'s "and the same
/// for every `masked_row` caller" reads as four separate gates; the truth is
/// one gate on this predicate, two structural, and one guard nobody calls.
///
/// Deliberately the same rule as [`row_offers_copy`] rather than a second
/// spelling of it: "nothing to copy" and "nothing to show" are the same fact
/// about a secret, and two predicates could drift into a row that copies and
/// is not drawn.
fn masked_row_visible(value: &str) -> bool {
    row_offers_copy(value)
}

/// The `otpauth` scheme, with its `//`, as [`totp_key_of`] matches it.
///
/// Matched with the `//` attached on purpose: `otpauth:` alone would accept
/// `otpauthx://...`, and a bare `otpauth` would fire on any stored value that
/// merely *contains* the word (`my-otpauth-backup-seed` is a perfectly
/// ordinary thing for a user to have typed into the field).
const OTPAUTH_SCHEME: &str = "otpauth://";

/// The query parameter that carries the seed, matched as a whole NAME.
const SECRET_PARAM: &str = "secret";

/// **The bare key behind whatever the vault stored in `LoginData::totp`.**
///
/// The user's words: "should drop and show just the key and not
/// `otpauth://totp/Offline%20one-time%20password?secret=`". Bitwarden stores
/// the field verbatim, so it holds whichever of two shapes the user pasted:
/// a bare base32 seed, or a full `otpauth://` URI. `9df8c3d`'s masked row and
/// its copy action were handed the stored string as-is, so for the second
/// shape the row showed the scheme, the label and the whole query string, and
/// the clipboard got the same.
///
/// **This is the seam.** The parsing lives here and not at the call site so
/// that the shapes can be enumerated by a unit test rather than inferred from
/// a rendered frame -- and so that the row and the clipboard cannot disagree,
/// which they would if either one re-derived the value for itself.
///
/// The rules, each one a row of `the_stored_value_reduces_to_its_bare_key`:
///
/// * **Not a URI: passed through, trimmed.** A bare base32 seed is the other
///   common shape and must survive untouched. A value that merely contains
///   the word `otpauth` is not a URI (see [`OTPAUTH_SCHEME`]).
/// * **The scheme match is case-insensitive**, `OTPAUTH://` included.
/// * **The parameter NAME is matched whole and case-insensitively**, so
///   `SECRET=` is the seed and `issuer=`, `algorithm=`, `digits=`, `period=`
///   and -- the one that a substring search gets wrong -- `issuersecret=` are
///   not.
/// * **First `secret=` wins** if the URI repeats it. There is no second seed
///   to choose between, and picking the last would mean a trailing
///   `&secret=` typo silently replacing a good key with nothing.
/// * **The value is percent-decoded.** The user's own example has the label
///   encoded and the seed can be too. `+` is left ALONE -- `otpauth` query
///   values are percent-encoded, not form-encoded, so a `+` is a literal `+`
///   and turning it into a space would corrupt a key that contains one.
/// * **A malformed `%` escape is left literal.** `%ZZ`, or a `%` in the last
///   two bytes, is copied through byte-for-byte rather than dropped or
///   treated as a parse failure: a key the user can see and fix beats a row
///   that vanished. Likewise a decode that is not valid UTF-8 falls back to
///   the raw parameter value.
/// * **A URI with no `secret=` at all, or with an empty one, yields the empty
///   string** -- and so does a URI with no query at all. That is deliberately
///   the same "nothing to show" that an absent seed produces, so
///   [`masked_row_visible`] hides the row entirely rather than offering an
///   eye that reveals nothing. A URI is not a key. The empty stored value
///   takes the pass-through arm and lands on `""` as well.
/// * **The fragment is cut off FIRST, and it is cut off at both ends of the
///   query.** A `?` that sits inside a `#fragment` is not the query's `?`,
///   so `#frag?secret=NOPE` yields nothing; and a `#frag` that follows a
///   real query is not part of the seed, so `?secret=GOOD22#frag` yields
///   `GOOD22` and not `GOOD22#frag`. Both directions are rows of the table.
///
/// **What this does NOT touch: the derived code.** `bw serve` is given
/// whatever is stored, URI and all, and generates the six digits from it.
/// That path is untouched -- this function is only ever asked about the row
/// the user reads and the value the user copies.
///
/// **Why `Zeroizing<String>` and not `String`.** The return value is a real
/// copy of the seed -- there is no borrowing a decoded key out of the stored
/// URI -- and the crate's `#[global_allocator]` probe scans blocks on their
/// way back to the allocator, so a plain `String` here would be the seed in
/// the clear at every free. Every intermediate in the decoder is `Zeroizing`
/// for the same reason, and
/// `the_seed_inside_a_uri_never_reaches_the_allocator_in_the_clear` asserts
/// it against the probe with its control first.
///
/// This is *not* a claim that the key is never plaintext anywhere: `masked_row`
/// still materialises a plain `String` for the run it hands egui once the row
/// is REVEALED (egui's galley cache holds that text past the frame anyway),
/// and `copy_text` takes an owned `String`, so the clipboard hand-off in
/// `vault_window::mod` is a plain copy too. Both are pre-existing trades,
/// stated rather than implied.
pub fn totp_key_of(stored: &str) -> Zeroizing<String> {
    let trimmed = stored.trim();
    let Some(after_scheme) = strip_prefix_ascii_case(trimmed, OTPAUTH_SCHEME) else {
        return Zeroizing::new(trimmed.to_string());
    };
    // The fragment first: `?` inside a `#fragment` is not the query's `?`.
    let no_fragment = after_scheme.split('#').next().unwrap_or("");
    let Some((_label, query)) = no_fragment.split_once('?') else {
        return Zeroizing::new(String::new());
    };
    for pair in query.split('&') {
        // A parameter with no `=` cannot be the seed; skipped rather than
        // treated as an empty `secret`, so a stray `&secret&` does not
        // shadow a real one further along.
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        if name.eq_ignore_ascii_case(SECRET_PARAM) {
            return trimmed_key(percent_decoded(value));
        }
    }
    Zeroizing::new(String::new())
}

/// **Exactly what [`DetailAction::CopyTotpSecret`] puts on the clipboard**,
/// or `None` for the items that put nothing there.
///
/// The whole of that arm's body in `vault_window::mod`, lifted here so it can
/// be called by a test: the clipboard itself lives behind `egui::Context` in
/// a closure no test in this crate renders as far as, so an assertion phrased
/// against a painted frame would have been an assertion about the row and not
/// about the copy -- which is precisely the pair this change exists to keep
/// in step. `the_clipboard_gets_the_key_not_the_uri` calls this.
///
/// `None` rather than `Some("")` for an item with no login, no seed, or a URI
/// with no usable `secret=`: those draw no row and so offer no copy, and an
/// empty `copy_text` would silently clear whatever the user had.
pub fn totp_secret_clipboard_text(item: &VaultItem) -> Option<Zeroizing<String>> {
    let stored = item.login.as_ref().and_then(|l| l.totp.as_ref())?;
    let key = totp_key_of(stored.as_str());
    (!key.is_empty()).then_some(key)
}

/// `str::strip_prefix`, case-insensitively over ASCII.
///
/// Guards the char boundary as well as the length so a multi-byte first
/// character cannot panic the slice -- `stored` is whatever the user typed.
fn strip_prefix_ascii_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len()
        && s.is_char_boundary(prefix.len())
        && s[..prefix.len()].eq_ignore_ascii_case(prefix)
    {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// Percent-decoding for one query-parameter value, keeping the plaintext in
/// `Zeroizing` buffers throughout. See [`totp_key_of`] for the `+`, the
/// malformed-escape and the non-UTF-8 rules.
fn percent_decoded(raw: &str) -> Zeroizing<String> {
    // The overwhelmingly common case -- a base32 seed has no `%` -- and it
    // avoids the byte buffer entirely.
    if !raw.contains('%') {
        return Zeroizing::new(raw.to_string());
    }
    let bytes = raw.as_bytes();
    // **The capacity is a REQUIREMENT, not a nicety.** A `Vec` that grows
    // frees its old block, and the freed block holds a partial plaintext
    // seed that `Zeroizing` never sees -- it wipes the buffer it owns at the
    // end, not the ones reallocation left behind. `bytes.len()` is enough
    // for every input because decoding only ever shortens: each `%HH` triple
    // becomes one byte and every other byte is copied one-for-one, so the
    // output length is always <= `bytes.len()` and this `Vec` never grows.
    // Any future decoder that can EXPAND its input must reserve for the
    // expansion here, or the probe guard
    // `the_seed_inside_a_uri_never_reaches_the_allocator_in_the_clear` is
    // being asked about a path that no longer holds.
    let mut out: Zeroizing<Vec<u8>> = Zeroizing::new(Vec::with_capacity(bytes.len()));
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = char::from(bytes[i + 1]).to_digit(16);
            let lo = char::from(bytes[i + 2]).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    // `String::from_utf8` would move the bytes out of the `Zeroizing`
    // wrapper; validating and copying keeps both buffers wiped.
    match std::str::from_utf8(&out) {
        Ok(decoded) => Zeroizing::new(decoded.to_string()),
        Err(_) => Zeroizing::new(raw.to_string()),
    }
}

/// Trims a decoded key without leaving an untrimmed plain copy behind, and
/// without reallocating in the usual case where there is nothing to trim.
fn trimmed_key(key: Zeroizing<String>) -> Zeroizing<String> {
    if key.trim().len() == key.len() {
        key
    } else {
        Zeroizing::new(key.trim().to_string())
    }
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
        chord_hint(ui, copy_shortcut_chord(which));
    }
}

/// One chord, painted on a row's control line. **The one place this pane draws
/// a key**, so the `MATCHED APP` card's open chord looks exactly like the copy
/// chords beside it rather than being a second treatment of the same idea --
/// which is what made it discoverable at all.
fn chord_hint(ui: &mut egui::Ui, chord: &str) {
    ui.label(
        RichText::new(chord)
            .size(CHORD_HINT_SIZE)
            .family(egui::FontFamily::Monospace)
            .color(theme::TEXT_GHOST),
    );
}

/// The size a chord hint is painted at, in one place so
/// [`chord_hint_width`] measures the run [`chord_hint`] will lay and not a
/// different one.
const CHORD_HINT_SIZE: f32 = 10.0;

/// How wide [`chord_hint`] will be.
///
/// **The `MATCHED APP` card has to know**, and nothing else on this pane does.
/// Every other row's value is short and leaves the control group room by
/// accident; the App row's value is an app's full name laid to an explicit
/// wrap width, and a value laid to the WHOLE value column leaves the control
/// group nothing -- which is precisely how `Remove` came to be drawn on top
/// of the card's notes. Measured through the same galley rather than
/// estimated, so the reservation and the drawing cannot disagree.
fn chord_hint_width(ui: &egui::Ui, chord: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(
            chord.to_string(),
            egui::FontId::new(CHORD_HINT_SIZE, egui::FontFamily::Monospace),
            theme::TEXT_GHOST,
        )
        .size()
        .x
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
}

/// **What the header gives up, in order, as the pane narrows -- and it is
/// never the title.**
///
/// The commit that made the controls claim their width first fixed a real
/// bug (the header's primary button, since removed, measured painting at
/// x = -34.5, entirely off a 298pt pane) and introduced its mirror image:
/// with the controls served first the
/// title got the remainder, the remainder at 298pt was 21.6pt, and egui
/// elided a 26-character name down to a lone "…" painted *inside the strip's
/// left padding, on top of the avatar*. A layout that fits by annihilating
/// its own subject is not a layout that fits, and the test of the day could
/// not see it: `Galley::text()` returns the job's SOURCE text, so every
/// painted-text assertion in this file reads the full name off a galley that
/// drew one ellipsis.
///
/// So the title is given a floor ([`TITLE_MIN`]) and the one thing left to
/// spend is **the single line**: below the width at which the controls and a
/// [`TITLE_MIN`]-wide title both fit beside the avatar, the controls move to
/// their own row under the title. The strip gets taller, which the design
/// does not draw -- but the design also does not draw a 298pt pane, and a
/// taller strip costs some body space while the alternative costs the item's
/// name.
///
/// **There used to be a rung above this one**: the header's primary button
/// carried a `CTRL+SHIFT+F` shortcut hint that was dropped first, being pure
/// redundancy. The button and its chord are both gone, so the ladder has one
/// rung and this function one decision. What has NOT changed is that the
/// stacked branch is still reached at the app's own minimum: the star and the
/// kebab plus their gap are 82pt, and at 298pt the strip has 250pt inside its
/// padding of which the avatar and its gap take 58, leaving 192 -- less than
/// the 82 + 14 + 120 one line needs.
///
/// Nothing is ever dropped, and no control is ever shrunk below its 34px hit
/// target.
///
/// Pure, and taking measured widths rather than measuring them, for this
/// file's standing reason: a decision reachable only from inside an eframe
/// closure is a decision that will not be tested.
fn header_layout(content_width: f32, controls: f32) -> HeaderLayout {
    // What is left of the strip's content box once the avatar and the gap
    // after it are taken -- the band the controls and the title share.
    let beside_avatar = content_width - HEADER_AVATAR - HEADER_GAP;
    // Stacked, when they do not both fit: the controls' own row is the full
    // content width rather than `beside_avatar`, so it has 58pt more to work
    // with than the one-line branch just rejected.
    HeaderLayout { stacked: controls + HEADER_GAP + TITLE_MIN > beside_avatar }
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
    // The window's one cache of "what is this executable really called, and
    // what does it look like" -- the SAME cache the edit form's app block
    // uses, held by `vault_window::mod`'s `run` and living as long as the
    // window. `&mut` because asking it about a path it has not seen is what
    // starts the (debounced, worker-thread) lookup; see `app_identity`.
    //
    // **Not a resolved name and icon passed in already**, which was the
    // smaller signature: the decision about WHICH path to resolve is
    // [`app_name_lookup_path`]'s and belongs on this side of the boundary
    // with the match it is about, and moving it into `vault_window::mod`
    // would put it where this file's tests cannot call it.
    apps: &mut crate::app_identity::AppIdentityCache,
    // `Settings::check_breaches`, as the window has it THIS frame -- the
    // preference is off by default and this pane is its first and only
    // reader. A bool rather than the whole `Settings` because that is the
    // entire question this pane is allowed to ask of it; see
    // [`should_check`], which is the only thing that consumes it.
    check_breaches: bool,
    // `Settings::reveal_totp_seed`, as the window has it THIS frame -- off
    // by default, and this pane is its first and only reader. A bool for the
    // same reason `check_breaches` is one: it is the entire question this
    // pane is allowed to ask of the settings.
    //
    // **Off means the row is NOT DRAWN**, not drawn empty and not drawn
    // disabled. See the call site below; the rule is the one the empty
    // password row and the unbound MATCHED APP card already follow.
    reveal_totp_seed: bool,
    // The window's one `BreachCache`, threaded through for the same reason
    // `apps` is: the answer comes off a worker thread, is keyed on the
    // password's five-character prefix, and has to outlive the frame that
    // asked for it. `&mut` because asking about a prefix it has not seen is
    // what starts the worker.
    //
    // **Passed even when `check_breaches` is false.** The gate is
    // [`should_check`] and it is read at exactly one place; a cache that is
    // sometimes absent would put a second gate in the signature, and two
    // gates is how "couldn't check" becomes "safe".
    breaches: &mut BreachCache,
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
    let login = item.login.as_ref();
    let username = login.and_then(|l| l.username.as_deref()).unwrap_or("");
    let password = login
        .and_then(|l| l.password.as_deref())
        .map(|p| p.as_str())
        .unwrap_or("");
    // **The TOTP seed, reduced to the bare key.** `LoginData::totp` is
    // `Option<Zeroizing<String>>` holding whatever the user pasted, which is
    // often a whole `otpauth://` URI; `totp_key_of` is the one place that
    // shape is understood, and it hands back a `Zeroizing<String>` so the
    // copy it has to make is not the seed in the clear. See its doc for the
    // rules and for what is deliberately NOT `Zeroizing` further down. The
    // `mod.rs` clipboard arm calls the same function on the same stored
    // value, so the row and the clipboard cannot disagree.
    //
    // `""` for an item with no seed -- and also for a URI that carries no
    // usable `secret=` -- which `masked_row_visible` then reads as "no row at
    // all", exactly as it does for an absent password.
    let totp_key = totp_key_of(
        login
            .and_then(|l| l.totp.as_deref())
            .map(|s| s.as_str())
            .unwrap_or(""),
    );
    let totp_secret = totp_key.as_str();
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

    // **The binding, derived up here beside `website` and for the same
    // reason**: CTRL+SHIFT+O is resolved before anything is drawn, and the
    // chord and the `MATCHED APP` card must not be able to disagree about
    // which app is bound. Both the parsed match and the raw presence of the
    // field, because they are three states and not two -- see
    // `app_card_body`.
    let app_match = crate::vault_bridge::extract_app_match(item);
    let app_field_present = crate::vault_bridge::has_app_match_field(item);
    // The app's real NAME and its icon, resolved once per path by the
    // window's cache -- off this thread, and never per frame. The path is
    // `app_name_lookup_path`'s choice, NOT `m.path`: a dead binding is
    // deliberately looked up under `""` so that nothing is probed and the raw
    // `process` is what comes back. The two arguments are deliberately
    // different fields and a test fixture in which they agree would not be
    // able to tell them apart.
    let (app_name, app_icon, app_pending) = {
        let (path, process) = match app_match.as_ref() {
            Some(m) => (app_name_lookup_path(m), m.process.as_str()),
            None => ("", ""),
        };
        let label = apps.label(ui.ctx(), path, process);
        (label.name.to_string(), label.icon.cloned(), label.pending)
    };
    if app_pending {
        // A channel is not input, and egui does not repaint for one. Same
        // cadence the edit form's app block asks for.
        ui.ctx()
            .request_repaint_after(crate::app_identity::AppIdentityCache::POLL_INTERVAL);
    }

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
    // The one non-copy chord, taken out of the queue in the same pass and for
    // the same reason. Applied at the very end, after the copies and after
    // any click, so a deliberate act on a specific control always wins. What
    // it resolves to is `app_name_open_action`'s decision -- the same one the
    // name-as-link and the footer's `Open` obey -- so a chord cannot start
    // something those two refuse.
    let open_app_chord =
        ui.input_mut(|i| consume_chord(i, OPEN_APP_CHORD.0, OPEN_APP_CHORD.1));

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
            // measurement said.** Drift between the room the strip reserves
            // and the room its controls take is the failure mode that put a
            // control off the edge of the pane once already.
            //
            // The star and the kebab, square at the strip's own control
            // height, plus the one gap between them -- the whole of what this
            // strip now draws. There is no per-item term any more: the worded
            // primary button that used to contribute one has been removed.
            let controls_width = theme::HEADER_BUTTON_HEIGHT * 2.0 + HEADER_GAP;
            let layout = header_layout(content_width, controls_width);

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
                // **The strip's worded "Fill in app" button used to sit
                // here, and was REMOVED at the user's request**: "Fill in
                // app button I think we can remove -- I'm not sure what it
                // does". It had become the second of two adjacent controls
                // acting on the matched app -- the other one, the app's
                // name as a link, opens it -- and one of the two typing the
                // user's credentials while the other launched a program was
                // not a distinction the strip made.
                //
                // **Every manual trigger has since gone.** The row context
                // menu's "Fill in app" entry followed this button out
                // (`item_list::menu_entries`), and `fill_target`,
                // `fill_item_into_app` and `app::find_window_for_process`
                // were deleted with them. What remains is autofill on
                // focus, the prompt overlay and the global hotkey -- all
                // three of which take their hwnd straight off a
                // `window_watch::ForegroundEvent` and never resolved one
                // through that function in the first place.
                //
                // **The host-process refusal lives in `match_engine`, not
                // on this path.** `MatchEngine::lookup` answers a
                // host-owned window from the title table and never reads a
                // stored process name, and `picker_ui::host_process_refusal`
                // stops such a match being saved at all. No surviving fill
                // path can reach a credential without passing them.
                //
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
    // `pane.right() - BODY_PAD_X` as they always did, and the bar takes the
    // OUTERMOST `theme::SCROLLBAR_WIDTH` of that padding -- flush to the pane's
    // edge, where the platform's own bars sit, so every point of the lane's
    // slack falls between the bar and the cards (18pt of the 24) rather than
    // half of it behind the bar (9pt and 9pt, which is what this pane shipped).
    // Same arrangement, and the same reason, as `item_list.rs`'s list; the
    // helper's doc comment is where that rule is argued.
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
                // **The hairline is owed only if the row below it is drawn.**
                // An empty password draws no row at all (see
                // `masked_row_visible`), and a rule left standing over the
                // gap would be a separator with nothing on one side of it --
                // exactly the "leftover separator where the row used to be"
                // this change exists to avoid. The Username row above is
                // unconditional, so this rule is always correctly placed when
                // it is drawn at all.
                if masked_row_visible(password) {
                    theme::row_rule(ui);
                    password_row(ui, password, &mut reveal.password, &mut action);
                }
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
                // **The seed, under the code, and only when BOTH are true.**
                // The preference is off unless the user turned it on, and the
                // item has to actually carry a seed. Either one false and
                // nothing is laid out at all -- no band, no hairline, no eye,
                // no zero-height placeholder and no alpha-0 ghost. Drawing it
                // invisible is not the same as not drawing it: an invisible
                // row still takes vertical space, still sits between two
                // hairlines and still answers `rect_of`, which is the exact
                // defect `masked_row_visible`'s doc was written about.
                //
                // The hairline is inside the guard for the same reason the
                // password row's is: a rule left standing over the gap is a
                // separator with nothing on one side of it.
                //
                // **This is a fifth production caller of `masked_row`, and it
                // gates the CALL on `masked_row_visible` rather than relying
                // on that function's own early return** -- which its doc
                // states is defence in depth and production-unreachable, and
                // this row does not change that.
                if reveal_totp_seed && masked_row_visible(totp_secret) {
                    theme::row_rule(ui);
                    masked_row(
                        ui,
                        TOTP_SECRET_LABEL,
                        totp_secret,
                        // Per-view, not sticky: this flag is cleared by the
                        // selection-change reset in `vault_window::mod`'s
                        // `run`, and there is nothing else on this path that
                        // remembers a reveal. See `RevealState::totp_secret`.
                        &mut reveal.totp_secret,
                        &mut action,
                        DetailAction::CopyTotpSecret,
                        // No chord. `CTRL+ALT+T` copies the CODE
                        // (`CopyShortcut::Totp`), and a second, near-identical
                        // chord for a value that is not the one the user just
                        // read off the screen is how a seed ends up on the
                        // clipboard by accident.
                        None,
                    );
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
    // **Any row that will actually be DRAWN**, not any entry: an entry whose
    // password is empty draws nothing (see `masked_row_visible`), and a
    // history of nothing but those would have put a PREVIOUS PASSWORDS
    // heading over an empty box -- the same "heading over no rows" this gate
    // already refuses for an absent history.
    if history
        .iter()
        .any(|entry| masked_row_visible(entry.password.as_str()))
    {
        card(ui, "PREVIOUS PASSWORDS", |ui| {
            history_rows(ui, &history, reveal, &mut action);
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
                RowShape::Columns,
            );
            if opened {
                action = DetailAction::OpenWebsite(website.to_string());
            }
        });
        ui.add_space(CARD_GAP);
    }

    // **Directly under AUTOFILL TARGETS, and NOT inside it** -- see
    // `APP_CARD_HEADING`. The cards above are the item's own contents, and
    // this one is about what Deskwarden does with them. NOTES follows it (see
    // below); the metadata strip closes the pane.
    //
    // `app_match` and `app_field_present` are derived at the top of this
    // function, beside `website` -- **the FIELD as well as the parsed match**,
    // because a field that will not parse is a binding the user can see in
    // every other Bitwarden client and must be able to clear from here, and
    // asking `app_match.is_some()` filed it as "no field at all" and hid the
    // card outright on a non-fillable kind. See `app_card_body`.
    if app_card_visible(app_field_present) {
        card(ui, APP_CARD_HEADING, |ui| {
            app_match_card(
                ui,
                app_match.as_ref(),
                app_field_present,
                website,
                ResolvedApp { name: &app_name, icon: app_icon.as_ref() },
                &mut action,
            );
        });
        ui.add_space(CARD_GAP);
    }

    // **ALWAYS THE LAST CARD.** The user asked for it in those words, and
    // "last" is asserted by PAINTED VERTICAL POSITION -- every other headed
    // card on the pane ends above this one's top edge -- rather than by where
    // this call sits in the source, by
    // `notes_is_the_last_card_for_every_kind_that_shows_it`.
    //
    // It sits here, after MATCHED APP and before the metadata strip, because
    // the strip is not a card: it has no heading, it is drawn as a bare
    // `egui::Frame` rather than through `card`, and it is the pane's footer
    // -- "updated N days ago", the strength word, the breach badge. Putting a
    // note under it would move the footer into the middle of the item's
    // contents, which is the one part of this the brief asked to leave alone.
    //
    // `DetailBody::NotesOnly` (a secure note) reaches this same call and no
    // other: a secure note draws no body card of its own, so its NOTES card
    // is last by the same rule as every other kind rather than by accident.
    if let Some(notes) = notes_text(item) {
        card(ui, "NOTES", |ui| {
            notes_body(ui, &item.id, notes, &mut action);
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
    // **Before the frame, not inside it.** `breach_segment` needs `&mut` the
    // cache and the closure below already borrows `ui`; asking here also
    // keeps the one call that can start a worker out of a paint callback.
    let segment = {
        let ctx = ui.ctx().clone();
        breach_segment(&ctx, check_breaches, kind, password, breaches)
    };
    egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0, theme::HAIRLINE))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let strip = RichText::new(metadata_line_for(kind, updated_days_ago, fill_count, password))
                .size(ROW_LABEL_SIZE)
                .color(theme::TEXT_FAINT);
            match segment {
                // **The untouched path**, and deliberately the same call this
                // card made before the badge existed rather than the pair
                // below with one side empty: with the preference off the
                // strip must be byte-identical AND pixel-identical, and the
                // only way to be sure of that is to run the same code.
                None => card_text(ui, strip),
                Some((text, color)) => card_text_pair(
                    ui,
                    strip,
                    // The leading space is the separator: `card_text_pair`
                    // zeroes the layout's item spacing so the two runs read
                    // as one sentence, and `strip_segment` owns the "\u{b7} "
                    // that follows it exactly as `metadata_line` owns the two
                    // before it.
                    RichText::new(format!(" {text}"))
                        .size(ROW_LABEL_SIZE)
                        .color(color),
                ),
            }
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
    // **After the copies and after every click**, so a deliberate act on a
    // specific control wins over a keystroke -- the ordering the copy chords
    // already have. No toast: this starts a program, and the program
    // appearing is the confirmation. `vault_window::mod` raises the
    // confirmation dialog whenever the plan carries vault `args`, and reports
    // a failure band when Windows refuses -- all of which this inherits by
    // reporting the very same `DetailAction::OpenApp` the footer's `Open`
    // does. There is still exactly one `launch_app` call in this program.
    if matches!(action, DetailAction::None) && open_app_chord {
        if let Some(m) = app_match.as_ref() {
            if let Some(open) = app_name_open_action(m, website) {
                action = open;
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

/// What the card says under the rows about a live binding: what focusing this
/// app does.
///
/// **One sentence, and it replaces the three [`trigger_caption`]s**, which
/// were the footer for the three pills this card no longer draws. Autofill is
/// one global preference now, not a per-item choice, so a caption naming *this
/// item's* stored mode was about to become the card's only remaining claim
/// that the mode still decides something -- and for `Auto` ("Fill immediately
/// when this app is focused.") a claim about behaviour that has been retired.
///
/// **Chosen to be true with the preference in EITHER state**, because this
/// pane does not read `settings.json` and inventing a second reader of it for
/// one sentence would be a second source of truth about what autofill does.
/// The hotkey arms for every match either way, so naming it is always
/// accurate; the prompt is described in Preferences, where it is set.
///
/// **Kept to two lines at the app's narrowest pane**, because this is a
/// wrapped footer line and every extra line is card height: the first draft
/// ran to four and pushed `MATCHED APP` itself off the top of a fully-scrolled
/// 298pt pane, which is the third time this card has been made unreachable.
/// `the_matched_app_card_is_reachable_on_the_shortest_window` is what caught
/// it and is what any re-wording has to answer to.
const APP_MATCH_BEHAVIOUR_NOTE: &str =
    "Press CTRL+ALT+B while this app is focused to fill it. See Prompt on match in Preferences.";

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

/// Which path this card hands [`AppIdentityCache::label`] to find the app's
/// real name and icon -- or `""`, meaning **do not look it up at all**.
///
/// The user's report: *"Matched app should show normal name with icon and not
/// mabl.exe"*. The edit form has done this since `4b05adb`; this is the same
/// cache, asked from the read pane, and it is asked through a function rather
/// than by passing `&m.path` at the call site so that the one case that must
/// NOT be dressed up is a decision a test can call.
///
/// **That case is a dead binding** ([`app_match_is_dead`]). Its `process` is a
/// host name -- `ApplicationFrameHost.exe` -- and its `path`, when one was
/// recorded, is the real `C:\Windows\System32\ApplicationFrameHost.exe`, which
/// exists, carries a `FileDescription` of *Application Frame Host*, and has a
/// perfectly good shell icon. Resolving it would paint a Windows internal's
/// name and icon directly above [`APP_MATCH_DEAD_NOTICE`] -- *Deskwarden is
/// ignoring this match, so it never fires* -- which is a broken binding
/// wearing the costume of a working one. The raw `process` is what the user
/// needs to see: it is the evidence of what went wrong, and it is the string
/// the notice's advice ("pick the app again, or Remove") is about.
///
/// `""` rather than a second `if` at the draw site, because
/// [`AppIdentityCache::label`] already answers an empty path from `process`
/// alone -- **no thread, no `fs::metadata`, no `SHGetFileInfoW`, no icon**.
/// One rule, expressed by choosing the input rather than by branching around
/// the output.
///
/// Every other case goes to the cache and is handled there, deliberately:
///
///  * a **Microsoft Store** match needs nothing special. Its `process` is the
///    packaged app's own exe (`Speedtest.exe`, not the frame host) and its
///    `path` is under `WindowsApps`, so the cache either reads the name and
///    icon or -- far more often, those directories being ACL'd -- fails to
///    open the file and degrades to the file name, which is the exe name this
///    row used to print. The word *hosted* still never reaches the screen:
///    [`APP_HOSTED_NOTE`] is what explains this match, unchanged.
///  * a **path that no longer exists** -- uninstalled, moved, or on a share
///    that is down -- degrades the same way: `fs::metadata` fails on the
///    worker, so the name falls back to the file name, no icon is fetched at
///    all, and no dialog is raised. The `Program file` row below is still
///    showing the full path, which is the thing the user has to act on.
fn app_name_lookup_path<'a>(m: &'a AppMatch) -> &'a str {
    if app_match_is_dead(m) {
        return "";
    }
    &m.path
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

/// Whether the READ pane draws a [`APP_CARD_HEADING`] card at all.
///
/// **A binding, or nothing.** The user's words: "Matched app - same, no show
/// if not present, only if edit or add window called". An item with no app
/// field gets no card, no heading and no placeholder tile -- not an invisible
/// one, none: the `if` below the call site skips the `card` call outright, so
/// the cards after it move up by the whole card and the pane gets shorter.
/// Pinned by `an_item_with_no_app_binding_draws_no_matched_app_card`, which
/// asserts the heading is absent AND that the pane shrank by the card's
/// height, and by its positive control
/// `an_item_with_an_app_binding_still_draws_it`.
///
/// This replaces a `has_field || kind_offers_fill(kind)`, which put an
/// "unbound" placeholder on every login whether or not one had ever been
/// bound. That card had a job -- it was the only thing in the app that said
/// app matching existed -- and the job has moved: **the edit and add forms
/// always offer the control**, because that is where a binding is created.
/// See `detail_edit.rs`, which is untouched by this change and is where that
/// promise is kept; `the_edit_form_offers_the_app_control_even_with_nothing_
/// bound` asserts it rather than assuming it.
///
/// **`has_field`, not "has a match this build could parse".** A secure note
/// whose app-match field was corrupted by hand elsewhere would otherwise have
/// the whole card suppressed -- an invisible binding the user cannot see or
/// remove, which is the exact defect this card was added to end. An
/// unreadable field IS a binding; only its text is lost.
fn app_card_visible(has_field: bool) -> bool {
    has_field
}

/// One row of the card: its label, the text in its value column, and whether
/// that text is the match's own value (so the row copies) or a placeholder
/// standing in for a value that was never recorded (so it does not).
#[derive(Debug, Clone, PartialEq, Eq)]
struct AppRow {
    label: &'static str,
    value: String,
    /// **What a click on the row puts on the clipboard, which is not always
    /// what the row shows.**
    ///
    /// Equal to `value` on every row but `App`, and the exception is the
    /// point of the field. That row now *displays* the app's real name --
    /// `Google Chrome` -- and it copies `chrome.exe`, because the clipboard
    /// is a thing the user is about to paste somewhere and the only place a
    /// matched app's identity is ever pasted (a shortcut, a script, a bug
    /// report, this app's own Program file box) wants the executable name.
    /// "Google Chrome" pastes into nothing.
    ///
    /// The exe name is therefore still on the card even though the row no
    /// longer prints it on its own line: the `Program file` row below is
    /// showing the full path, which ends in it.
    copy: String,
    /// `false` for a placeholder. It reaches [`copy_row`]'s `copyable`, so a
    /// row saying "Not recorded" is inert -- no tint, no hand, no tooltip, no
    /// toast -- for the same reason an empty Password row is (see
    /// [`row_offers_copy`]).
    ///
    /// Derived from [`Self::copy`] and not from `value`: a row that showed a
    /// resolved name and had nothing to copy would offer a tint, a hand and a
    /// toast over an empty clipboard, which is the exact defect
    /// [`row_offers_copy`] exists to refuse.
    real: bool,
    /// Whether this row is the one that carries the app's icon and its link
    /// -- true for `App` and nothing else. A flag rather than a `label ==
    /// "App"` test at the draw site, because a label is a word on the screen
    /// and this is a fact about the row.
    app: bool,
}

/// The placeholder for a `path` this match never captured -- every match saved
/// before the field existed, which is a shape still sitting in real vaults.
const APP_PATH_UNRECORDED: &str = "Not recorded";

/// The card's rows, in order, for a match that exists.
///
/// **The user asked for "name, path + keys", and this is that mapping made
/// explicit.** `process` is the name and `path` is the path. There is no third
/// row for `trigger`: the field is still stored, but nothing reads it and no
/// pane offers a choice about it (see [`TRIGGER_ORDER`]).
///
///  * **App** -- `name`: what the app is *called*, resolved from the
///    executable's version resource by [`AppIdentityCache`] and passed in.
///    The user's report was that this row said `mabl.exe` where the edit form
///    already said the app's real name. It still COPIES `process` (see
///    [`AppRow::copy`]) and it is still first: it is the app, and the thing
///    the match engine compares is the file name at the end of the path
///    directly below.
///  * **Window title** -- `title`, and ONLY when `hosted`. An unhosted title
///    is inert by design (see [`AppMatch::hosted`]): every one saved during
///    the one commit that recorded titles for every row is deliberately never
///    matched on, and drawing it here would tell the user it does something.
///  * **Program file** -- `path`, or [`APP_PATH_UNRECORDED`]. Shown as the
///    match stores it, NOT through `AppMatch::launchable_path`: that function
///    answers "is this safe to execute", this row answers "what did the picker
///    record", and showing nothing for a path that fails the launch check
///    would hide the very corruption a user needs to see in order to fix it.
fn app_match_rows(m: &AppMatch, name: &str) -> Vec<AppRow> {
    let mut rows = vec![AppRow {
        label: "App",
        // **`name`, not `m.process`** -- the whole of the user's report.
        // Resolved by `AppIdentityCache` off the path
        // [`app_name_lookup_path`] chose, and passed in rather than looked up
        // here because this function is pure and that lookup is file I/O on a
        // worker thread.
        value: name.to_string(),
        copy: m.process.clone(),
        real: row_offers_copy(&m.process),
        app: true,
    }];
    if m.hosted && !m.title.is_empty() {
        rows.push(AppRow {
            label: "Window title",
            value: m.title.clone(),
            copy: m.title.clone(),
            real: true,
            app: false,
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
        // The path itself, empty when there is none -- NOT the placeholder,
        // which is this pane's own word and must never be what a click puts
        // on the clipboard. Keeping it here is what lets `real` be one
        // expression over `copy` on every row.
        copy: m.path.clone(),
        real: recorded,
        app: false,
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
/// Chromium release adds some) and gives false assurance. What this crate
/// ships instead is consent on the exact string: [`command_line`] renders the
/// whole command line, the Open control puts it in its tooltip, and --
/// **because a tooltip requires hovering, and with two menu entries it
/// requires opening the menu first** -- `vault_window::mod` also refuses to
/// start a plan whose [`has_raw_args`](LaunchPlan::has_raw_args) is set until
/// the user has been shown that same string and clicked through it. A plan
/// with no stored arguments is not confirmed, so the prompt appears on the
/// launches that carry vault data onto a command line and on no others, which
/// is what keeps it from becoming a thing to click past.
///
/// Anything stronger -- refusing arguments outright on items shared into the
/// vault by an organisation, which is the actual vector -- is a product
/// decision about what Deskwarden will refuse to run, and needs
/// item-ownership data this crate does not carry to the launch site.
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
    /// Whether any part of `raw_tail` is `AppMatch::args` -- vault data on a
    /// command line, verbatim.
    ///
    /// **Not `!raw_tail.is_empty()`, and the difference is the whole point.**
    /// A tail can be non-empty with no stored arguments at all: an ordinary
    /// login with a website and a bare app binding produces a tail that is
    /// only the item's URL, and that URL went through [`quote_arg`] on its way
    /// there, so it is one positional argument and cannot become a flag. The
    /// dangerous half is `args`, which is passed through untouched (see this
    /// struct's doc) and can therefore be `--gpu-launcher=...`.
    ///
    /// `vault_window::mod` is what reads this: a plan with it set is confirmed
    /// against its own command line before anything is started, and a plan
    /// without it launches on the click, as every launch did before. Recorded
    /// here rather than re-derived from `raw_tail` at the launch site because
    /// the tail is already joined by then and no longer says which half is
    /// which -- and a launch gate that guessed would be a second, weaker copy
    /// of [`launch_tail`]'s rule.
    pub has_raw_args: bool,
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
        // `trim`, and the SAME trim `launch_tail` applies: a stored `args` of
        // "   " contributes nothing to the tail, so calling it "this command
        // line carries vault arguments" would put a confirmation in front of a
        // launch whose command line is the program and the URL and nothing
        // else.
        has_raw_args: !m.args.trim().is_empty(),
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

/// What clicking the App row's NAME does, and what [`OPEN_APP_CHORD`] does --
/// or `None`, meaning the name is **plain text, not a link**.
///
/// The user asked for the matched app's name to "be clickable like link and
/// have Ctrl + shortcut". This is that, and it is one expression so the link,
/// the chord and the footer's `Open` button cannot end up meaning three
/// different things.
///
/// **It goes through [`app_launch_plan`], which is the only constructor of a
/// [`LaunchPlan`] and the only gate.** Nothing here re-decides what may be
/// started: the link inherits, for free and unavoidably, that function's
/// refusal of a dead binding and of a Store app, the `launchable_path` shape
/// check, the confirmation dialog `vault_window::mod` raises whenever the plan
/// carries vault `args`, and the failure band that tells "moved or
/// uninstalled" apart from "Windows refused". There is exactly one
/// `launch_app` call in this program (`ea4028f`, with a wiring test that
/// counts it); this adds a second *way to ask*, not a second way to launch.
///
/// **`None` makes the name inert rather than a link that does nothing.** A
/// dead binding, a Store app, a match with no recorded `path` and a path
/// `launchable_path` refuses all reach here, and every one of them already has
/// its own sentence on the card ([`app_card_notes`], via [`app_open_refusal`])
/// saying why there is no Open. A blue, hand-cursored name beside that
/// sentence would be a fourth promise the card has just finished withdrawing.
fn app_name_open_action(m: &AppMatch, website: &str) -> Option<DetailAction> {
    app_launch_plan(m, website).map(DetailAction::OpenApp)
}

/// The website entry's label. A constant so the source pin and the draw site
/// cannot drift.
const OPEN_WEBSITE_LABEL: &str = "Open website";
/// The menu button's own label, when there are two choices behind it.
const OPEN_MENU_LABEL: &str = "Open";
const OPEN_MENU_HOVER: &str = "Open this item\u{2019}s app or its website";

/// The card's footer lines, under the rows: what focusing this app does, and
/// -- for a Store app -- why this match is keyed on a title.
///
/// **A dead match gets [`APP_MATCH_DEAD_NOTICE`] and NOTHING else.** See that
/// constant: [`APP_MATCH_BEHAVIOUR_NOTE`] is a claim about what focusing the
/// app does, and this binding does nothing at all. The hosted note goes with
/// it, because "matched by its window title" is exactly what a dead match
/// failed to be.
fn app_card_notes(m: &AppMatch) -> Vec<&'static str> {
    if app_match_is_dead(m) {
        return vec![APP_MATCH_DEAD_NOTICE];
    }
    let mut notes = vec![APP_MATCH_BEHAVIOUR_NOTE];
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

/// The three trigger modes, in the order they were offered, and the words the
/// three retired controls painted.
///
/// **Test-only, and retired.** `d5d5d64` took the read-only MATCHED APP card's
/// pills off; this pass took the edit form's and the app picker's off with
/// them. What a matched foreground window does is one global preference
/// ([`crate::settings::Settings::prompt_on_match`]) and nothing in this build
/// reads [`crate::app_match::AppMatch::trigger`], so a per-item control was a
/// choice that wrote a field nothing reads: persisted, superseding the item's
/// `revisionDate` to record it, and changing nothing observable.
///
/// These survive as FIXTURES rather than as production code, because the pins
/// that keep the controls gone have to name the exact words a reinstated one
/// would paint. Defined here once, so the read pane's pins and the edit form's
/// cannot drift on to different words and both pass against a live control.
///
/// The FIELD is a different question and is deliberately kept and preserved --
/// see [`crate::app_match::AppMatch::trigger`].
#[cfg(test)]
pub(crate) const TRIGGER_ORDER: [TriggerMode; 3] =
    [TriggerMode::Prompt, TriggerMode::Hotkey, TriggerMode::Auto];

/// A retired trigger pill's label. Exhaustive with no catch-all: a fourth
/// [`TriggerMode`] must be a compile error here rather than silently
/// inheriting a neighbour's name -- i.e. a mode no pin names cannot exist.
#[cfg(test)]
pub(crate) fn trigger_label(mode: TriggerMode) -> &'static str {
    match mode {
        TriggerMode::Prompt => "Prompt",
        TriggerMode::Hotkey => "Hotkey",
        TriggerMode::Auto => "Auto",
    }
}

/// The sentence a retired control drew under its pills, saying what the
/// selected mode did. Exhaustive for [`trigger_label`]'s reason.
#[cfg(test)]
pub(crate) fn trigger_caption(mode: TriggerMode) -> &'static str {
    match mode {
        TriggerMode::Prompt => "Show the overlay when this app is focused.",
        TriggerMode::Hotkey => "Fill only when the fill hotkey is pressed.",
        TriggerMode::Auto => "Fill immediately when this app is focused.",
    }
}

/// The card's body: the rows, the notes and Remove -- or, for an item bound to
/// nothing, one sentence saying so.
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
    // What the matched app is really CALLED, and what it looks like --
    // resolved by `vault_window::mod`'s `AppIdentityCache` off the path
    // [`app_name_lookup_path`] chose, on a worker thread, once per path. See
    // [`ResolvedApp`].
    app: ResolvedApp<'_>,
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
    // Resolved once for the row and for the chord, so a name that is a link
    // and a chord that does nothing cannot end up on the same card.
    let open = app_name_open_action(m, website);
    for (index, app_row) in app_match_rows(m, app.name).iter().enumerate() {
        if index > 0 {
            theme::row_rule(ui);
        }
        if app_row.app {
            app_name_row(ui, app_row, app.icon, open.as_ref(), action);
        } else if app_row.real {
            app_value_row(ui, app_row.label, &app_row.value, &app_row.copy, action);
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

    theme::row_rule(ui);
    app_card_footer(ui, &app_card_notes(m), &app_open_choices(m, website), action);
}

/// The word on the card's one destructive control, in one place, so the
/// button and every test that names it read one string.
const APP_REMOVE_LABEL: &str = "Remove";

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
/// What the `MATCHED APP` card was told the bound app is called and looks
/// like.
///
/// **A struct rather than two parameters, deliberately.** The one defect shape
/// this feature has already produced is an argument swapped or nulled at a
/// call site while the pure function underneath stayed exhaustively tested --
/// `apps.label(ctx, &app.path, &app.process)` becoming `(&app.process,
/// &app.process)`, which survived because the fixture's two inputs agreed. A
/// `&str` and an `Option<&TextureHandle>` cannot be transposed for each other,
/// and naming them here means the call site reads as the two facts it is
/// passing rather than as two positional arguments.
#[derive(Clone, Copy)]
pub struct ResolvedApp<'a> {
    /// `Google Chrome`, or the executable's file name when nothing could be
    /// resolved -- [`AppIdentityCache::label`] never yields nothing.
    pub name: &'a str,
    /// `None` whenever there is no icon: no path, an unreachable one, a
    /// directory, an icon-less binary, or a probe still out. Never a
    /// placeholder graphic -- an app with no icon simply has none.
    pub icon: Option<&'a egui::TextureHandle>,
}

/// How big the matched app's icon is drawn, and the gap between it and the
/// name. The edit form's app block draws the same icon at the same 18pt (see
/// `detail_edit::app_block`); one number in each file rather than a shared
/// constant would be two, so this is the read pane's and the sizes are held
/// together by `the_read_pane_draws_the_app_icon_at_the_edit_forms_size`.
const APP_ICON_SIZE: f32 = 18.0;
const APP_ICON_GAP: f32 = 8.0;

/// The `App` row: the icon, the app's real name, and -- when there is
/// something to open -- a link and a chord that open it.
///
/// **Three things one click can mean, split by where it lands**, which is the
/// Website row's own arrangement (see `draw_detail_read`) and is safe for the
/// same reason: `row_impl` senses the tile on a `UiBuilder` background, egui
/// registers that before the `Ui`'s children, and a click goes to the topmost
/// widget under the pointer and to nothing else. So the NAME opens the app and
/// the rest of the tile copies the executable name.
///
/// `open` is [`app_name_open_action`]'s answer and is not re-decided here.
/// `None` means the name is drawn as plain ink text with no hand cursor and no
/// click at all -- see that function for why a link that does nothing is worse
/// than no link.
fn app_name_row(
    ui: &mut egui::Ui,
    app_row: &AppRow,
    icon: Option<&egui::TextureHandle>,
    open: Option<&DetailAction>,
    action: &mut DetailAction,
) {
    // Measured on the card's own `Ui`, before `copy_row` builds the band --
    // the same fixed point `app_value_row` measures on, and for the same
    // reason: inside the band `available_width` is derived from a `max_rect`
    // the `ScrollArea` grew to fit last frame's widest child.
    // **Room is reserved for the icon AND for the chord**, because the name
    // is laid to an explicit width and whatever it takes, the control group
    // does not get. Leaving the chord out of this sum drew `CTRL+SHIFT+O` at
    // x = 308.4 on a 298pt pane -- off the edge, unreadable, and the same
    // shape as the footer collision one row below.
    let column = (app_card_value_width(ui)
        - if icon.is_some() { APP_ICON_SIZE + APP_ICON_GAP } else { 0.0 })
        .max(0.0);
    let chord = chord_hint_width(ui, OPEN_APP_CHORD.2) + CONTROL_GAP;
    let show_chord = open.is_some() && app_row_chord_fits(column, chord);
    let name_width = (column - if show_chord { chord } else { 0.0 }).max(0.0);
    let mut opened = false;
    copy_row(
        ui,
        app_row.label,
        |ui| {
            if let Some(texture) = icon {
                ui.add(
                    egui::Image::new(texture)
                        .fit_to_exact_size(egui::vec2(APP_ICON_SIZE, APP_ICON_SIZE)),
                );
                ui.add_space(APP_ICON_GAP);
            }
            // Laid HERE with an explicit wrap width, exactly as the
            // `Program file` path is: a resolved name is longer than the
            // `mabl.exe` this row used to print, and a run left to a
            // horizontal layout's `Extend` is what inflated this card to
            // 467.8pt in a 298pt pane once already. `break_anywhere` is not
            // needed -- a real app name has spaces -- but the width is.
            let mut job = egui::text::LayoutJob::simple(
                app_row.value.clone(),
                egui::FontId::new(ROW_VALUE_SIZE, egui::FontFamily::Proportional),
                if open.is_some() { theme::BLUE } else { theme::INK },
                name_width,
            );
            job.wrap.break_anywhere = true;
            let galley = ui.painter().layout_job(job);
            match open {
                Some(_) => {
                    opened = theme::link_galley(ui, galley)
                        .on_hover_text(format!("{APP_OPEN_HOVER} \u{b7} {}", OPEN_APP_CHORD.2))
                        .clicked();
                }
                None => {
                    ui.label(galley);
                }
            }
        },
        // The chord, on the control line where every other row's is -- when
        // it does something AND there is room for it. See
        // [`app_row_chord_fits`]; the link's own tooltip names it either way.
        |ui| {
            if show_chord {
                chord_hint(ui, OPEN_APP_CHORD.2);
            }
        },
        DetailAction::CopyValue(app_row.copy.clone()),
        // No COPY chord: this row's chord opens. `copy_row` would otherwise
        // paint it a second time and its tooltip would call it a copy.
        None,
        app_row.real,
        action,
        RowShape::Columns,
    );
    if opened {
        if let Some(open) = open {
            *action = open.clone();
        }
    }
}

/// What hovering the app's name promises. One string, because the link says it
/// and nothing else may. **It carries the chord**, which is what keeps the
/// shortcut discoverable on the narrow panes where [`app_row_chord_fits`]
/// declines to paint it.
const APP_OPEN_HOVER: &str = "Open this app";

/// The least room the app's NAME may be laid out in. Below this a name is
/// broken across a line per word or worse, and the row grows into a ribbon.
///
/// Enough for `Waypoint Browser` at [`ROW_VALUE_SIZE`] on one line, which is
/// about as short as a real `FileDescription` gets.
const APP_NAME_MIN_WIDTH: f32 = 90.0;

/// Whether the App row paints its chord hint, given the room its value column
/// has and what the hint costs.
///
/// **The chord is the first thing dropped, not the name.** At the app's
/// minimum window size the value column is about 95pt and `CTRL+SHIFT+O` is
/// about 72pt of it: reserving room for the hint unconditionally left the
/// name 15pt, which broke `Ledgerline Accounting Suite` into a column one
/// syllable wide and grew the card past the bottom of the pane. Reserving
/// nothing drew the hint at x = 308 on a 298pt pane, where egui culled it --
/// the pane does not scroll sideways, so an unpainted hint and an unreachable
/// one are the same thing.
///
/// So the hint is painted where it fits and omitted where it does not, and
/// [`APP_OPEN_HOVER`] names the chord in the link's tooltip on every pane --
/// which is the surface [`COPY_SHORTCUTS`]'s own doc calls this pane's
/// primary one for chords anyway.
///
/// Pure, and given both numbers rather than a `Ui`, so the decision is
/// callable without a frame.
fn app_row_chord_fits(column: f32, chord: f32) -> bool {
    column - chord >= APP_NAME_MIN_WIDTH
}

fn app_value_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &str,
    copy: &str,
    action: &mut DetailAction,
) {
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
        // `copy`, not `value`: the `Program file` row shows this pane's own
        // "Not recorded" placeholder when there is no path, and a click must
        // never put a word this pane invented on the clipboard. See
        // [`AppRow::copy`].
        DetailAction::CopyValue(copy.to_string()),
        None,
        row_offers_copy(copy),
        action,
        RowShape::Columns,
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
/// **It always stacks, and that is not a nicety.** Every other row on this
/// pane has a short value and one small control; this one carries a paragraph
/// and up to two buttons, and the label column is a fixed [`ROW_LABEL_WIDTH`]
/// whatever the pane's width is. At the
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
    // **Always stacked, and the side-by-side layout this replaced is the
    // user's second report about this card: "Remove button is on top of long
    // description - should be left aligned".**
    //
    // It was on top of it, literally and by construction. `row_body` lays a
    // band `left_to_right`, puts the label cell, then the VALUE, and only
    // then opens a `right_to_left` group for the controls -- so the controls
    // get whatever width the value left behind. The notes are drawn with
    // `set_max_width(app_card_value_width(ui))`, i.e. **the whole value
    // column**, so what they left behind was zero, and egui laid Remove and
    // Open right-aligned inside a zero-width region: on top of the sentence.
    // The `room < app_footer_controls_width` test above could never see it,
    // because it compared the controls against the column's full width while
    // the notes had already taken all of it.
    //
    // The obvious repair -- shrink the notes by the controls' width -- was
    // rejected: `app_card_notes` is never empty for any body this footer
    // draws (a bound match always carries its behaviour note, and
    // `app_notice_with_remove` passes exactly one sentence), and every one of
    // those sentences is a paragraph that wants the whole column. Squeezing a
    // 200-character notice into 162 - 110 = 52pt to keep two buttons on its
    // line trades a collision for a ten-line ribbon. So the controls get
    // their own line, always, left-aligned on the same [`CARD_PAD_X`] every
    // label above them sits on -- which is what the user asked for, and what
    // the narrow pane was already doing.
    //
    // The width test is gone with the branch it chose between. It answered
    // one question -- "do the controls fit beside the notes?" -- that now has
    // one answer.
    //
    // Only when there is something to say: an empty notes row would be a band
    // of padding above the controls.
    if !notes.is_empty() {
        row(ui, "", draw_notes, |_ui| {});
    }
    // **`app_card_content_width` and NOT `ui.available_width()`**, which is
    // the same distinction the rows above make: the `ScrollArea` grows its
    // content `Ui` to fit last frame's widest child, so `available_width` here
    // is a width this card may have grown for itself, and a wrapped row laid
    // out against it does not wrap where the pane ends. See
    // [`app_card_content_width`].
    let line = (app_card_content_width(ui) - f32::from(CARD_PAD_X) * 2.0).max(0.0);
    egui::Frame::new()
        .inner_margin(Margin::symmetric(CARD_PAD_X, ROW_PAD_Y))
        .show(ui, |ui| {
            ui.set_width(line);
            // `horizontal_wrapped` rather than a plain `horizontal`, so a
            // longer program name or a future third control takes a second
            // line instead of the fate this whole function exists to undo.
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = CONTROL_GAP;
                // Open first, so it reads first: it is the ordinary action
                // and Remove is the destructive one.
                app_card_open_control(ui, choices, action);
                app_card_remove_control(ui, action);
            });
        });
}

/// The card's one destructive control, in one place because [`app_card_footer`]
/// draws it for the bound card and for [`app_notice_with_remove`] alike.
///
/// **One click, no arming.** `confirm_click`'s two-click gate is reserved for
/// the item Delete, which trashes the whole item; this removes one custom
/// field, the card says so immediately by flipping to
/// [`APP_MATCH_EMPTY_NOTICE`], and that notice names the way to put it back.
/// Making this the third armed control on the pane would have cost
/// `draw_detail_read` another parameter and `vault_window::mod` another piece
/// of per-item pending state, for a click whose undo is four clicks in the
/// tray.
///
/// Hand-editing `process` and `path` is deliberately NOT offered here -- see
/// the module's own note on the card.
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

/// How a row arranges its label against its value.
///
/// The two-column form is the design, and every row on this pane uses it
/// wherever it fits. [`RowShape::Stacked`] is what a row falls back to when
/// it does NOT fit -- see [`masked_row`], which is the only thing that asks
/// for it and the only place the choice is made.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RowShape {
    /// Label in its fixed [`ROW_LABEL_WIDTH`] column, value beside it,
    /// controls right-aligned on the same line.
    Columns,
    /// Label on its own line, value and controls on the next -- the whole
    /// content box wide, because that is the width that was missing.
    Stacked,
}

/// The width a card row's content box really has: from the card's own left
/// edge to the right edge the reader can actually see, less the row's
/// `padding: 0 16px` on both sides.
///
/// **The clip rect, and NOT `ui.available_width()`** -- the distinction
/// [`app_card_content_width`] documents at length, and the one this whole
/// repair turns on. The body is a `ScrollArea`, which grows its content `Ui`
/// to fit last frame's widest child; a masked row that overflowed its card
/// grew it, and every later question asked of `available_width` then got the
/// overflowed width back and concluded there was room. Measured on the
/// previous-password rows at the narrowest pane: the card's own rect ends at
/// 274, and `available_width` inside a row said the line ran to 308.4 --
/// which is precisely where the reveal eye was being right-aligned to.
///
/// The clip rect is the viewport, and on this pane it already stops at the
/// cards' edge: `theme::scrollbar_in_gutter` hands the outermost
/// [`BODY_PAD_X`] to the scroll lane, so the `ScrollArea` clips its content
/// at `pane.right() - BODY_PAD_X` whether or not a bar is showing. Nothing
/// drawn inside a row can move it.
///
/// Takes the `Ui` a card's contents are drawn in, whose `max_rect().left()`
/// is the card's own left edge; [`row_line_width`] is the same quantity
/// asked from inside the row's frame.
fn row_content_width(ui: &egui::Ui) -> f32 {
    (ui.clip_rect().right() - ui.max_rect().left() - f32::from(CARD_PAD_X) * 2.0).max(0.0)
}

/// [`row_content_width`], asked from inside a row's own `Frame` -- where the
/// left padding has already been applied to `max_rect` and only the right
/// one is still owed.
fn row_line_width(ui: &egui::Ui) -> f32 {
    (ui.clip_rect().right() - ui.max_rect().left() - f32::from(CARD_PAD_X)).max(0.0)
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
    row_impl(ui, label, value, controls, egui::Sense::hover(), RowShape::Columns);
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
    // How the label sits against the value. [`RowShape::Columns`] for every
    // row whose contents fit the pane; see [`RowShape`].
    shape: RowShape,
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
        row_impl(ui, label, value, controls, egui::Sense::hover(), shape);
        return;
    }
    let response = row_impl(ui, label, value, controls, egui::Sense::click(), shape);
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
    shape: RowShape,
) -> egui::Response {
    let clickable = sense == egui::Sense::click();
    let scope = ui.scope_builder(egui::UiBuilder::new().sense(sense), |ui| {
        // The hover tint's slot, reserved BEFORE anything paints into this
        // row: the response that decides whether to fill it only exists once
        // the row has been laid out, and a fill added then would cover the
        // row's own text.
        let tint = ui.painter().add(egui::Shape::Noop);
        row_body(ui, label, value, controls, shape);
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
    shape: RowShape,
) {
    egui::Frame::new()
        .inner_margin(Margin::symmetric(CARD_PAD_X, ROW_PAD_Y))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // The fallback for a row that does not fit its card: the label
            // takes a line of its own and the value gets the whole content
            // box. **Room was what was missing, so room is what the second
            // line supplies** -- the same answer, and the same sentence,
            // [`app_card_footer`] reached for when its controls were laid on
            // top of its notes at [`NARROW`]. Chosen by [`masked_row`], never
            // here; see [`RowShape`].
            if shape == RowShape::Stacked {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, STACKED_LABEL_GAP);
                    // Not the fixed column: on this line there is nothing to
                    // line up with, and 130pt of reserved space beside a
                    // "3 days ago" would be a second, invisible indent.
                    // `row_line_width`, never `ui.available_width()`: see
                    // [`row_content_width`]. The width this row is trying to
                    // fit into cannot be read from a `Ui` a row like this one
                    // has already stretched.
                    let line = row_line_width(ui);
                    label_cell(ui, label, line, LabelFit::ToText);
                    let band = egui::vec2(line, ROW_CONTENT_HEIGHT);
                    ui.allocate_ui_with_layout(
                        band,
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            value(ui);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.spacing_mut().item_spacing.x = CONTROL_GAP;
                                    controls(ui);
                                },
                            );
                        },
                    );
                });
                return;
            }
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
                    label_cell(ui, label, ROW_LABEL_WIDTH, LabelFit::ToColumn);
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

/// Whether a label OCCUPIES the width it was laid out in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LabelFit {
    /// It does -- the fixed [`ROW_LABEL_WIDTH`] column of a
    /// [`RowShape::Columns`] row, whatever the label says.
    ToColumn,
    /// It does not: it takes only the width its glyphs need. For a
    /// [`RowShape::Stacked`] row, whose label is alone on its line with
    /// nothing to the right of it to line up.
    ToText,
}

/// A row's label column: exactly [`ROW_LABEL_WIDTH`] wide whatever the label
/// says, so the values beside it line up down the whole pane. Painted rather
/// than `ui.label`ed because a label allocates its own text width.
///
/// `column` is the width the text is WRAPPED in; `fit` says whether the cell
/// then takes that width or only its glyphs'. See [`LabelFit`].
fn label_cell(ui: &mut egui::Ui, label: &str, column: f32, fit: LabelFit) {
    let galley = ui.painter().layout(
        label.to_string(),
        egui::FontId::new(ROW_LABEL_SIZE, egui::FontFamily::Proportional),
        theme::TEXT_FAINT,
        column,
    );
    let width = match fit {
        LabelFit::ToColumn => column,
        LabelFit::ToText => galley.size().x,
    };
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(width, galley.size().y), egui::Sense::hover());
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

/// The NOTES card's body: **selectable text that also copies whole on a
/// click.**
///
/// The user asked for the note to be "selectable\copiable text full (on click)
/// or partially". Both halves at once, on the same pixels, which is the
/// two-meanings-for-one-click problem this pane already solves for the website
/// link -- and it is solved here the same way and with the same layering
/// argument (see [`copy_row`]):
///
/// * **The tile is sensed on a `UiBuilder` BACKGROUND**, which egui registers
///   when the `Ui` is created and therefore *before* its children. So the text
///   is on top of it and wins any click that lands on the text; the tile keeps
///   the padding around it.
/// * **The text is a read-only [`egui::TextEdit`]**, not a `ui.label`. egui
///   implements `TextBuffer` for `&str` with `is_mutable() == false`, which is
///   the framework's own read-only idiom: the caret, click-drag range
///   selection, double-click-a-word, Ctrl+A and the Ctrl+C that copies *that
///   range* all come from egui rather than from a hand-rolled hit test. A
///   `ui.label` -- what this card drew before -- has none of them.
///
/// **Click versus drag is the whole difficulty, and it is split by egui's own
/// distinction rather than by a rect.** `Response::clicked()` is true only for
/// a press and release that never passed the drag threshold; a press that
/// moves is a drag, and a drag over a `TextEdit` is a selection. So:
///
/// * A plain click anywhere on the card -- on the text or on the padding --
///   copies the WHOLE note. The text's own response is consulted for exactly
///   this reason: if only the tile's `clicked()` were read, a click on the
///   glyphs would be swallowed by the `TextEdit` and copy nothing, which is
///   the "a plain click must not be swallowed by the text" half.
/// * A click-drag across the glyphs selects a range and copies NOTHING. The
///   tile cannot swallow it (the text is topmost), and the text's `clicked()`
///   is false because it was a drag.
///
/// Both halves are pinned in one frame each, by
/// `clicking_a_note_copies_all_of_it` and
/// `a_note_can_be_selected_without_copying_the_whole_thing`, the way
/// `clicking_the_website_link_opens_it_without_copying` pins the link's.
///
/// The frame, the margin and the width are [`card_text`]'s, so the note still
/// sits on the same left edge as every row label on the pane.
fn notes_body(ui: &mut egui::Ui, item_id: &str, notes: &str, action: &mut DetailAction) {
    let scope = ui.scope_builder(
        egui::UiBuilder::new().sense(egui::Sense::click()),
        |ui| {
            // The hover tint's slot, reserved before anything paints into the
            // tile -- `row_impl`'s reason, unchanged: the response that
            // decides whether to fill it does not exist until the tile has
            // been laid out, and a fill added then would cover the note.
            let tint = ui.painter().add(egui::Shape::Noop);
            let text = egui::Frame::new()
                .inner_margin(Margin::symmetric(CARD_PAD_X, ROW_PAD_Y))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    // `&str`, so the buffer is immutable: the caret and the
                    // selection work, typing does nothing.
                    let mut source = notes;
                    egui::TextEdit::multiline(&mut source)
                        .id(notes_text_id(item_id))
                        // No box, no fill, no margin of its own: this must
                        // look exactly like the paragraph it replaced, whose
                        // left edge is the card's `CARD_PAD_X`.
                        .frame(egui::Frame::NONE)
                        .margin(Margin::ZERO)
                        // egui's default is four, which would have put three
                        // blank lines under a one-line note.
                        .desired_rows(1)
                        // Not `f32::INFINITY`: this pane is inside a
                        // `ScrollArea`, whose available width is finite and
                        // is the width the note must wrap at.
                        .desired_width(ui.available_width())
                        .font(egui::FontId::new(
                            ROW_VALUE_SIZE,
                            egui::FontFamily::Proportional,
                        ))
                        .text_color(theme::INK)
                        .show(ui)
                        .response
                })
                .inner;
            (tint, text)
        },
    );
    let (tint, text) = scope.inner;
    let tile = scope.response;
    // The same affordance every copy row has. Asked of BOTH responses because
    // only one of them is hovered at a time: over the glyphs it is the text,
    // over the padding it is the tile, and a tint that appeared over only the
    // padding would advertise half a click target.
    if tile.hovered() || text.hovered() {
        ui.painter().set(
            tint,
            egui::Shape::rect_filled(tile.rect, CornerRadius::ZERO, theme::CARD_TINT),
        );
    }
    // **`clicked()` on both, and `dragged()` on neither.** See the doc above:
    // a drag is a selection and reports no click, so this is the click half
    // and the selection half needs no code at all.
    if tile.clicked() || text.clicked() {
        *action = DetailAction::CopyValue(notes.to_string());
        note_copied(ui.ctx(), NOTES_COPY_LABEL);
    }
}

/// What the NOTES card's copy confirmation says. Named once so the toast and
/// the test cannot disagree about it.
const NOTES_COPY_LABEL: &str = "Notes";

/// The note's `TextEdit` id: **this constant AND the item's own id**.
///
/// **Named rather than left to egui's auto-id**, because a selection lives in
/// `TextEditState`, which is keyed on the widget's id, and an id derived from
/// source position is one `a_note_can_be_selected_without_copying_the_whole_
/// thing` cannot ask for -- so the selection half of this card would have had
/// no assertion at all. It is also what keeps the caret where the user put it
/// when the pane repaints.
///
/// **Keyed on the item, and that half is not about testability at all.** The
/// salt alone was a GLOBAL id, so every item this pane ever showed shared one
/// `TextEditState`. Drag-select thirty characters of one item's note, click
/// the next item in the list, and its note came up with its first thirty
/// characters already selected -- a selection the user never made, over text
/// they had not read, on the copy target of the whole card. Measured, before
/// this: `Some(CCursorRange { primary: CharIndex(30), secondary: CharIndex(0)
/// })` still loaded after the pane had been handed a different item with an
/// unrelated note.
///
/// So the id was chosen for a test and bought cross-item state no test
/// reached, which is this file's recurring defect. **Both are kept**: the id
/// is still something a test can name -- it just has to name the item too,
/// which the test already has -- and `a_notes_selection_does_not_follow_the_
/// user_to_the_next_item` is the assertion that the sharing is gone.
///
/// Salting rather than clearing the state on a change of item was the choice
/// because "the shown item changed" is not a fact this pane holds: it is
/// handed an item per frame and remembers nothing between frames, so a clear
/// would have needed new state whose only job was to notice. A distinct id
/// per item needs none: egui simply never finds the other item's cursor.
/// The cost is one `TextEditState` entry per item whose note the user has put
/// a caret in, for the life of the window -- bounded by the vault and by far
/// the smaller of the two prices.
///
/// **Two items with equal -- or empty -- ids would share one state again,
/// and nothing here defends against that, deliberately.** It is not a path
/// this app has. `draw_detail_read` has one call site, in
/// `vault_window/mod.rs`, and the item it is handed comes straight off the
/// loaded vault list, where the id is Bitwarden's own and is what the whole
/// pane -- copy, open, edit, delete -- already addresses the item by; two
/// list entries sharing one would break far more than a caret. An item this
/// app is still making has no id at all, and never reaches this function: it
/// is a `DetailMode::Create(EditDraft)`, whose note is a separate `TextEdit`
/// with an id egui allocates itself. A guard here would be a guard on a
/// caller that does not exist.
const NOTES_TEXT_ID_SALT: &str = "detail-notes-body";

fn notes_text_id(item_id: &str) -> egui::Id {
    egui::Id::new((NOTES_TEXT_ID_SALT, item_id))
}

/// [`card_text`] with a second run after the first: the same frame, the same
/// margin, the same width. The metadata strip's breach badge is the only
/// caller.
///
/// Two runs rather than one string because they are two colours -- a breached
/// password's half is the palette's red and the strip's half stays faint, and
/// one `RichText` cannot be both.
///
/// `horizontal_wrapped`, not `horizontal`: the breached segment is the
/// longest string this strip has ever carried, and at the detail column's
/// minimum width it does not fit on the line the strip is already using. A
/// non-wrapping row would push it past the card's right edge, and egui culls
/// shapes outside the screen rect entirely -- so the failure would not look
/// like an overflowing badge, it would look like no badge at all.
///
/// Item spacing is zeroed because the caller's segment carries its own
/// leading space; egui's default gap would put a wider hole before the
/// separator than between the strip's own two.
fn card_text_pair(ui: &mut egui::Ui, first: RichText, second: RichText) {
    egui::Frame::new()
        .inner_margin(Margin::symmetric(CARD_PAD_X, ROW_PAD_Y))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.label(first);
                ui.label(second);
            });
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
        RowShape::Columns,
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
///
/// **It is the one row that measures itself before it is drawn**, because it
/// is the one row that could not be made to fit any other way. In the
/// two-column shape the value gets
/// `row_content_width - ROW_LABEL_WIDTH - ROW_GAP` less its controls, which
/// on the narrowest the detail pane can be (298pt: 900 - 212 - 390) is 44pt.
/// A ten-bullet mask is 94.2pt. So every masked row on that pane overflowed
/// its card, and the reveal eye -- allocated after the value, wherever the
/// value happened to end -- was painted at x = 285.26..303.56 on a pane whose
/// cards end at 274 and whose own right edge is 298: the whole control inside
/// the scroll lane, 5.6pt of it clipped away entirely. Measured on the
/// previous-password rows, which are five of them in a column; the login
/// password, card number, card security code and SSH private key are the same
/// row and were the same defect.
///
/// Neither half of the repair is optional:
///
/// * **[`RowShape::Stacked`] when the columns do not fit**, which buys the
///   whole content box (218pt at 298) instead of 44. Not elision: this row's
///   value IS the point of the row, and `\u{2026}` where a password was is a
///   row that has stopped doing its job. Not a shorter mask either -- the
///   mask is short already, and a revealed password is longer than any of
///   them.
/// * **A wrap width on the value, always.** The masked run is a
///   `LayoutJob`, and a `LayoutJob`'s `wrap.max_width` defaults to infinity
///   -- which is why the run above ran past the card instead of folding. A
///   revealed 40-character password overflows even the stacked line, so the
///   line it is on is the width it wraps at.
///
/// **The two halves are pinned by different tests, and for one release they
/// were not.** `a9dad37`'s message says "Neither half of the repair is
/// optional... Pinned by `the_previous_password_rows_fit_the_narrowest_pane`
/// and `the_lane_leaves_the_cards_eighteen_points_of_clear_space`". That is
/// true of the STACKING half only. Both of those render with
/// `RevealState::default()`, so the only value either measures is the
/// ten-bullet mask -- and the mask fits the stacked line unwrapped, which is
/// exactly why the wrap makes no difference to them. Deleting
/// `job.wrap.max_width = room` below left all 1597 lib tests green while a
/// revealed previous password ran to x = 351.2 on a 298pt pane whose cards
/// end at 274: the same defect `a9dad37` exists to fix, one click away. The
/// wrapping half is pinned by
/// `a_revealed_previous_password_fits_the_narrowest_pane_too`, which reveals
/// five 41-character entries and is the only test here that sets a reveal
/// flag.
fn masked_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &str,
    revealed: &mut bool,
    action: &mut DetailAction,
    on_copy: DetailAction,
    hint: Option<CopyShortcut>,
) {
    // **Nothing to hide, so nothing to draw** -- see [`masked_row_visible`].
    // The return is BEFORE any allocation, so the row leaves no band, no
    // hairline slot and no eye behind it.
    //
    // **Unreachable from production, deliberately.** The callers do not merely
    // "gate their separators on the same predicate" -- they gate the CALL, so
    // no production path arrives here with an empty value at all (see
    // `masked_row_visible`'s doc for the four, one by one). This is the rule
    // stated where a fifth caller will read it, and the test that exercises it
    // calls this function directly.
    if !masked_row_visible(value) {
        return;
    }
    let shown = if *revealed {
        value.to_string()
    } else {
        "•".repeat(MASKED_BULLETS)
    };
    // What the control group on this row's value line really costs: the eye,
    // plus the chord this row paints beside it when it has one, plus the gap
    // between them. Read off the same helpers that draw them, so a retuned
    // control cannot leave this measurement behind.
    let controls_width = theme::EYE_TOGGLE_SIZE
        + hint.map_or(0.0, |which| {
            CONTROL_GAP + chord_hint_width(ui, copy_shortcut_chord(which))
        });
    let content = row_content_width(ui);
    
    // Laid out unwrapped, which is the question being asked: how wide does
    // this value WANT to be?
    let natural = ui
        .painter()
        .layout_job(theme::letterspaced_mono(
            &shown,
            MASKED_SIZE,
            MASKED_TRACKING,
            theme::INK,
        ))
        .size()
        .x;
    let beside_the_label = content - ROW_LABEL_WIDTH - ROW_GAP - controls_width;
    let shape = if natural <= beside_the_label {
        RowShape::Columns
    } else {
        RowShape::Stacked
    };
    // The width the value really has on the line it ends up on. `max(1.0)`
    // rather than `max(0.0)`: egui treats a zero wrap width as "no wrapping",
    // which is the behaviour this exists to withdraw.
    let room = match shape {
        RowShape::Columns => beside_the_label,
        RowShape::Stacked => content - controls_width,
    }
    .max(1.0);
    copy_row(
        ui,
        label,
        |ui| {
            // `font-family: ui-monospace; font-size: 15px; letter-spacing:
            // 0.08em` -- the tracking is what stops a bullet run reading as
            // one solid blob.
            let mut job =
                theme::letterspaced_mono(&shown, MASKED_SIZE, MASKED_TRACKING, theme::INK);
            job.wrap.max_width = room;
            // **`break_anywhere`, for the reason the `Program file` path row
            // gives**: egui breaks at word boundaries, and neither a bullet
            // run nor a password has any.
            job.wrap.break_anywhere = true;
            // **Laid here and handed over as a `Galley`, and that is not
            // style.** Given a `LayoutJob`, `Label` re-lays it and overwrites
            // `wrap.max_width` with its own -- `f32::INFINITY` inside a
            // horizontal layout, whose wrap mode is `Extend`. So the width set
            // two lines up would be discarded and the run would draw its full
            // length anyway, which is exactly the defect. Same discovery, and
            // the same repair, as `app_row_view`'s.
            let galley = ui.painter().layout_job(job);
            ui.label(galley);
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
        shape,
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
    // **`owed`, not `index > 0`.** An entry whose password is empty draws no
    // row (see `masked_row_visible`), and counting positions rather than
    // drawn rows would have put a hairline above the gap it left -- and, if
    // the empty one came first, a hairline above the very first row.
    let mut owed = false;
    for (index, entry) in history.iter().take(MAX_HISTORY_ROWS).enumerate() {
        if !masked_row_visible(entry.password.as_str()) {
            continue;
        }
        if owed {
            theme::row_rule(ui);
        }
        owed = true;
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
        if owed {
            theme::row_rule(ui);
        }
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

/// The TOTP countdown, as the user asked for it: **"9s left - show just 9s"**.
///
/// A pure function rather than a `format!` inside the paint closure, for this
/// file's standing reason -- a string reachable only through an `egui` closure
/// is a string no test can call -- and because the wording is the whole of the
/// change: `totp_countdown_reads_just_the_number_and_the_unit` calls this
/// directly and `a_totp_countdown_paints_just_the_seconds` reads it off the
/// painted frame, so reverting to `"{n}s left"` fails both.
///
/// The two ends read correctly with the short form and were checked rather
/// than assumed: a full window is `30s` and an expiring one is `0s`, both of
/// which are what a countdown says. `1s` is not pluralised because it is not a
/// sentence -- there is no word left to agree with the number.
fn totp_countdown_text(seconds_left: u8) -> String {
    format!("{seconds_left}s")
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
                RichText::new(totp_countdown_text(seconds_left))
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
        RowShape::Columns,
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
    pub(super) fn an_item(item_type: Option<i64>) -> VaultItem {
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
    pub(super) fn item_type_for(kind: ItemKind) -> Option<i64> {
        match kind {
            ItemKind::Login => Some(1),
            ItemKind::SecureNote => Some(2),
            ItemKind::Card => Some(3),
            ItemKind::Identity => Some(4),
            ItemKind::SshKey => Some(5),
            ItemKind::Unknown(t) => Some(t),
        }
    }

    pub(super) const EVERY_KIND: [ItemKind; 6] = [
        ItemKind::Login,
        ItemKind::SecureNote,
        ItemKind::Card,
        ItemKind::Identity,
        ItemKind::SshKey,
        ItemKind::Unknown(9),
    ];

    /// The `BreachCache` every harness in this module passes, alongside
    /// `check_breaches = false`.
    ///
    /// Its check answers `Unavailable` without touching anything, and
    /// `should_check` returning false means no harness here ever reaches it
    /// -- but if one ever did, the answer it would get is the one that says
    /// "nothing was checked", never `Safe`. `BreachCache::live` is not named
    /// anywhere in this file.
    pub(super) fn inert_breach_cache() -> crate::breach::BreachCache {
        crate::breach::BreachCache::new(std::sync::Arc::new(|_, _| {
            crate::breach::BreachStatus::Unavailable
        }))
    }

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
            draw_detail_read(
                ui,
                item,
                None,
                3,
                totp,
                delete_pending,
                &mut reveal,
                None,
                &mut crate::app_identity::AppIdentityCache::default(),
                false,
                false,
                &mut inert_breach_cache(),
            );
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
            images: Vec::new(),
            shapes: egui::Shape::Noop,
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
        collect_images(&all, &mut frame.images);
        frame.shapes = all;
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
            // There used to be a `!painted.contains("Fill in app")` here.
            // It is DELETED rather than kept, by this test's own stated
            // rule: the read pane's controls are DRAWN now, so their
            // absence has to be asserted against the shapes, and "asserting
            // the old strings here would be a test that cannot fail". With
            // the header's Fill button removed, no pane in this app paints
            // that word at all, so the assertion had become exactly the
            // always-true check the comment warns about. The shape
            // assertions below are what actually distinguish an
            // out-of-vault pane from a live one.
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

    /// Every TEXTURED rectangle -- which is what an `egui::Image` paints in
    /// this version of egui, and the only trace an icon leaves: it paints no
    /// string, and `collect_rects` throws the brush away and would report it
    /// as an ordinary transparent fill.
    fn collect_images(shape: &egui::Shape, out: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Rect(rect) if rect.brush.is_some() => out.push(rect.rect),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_images(shape, out);
                }
            }
            _ => {}
        }
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
            draw_detail_read(
                ui,
                item,
                None,
                3,
                totp,
                false,
                &mut reveal,
                None,
                &mut crate::app_identity::AppIdentityCache::default(),
                false,
                false,
                &mut inert_breach_cache(),
            );
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
        /// The window's `AppIdentityCache`, carried across frames exactly as
        /// `vault_window::mod`'s `run` carries it. Empty unless a test seeds
        /// it (see [`Pane::knows_app`]): nothing in this suite names a path
        /// that exists on the machine running it, so a real probe would
        /// always answer "gone" and the resolved-name half of this card
        /// would be untestable.
        apps: crate::app_identity::AppIdentityCache,
        /// `Settings::reveal_totp_seed`, carried across frames exactly as
        /// `vault_window::mod`'s `run` carries it. `false` unless a test asks
        /// for it (see [`Pane::revealing_secrets`]) -- which is the shipped
        /// default, so every existing test here keeps measuring the pane a
        /// user gets out of the box.
        reveal_totp_seed: bool,
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
        /// Every textured rect -- the matched app's icon is the only one this
        /// pane can paint. See [`collect_images`].
        images: Vec<egui::Rect>,
        /// This frame's whole shape tree, kept so a test can hand it to
        /// `theme::icon_probe` directly. The `Frame` fields above are each one
        /// probe's answer; a test asking "did the icon confuse ALL of them"
        /// needs the tree.
        shapes: egui::Shape,
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
                apps: crate::app_identity::AppIdentityCache::default(),
                reveal_totp_seed: false,
            }
        }

        /// The same pane, on a machine where the executable at `path` really
        /// is called `name`.
        ///
        /// **`path`, not `process`** -- which is the whole point of seeding
        /// through the cache rather than handing the pane a name. The cache is
        /// keyed on the path, so a wiring that passed `process` where the path
        /// goes finds nothing and paints the exe name, and every test below
        /// that names a resolved name fails. That is the exact substitution
        /// (`apps.label(ctx, &app.path, &app.process)` -> `(&app.process,
        /// &app.process)`) this feature has already shipped once.
        fn knows_app(mut self, path: &str, name: &str) -> Self {
            self.apps.seed_ready(path, name, None);
            self
        }

        /// The same, with an icon -- a 2x2 texture, because what is asserted
        /// is that an image is painted at all and at what size, never what is
        /// in it.
        fn knows_app_with_icon(mut self, path: &str, name: &str) -> Self {
            let texture = self.ctx.load_texture(
                "test-app-icon",
                egui::ColorImage::from_rgba_unmultiplied([2, 2], &[255u8; 16]),
                egui::TextureOptions::default(),
            );
            self.apps.seed_ready(path, name, Some(texture));
            self
        }

        /// The same pane, on a machine whose `settings.json` has the
        /// TOTP-secret preference turned ON.
        fn revealing_secrets(mut self) -> Self {
            self.reveal_totp_seed = true;
            self
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
                        &mut self.apps,
                        false,
                        self.reveal_totp_seed,
                        &mut inert_breach_cache(),
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
                images: Vec::new(),
                shapes: egui::Shape::Noop,
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
            collect_images(&all, &mut frame.images);
            frame.shapes = all;
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

    /// An AUTOFILL TARGETS card on a non-login would advertise a capability
    /// autofill refuses to provide for it, and LOGIN CREDENTIALS would head a
    /// section with no credentials under it.
    ///
    /// The "Fill in app" absence asserted below is a tombstone, not this
    /// test's subject: the header button that painted that label was removed
    /// in `7da1bba`, and the row that used to read `expected` for it is the
    /// one that would come back first if the button did.
    #[test]
    fn only_a_login_renders_the_autofill_targets_and_credentials_cards() {
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

            // The header's "Fill in app" button was removed at the user's
            // request. Asserted as an absolute over EVERY kind rather than
            // dropped: the row that used to read `expected` here is the one
            // that would come back first if the button did, and this fails
            // the moment any pane paints that label again.
            assert!(
                !contains(&texts, "Fill in app"),
                "{kind:?}: the removed header Fill button is painted again; painted: \
                 {texts:?}"
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

            // The header Fill button this row used to check is removed; the
            // AUTOFILL TARGETS card and the metadata strip below are the
            // surfaces `kind_offers_fill` still gates, and they are what
            // keeps this matrix meaningful. The absolute stays so the button
            // cannot creep back in unnoticed.
            assert!(
                !contains(&texts, "Fill in app"),
                "{kind:?}: the removed header Fill button is painted again"
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
            totp_secret: false,
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
                totp_secret: false,
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
            sequence: String::new(),
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
            sequence: String::new(),
            trigger: TriggerMode::Hotkey,
        }
    }

    /// `item` carrying `m` in the `deskwarden:app-match` custom field --
    /// through `vault_bridge::with_app_match`, the one producer of that
    /// field, so these tests read back exactly what a real save writes.
    fn bound_to(item: &VaultItem, m: &AppMatch) -> VaultItem {
        crate::vault_bridge::with_app_match(item, m)
    }

    /// **The read pane shows a binding and never an invitation.** The user:
    /// "Matched app - same, no show if not present, only if edit or add
    /// window called".
    ///
    /// This replaces `the_card_is_offered_wherever_a_match_would_do_something_
    /// and_shown_wherever_one_exists`, which pinned the OLD rule
    /// (`has_field || kind_offers_fill(kind)`) and would have passed for free
    /// after the change if it had merely been deleted. The positive half is
    /// unchanged and still bites; the negative half is inverted, and the last
    /// assertion names the retired rule so a revert to it fails here.
    #[test]
    fn the_card_is_shown_wherever_a_binding_exists_and_nowhere_else() {
        let mut visited = 0;
        for kind in EVERY_KIND {
            visited += 1;
            // A binding is NEVER hidden, whatever the item is: an app match
            // on a secure note is precisely the one a user needs to find in
            // order to remove it.
            assert!(
                app_card_visible(true),
                "a {kind:?} that IS bound to an app does not show that binding"
            );
            assert!(
                !app_card_visible(false),
                "an unbound {kind:?} still offers a MATCHED APP card in the read pane"
            );
        }
        assert_eq!(visited, EVERY_KIND.len(), "the loop visited nothing");
        assert!(visited > 0, "the loop visited nothing, so it proved nothing");
        // The control: the predicate really does answer differently, so
        // neither half above is satisfied by a constant.
        assert_ne!(app_card_visible(true), app_card_visible(false));
        // And it is no longer the OLD rule. `kind_offers_fill(Login)` is
        // true, so `has_field || kind_offers_fill(kind)` would put the card
        // on every unbound login -- the empty placeholder this change
        // removes.
        assert!(
            kind_offers_fill(ItemKind::Login) && !app_card_visible(false),
            "the card is gated on `kind_offers_fill` again"
        );
    }

    #[test]
    fn the_cards_rows_name_the_app_and_the_program_file() {
        // The resolved name is deliberately none of the three strings the
        // match itself carries: a fixture whose display name equalled its
        // process or its path's file name could not tell "the row shows the
        // resolved name" from "the row shows what it always did".
        let rows = app_match_rows(&a_desktop_match(), RESOLVED_NAME);
        assert_eq!(
            rows,
            vec![
                AppRow {
                    label: "App",
                    // What the user SEES: the app's real name ...
                    value: RESOLVED_NAME.to_string(),
                    // ... and what a click COPIES: the executable, which is
                    // the thing that pastes anywhere useful.
                    copy: "Ledgerline.exe".to_string(),
                    real: true,
                    app: true,
                },
                AppRow {
                    label: "Program file",
                    value: r"C:\Apps\Ledgerline\Ledgerline.exe".to_string(),
                    copy: r"C:\Apps\Ledgerline\Ledgerline.exe".to_string(),
                    real: true,
                    app: false,
                },
            ],
            "the rows are not the user's \"name, path\""
        );
    }

    /// What the version resource of the fixture's executable says the app is
    /// called. **Not `Ledgerline.exe`, and not `Ledgerline`**: it has to
    /// differ from the `process`, from the path's file name and from the
    /// path's own directory names, or a test that finds it on screen cannot
    /// say which of those it found.
    const RESOLVED_NAME: &str = "Ledgerline Accounting Suite";

    /// Every match saved before `path` existed -- a shape still sitting in
    /// real vaults. The row must say so, and must not offer to copy the words
    /// "Not recorded" onto the clipboard.
    #[test]
    fn a_match_that_recorded_no_program_file_says_so_and_that_row_is_inert() {
        let m = AppMatch::for_process("Ledgerline.exe", TriggerMode::Auto);
        let rows = app_match_rows(&m, RESOLVED_NAME);
        let path = rows
            .iter()
            .find(|r| r.label == "Program file")
            .expect("the card dropped the Program file row entirely");
        assert_eq!(path.value, "Not recorded");
        assert!(!path.real, "the placeholder would be copied to the clipboard");
        assert_eq!(
            path.copy, "",
            "the pane's own placeholder is what a click would put on the clipboard"
        );
        // Control: a match that DID record one is copyable, so `real` is not
        // simply always false.
        let recorded = app_match_rows(&a_desktop_match(), RESOLVED_NAME);
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
            !app_match_rows(&stored, RESOLVED_NAME).iter().any(|r| r.label == "Window title"),
            "an inert title is drawn as if it matched something"
        );
        // Control: the row exists at all, for the match that really is keyed
        // on its title.
        assert!(app_match_rows(&a_store_match(), RESOLVED_NAME)
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
        // The behaviour note is there too -- and it is the ONE note, not one
        // of three captions chosen by the item's stored `trigger`.
        assert!(notes.contains(&APP_MATCH_BEHAVIOUR_NOTE), "{notes:?}");
        for mode in TRIGGER_ORDER {
            assert!(
                !notes.contains(&trigger_caption(mode)),
                "the card still reports a per-item trigger caption ({mode:?}), which is a                  claim that the item's own mode decides what focusing this app does: {notes:?}"
            );
        }
        // Control: an ordinary app gets the note and NOT the Store note.
        let ordinary = app_card_notes(&a_desktop_match());
        assert_eq!(ordinary, vec![APP_MATCH_BEHAVIOUR_NOTE]);
    }

    /// **The fixture that deliberately differs.** `a_store_match` and
    /// `a_desktop_match` carry DIFFERENT stored triggers (`Hotkey` and
    /// `Prompt`), and the note they produce is the same one -- which is the
    /// whole of "the per-item mode no longer decides anything" as the card
    /// sees it. A card that still read `m.trigger` would give these two
    /// different sentences.
    #[test]
    fn the_behaviour_note_does_not_depend_on_the_items_stored_trigger() {
        let store = a_store_match();
        let desktop = a_desktop_match();
        assert_ne!(store.trigger, desktop.trigger, "the premise: the two fixtures differ");
        for mode in TRIGGER_ORDER {
            let m = AppMatch { trigger: mode, ..a_desktop_match() };
            assert_eq!(
                app_card_notes(&m),
                vec![APP_MATCH_BEHAVIOUR_NOTE],
                "a stored {mode:?} changed what the card says about this binding"
            );
        }
    }

    /// No pane draws these words any more. They are the fixtures the pins in
    /// this file and in `detail_edit` use to say so, and a fixture in which
    /// two modes share a word is a pin that cannot tell which control came
    /// back -- so their distinctness is still worth asserting.
    #[test]
    fn every_retired_trigger_mode_has_its_own_name_and_its_own_sentence() {
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
            "Remove",
            APP_MATCH_BEHAVIOUR_NOTE,
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

    /// Every heading `card` is ever called with in this pane, so "is NOTES
    /// below all the others" is asked of a list that cannot silently lose a
    /// card. `the_card_heading_list_is_complete` counts the `card(` call sites
    /// in this file against it, so a ninth card fails the build's own suite
    /// rather than quietly narrowing the ordering claim.
    const EVERY_CARD_HEADING: [&str; 9] = [
        "LOGIN CREDENTIALS",
        "CARD DETAILS",
        "IDENTITY",
        "SSH KEY",
        "UNSUPPORTED ITEM",
        "PREVIOUS PASSWORDS",
        "AUTOFILL TARGETS",
        APP_CARD_HEADING,
        "NOTES",
    ];

    /// The list above is the whole list. Counted off the source rather than
    /// trusted: `EVERY_CARD_HEADING` is what
    /// `notes_is_the_last_card_for_every_kind_that_shows_it` compares NOTES
    /// against, and a card missing from it is a card NOTES is never checked
    /// against -- an ordering claim that silently stops covering something.
    #[test]
    fn the_card_heading_list_is_complete() {
        let source = include_str!("detail.rs");
        // Eight literal-heading cards plus `card(ui, pane.heading, ..)`, whose
        // heading is `UNSUPPORTED ITEM` and is in the list under that name.
        // Real call sites only: a doc comment naming `card(ui, ...)` is a
        // mention, not a card, and three of them are in this file.
        let calls = source
            .lines()
            .map(str::trim_end)
            .filter(|line| line.trim_start().starts_with("card(ui, "))
            .count();
        assert_eq!(
            calls,
            EVERY_CARD_HEADING.len(),
            "this pane draws {calls} cards and EVERY_CARD_HEADING names {}; a card that is \
             not in that list is a card NOTES is never asserted to be below",
            EVERY_CARD_HEADING.len()
        );
    }

    impl Frame {
        /// Every card heading this frame painted, with its rect, top-down.
        ///
        /// **Painted, not expected**: egui culls shapes entirely outside the
        /// screen rect, so a card pushed out of view comes back from here as
        /// *nothing at all*. Every caller asserts the COUNT before it reads a
        /// single coordinate.
        fn card_headings(&self) -> Vec<(&str, egui::Rect)> {
            let mut found: Vec<(&str, egui::Rect)> = EVERY_CARD_HEADING
                .iter()
                .filter_map(|heading| {
                    let rects: Vec<egui::Rect> = self
                        .texts
                        .iter()
                        .filter(|(t, _)| t == heading)
                        .map(|(_, r)| *r)
                        .collect();
                    match rects.len() {
                        0 => None,
                        1 => Some((*heading, rects[0])),
                        n => panic!("{heading:?} was painted {n} times"),
                    }
                })
                .collect();
            found.sort_by(|a, b| a.1.top().total_cmp(&b.1.top()));
            found
        }
    }

    /// **NOTES is the last card, for every kind that shows one, measured by
    /// where it was PAINTED.**
    ///
    /// The user asked for "Notes should be always last section". It used to be
    /// drawn between PREVIOUS PASSWORDS and AUTOFILL TARGETS, so on a login it
    /// had two cards under it and on an app-bound item three.
    ///
    /// **Painted vertical position, not source order.** A test that read the
    /// order of the `card(..)` calls would be a restatement of the change; this
    /// one takes the top edge of every heading the frame really drew and
    /// insists NOTES has the largest. And the count is asserted FIRST: egui
    /// culls a card that has been pushed off the screen rect entirely, and a
    /// pane that drew only NOTES would satisfy "NOTES is last" while having
    /// lost every other card.
    ///
    /// **Run twice per kind: unbound, and bound to an app.** Removing the
    /// MATCHED APP card from unbound items (see `app_card_visible`) changes
    /// which card is last, so an ordering claim proved on one shape only is
    /// half a claim. `DetailBody::NotesOnly` -- a secure note, whose only card
    /// in the unbound pass IS its note -- is in the loop for the same reason:
    /// it must hold by the rule and not by there being nothing to be last of.
    ///
    /// **The login case is the loaded one, and it used not to be.** NOTES was
    /// moved out from between PREVIOUS PASSWORDS and AUTOFILL TARGETS -- and
    /// `a_noted_item` carries neither a website nor a password history, so
    /// neither of those two cards was in the frame and the ordering was proved
    /// against every card except the two it is about. The `Login` pass now
    /// uses a fixture that draws both, and asserts by name that it did.
    #[test]
    fn notes_is_the_last_card_for_every_kind_that_shows_it() {
        let mut visited = 0;
        for kind in EVERY_KIND {
            for bound in [false, true] {
                let base = match kind {
                    ItemKind::Login => a_noted_login_with_targets_and_history(),
                    _ => a_noted_item(item_type_for(kind)),
                };
                let item = if bound {
                    bound_to(&base, &a_desktop_match())
                } else {
                    base
                };
                let mut pane = Pane::new();
                let frame = pane.idle(&item, &TotpState::NoSecret);
                let headings = frame.card_headings();

                // THE COUNT, BEFORE ANY GEOMETRY. A culled card is no card.
                let expected = expected_card_count(kind, bound);
                assert_eq!(
                    headings.len(),
                    expected,
                    "{kind:?} (bound: {bound}) painted {:?}, expected {expected} cards; the \
                     whole pane painted {:?}",
                    headings.iter().map(|(h, _)| *h).collect::<Vec<_>>(),
                    frame.strings()
                );
                assert!(
                    headings.iter().any(|(h, _)| *h == "NOTES"),
                    "{kind:?} (bound: {bound}) drew no NOTES card at all"
                );
                if bound {
                    // The control on the control: with a binding there really
                    // IS another card, so "NOTES is last" is not being
                    // satisfied by NOTES being alone.
                    assert!(
                        headings.len() >= 2,
                        "{kind:?} bound to an app drew only {:?}",
                        headings
                    );
                }
                if kind == ItemKind::Login {
                    // The two cards NOTES was moved out from between. Named,
                    // because the whole point of the login fixture is that
                    // they are in the frame this assertion is made against --
                    // a count alone would be satisfied by two other cards.
                    for heading in ["AUTOFILL TARGETS", "PREVIOUS PASSWORDS"] {
                        assert!(
                            headings.iter().any(|(h, _)| *h == heading),
                            "the login fixture drew no {heading} card, so 'NOTES is below \
                             it' is not being asserted at all; cards: {:?}",
                            headings.iter().map(|(h, _)| *h).collect::<Vec<_>>()
                        );
                    }
                }

                let (last, last_rect) = *headings.last().expect("no cards at all");
                assert_eq!(
                    last, "NOTES",
                    "{kind:?} (bound: {bound}): the bottom-most card is {last:?} at y = {}, \
                     not NOTES; cards top-down: {:?}",
                    last_rect.top(),
                    headings
                );
                visited += 1;
            }
        }
        assert_eq!(
            visited,
            EVERY_KIND.len() * 2,
            "the loop visited {visited} panes, not {}",
            EVERY_KIND.len() * 2
        );
        assert!(visited > 0, "the loop visited nothing and passed green");
    }

    /// How many cards `an_item` of this kind draws. Written out rather than
    /// derived from the pane, because a count read back off the thing being
    /// measured agrees with itself whatever it does.
    fn expected_card_count(kind: ItemKind, bound: bool) -> usize {
        // NOTES, always -- `an_item` carries a note for every kind.
        let body = match kind {
            // A secure note's body IS its note: no body card of its own.
            ItemKind::SecureNote => 0,
            // LOGIN CREDENTIALS, plus the AUTOFILL TARGETS and PREVIOUS
            // PASSWORDS that `a_noted_login_with_targets_and_history` exists
            // to put on the pane.
            ItemKind::Login => 3,
            _ => 1,
        };
        body + 1 + usize::from(bound)
    }

    /// A login that draws **every card a login can**, carrying a note: a
    /// website (so AUTOFILL TARGETS is painted), two previous passwords (so
    /// PREVIOUS PASSWORDS is), and credentials.
    ///
    /// This is the fixture `notes_is_the_last_card_for_every_kind_that_shows_
    /// it` needs and `a_noted_item` is not: NOTES used to be drawn BETWEEN
    /// those two cards, and a fixture without them proves the ordering
    /// against every card except the two the change is about.
    fn a_noted_login_with_targets_and_history() -> VaultItem {
        let mut item = a_login_with_history(2);
        item.notes = Some(A_NOTE.to_string().into());
        item
    }

    /// `an_item` of `kind` carrying a note, which is what a NOTES card needs
    /// and what `an_item` does not supply on its own.
    fn a_noted_item(item_type: Option<i64>) -> VaultItem {
        let mut item = an_item(item_type);
        item.notes = Some(A_NOTE.to_string().into());
        item
    }

    /// Long enough that a drag across part of it is unambiguously partial.
    const A_NOTE: &str = "Recovery codes are in the safe on the second floor.";

    /// **A click on the note copies the whole note.** The user's "full (on
    /// click)".
    ///
    /// Both halves in one frame, as `clicking_the_website_link_opens_it_
    /// without_copying` does: the action reported, and -- on the next frame,
    /// which is where a toast raised by a click is painted -- the
    /// confirmation naming the field.
    #[test]
    fn clicking_a_note_copies_all_of_it() {
        let item = a_noted_item(Some(1));
        let notes = notes_text(&item)
            .expect("the fixture has no note, so this proves nothing")
            .to_string();
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        // The glyphs themselves, not the padding around them: a click that
        // landed on the card's margin would be answered by the tile and would
        // never test whether the text swallows a plain click.
        let text = laid_out.rect_of(&notes);
        let clicked = pane.click(&item, &TotpState::NoSecret, text.center());
        assert_eq!(
            clicked.action,
            DetailAction::CopyValue(notes.clone()),
            "clicking the note's own glyphs reported {:?}; the note is {notes:?}",
            clicked.action
        );
        let after = pane.idle(&item, &TotpState::NoSecret);
        assert!(
            after.painted(&copy_toast_text(NOTES_COPY_LABEL)),
            "the note copied without saying so; painted: {:?}",
            after.strings()
        );

        // The other half of the tile: a click on the card's padding, beside
        // the glyphs, copies too. Without this the change would pass while
        // only the text were clickable, which is not what the surrounding
        // rows do.
        let card = laid_out.filled_box_around(text, theme::CARD);
        let padding = egui::pos2(text.center().x, card.bottom() - 4.0);
        assert!(
            !text.contains(padding),
            "the 'padding' point {padding:?} is inside the glyphs {text:?}"
        );
        let mut pane = Pane::new();
        let _ = pane.idle(&item, &TotpState::NoSecret);
        let padded = pane.click(&item, &TotpState::NoSecret, padding);
        assert_eq!(
            padded.action,
            DetailAction::CopyValue(notes.clone()),
            "clicking the note card beside its glyphs reported {:?}",
            padded.action
        );
    }

    /// **A drag across the note selects part of it and copies nothing.** The
    /// user's "or partially".
    ///
    /// This is the half the tile could have swallowed. `copy_row`'s layering
    /// gives the topmost widget the click, and the read-only `TextEdit` IS
    /// topmost, so the drag reaches egui's own selection machinery; the tile
    /// under it must not turn that gesture into a copy.
    ///
    /// Both halves in the frame the release lands in: a real character range
    /// is selected, and no copy was reported.
    #[test]
    fn a_note_can_be_selected_without_copying_the_whole_thing() {
        let item = a_noted_item(Some(1));
        let notes = notes_text(&item).expect("the fixture has no note").to_string();
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        let text = laid_out.rect_of(&notes);
        // A press near the start of the run and a release well along it --
        // far enough that egui calls it a drag rather than a click.
        let from = egui::pos2(text.left() + 2.0, text.center().y);
        let to = egui::pos2(text.left() + text.width() * 0.6, text.center().y);
        assert!(
            (to.x - from.x) > 30.0,
            "the drag is only {}pt long, which egui may still call a click",
            to.x - from.x
        );

        let pressed = pane.frame(
            &item,
            &TotpState::NoSecret,
            vec![
                egui::Event::PointerMoved(from),
                egui::Event::PointerButton {
                    pos: from,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert_eq!(
            pressed.action,
            DetailAction::None,
            "the press alone already copied"
        );
        let dragged = pane.frame(
            &item,
            &TotpState::NoSecret,
            vec![egui::Event::PointerMoved(to)],
        );
        let released = pane.frame(
            &item,
            &TotpState::NoSecret,
            vec![
                egui::Event::PointerMoved(to),
                egui::Event::PointerButton {
                    pos: to,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let _ = dragged;

        // HALF ONE: a real, PARTIAL selection exists.
        let range = notes_selection(&pane.ctx, &item.id)
            .expect("the note carries no text cursor at all, so nothing can be selected");
        let selected = range.primary.index.0.abs_diff(range.secondary.index.0);
        assert!(
            selected > 0,
            "the drag selected nothing: the cursor sits at {range:?}"
        );
        assert!(
            selected < notes.chars().count(),
            "the drag selected the whole note ({selected} chars), so 'partially' is not \
             distinguishable from 'fully'"
        );

        // HALF TWO: and it copied nothing.
        assert_eq!(
            released.action,
            DetailAction::None,
            "a selection drag across the note reported {:?} -- the copy tile swallowed it",
            released.action
        );
        let after = pane.idle(&item, &TotpState::NoSecret);
        assert!(
            !after.painted(&copy_toast_text(NOTES_COPY_LABEL)),
            "a selection drag raised a copy confirmation; painted: {:?}",
            after.strings()
        );
    }

    /// The note's text cursor for ONE item, straight out of egui's own
    /// `TextEdit` state -- which is the only place a selection exists, and the
    /// reason the widget is given a named id.
    ///
    /// The item id is a parameter rather than baked in precisely because the
    /// widget id carries it: that is what makes
    /// `a_notes_selection_does_not_follow_the_user_to_the_next_item` able to
    /// ask the question "and what does the OTHER item's note think" at all.
    fn notes_selection(
        ctx: &egui::Context,
        item_id: &str,
    ) -> Option<egui::text_selection::CCursorRange> {
        egui::text_edit::TextEditState::load(ctx, notes_text_id(item_id))?.cursor.char_range()
    }

    /// A second noted item, distinct from `a_noted_item(Some(1))` in the only
    /// two ways that matter here: **a different id, and a different note.**
    ///
    /// The NAME is deliberately left alone -- both are `an_item`'s "Sample" --
    /// so an id salted with the item's name rather than its id is not told
    /// apart from the real thing by accident.
    fn another_noted_item() -> VaultItem {
        let mut item = a_noted_item(Some(1));
        item.id = "id-2".to_string();
        item.notes = Some(ANOTHER_NOTE.to_string().into());
        item
    }

    /// Long, and sharing no prefix with [`A_NOTE`], so a range measured on one
    /// is meaningless on the other.
    const ANOTHER_NOTE: &str = "Spare key with the neighbour at number eleven.";

    /// **A selection made in one item's note does not follow the user to the
    /// next item.**
    ///
    /// The note's `TextEdit` id used to be one global constant, so egui's
    /// `TextEditState` -- which is keyed on that id -- was shared by every
    /// item this pane ever showed. Drag thirty characters of one note, click
    /// the next item, and its note came up with its first thirty characters
    /// already selected: a selection over text the user had not read, on the
    /// copy target of the whole card. See [`NOTES_TEXT_ID_SALT`].
    ///
    /// **Three assertions, and the first two are the controls.** A test that
    /// only asked the second item for a selection would pass just as well
    /// against a pane that had stopped drawing notes at all, or against one
    /// where the drag never selected anything in the first place.
    #[test]
    fn a_notes_selection_does_not_follow_the_user_to_the_next_item() {
        let first = a_noted_item(Some(1));
        let second = another_noted_item();
        assert_ne!(first.id, second.id, "the two fixtures are the same item");
        assert_eq!(
            first.name, second.name,
            "the fixtures differ by name too, so an id salted with the NAME would pass"
        );

        let mut pane = Pane::new();
        let laid_out = pane.idle(&first, &TotpState::NoSecret);
        let text = laid_out.rect_of(A_NOTE);
        let from = egui::pos2(text.left() + 2.0, text.center().y);
        let to = egui::pos2(text.left() + text.width() * 0.6, text.center().y);
        let _ = pane.frame(
            &first,
            &TotpState::NoSecret,
            vec![
                egui::Event::PointerMoved(from),
                egui::Event::PointerButton {
                    pos: from,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let _ = pane.frame(&first, &TotpState::NoSecret, vec![egui::Event::PointerMoved(to)]);
        let _ = pane.frame(
            &first,
            &TotpState::NoSecret,
            vec![
                egui::Event::PointerMoved(to),
                egui::Event::PointerButton {
                    pos: to,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );

        // CONTROL ONE: the drag really did select part of the first note.
        // Without this the whole test is satisfied by a pane on which nothing
        // is ever selectable.
        let selected = notes_selection(&pane.ctx, &first.id)
            .map(|r| r.primary.index.0.abs_diff(r.secondary.index.0))
            .unwrap_or(0);
        assert!(
            selected > 0,
            "the drag selected nothing in the first item's note, so there is no state              for the second item to inherit and this test proves nothing"
        );

        // The pane is now handed a DIFFERENT item -- the list click, as far as
        // this function can see it.
        let switched = pane.idle(&second, &TotpState::NoSecret);

        // CONTROL TWO: the second item's note really is on screen. egui culls
        // a shape outside the screen rect entirely, and a pane that drew no
        // note at all would carry no selection either.
        assert!(
            switched.painted(ANOTHER_NOTE),
            "the second item's note was never painted, so 'no selection' is not a              claim about a note; the frame painted: {:?}",
            switched.strings()
        );
        assert!(
            !switched.painted(A_NOTE),
            "the pane is still painting the FIRST item's note, so it was never handed              the second one"
        );

        // THE FINDING: nothing is selected in it.
        let inherited = notes_selection(&pane.ctx, &second.id);
        let inherited_len = inherited
            .map(|r| r.primary.index.0.abs_diff(r.secondary.index.0))
            .unwrap_or(0);
        assert_eq!(
            inherited_len, 0,
            "the second item's note came up with {inherited_len} characters already              selected ({inherited:?}) -- the first item's selection followed the user"
        );

        // And the first item's own selection is still its own: the sharing is
        // gone because the ids differ, not because the state was wiped.
        let kept = notes_selection(&pane.ctx, &first.id)
            .map(|r| r.primary.index.0.abs_diff(r.secondary.index.0))
            .unwrap_or(0);
        assert_eq!(
            kept, selected,
            "the first item's selection was destroyed rather than kept apart, so              coming back to it loses the caret"
        );
    }

    /// **Every chord this pane owns still fires while the note holds keyboard
    /// focus.**
    ///
    /// The note is a multiline `TextEdit` and a focused one is a keyboard
    /// consumer: egui's own text machinery takes the arrows, Home/End and
    /// Ctrl+A out of the event queue before anything downstream sees them.
    /// Nothing had ever asked whether CTRL+B, CTRL+U and CTRL+SHIFT+U survive
    /// that.
    ///
    /// They do, and the reason is ORDER, not luck: [`draw_detail_read`] calls
    /// `consume_chord` at the top of the function, before a single card is
    /// laid out, so the chord is out of the queue before the note exists this
    /// frame. This test is what makes that an assertion rather than a reading
    /// of the source -- and what fails if a later change either moves the
    /// consumption below the body or teaches it to stand down when something
    /// has focus.
    ///
    /// The focus is asserted, not assumed: a test that thought it had focused
    /// the note and had not would be an ordinary chord test wearing a
    /// misleading name.
    #[test]
    fn every_pane_chord_still_fires_while_the_note_holds_focus() {
        let mut item = a_login();
        item.notes = Some(A_NOTE.to_string().into());

        let mut fired = 0;
        for (chord, events, expected) in [
            ("CTRL+B", ctrl(egui::Key::B), DetailAction::CopyPassword),
            ("CTRL+U", ctrl(egui::Key::U), DetailAction::CopyUsername),
            (
                "CTRL+SHIFT+U",
                vec![egui::Event::Key {
                    key: egui::Key::U,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                }],
                DetailAction::CopyValue(WEBSITE.to_string()),
            ),
        ] {
            let mut pane = Pane::new();
            let laid_out = pane.idle(&item, &TotpState::NoSecret);
            assert!(
                laid_out.painted(A_NOTE),
                "{chord}: the fixture drew no note at all, so nothing can hold focus"
            );

            // The note is focused the way the window's own Ctrl+K focuses the
            // search box -- through egui's memory -- and then given a frame to
            // take it up.
            pane.ctx.memory_mut(|m| m.request_focus(notes_text_id(&item.id)));
            let _ = pane.idle(&item, &TotpState::NoSecret);
            assert!(
                pane.ctx.memory(|m| m.has_focus(notes_text_id(&item.id))),
                "{chord}: the note never took keyboard focus, so this frame is not the                  case this test is named for"
            );

            let after = pane.frame(&item, &TotpState::NoSecret, events);
            assert_eq!(
                after.action, expected,
                "{chord} reported {:?} while the note had focus -- the text field ate it",
                after.action
            );
            // Still focused afterwards: the chord was answered without the
            // pane having to take the note's focus away to do it, which would
            // move the user's caret out from under them.
            assert!(
                pane.ctx.memory(|m| m.has_focus(notes_text_id(&item.id))),
                "{chord} stole the note's focus to fire"
            );
            fired += 1;
        }
        assert_eq!(fired, 3, "the chord loop visited {fired} chords, not 3");
    }

    /// **An empty password draws no row: no label, no bullets, no eye, and no
    /// space where it was.** The user: "if password is empty - show no mask
    /// (and no field at all)".
    ///
    /// Drawing it invisible is not the same as not drawing it, so this asserts
    /// three separate things a transparent or zero-size row would each fail:
    /// the label is absent from the painted frame, the card is SHORTER by at
    /// least a whole row, and everything below the card moved up by exactly
    /// that much.
    #[test]
    fn an_empty_password_draws_no_row_at_all() {
        let label = copy_shortcut_label(CopyShortcut::Password);
        let full = a_login();
        let empty = a_login_with_no_credentials();

        let mut pane = Pane::new();
        let with = pane.idle(&full, &TotpState::NoSecret);
        let mut pane = Pane::new();
        let without = pane.idle(&empty, &TotpState::NoSecret);

        // The premise: the pane really is drawing the card in both passes, so
        // an absent label cannot be "the pane drew nothing".
        for frame in [&with, &without] {
            assert!(
                frame.painted("LOGIN CREDENTIALS"),
                "the login card was not drawn at all: {:?}",
                frame.strings()
            );
            assert!(
                frame.painted("Username"),
                "the username row was not drawn either: {:?}",
                frame.strings()
            );
        }

        // 1. THE LABEL IS GONE -- not merely the bullets.
        assert!(
            !without.painted(label),
            "an empty password still paints its {label:?} label: {:?}",
            without.strings()
        );
        assert!(
            !without.strings().iter().any(|t| t.starts_with('\u{2022}')),
            "an empty password still paints a mask: {:?}",
            without.strings()
        );

        // 2. THE CARD IS SHORTER BY A WHOLE ROW.
        let card_with = with.filled_box_around(with.rect_of("LOGIN CREDENTIALS"), theme::CARD);
        let card_without =
            without.filled_box_around(without.rect_of("LOGIN CREDENTIALS"), theme::CARD);
        let shrank = card_with.height() - card_without.height();
        let a_row = ROW_CONTENT_HEIGHT + 2.0 * f32::from(ROW_PAD_Y);
        assert!(
            shrank >= a_row,
            "the card lost only {shrank}pt where a row is {a_row}pt -- the row is still \
             taking its space, so it was hidden rather than skipped"
        );

        // 3. AND NO HAIRLINE LEFT STANDING OVER THE GAP. A row rule is a 1pt
        //    `theme::CANVAS` fill (see `theme::row_rule`), and the login card
        //    has exactly one -- between Username and Password -- when both rows
        //    are there and none at all when only Username is. A separator with
        //    nothing on one side of it is the other way a removed row leaves a
        //    trace, and it costs about a point, which the height check above is
        //    too coarse to see.
        let rules = |frame: &Frame, card: egui::Rect| {
            frame
                .rects
                .iter()
                .filter(|(rect, fill)| {
                    *fill == theme::CANVAS
                        && (rect.height() - 1.0).abs() < 0.5
                        && card.contains_rect(*rect)
                })
                .count()
        };
        assert_eq!(
            rules(&with, card_with),
            1,
            "the two-row login card does not draw one row rule, so counting them proves \r
             nothing"
        );
        assert_eq!(
            rules(&without, card_without),
            0,
            "a separator is still drawn where the password row was"
        );

        // 4. AND EVERYTHING BELOW MOVED UP BY EXACTLY THAT MUCH -- no gap
        // left behind, no separator standing over nothing.
        assert_eq!(
            with.rect_of("AUTOFILL TARGETS").top() - without.rect_of("AUTOFILL TARGETS").top(),
            shrank,
            "the cards below the login card did not move up by the row's whole height"
        );
    }

    /// **`masked_row` itself, called with nothing to hide, allocates nothing
    /// at all.**
    ///
    /// This exists because deleting `masked_row`'s own guard left all 1828
    /// tests green. Every caller gates the row on `masked_row_visible` too --
    /// it has to, because the hairline above the row goes with the row -- so
    /// no pane test can reach the function with an empty value, and the rule
    /// inside it was unreachable code that a mutant could delete for free.
    /// That is this file's standing defect exactly: a change correct in
    /// isolation that does not reach the behaviour it claims.
    ///
    /// So the function is called DIRECTLY, on a bare `Ui`, with no caller to
    /// have already decided. What is measured is the height the row took and
    /// the strings it painted -- not a flag it returned.
    #[test]
    fn masked_row_lays_out_nothing_at_all_for_an_empty_value() {
        // (value, expected rows painted) -- the negative and its control, in
        // one loop over one harness, so neither can pass on a difference in
        // how it was set up.
        let mut checked = 0;
        let mut heights = Vec::new();
        for (value, should_draw) in [("", false), ("hunter2", true)] {
            checked += 1;
            let ctx = egui::Context::default();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(PANE, PANE),
                )),
                ..Default::default()
            };
            let _ = ctx.run_ui(input.clone(), |_ui| {});
            theme::apply(&ctx);
            let _ = ctx.run_ui(input.clone(), |_ui| {});

            let mut taken = 0.0;
            let mut revealed = false;
            let mut action = DetailAction::None;
            let output = ctx.run_ui(input, |ui| {
                let before = ui.min_rect().height();
                masked_row(
                    ui,
                    "Password",
                    value,
                    &mut revealed,
                    &mut action,
                    DetailAction::CopyPassword,
                    Some(CopyShortcut::Password),
                );
                taken = ui.min_rect().height() - before;
            });
            let all =
                egui::Shape::Vec(output.shapes.iter().map(|c| c.shape.clone()).collect());
            let mut texts = Vec::new();
            collect_text_rects(&all, &mut texts);
            let strings: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
            let eyes = theme::icon_probe::eyes(&all);
            heights.push(taken);

            if should_draw {
                assert!(
                    strings.contains(&"Password"),
                    "a real value drew no label: {strings:?}"
                );
                assert!(
                    strings.contains(&"\u{2022}".repeat(MASKED_BULLETS).as_str()),
                    "a real value drew no {MASKED_BULLETS}-bullet mask: {strings:?}"
                );
                assert_eq!(eyes.len(), 1, "a real value offers no reveal eye");
                assert!(taken > 0.0, "a real value took no vertical space at all");
            } else {
                assert!(
                    strings.is_empty(),
                    "an empty value painted {strings:?} -- the row was drawn, not skipped"
                );
                assert_eq!(
                    eyes.len(),
                    0,
                    "an empty value still offers an eye to reveal it"
                );
                assert_eq!(
                    taken, 0.0,
                    "an empty value took {taken}pt of vertical space, so it was hidden \
                     rather than skipped"
                );
            }
        }
        assert_eq!(checked, 2, "the loop visited nothing");
        assert!(
            heights[1] > heights[0],
            "both passes took the same {:?}pt, so the harness is measuring nothing",
            heights
        );
    }

    /// The positive control for the rule above: a password that IS there still
    /// draws its whole row -- label, the full bullet run, and the reveal eye.
    /// Without it "hide an empty password" is satisfied by hiding every
    /// password.
    #[test]
    fn a_non_empty_password_still_draws_its_whole_row() {
        let label = copy_shortcut_label(CopyShortcut::Password);
        let mut pane = Pane::new();
        let frame = pane.idle(&a_login(), &TotpState::NoSecret);
        assert!(
            frame.painted(label),
            "a real password lost its {label:?} label: {:?}",
            frame.strings()
        );
        assert!(
            frame.painted(&"\u{2022}".repeat(MASKED_BULLETS)),
            "a real password draws no {MASKED_BULLETS}-bullet mask: {:?}",
            frame.strings()
        );
        assert_eq!(
            frame.eyes.len(),
            1,
            "a real password row offers {} reveal eyes, not one",
            frame.eyes.len()
        );
    }

    /// **The reveal eye is not offered for an empty value**, kept as its own
    /// assertion even though the row's absence implies it: it is cheap, and it
    /// fails loudly against the one wrong way to satisfy the rule -- drawing
    /// the row transparent or zero-size, which leaves the eye in the shape
    /// tree where `theme::icon_probe` finds it by geometry rather than by any
    /// string.
    #[test]
    fn the_reveal_eye_is_not_offered_for_an_empty_value() {
        let mut pane = Pane::new();
        let frame = pane.idle(&a_login_with_no_credentials(), &TotpState::NoSecret);
        assert!(
            frame.painted("LOGIN CREDENTIALS"),
            "the pane drew no card at all, so an absent eye proves nothing: {:?}",
            frame.strings()
        );
        assert_eq!(
            frame.eyes.len(),
            0,
            "an item with no password still offers {} eye(s) to reveal it",
            frame.eyes.len()
        );
        // The control, on the same probe: an item that HAS one gets an eye,
        // so zero above is a decision and not a probe that never finds any.
        let mut pane = Pane::new();
        let filled = pane.idle(&a_login(), &TotpState::NoSecret);
        assert_eq!(filled.eyes.len(), 1, "the eye probe finds nothing at all");
    }

    /// **The read pane shows no MATCHED APP card when nothing is bound.**
    /// The user: "Matched app - same, no show if not present".
    ///
    /// The heading's absence AND the height: an alpha-0 or zero-size card
    /// would leave the pane exactly as tall, so the second half is what
    /// separates "not drawn" from "drawn invisibly".
    #[test]
    fn an_item_with_no_app_binding_draws_no_matched_app_card() {
        let mut unbound = a_login();
        unbound.notes = Some(A_NOTE.to_string().into());
        let bound = bound_to(&unbound, &a_desktop_match());
        assert!(
            !crate::vault_bridge::has_app_match_field(&unbound),
            "the fixture already carries a binding"
        );

        let mut pane = Pane::new();
        let without = pane.idle(&unbound, &TotpState::NoSecret);
        let mut pane = Pane::new();
        let with = pane.idle(&bound, &TotpState::NoSecret);

        assert!(
            !without.painted(APP_CARD_HEADING),
            "an unbound item still draws the {APP_CARD_HEADING:?} card: {:?}",
            without.strings()
        );
        // And none of its contents by another name.
        // The card's own row labels. Deliberately NOT a string out of
        // `detail_edit.rs`: that file is another implementer's and is mid-edit,
        // and a needle here that only exists in its working copy would make
        // this test green for a reason outside this change.
        for absent in ["App", "Program file"] {
            assert!(
                !without.painted(absent),
                "an unbound item still draws the card's {absent:?}: {:?}",
                without.strings()
            );
        }

        // The height. NOTES is the last card on both panes, so where its
        // heading sits is where the pane's contents end.
        let gap = without.rect_of("NOTES").top() - with.rect_of("NOTES").top();
        assert!(
            gap < 0.0,
            "the unbound pane is not shorter: NOTES sits at {} with a binding and {} \
             without one, so the card is still taking its space",
            with.rect_of("NOTES").top(),
            without.rect_of("NOTES").top()
        );
        // What disappeared, MEASURED off the bound pane rather than computed
        // from the constants: the card's own painted box, plus the gap between
        // its bottom edge and the card that follows it. `CARD_GAP` is not that
        // gap on its own -- the cards also carry the scroll area's item
        // spacing -- which is why this is read and not stated.
        let card = with.filled_box_around(with.rect_of(APP_CARD_HEADING), theme::CARD);
        let next = with.filled_box_around(with.rect_of("NOTES"), theme::CARD);
        let expected = card.height() + (next.top() - card.bottom());
        assert!(
            expected > card.height(),
            "the cards are not separated at all, so this is not the measurement it says"
        );
        assert_eq!(
            -gap, expected,
            "the pane shrank by {}pt where the card and the gap after it are {expected}pt \
             -- a hole was left where the card used to be",
            -gap
        );
    }

    /// The positive control: an item that IS bound still shows every part of
    /// the card. Without it the hiding rule is satisfied by hiding the card
    /// always.
    #[test]
    fn an_item_with_an_app_binding_still_draws_it() {
        let bound = bound_to(&a_login(), &a_desktop_match());
        let mut pane = Pane::new();
        let frame = pane.idle(&bound, &TotpState::NoSecret);
        let mut checked = 0;
        for needle in [APP_CARD_HEADING, "App", "Program file", "Remove"] {
            checked += 1;
            assert!(
                frame.painted(needle),
                "a bound item is missing the card's {needle:?}: {:?}",
                frame.strings()
            );
        }
        assert_eq!(checked, 4, "the loop asserted nothing");
    }

    /// **The exception, and exactly as much of it as this change owns.**
    ///
    /// The read pane stops offering the MATCHED APP card when nothing is
    /// bound. The edit and add forms must go on offering their app control,
    /// because that is the only place a binding is made -- and that control
    /// lives in `detail_edit.rs`, which this change does not touch and which
    /// another implementer holds. So what is asserted here is the half that
    /// is mine: the read pane's gate has NOT reached the edit form.
    ///
    /// * The block is still drawn by the edit form, rendered rather than read
    ///   off the source -- so a `draw_detail_edit` that had lost it fails
    ///   here.
    /// * And `app_card_visible` -- the predicate that hides the read card --
    ///   is named nowhere in that file, which is the mutation this test
    ///   exists for: gating the edit form on the read pane's rule would hide
    ///   the one control that can create a binding, and the app would have no
    ///   way to bind an app at all.
    ///
    /// The unbound half of the edit form -- an add affordance on a draft that
    /// carries no binding -- is `detail_edit.rs`'s own to make and to guard;
    /// see its `app_add_block`. It is deliberately NOT asserted from here,
    /// because a test in this file that depended on a symbol that file has
    /// not landed yet would be green for a reason outside this change.
    #[test]
    fn the_edit_form_offers_the_app_control_even_with_nothing_bound() {
        use crate::vault_window::detail_edit;
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(PANE, PANE),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input.clone(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input.clone(), |_ui| {});

        let bound = bound_to(&a_login(), &a_desktop_match());
        let mut draft = detail_edit::EditDraft::from_item(&bound);
        // The premise: the form really was handed a binding to draw.
        assert!(
            draft.app.is_some(),
            "the fixture gave the edit form no app draft, so this proves nothing"
        );

        let mut apps = crate::app_identity::AppIdentityCache::default();
        let output = ctx.run_ui(input, |ui| {
            let _ = detail_edit::draw_detail_edit(
                ui,
                &mut draft,
                &[],
                false,
                &mut apps,
                Some(&bound),
                &TotpState::NoSecret,
            );
        });
        let all = egui::Shape::Vec(output.shapes.iter().map(|c| c.shape.clone()).collect());
        let mut texts = Vec::new();
        collect_text_rects(&all, &mut texts);
        let strings: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
        assert!(
            strings.contains(&detail_edit::APP_BLOCK_HEADING),
            "the edit form no longer draws {:?}, so there is no app control there at \
             all: {strings:?}",
            detail_edit::APP_BLOCK_HEADING
        );

        // And the read pane's gate is not in that file.
        let edit_source = include_str!("detail_edit.rs");
        assert!(
            !edit_source.contains("app_card_visible"),
            "the read pane's `app_card_visible` has been wired into the edit form, which \
             is where a binding is created"
        );
        // The control on that scan: it can find a name that IS there, so an
        // absence above is an absence and not a scan that matches nothing.
        assert!(edit_source.contains("APP_BLOCK_HEADING"));
    }

    /// **"9s left - show just 9s".** The decision, called directly.
    #[test]
    fn totp_countdown_reads_just_the_number_and_the_unit() {
        assert_eq!(totp_countdown_text(9), "9s");
        // The two ends, checked rather than assumed to read sensibly.
        assert_eq!(totp_countdown_text(30), "30s");
        assert_eq!(totp_countdown_text(0), "0s");
        // ... and the wording it replaced is gone, from every value.
        for n in 0..=30u8 {
            let text = totp_countdown_text(n);
            assert!(
                !text.contains("left"),
                "the countdown reads {text:?} at {n} seconds"
            );
            assert_eq!(text, format!("{n}s"));
        }
    }

    /// The same, off the painted frame -- so the short string is what the row
    /// really draws and not merely what a helper returns. The row's other two
    /// parts are asserted alongside it: a countdown column that changed width
    /// must not have pushed the code or the track out of the card.
    #[test]
    fn a_totp_countdown_paints_just_the_seconds() {
        let (item, totp) = a_login_with_a_code();
        let mut pane = Pane::new();
        let frame = pane.idle(&item, &totp);
        let seconds = match &totp {
            TotpState::Code { seconds_left, .. } => *seconds_left,
            other => panic!("the fixture is {other:?}, which draws no countdown"),
        };
        assert!(
            frame.painted(&format!("{seconds}s")),
            "the countdown does not read {seconds}s; painted: {:?}",
            frame.strings()
        );
        assert!(
            !frame.strings().iter().any(|t| t.contains("s left")),
            "the countdown still says \"s left\": {:?}",
            frame.strings()
        );
        // The layout, MEASURED: the whole row is still inside its card, and
        // the code is still to the left of the countdown with the track
        // between them.
        let card = frame.filled_box_around(frame.rect_of("LOGIN CREDENTIALS"), theme::CARD);
        let countdown = frame.rect_of(&format!("{seconds}s"));
        assert!(
            card.contains_rect(countdown),
            "the countdown at {countdown:?} is outside its card {card:?}"
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

    /// **The pills are gone from this card, and the setting is global.**
    ///
    /// This replaces `clicking_a_trigger_pill_reports_the_mode_it_names`,
    /// which drove the three pills and asserted the action each one reported.
    /// What a matched window does is
    /// `settings::Settings::prompt_on_match` now -- one switch in
    /// Preferences -- so a per-item control here would write a field nothing
    /// reads, supersede the item's `revisionDate` for it, and change nothing
    /// the user can observe.
    ///
    /// Driven at the pane rather than asserted on `app_match_card`'s source,
    /// and on a LIVE binding, because a live binding is precisely where all
    /// three were painted before this change.
    #[test]
    fn no_binding_gets_trigger_pills_because_the_setting_is_global() {
        let item = bound_to(&a_login(), &a_desktop_match());
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);

        // The premise: this pane IS drawing the card, so the absences below
        // are about the pills and not about a card that vanished.
        assert!(laid_out.painted("MATCHED APP"), "{:?}", laid_out.strings());
        assert!(laid_out.painted(APP_MATCH_BEHAVIOUR_NOTE), "{:?}", laid_out.strings());

        for mode in TRIGGER_ORDER {
            assert!(
                !laid_out.painted(trigger_label(mode)),
                "the {mode:?} pill is still drawn on the MATCHED APP card: {:?}",
                laid_out.strings()
            );
        }
        assert!(
            !laid_out.painted("Autofill"),
            "the row the pills sat in is still drawn, so the card still presents a per-item              autofill setting: {:?}",
            laid_out.strings()
        );

        // And nothing on the card reports a trigger change either -- a pill
        // drawn with a zero-size label would pass every assertion above.
        //
        // Spelled as "this is the action it DOES report" rather than as "it is
        // not the trigger variant": that variant is gone from the enum, so a
        // test naming it would not compile, and a negative would be satisfied
        // by any wrong action at all.
        let note = pane.click(
            &item,
            &TotpState::NoSecret,
            laid_out.rect_of(APP_MATCH_BEHAVIOUR_NOTE).center(),
        );
        assert_eq!(
            note.action,
            DetailAction::None,
            "clicking the behaviour note reported {:?}",
            note.action
        );
        // The App row IS clickable -- it copies the process name -- so what is
        // asserted about it is that copying is the WHOLE of what it does.
        let row = pane.click(&item, &TotpState::NoSecret, laid_out.rect_of("App").center());
        assert_eq!(
            row.action,
            DetailAction::CopyValue(a_desktop_match().process),
            "clicking the App row reported {:?}, which is not the copy it is there for",
            row.action
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
            sequence: String::new(),
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
                        sequence: String::new(),
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
            !notes.contains(&APP_MATCH_BEHAVIOUR_NOTE),
            "a dead binding still claims the fill hotkey does something for it: {notes:?}"
        );
        // The controls, on a LIVE match: the behaviour note is there and the
        // dead notice is not, so neither assertion above is satisfied by a
        // constant.
        let live = app_card_notes(&a_store_match());
        assert!(live.contains(&APP_MATCH_BEHAVIOUR_NOTE), "{live:?}");
        assert!(!live.iter().any(|n| n.contains("ignoring")), "{live:?}");
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

        // The control: the SAME pane on a LIVE match paints the behaviour
        // note, so the absences above are about this binding and not about a
        // card that stopped drawing anything.
        //
        // It does NOT paint the pills -- no binding does any more -- so the
        // pill assertions above are held by
        // `no_binding_gets_trigger_pills_because_the_setting_is_global`,
        // which asserts their absence on a LIVE match where their presence
        // was the previous behaviour.
        let live = bound_to(&a_login(), &a_store_match());
        let live_frame = pane.idle(&live, &TotpState::NoSecret);
        assert!(live_frame.painted(APP_MATCH_BEHAVIOUR_NOTE));
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
            value: Some(Zeroizing::new(value.to_string())),
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
            app_card_visible(true),
            "a secure note whose app-match field is corrupt cannot see it"
        );
        assert!(
            !app_card_visible(false),
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

        // The control: an item with NO field gets NO CARD AT ALL, so the
        // assertions above are about the corrupted field and not about a card
        // that now always says both. This used to read "the empty card names
        // Add app..."; the empty card is gone (see `app_card_visible`), and the
        // control is the stronger one that replaced it.
        //
        // Two kinds, because the retired rule was `has_field ||
        // kind_offers_fill(kind)`: a secure note failed it and a login passed
        // it, so a control on the secure note alone would have gone on passing
        // while every login still carried an empty card.
        for kind in [ItemKind::SecureNote, ItemKind::Login] {
            let mut pane = Pane::new();
            let bare = pane.idle(&an_item(item_type_for(kind)), &TotpState::NoSecret);
            assert!(
                !bare.painted(APP_CARD_HEADING),
                "a {kind:?} bound to nothing draws an app card: {:?}",
                bare.strings()
            );
            assert!(
                !bare.strings().iter().any(|t| t.contains("No app is matched")),
                "{kind:?}: {:?}",
                bare.strings()
            );
            assert!(!bare.painted("Remove"), "{kind:?}: {:?}", bare.strings());
            assert!(
                !bare.strings().iter().any(|t| t.contains("cannot be read")),
                "{kind:?}: {:?}",
                bare.strings()
            );
        }
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
        // **`Username` only, and the missing `Password` is the point.** An
        // empty password now draws NO ROW AT ALL (see `masked_row_visible`),
        // which is a strictly stronger promise than the inert row this test
        // was written for -- there is no rect to hover, click or hit. The
        // Password case did not stop being guarded: it moved to
        // `an_empty_password_draws_no_row_at_all`, and the line below keeps it
        // biting here too.
        {
            let mut pane = Pane::new();
            let laid_out = pane.idle(&a_login_with_no_credentials(), &TotpState::NoSecret);
            assert!(
                !laid_out.painted(copy_shortcut_label(CopyShortcut::Password)),
                "an empty password is back to drawing a row; painted: {:?}",
                laid_out.strings()
            );
        }
        for (label, toast) in [("Username", "Username copied")] {
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
        // **`Username` only, and the missing `Password` is the point.** An
        // empty password now draws NO ROW AT ALL (see `masked_row_visible`),
        // which is a strictly stronger promise than the inert row this test
        // was written for -- there is no rect to hover, click or hit. The
        // Password case did not stop being guarded: it moved to
        // `an_empty_password_draws_no_row_at_all`, and the line below keeps it
        // biting here too.
        {
            let mut pane = Pane::new();
            let laid_out = pane.idle(&a_login_with_no_credentials(), &TotpState::NoSecret);
            assert!(
                !laid_out.painted(copy_shortcut_label(CopyShortcut::Password)),
                "an empty password is back to drawing a row; painted: {:?}",
                laid_out.strings()
            );
        }
        for label in ["Username"] {
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
        // **`Username` only, and the missing `Password` is the point.** An
        // empty password now draws NO ROW AT ALL (see `masked_row_visible`),
        // which is a strictly stronger promise than the inert row this test
        // was written for -- there is no rect to hover, click or hit. The
        // Password case did not stop being guarded: it moved to
        // `an_empty_password_draws_no_row_at_all`, and the line below keeps it
        // biting here too.
        {
            let mut pane = Pane::new();
            let laid_out = pane.idle(&a_login_with_no_credentials(), &TotpState::NoSecret);
            assert!(
                !laid_out.painted(copy_shortcut_label(CopyShortcut::Password)),
                "an empty password is back to drawing a row; painted: {:?}",
                laid_out.strings()
            );
        }
        for (label, tooltip) in [("Username", "Click to copy · CTRL+U")] {
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
                totp_secret: false,
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
                totp_secret: false,
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
                totp_secret: false,
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
                totp_secret: false,
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
            totp_secret: false,
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
            totp_secret: false,
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

    // `the_header_primary_button_is_the_designs_34px_filled_control` stood
    // here and is DELETED. Every one of its assertions was about the header's
    // "Fill in app" button -- that a 34px `theme::BLUE`-filled rect existed,
    // that the label was 13px, that a `CTRL+SHIFT+F` hint sat beside it at
    // 10px monospace. The button was removed at the user's request, so there
    // is no subject left: the test could only be made to pass by asserting
    // something else. The strip's remaining controls have their own test
    // immediately below, which no longer measures itself against the button.
    //
    // `theme::header_primary_button` and `header_primary_button_width` were
    // left `pub` and unused by that commit, with no test anywhere in the crate
    // once this one went -- so they have since been DELETED, together with
    // their private galley/width helpers and the `HEADER_PRIMARY_*` numbers.
    // `theme.rs` carries the tombstone. `theme::HEADER_BUTTON_HEIGHT` stays,
    // and is what the band below is now derived from.

    /// The two drawn controls are square at the strip's own 34px control
    /// height, so their HIT TARGETS are the strip's full 34px band rather
    /// than being only as big as the marks they paint.
    ///
    /// A star drawn at its own 18px would look identical in a screenshot and
    /// be half as easy to hit, which is exactly the kind of regression a
    /// shape-drawn control invites: nothing about the painted geometry says
    /// how big the clickable area is.
    ///
    /// **The band used to be read off the header's "Fill in app" button**,
    /// which stood between these two and was the one 34px `theme::BLUE` rect
    /// in the strip. That button was removed at the user's request, so the
    /// band is now derived from [`theme::HEADER_BUTTON_HEIGHT`] -- the very
    /// constant the strip lays itself out with -- centred on each mark. The
    /// defect this was written for is untouched by that change: an 18px star
    /// still fails the edge click 15pt above its own centre, and the two
    /// controls are still required to share one centre line.
    #[test]
    fn the_star_and_the_kebab_share_the_strips_34px_hit_target() {
        let item = a_login();
        let mut pane = Pane::new();
        let frame = pane.idle(&item, &TotpState::NoSecret);
        let star = frame.star().rect;
        let kebab = frame.kebab();
        let band = theme::HEADER_BUTTON_HEIGHT;
        assert_eq!(band, 34.0, "the strip's control height is no longer the design's 34px");

        // The marks are smaller than the band they sit in, so the painted
        // geometry alone cannot say how big the hit target is. Two things
        // can: that both sit on ONE centre line, and that a click near the
        // TOP EDGE of the 34px band -- well outside the marks themselves --
        // still activates each of them.
        assert!(
            (star.center().y - kebab.center().y).abs() <= 0.5,
            "the star and the kebab are not on one centre line: {star:?} {kebab:?}"
        );
        for (name, mark) in [("star", star), ("kebab", kebab)] {
            assert!(
                mark.height() < band,
                "the {name}'s painted mark already fills the whole 34px band, so the \
                 edge click below proves nothing about its hit target"
            );
        }

        let corner =
            |mark: egui::Rect| egui::pos2(mark.center().x, mark.center().y - band / 2.0 + 2.0);

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
    /// [`MIN_PANE`]: 298pt. With four worded buttons in this strip, the
    /// primary ("Fill in app", since removed) was measured painting at
    /// x = -34.5..21.9 -- entirely off the pane -- and "Favourite"
    /// overlapping the item's own title. Nothing caught it, because nothing
    /// tried that width.
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
        // The header's "Fill in app" button was a third entry here until it
        // was removed at the user's request. The two that remain are the two
        // the original defect actually mispainted -- the star sat on the
        // avatar at 27.3..45.9 -- so every clause below still fails against
        // the state this test was written for.
        let controls = [
            ("the favourite star", frame.star().rect),
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

        // 4pt steps through the rearrangement threshold and out the far side
        // of it. There used to be two -- ~420pt, where the controls came back
        // onto the title's line, and ~497pt, where the removed Fill button's
        // shortcut hint returned. With the button and its hint gone the
        // controls are a fixed 82pt, so the one threshold left is ~322pt
        // (82 + 14 + TITLE_MIN, plus the avatar's 58 and the strip's 48 of
        // padding). MIN_PANE is 298, below it, so this sweep still starts on
        // the stacked side and crosses to the other -- which is the whole
        // point of a band.
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

    /// **`header_layout` still has two answers, and the stacked one is still
    /// the answer at the app's own minimum.**
    ///
    /// Removing the header's "Fill in app" button took a rung off this
    /// ladder: the strip's controls used to be a per-item width with a
    /// shortcut hint that could be shed, and are now a fixed star + gap +
    /// kebab. A collapse that far can quietly leave a function with one
    /// reachable branch, and this is the pane whose controls were once
    /// painted at x = -34.5 -- so the branch that keeps them off the title at
    /// 298pt is asserted directly rather than inferred from the sweep above.
    ///
    /// The controls' width is spelled the way `draw_detail_read` spells it,
    /// and the content width the way the strip's `Frame` produces it, so this
    /// cannot pass on numbers the real header does not use.
    #[test]
    fn the_header_stacks_at_the_minimum_pane_and_does_not_on_a_wide_one() {
        let controls = theme::HEADER_BUTTON_HEIGHT * 2.0 + HEADER_GAP;
        let content = |pane: f32| pane - f32::from(HEADER_PAD_X) * 2.0;

        assert!(
            header_layout(content(MIN_PANE), controls).stacked,
            "at the app's minimum the controls and a {TITLE_MIN}pt title are being kept on \
             one line -- which is how the strip painted a control off the pane before"
        );
        assert!(
            !header_layout(content(PANE), controls).stacked,
            "the ordinary {PANE}pt pane is stacking, so the unstacked branch is dead"
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

    // ------------------------------------------------ the TOTP SECRET row

    /// The seed every fixture below hides. Long, distinctive, and not a
    /// lookalike of anything else in this file, so a painted-strings search
    /// for it cannot match some other row by accident.
    const SEED: &str = "JBSWY3DPEHPK3PXPSEEDSEED";

    /// A login with a TOTP seed on it. `a_login` deliberately has `totp:
    /// None`, so this is the only fixture here that can draw the secret row
    /// at all.
    fn a_login_with_a_seed(id: &str, seed: &str) -> VaultItem {
        let mut item = a_login();
        item.id = id.to_string();
        if let Some(login) = item.login.as_mut() {
            login.totp = Some(seed.to_string().into());
        }
        item
    }

    /// A live code, so the pane draws the One-time code row the secret row
    /// goes under. The digits are deliberately not a prefix of [`SEED`].
    fn a_live_code() -> TotpState {
        TotpState::Code { code: "418902".to_string(), seconds_left: 19 }
    }

    /// The bullets `masked_row` paints, as a whole string.
    fn mask() -> String {
        "\u{2022}".repeat(MASKED_BULLETS)
    }

    /// The `LOGIN CREDENTIALS` card's own box, so the two passes can be
    /// compared on HEIGHT and not merely on which strings appeared.
    fn login_card(frame: &Frame) -> egui::Rect {
        frame.filled_box_around(frame.rect_of("LOGIN CREDENTIALS"), theme::CARD)
    }

    /// One row rule inside `card` -- a 1pt `theme::CANVAS` fill, the shape
    /// `theme::row_rule` paints. A removed row leaves its hairline standing
    /// about a point tall, which a height comparison is too coarse to see.
    fn rules_in(frame: &Frame, card: egui::Rect) -> usize {
        frame
            .rects
            .iter()
            .filter(|(rect, fill)| {
                *fill == theme::CANVAS
                    && (rect.height() - 1.0).abs() < 0.5
                    && card.contains_rect(*rect)
            })
            .count()
    }

    /// The reveal eye nearest `row_y` -- the one belonging to the row whose
    /// label sits there. Asserts it really is on that row rather than
    /// returning whichever eye happened to be closest.
    ///
    /// **The frame a click is DELIVERED on still paints the old state.**
    /// `masked_row` has already chosen its bullets by the time egui reports
    /// the click, so every test below that reads a revealed value takes the
    /// NEXT frame -- see `reveal_by_clicking`. `clicking_the_eye_reveals_
    /// without_copying` never noticed because it asserts on the flag.
    fn eye_on_row(frame: &Frame, row_y: f32) -> egui::Rect {
        assert!(!frame.eyes.is_empty(), "the frame painted no eye at all");
        let eye = *frame
            .eyes
            .iter()
            .min_by(|a, b| {
                (a.center().y - row_y)
                    .abs()
                    .total_cmp(&(b.center().y - row_y).abs())
            })
            .expect("the frame painted an eye");
        assert!(
            (eye.center().y - row_y).abs() < ROW_CONTENT_HEIGHT,
            "the nearest eye is on a different row entirely: eye {eye:?}, row at {row_y}"
        );
        eye
    }

    /// Clicks the eye on the row whose label sits at `row_y` and returns the
    /// frame AFTER the click -- the first one that can paint the new state.
    fn reveal_by_clicking(
        pane: &mut Pane,
        frame: &Frame,
        item: &VaultItem,
        totp: &TotpState,
        row_y: f32,
    ) -> Frame {
        let eye = eye_on_row(frame, row_y);
        let _delivering = pane.click(item, totp, eye.center());
        pane.idle(item, totp)
    }

    /// **Off means the row is not drawn -- not drawn empty, not drawn
    /// disabled, not drawn at alpha 0 and not drawn at zero size.**
    ///
    /// Four instruments, because each one alone has a way of passing on a row
    /// that is still there: the LABEL is absent from the painted frame (an
    /// alpha-0 label is still a painted galley, so this catches that), the
    /// card is exactly as TALL as it is with no row (a zero-size or
    /// off-screen row would pass the label check but not this one), the card
    /// carries no extra reveal EYE or row rule, and no second bullet run.
    ///
    /// Every negative half is paired with the setting-ON pass in this same
    /// test, so none of them can pass against a pane that drew nothing.
    #[test]
    fn the_secret_row_is_not_drawn_when_the_setting_is_off() {
        let item = a_login_with_a_seed("seed-off", SEED);

        let mut pane_off = Pane::new();
        let off = pane_off.idle(&item, &a_live_code());
        let mut pane_on = Pane::new().revealing_secrets();
        let on = pane_on.idle(&item, &a_live_code());

        // The premise: BOTH passes really drew the card and the code row, so
        // an absent label cannot be "the pane drew nothing this time".
        for (name, frame) in [("off", &off), ("on", &on)] {
            assert!(
                frame.painted("LOGIN CREDENTIALS"),
                "{name}: the login card was not drawn at all: {:?}",
                frame.strings()
            );
            assert!(
                frame.painted(copy_shortcut_label(CopyShortcut::Totp)),
                "{name}: the One-time code row the secret row goes under was not drawn: {:?}",
                frame.strings()
            );
        }

        // 1. THE LABEL IS GONE. Not faint, not clipped -- absent from the
        //    frame's own shape list.
        assert!(
            !off.painted(TOTP_SECRET_LABEL),
            "the setting is off and the pane still painted {TOTP_SECRET_LABEL:?}: {:?}",
            off.strings()
        );
        assert!(
            on.painted(TOTP_SECRET_LABEL),
            "with the setting ON the row is missing too, so the check above is vacuous: {:?}",
            on.strings()
        );

        // 2. THE CARD IS EXACTLY AS TALL as one with no such row. A row
        //    hidden rather than skipped still takes its band, and this is the
        //    only instrument that can tell the two apart.
        let (card_off, card_on) = (login_card(&off), login_card(&on));
        assert!(card_off.height() > 0.0, "the card has no box at all: {card_off:?}");
        let grew = card_on.height() - card_off.height();
        let a_row = ROW_CONTENT_HEIGHT + 2.0 * f32::from(ROW_PAD_Y);
        assert!(
            grew >= a_row,
            "turning the setting ON grew the card by only {grew}pt where a row is {a_row}pt, so \
             this instrument cannot see a row's worth of height at all"
        );

        // 3. NO EXTRA EYE, and no extra hairline. A row drawn at alpha 0
        //    still paints its eye's strokes and still sits under a rule.
        assert_eq!(
            on.eyes.len(),
            off.eyes.len() + 1,
            "the ON pass did not add exactly one reveal eye ({} -> {}), so counting eyes cannot \
             show the OFF pass is missing one",
            off.eyes.len(),
            on.eyes.len()
        );
        assert_eq!(
            rules_in(&on, card_on),
            rules_in(&off, card_off) + 1,
            "the ON pass did not add exactly one row rule, so a leftover hairline in the OFF \
             pass could not have been seen either"
        );

        // 4. AND NO MASK. The password row paints one bullet run; two would
        //    be a secret row drawn with its label suppressed.
        let masks = |frame: &Frame| frame.strings().iter().filter(|t| **t == mask()).count();
        assert_eq!(masks(&off), 1, "off: {:?}", off.strings());
        assert_eq!(masks(&on), 2, "on: {:?}", on.strings());
    }

    /// **No seed on the item means no row, even with the setting on** -- for
    /// the same reason an empty password draws no row: a mask is a claim that
    /// there is a secret behind it.
    ///
    /// `masked_row`'s own empty-value guard is not what is being tested here.
    /// That guard is documented as defence in depth and production-
    /// unreachable; the CALL is gated, and this test is about the call.
    #[test]
    fn the_secret_row_is_not_drawn_for_an_item_with_no_seed() {
        // Same preference, same TOTP state, two items -- so the only
        // difference between the passes is whether the seed is there.
        let seedless = a_login();
        let seeded = a_login_with_a_seed("seed-present", SEED);
        assert!(
            seedless.login.as_ref().unwrap().totp.is_none(),
            "the premise: the fixture really carries no seed"
        );

        let mut pane = Pane::new().revealing_secrets();
        let without = pane.idle(&seedless, &a_live_code());
        let mut pane = Pane::new().revealing_secrets();
        let with = pane.idle(&seeded, &a_live_code());

        for (name, frame) in [("without", &without), ("with", &with)] {
            assert!(
                frame.painted("LOGIN CREDENTIALS"),
                "{name}: the card was not drawn at all: {:?}",
                frame.strings()
            );
            assert!(
                frame.painted(copy_shortcut_label(CopyShortcut::Totp)),
                "{name}: the One-time code row was not drawn: {:?}",
                frame.strings()
            );
        }

        assert!(
            !without.painted(TOTP_SECRET_LABEL),
            "an item with no seed still painted a secret row: {:?}",
            without.strings()
        );
        assert!(
            with.painted(TOTP_SECRET_LABEL),
            "the control: an item WITH a seed draws the row on the same setting: {:?}",
            with.strings()
        );

        let (card_without, card_with) = (login_card(&without), login_card(&with));
        let a_row = ROW_CONTENT_HEIGHT + 2.0 * f32::from(ROW_PAD_Y);
        assert!(
            card_with.height() - card_without.height() >= a_row,
            "the seedless card is not a whole row shorter ({} vs {}), so the row is taking its \
             space and was hidden rather than skipped",
            card_without.height(),
            card_with.height()
        );
        assert_eq!(
            with.eyes.len(),
            without.eyes.len() + 1,
            "the seedless pass did not lose exactly one reveal eye"
        );
        assert_eq!(
            rules_in(&with, card_with),
            rules_in(&without, card_without) + 1,
            "a hairline was left standing where the seedless item's row would have been"
        );
    }

    /// **The positive control for both hiding rules**, so neither can pass by
    /// hiding everything: with the setting on and a seed present the row IS
    /// drawn, under the code row, inside the card, masked, with a reveal eye
    /// that works.
    #[test]
    fn the_secret_row_is_drawn_with_its_reveal_when_the_setting_is_on_and_the_item_has_a_seed() {
        let item = a_login_with_a_seed("seed-on", SEED);
        let mut pane = Pane::new().revealing_secrets();
        let frame = pane.idle(&item, &a_live_code());

        assert!(frame.painted(TOTP_SECRET_LABEL), "got {:?}", frame.strings());

        // UNDER the One-time code row, which is the placement the request
        // asked for -- read off the paint, not off the source order.
        let code = frame.rect_of(copy_shortcut_label(CopyShortcut::Totp));
        let secret = frame.rect_of(TOTP_SECRET_LABEL);
        assert!(
            secret.height() > 0.0 && secret.width() > 0.0,
            "the label has no box, so every geometry check below is about nothing: {secret:?}"
        );
        assert!(code.height() > 0.0);
        assert!(
            code.bottom() <= secret.top(),
            "the secret row is not under the code row: code at {code:?}, secret at {secret:?}"
        );
        assert!(
            secret.top() - code.top() > 1.0,
            "the two rows are at the same height, so the comparison above is reading one number \
             twice"
        );
        // ...and inside the login card, not floating in the pane.
        assert!(
            login_card(&frame).contains_rect(secret),
            "the secret row is outside the LOGIN CREDENTIALS card"
        );

        // MASKED, with an eye that is not struck through -- the only visible
        // difference between a masked row and a revealed one.
        assert_eq!(
            frame.strings().iter().filter(|t| **t == mask()).count(),
            2,
            "two bullet runs expected (password and secret); got {:?}",
            frame.strings()
        );
        assert_eq!(frame.struck_eyes(), 0, "nothing is revealed yet");

        // THE EYE WORKS, which is the half of "with its reveal" that a
        // painted icon does not prove.
        let after = reveal_by_clicking(&mut pane, &frame, &item, &a_live_code(), secret.center().y);
        assert!(
            pane.reveal.totp_secret,
            "the click set no reveal flag at all, so it hit nothing"
        );
        assert!(
            after.painted(SEED),
            "the eye did not reveal the seed: {:?}",
            after.strings()
        );
        assert_eq!(
            after.strings().iter().filter(|t| **t == mask()).count(),
            1,
            "the password row's mask should be the only one left; got {:?}",
            after.strings()
        );
        assert_eq!(after.struck_eyes(), 1, "exactly one eye is struck through");
    }

    /// **Masked until asked, and the plaintext is never in the frame before
    /// then** -- asserted on the glyphs egui actually laid out and not only
    /// on the source strings, because an elided run reports the string it was
    /// asked to draw rather than what it drew.
    #[test]
    fn the_secret_is_masked_until_revealed_and_never_painted_in_the_clear() {
        let item = a_login_with_a_seed("seed-mask", SEED);
        let mut pane = Pane::new().revealing_secrets();
        let masked = pane.idle(&item, &a_live_code());

        assert!(
            masked.painted(TOTP_SECRET_LABEL),
            "the row is missing entirely, so 'it is masked' is vacuous: {:?}",
            masked.strings()
        );
        // Nothing in the frame carries the seed -- not as a whole string, not
        // as a substring of some longer run, and not in the glyphs.
        for (text, _) in &masked.texts {
            assert!(!text.contains(SEED), "the seed was painted in the clear: {text:?}");
        }
        for (source, rendered, _) in &masked.rendered {
            assert!(!source.contains(SEED), "the seed reached a layout job: {source:?}");
            assert!(!rendered.contains(SEED), "the seed was laid out as glyphs: {rendered:?}");
        }
        // Nor any prefix of it worth having: a mask that leaked the first
        // third would pass every check above.
        let head = &SEED[..8];
        for (source, rendered, _) in &masked.rendered {
            assert!(
                !source.contains(head) && !rendered.contains(head),
                "the first {} characters of the seed were painted: {source:?} / {rendered:?}",
                head.len()
            );
        }
        // And the length is not leaked either: the run is `MASKED_BULLETS`
        // long whatever the seed is.
        assert!(
            masked.strings().iter().any(|t| *t == mask()),
            "the row painted no mask at all: {:?}",
            masked.strings()
        );
        assert_ne!(
            SEED.len(),
            MASKED_BULLETS,
            "the fixture's seed is exactly as long as the mask, which would make the check above \
             unable to tell a leak of the length from a fixed-length run"
        );

        // THE CONTROL, in this same test: revealed, the glyphs DO carry the
        // seed. Without it every assertion above would pass against a pane
        // that cannot paint the seed under any circumstances.
        let row_y = masked.rect_of(TOTP_SECRET_LABEL).center().y;
        let revealed = reveal_by_clicking(&mut pane, &masked, &item, &a_live_code(), row_y);
        assert!(
            revealed.rendered.iter().any(|(_, glyphs, _)| glyphs.contains(SEED)),
            "the control: the seed is not laid out even when revealed, so this harness cannot \
             see it and the assertions above are about an instrument that is blind: {:?}",
            revealed.strings()
        );
    }

    /// **The reveal is per-view.** A seed revealed on one item comes up
    /// masked on the next -- which matters more than it does for a password,
    /// because a seed is not rotated afterwards.
    ///
    /// The pane is driven exactly as `vault_window::mod`'s `run` drives it:
    /// the `RevealState` lives across frames and is re-assigned wholesale on
    /// a selection change. So this test fails for a reveal held ANYWHERE else
    /// -- an `egui` memory entry under a fixed id, a `Context` data slot, a
    /// `static` -- which is exactly how a note's selection once followed the
    /// user to the next item (`8973e9e`).
    ///
    /// **The two fixtures differ only by id and seed and share a NAME**, so a
    /// reveal keyed on the item's name rather than cleared outright is not
    /// mistaken for the fix.
    #[test]
    fn revealing_the_secret_on_one_item_does_not_reveal_it_on_the_next() {
        const OTHER_SEED: &str = "MZXW6YTBOI7777OTHERSEEDX";
        let first = a_login_with_a_seed("item-a", SEED);
        let second = a_login_with_a_seed("item-b", OTHER_SEED);
        assert_eq!(first.name, second.name, "the fixtures must share a name");
        assert_ne!(first.id, second.id, "...and differ by id");
        assert_ne!(SEED, OTHER_SEED);

        let mut pane = Pane::new().revealing_secrets();
        let shown = pane.idle(&first, &a_live_code());
        let row_y = shown.rect_of(TOTP_SECRET_LABEL).center().y;

        // THE PREMISE: the click really revealed the first item's seed.
        let revealed = reveal_by_clicking(&mut pane, &shown, &first, &a_live_code(), row_y);
        assert!(
            revealed.painted(SEED),
            "the premise: the eye did not reveal the first item's seed at all, so the rest of \
             this test is about nothing: {:?}",
            revealed.strings()
        );
        assert!(
            pane.reveal.totp_secret,
            "the flag being watched is not the one the click set"
        );

        // THE SELECTION CHANGES, exactly as `run` does it.
        pane.reveal = RevealState::default();
        let next = pane.idle(&second, &a_live_code());

        assert!(
            next.painted(TOTP_SECRET_LABEL),
            "the second item drew no secret row at all, so 'its seed is masked' is vacuous: {:?}",
            next.strings()
        );
        assert!(
            !next.painted(OTHER_SEED),
            "the second item's seed came up REVEALED -- the reveal followed the user to the \
             next item: {:?}",
            next.strings()
        );
        for (source, glyphs, _) in &next.rendered {
            assert!(
                !glyphs.contains(OTHER_SEED) && !glyphs.contains(SEED),
                "a seed was laid out on the next item: {source:?} / {glyphs:?}"
            );
        }
        assert!(
            next.strings().iter().any(|t| *t == mask()),
            "the second item's secret row is not masked: {:?}",
            next.strings()
        );
        assert_eq!(next.struck_eyes(), 0, "an eye is still drawn in the revealed state");

        // ...AND THE CONTROL: the second item's seed CAN be revealed on its
        // own row, so "masked" above is not "this pane can never show it".
        let row_y = next.rect_of(TOTP_SECRET_LABEL).center().y;
        let again = reveal_by_clicking(&mut pane, &next, &second, &a_live_code(), row_y);
        assert!(
            again.painted(OTHER_SEED),
            "the control: the second item's own eye does not reveal its seed: {:?}",
            again.strings()
        );
    }

    /// **The seed does not reach the allocator in the clear while the row is
    /// masked** -- the crate's `#[global_allocator]` probe, with the control
    /// asserted FIRST, because a probe reporting clean while blind is this
    /// codebase's signature failure.
    ///
    /// **What this does NOT claim.** `masked_row` materialises
    /// `value.to_string()` -- a plain `String`, not `Zeroizing` -- for the run
    /// it hands to egui while the row is REVEALED, exactly as the password
    /// row has always done, and egui's galley cache holds that text past the
    /// frame anyway, which is why a `Zeroizing` draft buffer there would be a
    /// guarantee in name only. So the claim is about the MASKED path, which
    /// is the state the row is in unless the user clicks the eye.
    ///
    /// Everything up to the paint borrows rather than copies:
    /// `LoginData::totp` is `Option<Zeroizing<String>>`, `draw_detail_read`'s
    /// `totp_secret` binding is a `&str` into it, and `masked_row` is handed
    /// that `&str`.
    #[test]
    fn the_totp_seed_is_masked_and_never_painted_in_the_clear() {
        use crate::login_ui::password_lifetime_tests::{plaintext_reached_the_allocator, PROBE};

        // THE CONTROL, first: an ordinary `String` holding the probe bytes is
        // seen going past the allocator. Built before the watch is armed.
        let bare = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "control: an ordinary String's plaintext went past the allocator unnoticed, so the \
             assertion below is about an instrument that sees nothing"
        );

        // The item, the pane and its fonts are all built OUTSIDE the watched
        // region: what is measured is what the masked paint does with the
        // seed, not what the scaffolding does on its way in.
        let item = a_login_with_a_seed("seed-probe", PROBE);
        let mut pane = Pane::new().revealing_secrets();
        let warm = pane.idle(&item, &a_live_code());
        assert!(
            warm.painted(TOTP_SECRET_LABEL),
            "the premise: the row under test is not being drawn at all: {:?}",
            warm.strings()
        );
        assert!(!warm.painted(PROBE), "the premise: the row starts masked");

        let mut drew = false;
        assert!(
            !plaintext_reached_the_allocator(|| {
                let frame = pane.idle(&item, &a_live_code());
                drew = frame.painted(TOTP_SECRET_LABEL) && !frame.painted(PROBE);
            }),
            "painting the MASKED secret row released a copy of the seed to the allocator"
        );
        // The closure really drew the masked row, so it cannot have been
        // optimised into nothing.
        assert!(drew, "the watched frame did not draw a masked secret row");
    }

    // ------------------------------- the stored value reduced to its key

    /// The `otpauth://` URI from the user's report, carrying [`SEED`] and an
    /// encoded label, so a row drawn from it is unmistakably wrong if the
    /// reduction is skipped.
    const SEED_URI: &str = concat!(
        "otpauth://totp/Offline%20one-time%20password",
        "?secret=JBSWY3DPEHPK3PXPSEEDSEED&issuer=Ledgerline"
    );

    /// A real `otpauth://` URI that carries no key at all. Not malformed --
    /// it simply has no `secret=` -- because "a URI is not a key" is the rule
    /// under test and a garbage string would prove nothing about it.
    const KEYLESS_URI: &str =
        "otpauth://totp/Offline%20one-time%20password?issuer=Ledgerline&period=30";

    /// **Every shape the field can hold, and what the row and the clipboard
    /// get for it.** The table is [`totp_key_of`]'s whole contract; its doc
    /// states the same rules in prose and this is what holds them.
    ///
    /// The table is asserted non-empty and every row is counted, so neither
    /// an empty literal nor a loop that stopped early can pass as green.
    #[test]
    fn the_stored_value_reduces_to_its_bare_key() {
        // (what the vault stored, what the pane shows and copies, why)
        let table: &[(&str, &str, &str)] = &[
            (SEED, SEED, "a bare base32 seed is the other common shape and must survive untouched"),
            ("  JBSWY3DPEHPK3PXP  ", "JBSWY3DPEHPK3PXP", "a pasted seed is trimmed"),
            ("", "", "an empty field yields nothing, which hides the row"),
            ("   ", "", "whitespace only is an empty field"),
            (SEED_URI, SEED, "THE REPORTED DEFECT: the whole URI reduces to its key"),
            (
                "otpauth://totp/L?issuer=Acme&algorithm=SHA1&secret=ABC234&digits=6&period=30",
                "ABC234",
                "the key is found among the other parameters wherever it sits",
            ),
            (KEYLESS_URI, "", "a URI with no secret= is not a key"),
            ("otpauth://totp/L?secret=&issuer=Acme", "", "an empty secret= is not a key either"),
            (
                "otpauth://totp/L",
                "",
                "a URI with no query at all is not a key. NOTE this row alone holds \
                 NOTHING about the `no query` early return: delete that return, let \
                 the whole path fall through AS the query, and `totp/L` still has no \
                 `=`, so the loop skips it and the answer is still \"\". The row below \
                 is the one that holds it",
            ),
            (
                "otpauth://secret=NOPE",
                "",
                "a path that would READ AS A QUERY if the `no query` early return \
                 were deleted: with the return gone the loop finds `secret=NOPE` in \
                 the PATH and the row shows -- and the clipboard gets -- a key the \
                 user never stored, out of a URI that has no query at all",
            ),
            (
                "otpauth://totp/L#secret=NOPE",
                "",
                "a fragment with no query at all is not a key. NOTE this row alone holds \
                 NOTHING about the fragment strip: with the strip deleted there is still \
                 no `?`, so the split still fails and the answer is still \"\". The two \
                 rows below are the ones that hold it",
            ),
            (
                "otpauth://totp/L#frag?secret=NOPE",
                "",
                "a `?` INSIDE the fragment is not the query's `?`: delete the fragment \
                 strip and this reads a key straight out of the fragment",
            ),
            (
                "otpauth://totp/L?secret=GOOD22#frag",
                "GOOD22",
                "a fragment AFTER a real query is cut off the key: delete the fragment \
                 strip and the row shows -- and the clipboard gets -- `GOOD22#frag`",
            ),
            (
                "otpauth://totp/L?secret=AB%43D%2fE",
                "ABCD/E",
                "the value is percent-decoded, upper- and lower-case escapes alike",
            ),
            (
                "otpauth://totp/L?secret=%C3%A9KEY",
                "\u{e9}KEY",
                "a multi-byte escape decodes to one character",
            ),
            (
                "otpauth://totp/L?secret=%20AB234%20",
                "AB234",
                "the key is trimmed AFTER decoding, not only before",
            ),
            (
                "otpauth://totp/L?secret=AB%ZZCD",
                "AB%ZZCD",
                "a malformed escape is left literal rather than dropped",
            ),
            (
                "otpauth://totp/L?secret=ABCD%",
                "ABCD%",
                "a % as the LAST byte is left literal. NOTE this position alone holds \
                 NOTHING about the bound `i + 2 < bytes.len()`: here `i + 1 < len` is \
                 already false too, so the off-by-one mutant never fires on this row. \
                 The row below is the position that holds it",
            ),
            (
                "otpauth://totp/L?secret=ABCD%A",
                "ABCD%A",
                "a % as the SECOND-TO-LAST byte -- the position `i + 2 < bytes.len()` \
                 exists for. Weaken the bound to `i + 1 < bytes.len()` and the decoder \
                 reads bytes[i + 2] one past the end and PANICS, on a value the user \
                 typed into their own vault. The shipped bound is what stops that",
            ),
            (
                "otpauth://totp/L?secret=%FF%FE",
                "%FF%FE",
                "a decode that is not UTF-8 falls back to the raw value",
            ),
            (
                "otpauth://totp/L?secret=A+B234",
                "A+B234",
                "a + is a literal +: otpauth values are percent-encoded, not form-encoded",
            ),
            (
                "OTPAUTH://TOTP/L?SECRET=ABC234",
                "ABC234",
                "the scheme and the parameter name are both matched case-insensitively",
            ),
            (
                "my-otpauth-backup-seed",
                "my-otpauth-backup-seed",
                "a value that merely CONTAINS the word is not a URI",
            ),
            (
                "otpauth:totp/L?secret=NOPE",
                "otpauth:totp/L?secret=NOPE",
                "the scheme is anchored WITH its slashes: a bare otpauth: is not a URI this row \
                 reduces, and a mutant that dropped the // from OTPAUTH_SCHEME survived until \
                 this row existed",
            ),
            (
                "otpauth-migration://offline?data=xyz",
                "otpauth-migration://offline?data=xyz",
                "a different scheme sharing the first letters is not otpauth://",
            ),
            (
                "otpauth://totp/L?issuersecret=NOPE&secret=YES234",
                "YES234",
                "the NAME is matched whole, so issuersecret= does not win",
            ),
            (
                "otpauth://totp/L?issuersecret=NOPE",
                "",
                "...and on its own issuersecret= is still not a key",
            ),
            (
                "otpauth://totp/L?secretx=NOPE&secret=YES234",
                "YES234",
                "the name is matched whole at BOTH ends. The two issuersecret= rows \
                 hold only the prefix end -- weaken `eq_ignore_ascii_case` to \
                 `starts_with` and they still pass, while secretx= walks off with \
                 the key. This row is the suffix end",
            ),
            (
                "otpauth://totp/L?secret=FIRST2&secret=SECOND",
                "FIRST2",
                "the FIRST secret= wins, so a trailing typo cannot blank a good key",
            ),
            (
                "otpauth://totp/L?secret",
                "",
                "a parameter with no = is skipped, not read as an empty secret",
            ),
            (
                "otpauth://totp/L?secret&secret=REAL22",
                "REAL22",
                "...so a stray bare secret does not shadow the real one",
            ),
            (
                "\u{e9}otpauth://x",
                "\u{e9}otpauth://x",
                "a multi-byte FIRST char is not a URI. NOTE byte 10 of this value is still \
                 a char boundary, so this row alone holds nothing about the boundary \
                 check -- the row below is the one that holds it",
            ),
            (
                "otpauth:/\u{e9}",
                "otpauth:/\u{e9}",
                "a multi-byte char STRADDLING the end of the scheme: byte 10 falls inside \
                 it, so `is_char_boundary` is what stops `&s[..10]` panicking here",
            ),
        ];
        assert!(
            !table.is_empty(),
            "the table is empty, so the loop below asserts nothing at all"
        );
        assert!(
            SEED_URI.contains(SEED) && !KEYLESS_URI.contains(SEED),
            "the fixtures do not hold the relationship every row below assumes"
        );

        let mut ran = 0usize;
        for (stored, want, why) in table {
            assert_eq!(totp_key_of(stored).as_str(), *want, "{why} -- stored {stored:?}");
            ran += 1;
        }
        assert_eq!(ran, table.len(), "not every row of the table ran");
    }

    /// **A URI with no `secret=` draws no row at all** -- the same rule as an
    /// absent seed and an empty password, because a URI is not a key and a
    /// mask over one would be an eye offering to reveal nothing.
    ///
    /// Four instruments, for the reason
    /// `the_secret_row_is_not_drawn_for_an_item_with_no_seed` gives: the
    /// label is absent (an alpha-0 label is still a painted galley), the card
    /// is a whole row SHORTER (a zero-height or off-screen row would pass the
    /// label check -- egui culls a shape pushed off the screen rect entirely,
    /// so it comes back as nothing), one fewer eye and one fewer hairline.
    ///
    /// The expected row count is asserted BEFORE any geometry is read.
    #[test]
    fn a_uri_with_no_secret_draws_no_row() {
        let keyless = a_login_with_a_seed("uri-no-secret", KEYLESS_URI);
        let keyed = a_login_with_a_seed("uri-with-secret", SEED_URI);

        let mut pane = Pane::new().revealing_secrets();
        let without = pane.idle(&keyless, &a_live_code());
        let mut pane = Pane::new().revealing_secrets();
        let with = pane.idle(&keyed, &a_live_code());

        // The premise: both passes really drew the card and the code row, so
        // an absent label cannot be "the pane drew nothing this time".
        for (name, frame) in [("keyless", &without), ("keyed", &with)] {
            assert!(
                frame.painted("LOGIN CREDENTIALS"),
                "{name}: the card was not drawn at all: {:?}",
                frame.strings()
            );
            assert!(
                frame.painted(copy_shortcut_label(CopyShortcut::Totp)),
                "{name}: the One-time code row was not drawn: {:?}",
                frame.strings()
            );
        }
        // The expected number of secret rows, before a single rect is read.
        assert_eq!(
            with.strings().iter().filter(|t| **t == TOTP_SECRET_LABEL).count(),
            1,
            "the control pass did not draw exactly one secret row: {:?}",
            with.strings()
        );
        assert_eq!(
            without.strings().iter().filter(|t| **t == TOTP_SECRET_LABEL).count(),
            0,
            "a URI with no secret= still painted a secret row: {:?}",
            without.strings()
        );
        // ...and neither pass leaked the URI itself onto the pane.
        for (name, frame) in [("keyless", &without), ("keyed", &with)] {
            assert!(
                !frame.strings().iter().any(|t| t.contains("otpauth")),
                "{name}: the stored URI reached the pane: {:?}",
                frame.strings()
            );
        }

        let (card_without, card_with) = (login_card(&without), login_card(&with));
        let a_row = ROW_CONTENT_HEIGHT + 2.0 * f32::from(ROW_PAD_Y);
        assert!(
            card_with.height() - card_without.height() >= a_row,
            "the keyless card is not a whole row shorter ({} vs {}), so the row is taking its \
             space and was hidden rather than skipped",
            card_without.height(),
            card_with.height()
        );
        assert_eq!(
            with.eyes.len(),
            without.eyes.len() + 1,
            "the keyless pass did not lose exactly one reveal eye"
        );
        assert_eq!(
            rules_in(&with, card_with),
            rules_in(&without, card_without) + 1,
            "a hairline was left standing where the keyless item's row would have been"
        );
    }

    /// **The row shows the KEY, not the stored URI** -- the user's request,
    /// read off the revealed frame rather than off [`totp_key_of`], which the
    /// table already covers. A reduction that is right and a pane that hands
    /// `masked_row` the raw field anyway is exactly the pair this file's
    /// findings keep being.
    #[test]
    fn the_revealed_row_shows_the_key_and_not_the_uri() {
        let item = a_login_with_a_seed("uri-revealed", SEED_URI);
        let mut pane = Pane::new().revealing_secrets();
        let frame = pane.idle(&item, &a_live_code());
        assert!(
            frame.painted(TOTP_SECRET_LABEL),
            "the premise: no secret row was drawn at all: {:?}",
            frame.strings()
        );

        let row_y = frame.rect_of(TOTP_SECRET_LABEL).center().y;
        let after = reveal_by_clicking(&mut pane, &frame, &item, &a_live_code(), row_y);
        assert!(
            pane.reveal.totp_secret,
            "the click set no reveal flag, so it hit nothing"
        );
        assert!(
            after.painted(SEED),
            "the revealed row is not the bare key: {:?}",
            after.strings()
        );
        assert!(
            !after.painted(SEED_URI) && !after.strings().iter().any(|t| t.contains("otpauth")),
            "the revealed row still carries the scheme, label or query: {:?}",
            after.strings()
        );
        // The glyphs, not only the source strings: an elided run reports what
        // it was asked to draw. The label is encoded in the URI, so this also
        // catches a reduction that decoded but did not strip.
        assert_eq!(
            after.rendered_glyphs(SEED),
            SEED,
            "the key was laid out as something other than itself"
        );
    }

    /// **The clipboard gets the key, not the URI.** Asserted on
    /// [`totp_secret_clipboard_text`], which is the entire body of
    /// `vault_window::mod`'s `DetailAction::CopyTotpSecret` arm -- the
    /// clipboard itself is behind an `egui::Context` no test here renders as
    /// far as, so a frame-shaped assertion would have been about the row
    /// again and not about the copy.
    ///
    /// The last assertion is the one that makes the rest reach anything: the
    /// arm really calls this function.
    #[test]
    fn the_clipboard_gets_the_key_not_the_uri() {
        let from_uri = a_login_with_a_seed("clip-uri", SEED_URI);
        let copied = totp_secret_clipboard_text(&from_uri).expect("a keyed URI offers a copy");
        assert_eq!(copied.as_str(), SEED, "the clipboard got something other than the key");
        assert!(
            !copied.contains("otpauth") && !copied.contains('?') && !copied.contains('='),
            "the clipboard got URI syntax: {:?}",
            copied.as_str()
        );

        // What is SHOWN and what is COPIED are the same string -- the whole
        // point of one seam rather than two derivations.
        let mut pane = Pane::new().revealing_secrets();
        let frame = pane.idle(&from_uri, &a_live_code());
        let row_y = frame.rect_of(TOTP_SECRET_LABEL).center().y;
        let shown = reveal_by_clicking(&mut pane, &frame, &from_uri, &a_live_code(), row_y);
        assert!(
            shown.painted(copied.as_str()),
            "the pane shows one value and the clipboard gets another: copied {:?}, painted {:?}",
            copied.as_str(),
            shown.strings()
        );

        // Nothing to copy where nothing is drawn.
        assert!(
            totp_secret_clipboard_text(&a_login_with_a_seed("clip-keyless", KEYLESS_URI)).is_none(),
            "a URI with no secret= offered a copy, which would clear the clipboard"
        );
        assert!(
            totp_secret_clipboard_text(&a_login()).is_none(),
            "an item with no seed at all offered a copy"
        );

        // The arm is wired to this function. One line, no newline in the
        // needle -- this tree is CRLF and a multi-line needle matches nothing.
        let wiring = include_str!("mod.rs");
        assert_eq!(
            wiring.matches("detail::totp_secret_clipboard_text(item)").count(),
            1,
            "the CopyTotpSecret arm does not call totp_secret_clipboard_text, so everything \
             asserted above is about a function nothing runs"
        );
    }

    /// **The positive control: a bare seed is shown and copied unchanged.**
    /// Without it every assertion above could be satisfied by a reduction
    /// that returns the empty string for everything.
    #[test]
    fn a_bare_seed_still_shows_and_copies_unchanged() {
        let item = a_login_with_a_seed("bare-seed", SEED);

        let copied = totp_secret_clipboard_text(&item).expect("a bare seed offers a copy");
        assert_eq!(copied.as_str(), SEED, "the bare seed was altered on its way to the clipboard");

        let mut pane = Pane::new().revealing_secrets();
        let frame = pane.idle(&item, &a_live_code());
        assert!(
            frame.painted(TOTP_SECRET_LABEL),
            "the bare-seed row is no longer drawn at all: {:?}",
            frame.strings()
        );
        assert!(!frame.painted(SEED), "the row does not start masked");

        let row_y = frame.rect_of(TOTP_SECRET_LABEL).center().y;
        let after = reveal_by_clicking(&mut pane, &frame, &item, &a_live_code(), row_y);
        assert_eq!(
            after.rendered_glyphs(SEED),
            SEED,
            "the revealed bare seed is not itself: {:?}",
            after.strings()
        );
    }

    /// **The seed inside a URI does not reach the allocator in the clear
    /// either** -- the probe again, with its control asserted FIRST, because
    /// a probe reporting clean while blind is this codebase's signature
    /// failure.
    ///
    /// `the_totp_seed_is_masked_and_never_painted_in_the_clear` covers the
    /// pass-through path, where nothing is copied at all. This one covers the
    /// path [`totp_key_of`] added: a stored URI whose `secret=` is
    /// percent-encoded, so the scheme match, the byte buffer, the UTF-8
    /// validation and the trim all run over the probe bytes inside the
    /// watched window. Every buffer on that path is `Zeroizing`; if any one
    /// of them were a plain `String` or `Vec<u8>` this would fail.
    ///
    /// Same caveat as the sibling test: the claim is about the MASKED row.
    /// `masked_row` materialises a plain `String` once REVEALED, as it always
    /// has, and egui's galley cache holds that text past the frame anyway.
    #[test]
    fn the_seed_inside_a_uri_never_reaches_the_allocator_in_the_clear() {
        use crate::login_ui::password_lifetime_tests::{plaintext_reached_the_allocator, PROBE};

        // THE CONTROL, first: an ordinary `String` holding the probe bytes is
        // seen going past the allocator. Built before the watch is armed.
        let bare = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "control: an ordinary String's plaintext went past the allocator unnoticed, so the \
             assertion below is about an instrument that sees nothing"
        );

        // A URI whose secret is the probe, with an escape in front of it so
        // the decoder's byte buffer and the trim both run over those bytes.
        let stored = format!("otpauth://totp/Offline%20code?secret=%20{PROBE}&issuer=Acme");
        assert_eq!(
            totp_key_of(&stored).as_str(),
            PROBE,
            "the premise: this fixture does not reduce to the probe, so the watch below is \
             armed over the wrong bytes"
        );

        let item = a_login_with_a_seed("uri-probe", &stored);
        let mut pane = Pane::new().revealing_secrets();
        let warm = pane.idle(&item, &a_live_code());
        assert!(
            warm.painted(TOTP_SECRET_LABEL),
            "the premise: the row under test is not being drawn at all: {:?}",
            warm.strings()
        );
        assert!(!warm.painted(PROBE), "the premise: the row starts masked");

        let mut drew = false;
        assert!(
            !plaintext_reached_the_allocator(|| {
                let frame = pane.idle(&item, &a_live_code());
                drew = frame.painted(TOTP_SECRET_LABEL) && !frame.painted(PROBE);
            }),
            "reducing and painting the MASKED secret row released a copy of the seed to the \
             allocator"
        );
        assert!(drew, "the watched frame did not draw a masked secret row");

        // The clipboard seam runs the same reduction; it must not leak on the
        // way either. `copy_text` itself takes an owned `String` and is the
        // documented plain copy, so the assertion stops at this function.
        let mut got = false;
        assert!(
            !plaintext_reached_the_allocator(|| {
                got = totp_secret_clipboard_text(&item).is_some_and(|k| k.as_str() == PROBE);
            }),
            "deriving the clipboard text released a copy of the seed to the allocator"
        );
        assert!(got, "the watched clipboard derivation did not produce the key");
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
                totp_secret: false,
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
        // **The same NAME, deliberately.** Two `Gmail` logins is an ordinary
        // vault, and while these two fixtures differed in name as well as id
        // this test could not say which of the two the toast was keyed on:
        // substituting `&item.name` for `&item.id` at
        // `forget_copy_toast_on_item_change`'s call site passed the whole
        // suite. Under that mutant these two items are one item, the
        // confirmation follows the pane across the selection change, and
        // renaming an item mid-toast clears a live one. The id is the only
        // thing that differs here, so only the id can be what carries the
        // assertions below.
        other.name = item.name.clone();
        assert_ne!(item.id, other.id, "the two fixtures are the same item");
        assert_eq!(
            item.name, other.name,
            "the two fixtures differ by name as well as id, so this test cannot tell a \
             toast keyed on the id from one keyed on the name"
        );

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

    /// The other door out of an item, which the read pane cannot see.
    ///
    /// `forget_copy_toast_on_item_change` fires on a *different* item. Opening
    /// the editor and cancelling back, or deselecting and reselecting, never
    /// changes the item -- so nothing fired, the toast was still in the map,
    /// the recorded id still matched, and the confirmation came back for a
    /// copy the user had looked away from five seconds ago. That is the
    /// resurrection `forget_copy_toast_on_item_change`'s doc sets out to make
    /// impossible, reached by a route it does not watch.
    ///
    /// `vault_window::mod` is what calls this on those routes (its own suite
    /// pins the three call sites); here is the primitive doing what it claims,
    /// on a real pane, with a real toast up.
    #[test]
    fn a_confirmation_does_not_survive_the_pane_being_looked_away_from() {
        let item = a_login();
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        let row = laid_out.rect_of("Password");
        let copied = pane.click(&item, &TotpState::NoSecret, row.center());
        assert!(
            copied.painted("Password copied"),
            "nothing was confirmed, so the assertions below are about a toast that never \
             existed -- painted: {:?}",
            copied.strings()
        );
        // Control: the SAME item redrawn keeps it, so what the clear below
        // does is not something a redraw was going to do anyway.
        assert!(
            pane.idle(&item, &TotpState::NoSecret).painted("Password copied"),
            "a plain redraw already lost the confirmation, so this test cannot show that \
             the clear is what removed it"
        );

        // The user leaves the read pane: the editor opens, or the selection
        // is dropped. No item change -- the item is the same one.
        forget_copy_toast(&pane.ctx);
        // The record went with the toast, so the item-change check starts
        // from nothing rather than from a stale id that would make it a
        // no-op. Asserted HERE, before the pane redraws: the next
        // `draw_detail_read` records the item again, which is correct and
        // would hide a clear that had left it behind.
        assert_eq!(
            pane.ctx.data(|data| data.get_temp::<String>(copy_toast_item_id())),
            None,
            "the toast is gone but the item it belonged to is still recorded"
        );

        let back = pane.idle(&item, &TotpState::NoSecret);
        assert!(
            !back.painted("Password copied"),
            "the confirmation came back on the SAME item after the user looked away -- a \
             round trip through the edit pane, or a deselect and reselect, resurrects it; \
             painted: {:?}",
            back.strings()
        );
        // And it stays gone, rather than being suppressed for one frame.
        let again = pane.idle(&item, &TotpState::NoSecret);
        assert!(
            !again.painted("Password copied"),
            "the confirmation was hidden for a frame rather than cleared; painted: {:?}",
            again.strings()
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
            sequence: String::new(),
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

    /// **What the launch gate reads, and what it must not read.**
    ///
    /// `vault_window::mod`'s `launch_needs_confirmation` asks the plan whether
    /// any of its command line is `AppMatch::args` -- vault data passed
    /// through untouched. The tempting substitute at the launch site is
    /// `!raw_tail.is_empty()`, and it is wrong in the common direction: an
    /// ordinary login with a website and a bare app binding has a non-empty
    /// tail made of nothing but its own quoted URL, and confirming *that*
    /// would put a dialog in front of the launch every user makes every day.
    /// The two cases are pinned apart here because the mod-side gate is one
    /// field read and cannot tell them apart on its own.
    #[test]
    fn only_stored_arguments_mark_a_plan_as_carrying_vault_data() {
        const WEB: &str = "https://app.ledgerline.com/";

        let bare = app_launch_plan(&a_desktop_match(), "").expect("a live desktop match");
        assert_eq!(bare.raw_tail, "");
        assert!(!bare.has_raw_args, "a match with no stored arguments carries none");

        // A tail that is ONLY the item's URL. Non-empty, and still nothing to
        // confirm: it went through `quote_arg`, so it is one positional
        // argument and cannot become a flag.
        let url_only = app_launch_plan(&a_desktop_match(), WEB).expect("a live desktop match");
        assert!(!url_only.raw_tail.is_empty(), "the URL is on the command line");
        assert!(
            !url_only.has_raw_args,
            "a plan whose whole tail is the item's own quoted URL claims to carry vault \
             arguments, so every ordinary Open would be confirmed and the dialog becomes \
             something to click past"
        );

        // Whitespace-only `args` contributes nothing to the tail -- the same
        // `trim` `launch_tail` applies -- so it is not "arguments" either.
        let blank = AppMatch { args: "   ".to_string(), ..a_desktop_match() };
        let blank = app_launch_plan(&blank, "").expect("a live desktop match");
        assert_eq!(blank.raw_tail, "");
        assert!(
            !blank.has_raw_args,
            "a stored `args` of whitespace produces an empty tail but claims to carry \
             arguments, so the confirmation would show a command line with nothing extra in it"
        );

        // And the real thing: the payload the review found.
        let hostile = AppMatch {
            args: r#"--gpu-launcher="cmd /c calc.exe""#.to_string(),
            ..a_desktop_match()
        };
        let hostile = app_launch_plan(&hostile, WEB).expect("a live desktop match");
        assert!(
            hostile.has_raw_args,
            "a plan carrying --gpu-launcher out of the vault does not ask to be confirmed"
        );
        assert!(
            command_line(&hostile).contains(r#"--gpu-launcher="cmd /c calc.exe""#),
            "the string the confirmation would show does not contain the payload that runs: {}",
            command_line(&hostile)
        );
    }

    #[test]
    fn the_command_line_quotes_a_program_path_with_a_space_in_it() {
        let plan = LaunchPlan {
            program: r"C:\Program Files\App\App.exe".to_string(),
            raw_tail: String::new(),
            has_raw_args: false,
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
        // The behaviour note is still there: the binding still fires.
        assert!(notes.contains(&APP_MATCH_BEHAVIOUR_NOTE), "{notes:?}");

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
            value: Some(Zeroizing::new("{not json".to_string())),
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
    // -----------------------------------------------------------------
    // The app's real NAME and its icon, the link on it, and the chord.
    //
    // The user's three reports about this one card: "Matched app should show
    // normal name with icon and not mabl.exe", "should be clickable like link
    // and have Ctrl + shortcut", and "Matched app Remove button is on top of
    // long description - should be left aligned".
    // -----------------------------------------------------------------

    /// What `chrome.exe`'s version resource says the app is called.
    ///
    /// **Deliberately unlike every other string in the fixture.** It is not
    /// the `process`, not the path's file name, not any directory in the
    /// path, and not a prefix or suffix of any of them -- so a test that
    /// finds it on screen has found the resolved name and nothing else.
    /// `apps.label(ctx, &m.path, &m.process)` mutated to `(&m.process,
    /// &m.process)` is the exact shape that survived once already, because
    /// the fixture it was tested against had a path and a process that agreed.
    const CHROME_NAME: &str = "Waypoint Browser";
    /// The path `a_browser_match` records -- the cache's KEY, spelled once.
    const CHROME_PATH: &str = r"C:\Program Files\Google\Chrome\Application\chrome.exe";
    /// A dead binding with a REAL program file: the frame host's own
    /// executable, which exists on every Windows machine, carries a
    /// `FileDescription` and has a shell icon. That is what makes "a dead
    /// binding must not be dressed up" a decision rather than an accident.
    fn a_dead_match() -> AppMatch {
        let mut dead = AppMatch::for_process("ApplicationFrameHost.exe", TriggerMode::Prompt);
        dead.path = r"C:\Windows\System32\ApplicationFrameHost.exe".to_string();
        dead
    }

    /// A pane that has already resolved the browser fixture's name.
    fn a_pane_that_knows_chrome() -> Pane {
        Pane::new().knows_app(CHROME_PATH, CHROME_NAME)
    }

    /// The one keystroke `OPEN_APP_CHORD` names.
    fn open_app_chord_events() -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key: OPEN_APP_CHORD.1,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: OPEN_APP_CHORD.0,
        }]
    }

    /// The premise for every assertion below: the constants really are what
    /// the fixture carries, and they really are distinguishable. A
    /// `CHROME_PATH` that had drifted from `a_browser_match` would seed a key
    /// nothing asks about, and every test here would then be asserting about
    /// the unresolved fallback while looking like it was not.
    #[test]
    fn the_seeded_path_is_the_path_the_fixture_records() {
        assert_eq!(a_browser_match().path, CHROME_PATH);
        assert_ne!(CHROME_NAME, a_browser_match().process);
        assert_ne!(
            Some(CHROME_NAME),
            crate::app_identity::file_name_of(CHROME_PATH),
            "the resolved name equals the path's own file name, so finding it on screen \
             could not tell a resolved name from the fallback"
        );
        assert!(
            !CHROME_PATH.contains(CHROME_NAME),
            "the resolved name is a substring of the path, so the Program file row alone \
             would satisfy a `painted` assertion about it"
        );
    }

    /// **The report.** The card says what the app is called; it does not say
    /// `chrome.exe` where the name goes.
    #[test]
    fn the_matched_app_card_shows_the_apps_real_name_and_not_the_executable() {
        let item = bound_to(&a_login_on_the_web(), &a_browser_match());
        let mut pane = a_pane_that_knows_chrome();
        let frame = pane.idle(&item, &TotpState::NoSecret);

        assert!(
            frame.painted(CHROME_NAME),
            "the App row still does not say what the app is called; painted {:?}",
            frame.strings()
        );
        // And it really is the App row's value, in the same value column the
        // Program file row's is -- not some other string on the pane.
        assert_eq!(
            frame.rect_of(CHROME_NAME).left(),
            frame.rect_of(CHROME_PATH).left(),
            "the resolved name is not in the value column the Program file row is in"
        );
        assert!(
            !frame.painted("chrome.exe"),
            "the executable name is still drawn on its own line: {:?}",
            frame.strings()
        );
    }

    /// **The wiring, as the substitution that has already shipped once.**
    ///
    /// The cache is keyed on the PATH. A `draw_detail_read` that handed it
    /// `process` where the path goes finds nothing, and the row falls back.
    /// Seeding under `process` and demanding that the name does NOT appear is
    /// that mutation, run.
    #[test]
    fn the_name_is_looked_up_by_the_path_and_not_by_the_process() {
        let m = a_browser_match();
        let item = bound_to(&a_login_on_the_web(), &m);
        let mut pane = Pane::new().knows_app(&m.process, CHROME_NAME);
        let frame = pane.idle(&item, &TotpState::NoSecret);
        assert!(
            !frame.painted(CHROME_NAME),
            "a name seeded under the PROCESS reached the screen, so the card is asking \
             the cache about the wrong string: {:?}",
            frame.strings()
        );
        // The control on the control: seeded under the PATH, it does.
        let mut right = a_pane_that_knows_chrome();
        assert!(right.idle(&item, &TotpState::NoSecret).painted(CHROME_NAME));
    }

    /// **A path that no longer exists** -- uninstalled, moved, or a share
    /// that is down. Nothing resolves, so the row is the executable name it
    /// always was: no blank, no spinner, no error, no dialog.
    #[test]
    fn an_app_whose_executable_is_gone_is_still_named_by_its_executable() {
        let item = bound_to(&a_login_on_the_web(), &a_browser_match());
        // No seed at all, and `CHROME_PATH` names nothing on a machine
        // running this suite: the probe is the real one and it fails.
        let mut pane = Pane::new();
        let frame = pane.idle(&item, &TotpState::NoSecret);
        assert!(
            frame.painted("chrome.exe"),
            "an app whose file is gone has no name at all on the card: {:?}",
            frame.strings()
        );
        // And the path is still on the card, because it is the thing the
        // user has to act on.
        assert!(frame.painted(CHROME_PATH), "{:?}", frame.strings());
        // No icon, and nothing that reads as an error.
        assert!(frame.images.is_empty(), "an icon was drawn for a file that is not there");
    }

    /// **A dead binding is shown plainly, as the raw name it is.**
    ///
    /// Its `process` is the frame host, whose real executable exists in
    /// System32 with a `FileDescription` and an icon of its own. Dressing it
    /// up would put a Windows internal's name and icon above the sentence
    /// saying this binding never fires.
    #[test]
    fn a_dead_binding_is_not_dressed_up_with_a_resolved_name() {
        let dead = a_dead_match();
        assert!(app_match_is_dead(&dead), "the premise");
        let item = bound_to(&a_login_on_the_web(), &dead);
        // Seeded under the RIGHT key: this pane COULD resolve it. It must
        // decline to ask.
        let mut pane = Pane::new().knows_app(&dead.path, "Application Frame Host");
        let frame = pane.idle(&item, &TotpState::NoSecret);

        assert!(
            !frame.painted("Application Frame Host"),
            "a dead binding was given a friendly name: {:?}",
            frame.strings()
        );
        assert!(
            frame.painted("ApplicationFrameHost.exe"),
            "the raw process name -- the evidence of what went wrong -- is gone: {:?}",
            frame.strings()
        );
        assert!(
            frame.painted(APP_MATCH_DEAD_NOTICE),
            "the premise: this card really is saying the binding is dead"
        );
    }

    /// And it is [`app_name_lookup_path`] that refuses it, not the drawing.
    #[test]
    fn a_dead_binding_is_looked_up_under_no_path_at_all() {
        let live = a_browser_match();
        assert_eq!(
            app_name_lookup_path(&live),
            CHROME_PATH,
            "a live binding must be looked up under its own program file"
        );
        assert_ne!(
            app_name_lookup_path(&live),
            live.process,
            "the lookup key is the path, and this fixture's path and process differ -- if \
             this ever passes, the fixture has stopped being able to tell them apart"
        );
        assert_eq!(
            app_name_lookup_path(&a_dead_match()),
            "",
            "a dead binding is handed a real path, so a probe is spawned, a shell icon is \
             fetched and a Windows internal is named on the card"
        );
    }

    /// A Microsoft Store match is NOT special-cased: it is looked up like any
    /// other, and the word `hosted` still never reaches the screen.
    #[test]
    fn a_store_match_is_looked_up_like_any_other_and_never_says_hosted() {
        let m = a_store_match();
        assert_eq!(
            app_name_lookup_path(&m),
            m.path,
            "a Store match's own recorded program file is what its name comes from"
        );
        let item = bound_to(&a_login_on_the_web(), &m);
        let mut pane = Pane::new().knows_app(&m.path, "Speedtest by Ookla");
        let frame = pane.idle(&item, &TotpState::NoSecret);
        assert!(frame.painted("Speedtest by Ookla"), "{:?}", frame.strings());
        for word in ["hosted", "FrameHost"] {
            assert!(
                !frame.strings().iter().any(|s| s.contains(word)),
                "{word:?} reached the screen: {:?}",
                frame.strings()
            );
        }
        // And it is still explained, and still not launchable.
        assert!(frame.painted(APP_HOSTED_NOTE), "{:?}", frame.strings());
        assert!(frame.painted(APP_OPEN_HOSTED_NOTE), "{:?}", frame.strings());
    }

    // -- the icon -------------------------------------------------------

    /// The icon is painted, at the size the edit form paints it, beside the
    /// name -- and the same card without one paints no image at all, which is
    /// what makes the assertion about the icon and not about some other
    /// texture this pane happens to draw.
    #[test]
    fn the_read_pane_draws_the_app_icon_at_the_edit_forms_size() {
        let item = bound_to(&a_login_on_the_web(), &a_browser_match());

        let mut without = a_pane_that_knows_chrome();
        let bare = without.idle(&item, &TotpState::NoSecret);
        assert!(
            bare.images.is_empty(),
            "this pane paints a texture even with no app icon, so finding one below \
             proves nothing: {:?}",
            bare.images
        );

        let mut with = Pane::new().knows_app_with_icon(CHROME_PATH, CHROME_NAME);
        let frame = with.idle(&item, &TotpState::NoSecret);
        assert_eq!(frame.images.len(), 1, "expected exactly one icon: {:?}", frame.images);
        let icon = frame.images[0];
        assert!(
            (icon.width() - APP_ICON_SIZE).abs() < 0.01
                && (icon.height() - APP_ICON_SIZE).abs() < 0.01,
            "the icon is {}x{}pt, not the {APP_ICON_SIZE}pt the edit form draws",
            icon.width(),
            icon.height()
        );
        // Beside the name, to its LEFT, on the same line.
        let name = frame.rect_of(CHROME_NAME);
        assert!(
            icon.right() <= name.left() + 0.01,
            "the icon at {icon:?} overlaps the name at {name:?}"
        );
        assert!(
            (icon.center().y - name.center().y).abs() < APP_ICON_SIZE,
            "the icon at {icon:?} is not on the name's line at {name:?}"
        );
    }

    /// **The `icon_probe` tripwire, answered here rather than assumed.**
    ///
    /// Every drawn icon in this app is identified by vertex count alone, and
    /// `theme::no_two_drawn_icons_share_a_vertex_count` is a live guard
    /// against two colliding. This card now paints a bitmap. Measured, on the
    /// real pane: adding the icon changes no probe's answer.
    #[test]
    fn the_app_icon_is_not_mistaken_for_any_drawn_icon() {
        let item = bound_to(&a_login_on_the_web(), &a_browser_match());
        let mut without = a_pane_that_knows_chrome();
        let bare = without.idle(&item, &TotpState::NoSecret);
        let mut with = Pane::new().knows_app_with_icon(CHROME_PATH, CHROME_NAME);
        let frame = with.idle(&item, &TotpState::NoSecret);

        assert_eq!(frame.images.len(), 1, "the premise: an icon really was painted");
        assert_eq!(
            frame.stars.len(),
            bare.stars.len(),
            "the app icon was counted as a favourite star"
        );
        assert_eq!(frame.eyes.len(), bare.eyes.len(), "the app icon was counted as an eye");
        assert_eq!(
            frame.kebab_dots.len(),
            bare.kebab_dots.len(),
            "the app icon was counted as a kebab dot"
        );
        assert_eq!(
            frame.segments.len(),
            bare.segments.len(),
            "the app icon was counted as a line segment"
        );
        assert_eq!(
            theme::icon_probe::gears(&frame.shapes).len(),
            theme::icon_probe::gears(&bare.shapes).len(),
            "the app icon was counted as a gear"
        );
        // The control: the probes are not blind on this pane -- it really
        // does draw eyes and a star for this item, so the equalities above
        // are not 0 == 0.
        assert!(
            !bare.eyes.is_empty() && !bare.stars.is_empty(),
            "no drawn icon is on this pane at all"
        );
    }

    // -- the link and the chord -----------------------------------------

    /// **Clicking the NAME opens the app, and copies nothing** -- the split
    /// the Website row already makes -- and it reports the very action the
    /// footer's Open reports.
    #[test]
    fn clicking_the_apps_name_opens_it_without_copying() {
        let m = a_browser_match();
        let item = bound_to(&a_login_on_the_web(), &m);
        let mut pane = a_pane_that_knows_chrome();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        let name = laid_out.rect_of(CHROME_NAME);

        let clicked = pane.click(&item, &TotpState::NoSecret, name.center());
        assert_eq!(
            clicked.action,
            DetailAction::OpenApp(app_launch_plan(&m, WEB).expect("a launchable match")),
            "clicking the app's name reported {:?}",
            clicked.action
        );
        // And it is the SAME action the Open control reports, not a
        // look-alike built here: one launch path, asked from two places.
        assert_eq!(
            Some(clicked.action),
            app_open_choices(&m, WEB).first().map(open_choice_action),
            "the link and the Open control do different things"
        );
    }

    /// And a click anywhere else in the same tile copies the EXECUTABLE name
    /// -- not the friendly one, which pastes into nothing.
    #[test]
    fn clicking_elsewhere_in_the_app_tile_copies_the_executable_name() {
        let item = bound_to(&a_login_on_the_web(), &a_browser_match());
        let mut pane = a_pane_that_knows_chrome();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        let label = laid_out.rect_of("App");

        let clicked = pane.click(&item, &TotpState::NoSecret, label.center());
        assert_eq!(
            clicked.action,
            DetailAction::CopyValue("chrome.exe".to_string()),
            "the App tile copied {:?} -- the clipboard wants the executable, which is what \
             pastes into a shortcut, a script or this app's own Program file box",
            clicked.action
        );
    }

    /// **A name with nothing to open is plain text.** A blue, hand-cursored
    /// link that silently does nothing is the defect class this card has
    /// spent the day removing, so the click falls through to the tile's copy.
    #[test]
    fn a_name_with_no_open_is_not_a_link() {
        let mut no_path = a_browser_match();
        no_path.path = String::new();
        assert!(app_name_open_action(&no_path, WEB).is_none(), "the premise");
        let item = bound_to(&a_login_on_the_web(), &no_path);
        // With no path there is nothing to resolve: the name IS the process,
        // which is what the cache answers an empty path with.
        let mut pane = Pane::new();
        let laid_out = pane.idle(&item, &TotpState::NoSecret);
        let name = laid_out.rect_of("chrome.exe");

        let clicked = pane.click(&item, &TotpState::NoSecret, name.center());
        assert_eq!(
            clicked.action,
            DetailAction::CopyValue("chrome.exe".to_string()),
            "clicking the name of an app that cannot be opened reported {:?}",
            clicked.action
        );
        // The refusal is still on screen, which is the other half: the card
        // says why there is no Open rather than merely omitting one.
        assert!(laid_out.painted(APP_OPEN_NO_PATH_NOTE), "{:?}", laid_out.strings());
        // And the chord is not advertised either.
        assert!(
            !laid_out.painted(OPEN_APP_CHORD.2),
            "a chord that does nothing is painted on the row: {:?}",
            laid_out.strings()
        );
    }

    /// The chord opens the app, through the same one action.
    #[test]
    fn the_open_chord_opens_the_matched_app() {
        let m = a_browser_match();
        let item = bound_to(&a_login_on_the_web(), &m);
        let mut pane = a_pane_that_knows_chrome();
        let frame = pane.frame(&item, &TotpState::NoSecret, open_app_chord_events());
        assert_eq!(
            frame.action,
            DetailAction::OpenApp(app_launch_plan(&m, WEB).expect("a launchable match")),
            "{} reported {:?}",
            OPEN_APP_CHORD.2,
            frame.action
        );
    }

    /// And it does nothing at all on a binding that may not be launched --
    /// the same refusal the link and the button obey, because it is the same
    /// function. A dead binding, a Store app, a match with no program file,
    /// and a match whose recorded path `launchable_path` rejects: **all
    /// FOUR** shapes `app_launch_plan` turns down. The fourth was pinned at
    /// `app_open_refusal` alone and never through a real keystroke, which is
    /// the one refusal shape that had no end-to-end witness.
    #[test]
    fn the_open_chord_is_refused_wherever_open_is() {
        let no_path = AppMatch {
            path: String::new(),
            ..a_browser_match()
        };
        let bad_path = AppMatch {
            path: r"C:\Apps\..\chrome.exe".to_string(),
            ..a_browser_match()
        };
        // The premise for the fourth: it is refused for the PATH and not for
        // being dead or hosted, so it is genuinely the shape the other three
        // do not cover.
        assert!(bad_path.launchable_path().is_none(), "the bad-path fixture is launchable");
        assert!(!app_match_is_dead(&bad_path) && !bad_path.hosted);
        for m in [a_dead_match(), a_store_match(), no_path, bad_path] {
            let item = bound_to(&a_login_on_the_web(), &m);
            let mut pane = Pane::new();
            let frame = pane.frame(&item, &TotpState::NoSecret, open_app_chord_events());
            assert_eq!(
                frame.action,
                DetailAction::None,
                "{} started {:?}, which the card refuses to offer an Open for",
                OPEN_APP_CHORD.2,
                m.process
            );
        }
    }

    /// The chord is **discoverable the way the copy chords are**: painted on
    /// the row's control line, in the same place, read from the same tuple
    /// the handler is wired to.
    #[test]
    fn the_open_chord_is_painted_on_the_app_row() {
        let item = bound_to(&a_login_on_the_web(), &a_browser_match());
        let mut pane = a_pane_that_knows_chrome();
        let frame = pane.idle(&item, &TotpState::NoSecret);
        let chord = frame.rect_of(OPEN_APP_CHORD.2);
        let name = frame.rect_of(CHROME_NAME);
        assert!(
            (chord.center().y - name.center().y).abs() < ROW_CONTENT_HEIGHT,
            "the chord at {chord:?} is not on the App row at {name:?}"
        );
        assert!(
            chord.left() > name.right(),
            "the chord at {chord:?} is not in the control group, right of the name at {name:?}"
        );
    }

    /// **No two chords on this pane are the same keystroke** -- the copies
    /// AND the open, which is why `pane_chords` exists rather than the guard
    /// staying on `COPY_SHORTCUTS` alone.
    #[test]
    fn no_two_pane_chords_share_a_keystroke() {
        let all = pane_chords();
        assert_eq!(all.len(), COPY_SHORTCUTS.len() + 1, "a binding fell out of the guard");
        for (index, (name, modifiers, key, chord)) in all.iter().enumerate() {
            assert_eq!(
                *chord,
                format!(
                    "CTRL{}+{}",
                    if modifiers.shift { "+SHIFT" } else { "" },
                    key.name()
                ),
                "{name}'s chord does not spell the keys it is bound to"
            );
            for (other, other_modifiers, other_key, _) in &all[index + 1..] {
                assert!(
                    !(modifiers == other_modifiers && key == other_key),
                    "{name} and {other} are both bound to {modifiers:?}+{key:?}"
                );
            }
        }
    }

    /// **What the chord hint costs, and what is dropped when it will not
    /// fit** -- the name is never the thing that gives way.
    ///
    /// Both numbers, not a `Ui`: the decision is callable without a frame,
    /// and the two directions are asserted against the same threshold the
    /// drawing uses.
    #[test]
    fn the_chord_hint_is_dropped_before_the_apps_name_is_squeezed() {
        // A column that can hold the name AND the hint.
        assert!(app_row_chord_fits(APP_NAME_MIN_WIDTH + 80.0, 72.0));
        // Exactly enough is enough ...
        assert!(app_row_chord_fits(APP_NAME_MIN_WIDTH + 72.0, 72.0));
        // ... and one point less is not.
        assert!(!app_row_chord_fits(APP_NAME_MIN_WIDTH + 71.0, 72.0));
        // The case that produced the defect: the minimum window's value
        // column is about 95pt and `CTRL+SHIFT+O` about 72pt of it.
        assert!(
            !app_row_chord_fits(95.0, 72.0),
            "at the app's minimum window size the hint is reserved room, which leaves the \
             name 23pt and breaks it into a ribbon"
        );
    }

    /// At the app's minimum window size the card fits, the app's real name is
    /// laid out **whole** -- not elided -- and the chord is not painted; the
    /// link's own tooltip is what still names it.
    #[test]
    fn at_the_minimum_window_size_the_name_survives_and_the_chord_moves_to_the_tooltip() {
        let item = bound_to(&a_login_on_the_web(), &a_browser_match());
        let mut pane = Pane::wide(MIN_PANE).knows_app(CHROME_PATH, CHROME_NAME);
        let frame = pane.idle(&item, &TotpState::NoSecret);

        assert_eq!(
            frame.rendered_glyphs(CHROME_NAME),
            CHROME_NAME,
            "the app's name was elided on a {MIN_PANE}pt pane -- the glyphs really laid are \
             not the whole name"
        );
        let name = frame.rect_of(CHROME_NAME);
        assert!(
            name.right() <= MIN_PANE,
            "the name is painted to x = {} on a {MIN_PANE}pt pane that does not scroll \
             sideways",
            name.right()
        );
        assert!(
            !frame.painted(OPEN_APP_CHORD.2),
            "the chord is painted on a pane with no room for it, which is where egui culled \
             it off the right edge: {:?}",
            frame.strings()
        );
        // **And the name was not squeezed to make room for it.** The line
        // above is satisfied two ways -- the hint correctly omitted, and the
        // hint drawn past the pane and culled -- and only this tells them
        // apart: reserving room for the hint here leaves the name about 15pt,
        // which lays `Waypoint Browser` out one syllable per line.
        assert!(
            name.height() <= ROW_VALUE_SIZE * 3.0,
            "the app's name occupies {}pt of height on a {MIN_PANE}pt pane -- it has been \
             broken into a ribbon to keep the chord hint on the line",
            name.height()
        );

        // ... and it is still discoverable, on the surface `COPY_SHORTCUTS`
        // calls this pane's primary one for chords.
        let hovered = pane.hover_settled(&item, &TotpState::NoSecret, name.center());
        assert!(
            hovered
                .strings()
                .iter()
                .any(|s| s.contains(OPEN_APP_CHORD.2) && s.contains(APP_OPEN_HOVER)),
            "hovering the app's name on a narrow pane names neither the act nor the chord: \
             {:?}",
            hovered.strings()
        );

        // The control on the control: on a comfortable pane the hint IS
        // painted, so the absence above is about the width and not about the
        // hint having been deleted.
        let mut wide = Pane::wide(PANE).knows_app(CHROME_PATH, CHROME_NAME);
        assert!(
            wide.idle(&item, &TotpState::NoSecret).painted(OPEN_APP_CHORD.2),
            "the chord is painted on no pane at all"
        );
    }

    // -- the footer: Remove, left-aligned, off the notes -----------------

    /// **The user's third report: "Matched app Remove button is on top of
    /// long description - should be left aligned".**
    ///
    /// It really was on top of it: the notes were drawn with
    /// `set_max_width(app_card_value_width(ui))` -- the whole value column --
    /// and `row_body` then opened the control group in what was left, which
    /// was nothing. Asserting that each rect is inside the pane could not
    /// have caught it; two rects can both be on screen and still be drawn on
    /// top of each other. So this asserts NON-INTERSECTION, explicitly, at
    /// the app's minimum window size and at a comfortable one, with a
    /// resolved name on the card.
    #[test]
    fn the_footers_controls_never_overlap_the_cards_notes() {
        let item = bound_to(&a_login_on_the_web(), &a_browser_match());
        for width in [MIN_PANE, PANE] {
            let mut pane = Pane::wide(width).knows_app(CHROME_PATH, CHROME_NAME);
            let frame = pane.idle(&item, &TotpState::NoSecret);
            let note = frame.rect_of(APP_MATCH_BEHAVIOUR_NOTE);
            for control in [APP_REMOVE_LABEL, OPEN_MENU_LABEL] {
                let rect = frame.rect_of(control);
                assert!(
                    !rect.intersects(note),
                    "on a {width}pt pane {control:?} is painted at {rect:?}, on top of the \
                     note at {note:?}"
                );
                // Below it, not beside it -- which is what "its own line"
                // means and what non-intersection alone would not pin (two
                // rects on one line miss each other by a hair and still read
                // as the old layout).
                assert!(
                    rect.top() >= note.bottom(),
                    "on a {width}pt pane {control:?} at {rect:?} is on the note's line \
                     at {note:?}"
                );
            }
            // **Left aligned**, the user's word: the controls line starts on
            // the card's own left edge, where the labels are -- NOT out in
            // the value column where the notes and every other row's value
            // sit. Open is the leftmost of the two.
            let open = frame.rect_of(OPEN_MENU_LABEL);
            let label = frame.rect_of("App");
            assert!(
                open.left() < note.left(),
                "on a {width}pt pane the controls line starts at {} -- out in the value \
                 column, which begins at {}",
                open.left(),
                note.left()
            );
            assert!(
                open.left() >= label.left(),
                "on a {width}pt pane the controls line starts at {}, left of the card's own \
                 label column at {}",
                open.left(),
                label.left()
            );
            assert!(
                frame.rect_of(APP_REMOVE_LABEL).left() > open.left(),
                "Remove is not after Open on the controls line"
            );
        }
    }

    /// The premise for the test above, and the reason it is worth running at
    /// `PANE`: the wide pane is where the overlap happened. If this ever
    /// fails, the note has stopped being drawn and the non-intersection above
    /// is vacuous.
    #[test]
    fn the_card_really_does_carry_a_note_at_every_width() {
        let item = bound_to(&a_login_on_the_web(), &a_browser_match());
        for width in [MIN_PANE, PANE] {
            let mut pane = Pane::wide(width);
            let frame = pane.idle(&item, &TotpState::NoSecret);
            assert!(
                frame.painted(APP_MATCH_BEHAVIOUR_NOTE),
                "no note on a {width}pt pane: {:?}",
                frame.strings()
            );
            // The glyphs really laid, not the source: a note elided away
            // would still have a rect for the test above to miss.
            assert_eq!(
                frame.rendered_glyphs(APP_MATCH_BEHAVIOUR_NOTE),
                APP_MATCH_BEHAVIOUR_NOTE,
                "the note was elided on a {width}pt pane"
            );
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
/// What counts as **painted**, for the geometry suites on both panes.
///
/// One copy, deliberately. This function is subtle in three separate ways
/// (the alpha test, the `Rect::NOTHING` test, and the stroke expansion), it
/// was written once for `detail_edit.rs` and immediately needed by
/// `detail.rs`, and a second copy is how the two would drift. It lives here
/// rather than in `vault_window/mod.rs` because `detail_edit.rs` already
/// depends on this module (`detail::TotpState`) and nothing depends on
/// `detail_edit.rs`, so this is the direction that adds no edge.
#[cfg(test)]
pub(crate) mod shape_ink {
    use eframe::egui;

    /// Ink a reader could actually see, and the box it covers -- for ANY
    /// leaf shape, including [`egui::Shape::Rect`].
    ///
    /// `None` for four kinds of shape that are allocated but not drawn, each
    /// of which is a vacuous assertion this crate has shipped or nearly did:
    ///
    /// * [`egui::Shape::Noop`], which paints nothing by definition, and
    ///   [`egui::Shape::Vec`] and [`egui::Shape::Text`], which are not leaves:
    ///   a `Vec` is recursed into by the caller's `walk` and a galley's ink is
    ///   its GLYPH boxes, which `Shape::visual_bounding_rect` does not answer
    ///   (it answers the layout job's box, which inside a wrapped row is the
    ///   whole wrap width).
    /// * A shape whose fill AND whose stroke are transparent. That is the
    ///   alpha-0 trap these panes already met once: egui's floating scroll bar
    ///   is allocated at full size and drawn at alpha 0 while the pointer is
    ///   away, and a sibling test once certified the placement of exactly
    ///   such a bar. The test is on the ALPHA, not on `Stroke::is_empty`,
    ///   which only recognises the single colour `Color32::TRANSPARENT` and
    ///   would call an invisible red stroke visible.
    /// * A shape whose visual bounds are empty or infinite -- a zero-area
    ///   shape covers no pixel, and `visual_bounding_rect` answers
    ///   `Rect::NOTHING` for several of the cases above.
    ///
    /// Everything else is ink, boxed by `visual_bounding_rect()`.
    ///
    /// **One caveat on that sentence, recorded rather than fixed.** A
    /// [`egui::epaint::RectShape`] carries a `brush` as well as a `fill` -- a
    /// textured or image rect -- and the visibility test above reads only
    /// `fill.a()` and the stroke. A rect with an alpha-0 `fill`, no stroke and
    /// a live `brush` paints a picture and is reported here as `None`.
    /// Neither pane paints one today (nothing in this crate sets `brush`;
    /// the icons are `Shape::Mesh` and `Shape::Path`), so no assertion is
    /// currently blind because of it -- but the first image rect to arrive
    /// would be invisible to every lane and overflow test in this file, which
    /// is the same shape of blindness the `_ => {}` arm had. Left alone
    /// deliberately: a `brush` clause with nothing that can exercise it is an
    /// untested branch pretending to be coverage.
    ///
    /// **`Rect` is in here, and the box it reports is NOT `RectShape::rect`.**
    /// That was this function's own blind spot for one commit: `walk` recorded
    /// the geometric rect and routed everything else through here, and the
    /// doc-comment said `Rect` was "handled by its own arm". It is, for the
    /// scroll bar's WIDTH -- but the ink is not the rect.
    /// `RectShape::visual_bounding_rect` expands it by the whole stroke width
    /// for [`egui::StrokeKind::Outside`], by half the stroke for `Middle`, by
    /// `blur_width / 2` (which is how epaint renders every `Shadow` this app
    /// paints), and then by whatever `angle` adds. Measured on a 298pt pane
    /// with a box recorded at 280..298: a 6pt outside stroke covers 274..304,
    /// a 20pt blur covers 270..308, and a 0.6 rad rotation covers
    /// 275.9..302.1. All three are ink past the pane's edge that the recorded
    /// rect is silent about.
    ///
    /// Where the colour is a UV callback its alpha is not knowable here, so it
    /// counts as ink: for a test whose job is to catch things out of bounds,
    /// that is the safe direction.
    pub(crate) fn ink_of(shape: &egui::Shape) -> Option<(&'static str, egui::Rect)> {
        use egui::epaint::{ColorMode, PathStroke};
        let stroked = |stroke: &egui::Stroke| stroke.width > 0.0 && stroke.color.a() > 0;
        let path_stroked = |stroke: &PathStroke| {
            stroke.width > 0.0
                && match &stroke.color {
                    ColorMode::Solid(color) => color.a() > 0,
                    ColorMode::UV(_) => true,
                }
        };
        let (kind, visible) = match shape {
            egui::Shape::Rect(s) => ("a box", s.fill.a() > 0 || stroked(&s.stroke)),
            egui::Shape::Circle(s) => ("a circle", s.fill.a() > 0 || stroked(&s.stroke)),
            egui::Shape::Ellipse(s) => ("an ellipse", s.fill.a() > 0 || stroked(&s.stroke)),
            egui::Shape::LineSegment { stroke, .. } => ("a line", stroked(stroke)),
            egui::Shape::Path(s) => ("a path", s.fill.a() > 0 || path_stroked(&s.stroke)),
            egui::Shape::QuadraticBezier(s) => {
                ("a quadratic curve", s.fill.a() > 0 || path_stroked(&s.stroke))
            }
            egui::Shape::CubicBezier(s) => {
                ("a cubic curve", s.fill.a() > 0 || path_stroked(&s.stroke))
            }
            egui::Shape::Mesh(_) => ("a mesh", true),
            egui::Shape::Callback(_) => ("a backend callback", true),
            egui::Shape::Noop | egui::Shape::Vec(_) | egui::Shape::Text(_) => return None,
        };
        if !visible {
            return None;
        }
        let bounds = shape.visual_bounding_rect();
        (bounds.is_positive() && bounds.is_finite()).then_some((kind, bounds))
    }

    /// The box a galley's GLYPHS really cover, in absolute coordinates, or
    /// `None` for a run that laid out no glyph at all.
    ///
    /// Not `Galley::size()`, which is the box the LAYOUT was given: inside a
    /// `horizontal_wrapped` row that is the whole wrap width, so a 40pt word
    /// reports a 93.7pt box and appears to spill into a lane it never
    /// touches. Every one of this crate's earlier geometry blindnesses has
    /// been a galley answering about the layout job rather than about the
    /// pixels.
    pub(crate) fn glyph_ink(text: &egui::epaint::TextShape) -> Option<egui::Rect> {
        let mut ink: Option<egui::Rect> = None;
        for row in text.galley.rows.iter() {
            for glyph in row.glyphs.iter() {
                let at = text.pos + row.pos.to_vec2() + glyph.pos.to_vec2();
                let box_ = egui::Rect::from_min_size(at, glyph.size());
                ink = Some(ink.map_or(box_, |r: egui::Rect| r.union(box_)));
            }
        }
        ink
    }
}

#[cfg(test)]
mod read_pane_scroll_tests {
    use super::*;
    use super::shape_ink::{glyph_ink, ink_of};

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
    /// filled rectangle -- and, since `5fc41ef`'s sibling fix on the edit
    /// pane, everything else that lays down ink.
    ///
    /// The first two fields are **not a partition of what a frame draws**,
    /// and this suite spent its whole life believing they were. `walk` ended
    /// `_ => {}`, so a circle, a line, a path, a curve or a mesh was
    /// DISCARDED, and `the_lane_leaves_the_cards_eighteen_points_of_clear_
    /// space` -- which documents itself as counting "anything visibly painted
    /// in the lane, whatever it is" -- counted only `Shape::Rect`. A filled
    /// circle at x = 274..290 and a 3pt line at 274.5..298.5, both squarely
    /// inside the 18pt that test names, left the entire suite green. The runs
    /// were not consulted either, so a LABEL drawn into the lane was equally
    /// invisible.
    #[derive(Default)]
    struct Shot {
        runs: Vec<(String, String, egui::Rect)>,
        rects: Vec<(egui::Rect, egui::Color32)>,
        /// The box each run's GLYPHS really cover, which is not the box in
        /// `runs` -- that one is `Galley::size()`, the layout job's width.
        /// See [`shape_ink::glyph_ink`].
        glyphs: Vec<(String, egui::Rect)>,
        /// The ink each filled rect really lays down, which is not `rects`'s
        /// box: a stroke, a `blur_width` (how epaint renders every `Shadow`)
        /// and an `angle` all put ink outside `RectShape::rect`. Kept apart
        /// from `rects` because `bar_rects` needs the geometric rect to
        /// measure the bar's WIDTH against, and this to ask the other
        /// question -- where the ink actually goes.
        rect_ink: Vec<(egui::Rect, egui::Color32)>,
        /// Every OTHER shape that lays down ink, named and boxed: carets,
        /// icons, circles, lines, curves, meshes. See [`shape_ink::ink_of`].
        marks: Vec<(&'static str, egui::Rect)>,
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
                if let Some(ink) = glyph_ink(text) {
                    shot.glyphs.push((text.galley.text().to_string(), ink));
                }
            }
            egui::Shape::Rect(rect) => {
                shot.rects.push((rect.rect, rect.fill));
                if let Some((_, ink)) = ink_of(shape) {
                    shot.rect_ink.push((ink, rect.fill));
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, shot);
                }
            }
            // NOT `_ => {}`. That is what this suite used to end with, and it
            // threw away every circle, line, path, curve and mesh the pane
            // drew. See [`Shot`] and [`shape_ink::ink_of`].
            other => {
                if let Some(mark) = ink_of(other) {
                    shot.marks.push(mark);
                }
            }
        }
    }

    /// A pane of a chosen SIZE -- which the `Pane` harness above cannot do,
    /// being fixed at `PANE` tall, and the height is the entire subject here.
    struct ShortPane {
        ctx: egui::Context,
        size: egui::Vec2,
        reveal: RevealState,
        apps: crate::app_identity::AppIdentityCache,
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
                apps: crate::app_identity::AppIdentityCache::default(),
            }
        }

        /// See [`Pane::knows_app`]. Keyed on the PATH, deliberately.
        fn knows_app(mut self, path: &str, name: &str) -> Self {
            self.apps.seed_ready(path, name, None);
            self
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
                        &mut self.apps,
                        false,
                        false,
                        &mut super::tests::inert_breach_cache(),
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

        /// Scrolls the body to its bottom and then back UP, one wheel notch
        /// at a time, until every one of `sources` is painted wholly inside
        /// the pane -- or fails naming the one that never arrived.
        ///
        /// **This replaces `scroll_to_bottom` in the reachability tests, and
        /// it is a stronger question, not a weaker one.** `scroll_to_bottom`
        /// asked "is the card visible at ONE particular scroll offset", which
        /// stopped being the right question the moment NOTES moved below the
        /// MATCHED APP card: the card is still perfectly reachable, it is
        /// simply no longer the last thing on the pane. What the user needs is
        /// that SOME sequence of scrolls brings the whole control into view,
        /// and that is what this looks for. A card that is culled, clipped or
        /// laid out past the pane's right edge at every offset still fails.
        fn scroll_until_all_visible(&mut self, item: &VaultItem, sources: &[&str]) -> Shot {
            let bounds = self.bounds();
            let all_in = |shot: &Shot| {
                sources.iter().all(|source| {
                    shot.rect_of(source)
                        .is_some_and(|rect| bounds.contains_rect(rect))
                })
            };
            let mut shot = self.scroll_to_bottom(item);
            for _ in 0..120 {
                if all_in(&shot) {
                    return shot;
                }
                shot = self.frame(
                    item,
                    vec![
                        egui::Event::PointerMoved(bounds.center()),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: egui::vec2(0.0, 12.0),
                            modifiers: egui::Modifiers::NONE,
                            phase: egui::TouchPhase::Move,
                        },
                    ],
                );
            }
            let missing: Vec<&str> = sources
                .iter()
                .copied()
                .filter(|source| {
                    !shot
                        .rect_of(source)
                        .is_some_and(|rect| bounds.contains_rect(rect))
                })
                .collect();
            panic!(
                "no scroll offset puts {missing:?} wholly inside the {}x{}pt pane; the \
                 last frame painted {:?}",
                bounds.width(),
                bounds.height(),
                shot.sources()
            );
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
                sequence: String::new(),
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
        // **Resolved, and to something LONGER than the exe name it replaced.**
        // The card now draws an app's real name where `Ledgerline.exe` used
        // to be, and a longer run in a fixed column is more width, not less --
        // which is the shape that put this card 467.8pt inside a 298pt pane
        // in the first place. Seeded on the PATH, which is the cache's key.
        let mut pane = ShortPane::new(egui::vec2(NARROW, SHORT)).knows_app(
            r"C:\Deskwarden Test\Ledgerline\Ledgerline.exe",
            TALL_ITEM_APP_NAME,
        );
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

        // **Not `scroll_to_bottom` any more, and the reason is a change in
        // what is at the bottom, not a weakening of the question.** NOTES is
        // now the pane's last card, so the app card no longer happens to be
        // flush with the end of the content -- it is still perfectly
        // reachable, which is what this test is about. See
        // `scroll_until_all_visible`: it demands that ONE offset shows every
        // one of these at once, wholly inside the pane, which is strictly
        // more than the old assertion asked at a fixed offset.
        let sources = [
            APP_CARD_HEADING,
            // The control commit `a33b75e` added, which was the single least
            // reachable thing on the pane.
            "Open Ledgerline.exe",
            // **The only way to undo an app binding from this pane**, and the
            // one this list did not name: it was not painted at all below
            // about 600pt of pane, and the vertical-only `assert_visible`
            // could not have said so if it had been named.
            "Remove",
            "App",
            "Program file",
            // **The VALUES, not only the labels.** Every label on this card
            // sits at x = 41 whatever the pane is, so a list of labels alone
            // is satisfied by a card laid out at any width at all: with the
            // `Program file` path drawn unwrapped the card measured 467.8pt
            // inside a 298pt pane and every label above still passed. The
            // path is the widest thing this pane can be asked to draw, and
            // it is the one that inflated the card.
            //
            // The App row's value is the RESOLVED name now -- longer than the
            // `Ledgerline.exe` this list used to name, and the reason this
            // test is worth re-running for this change at all.
            TALL_ITEM_APP_NAME,
            r"C:\Deskwarden Test\Ledgerline\Ledgerline.exe",
            // The sentence the controls used to be drawn on top of. It is
            // the card's ONE behaviour note now, and it is markedly longer
            // than the trigger caption it replaced -- more lines of wrapped
            // text in the same column, which is more card height, which is
            // exactly the direction that made this card unreachable twice.
            // Removing the pill row took a row off; this put text back on.
            APP_MATCH_BEHAVIOUR_NOTE,
        ];
        let after = pane.scroll_until_all_visible(&item, &sources);
        for source in sources {
            assert_visible(&after, source, bounds);
        }

        // **The pills are gone.** Not merely absent from the list above --
        // asserted absent, on the one pane and the one item where they were
        // painted before this change. `painted` is an exact match on a
        // rendered run, so this cannot be satisfied by the word "Prompt"
        // inside the behaviour note.
        for mode in TRIGGER_ORDER {
            assert!(
                after.rect_of(trigger_label(mode)).is_none(),
                "the {mode:?} pill is still on the MATCHED APP card. Autofill is one global                  preference now, and a per-item control that writes a field nothing reads is                  a setting that does nothing: painted {:?}",
                after.sources()
            );
        }

        // **Nothing on the card is drawn on top of anything else on it.**
        // `assert_visible` says every control is on the pane; it cannot say
        // they are not stacked, which is exactly what the user reported.
        let note = after.rect_of(APP_MATCH_BEHAVIOUR_NOTE).expect("the note");
        for control in ["Open Ledgerline.exe", "Remove"] {
            let rect = after.rect_of(control).expect("the control");
            assert!(
                !rect.intersects(note),
                "{control:?} at {rect:?} is painted on top of the note at {note:?}"
            );
        }
    }

    /// What the tall fixture's executable is called. Unlike its `process`,
    /// unlike its path's file name, and unlike every directory in its path --
    /// see `tests::CHROME_NAME` for why that matters.
    const TALL_ITEM_APP_NAME: &str = "Ledgerline Accounting Suite";

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
            after.rect_of("NOTES").is_some(),
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

    /// **The NOTES card at the app's minimum window size.** Three times a
    /// text or layout change in this file has pushed a control out of the
    /// scroll viewport, and this change did two of the things that do it: it
    /// moved a card to the end of the pane, and it replaced a `ui.label`
    /// with a `TextEdit`, whose wrap width is its own rather than the
    /// layout's.
    ///
    /// So: the note's own glyphs, at 298x600, wholly inside the pane on BOTH
    /// axes and not elided. `assert_visible` is the same check the MATCHED
    /// APP controls get, for the same reason -- egui culls a run laid out
    /// past the clip rect entirely, so an overflowing note would come back
    /// as no note at all rather than as a note sticking out.
    #[test]
    fn the_note_fits_and_is_reachable_on_the_shortest_window() {
        let mut pane = ShortPane::new(egui::vec2(NARROW, SHORT));
        let item = the_tallest_item();
        let note = item
            .notes
            .as_deref()
            .expect("the tallest item has no note, so this proves nothing")
            .to_string();
        let bounds = pane.bounds();

        // The premise: the note is NOT on the pane before scrolling, so what
        // follows is about a card that had to be scrolled to.
        let before = pane.idle(&item);
        assert!(
            before.rect_of("NOTES").is_none(),
            "the tallest item already fits, so this exercises no scrolling: {:?}",
            before.sources()
        );
        assert_visible(&before, "IDENTITY", bounds);

        let after = pane.scroll_until_all_visible(&item, &["NOTES", &note]);
        assert_visible(&after, "NOTES", bounds);
        assert_visible(&after, &note, bounds);

        // And it really is the LAST card at this width too -- the ordering
        // claim is asserted on the wide pane by
        // `notes_is_the_last_card_for_every_kind_that_shows_it`, and a card
        // that reordered only when the pane got narrow would slip past it.
        // A SECOND scroll offset, because at 298x600 the app card is 270pt
        // tall and the frame that shows the note at the very bottom has
        // already carried the app card's heading off the top. Asked for as
        // one offset that shows both headings, which exists and is what an
        // ordering comparison needs.
        let both = pane.scroll_until_all_visible(&item, &["NOTES", APP_CARD_HEADING]);
        let notes_top = both.rect_of("NOTES").expect("just asserted visible").top();
        let app_top = both
            .rect_of(APP_CARD_HEADING)
            .expect("just asserted visible")
            .top();
        assert!(
            notes_top > app_top,
            "at {NARROW}pt NOTES sits at y = {notes_top} and MATCHED APP at {app_top}"
        );
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
            after.rect_of("NOTES").is_some(),
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

    /// Frames enough for egui to fade a floating scroll bar all the way in,
    /// with the pointer parked over the pane throughout.
    ///
    /// **The stated reason used to be folklore, and the real one is the
    /// opposite fade.** This comment claimed egui emits both of the bar's
    /// rects at alpha 0 on frame 1 and that twenty frames is what makes a
    /// VISIBLE bar exist. It does not reproduce: set to **1**,
    /// `the_body_scroll_bar_is_flush_to_the_outer_edge_of_its_own_lane`
    /// still passes, because `ShortPane::new`'s two throwaway frames plus
    /// the parked pointer already yield a visible bar. What makes a bar
    /// visible is the POINTER, not the count.
    ///
    /// The count is load-bearing for the other direction, on the pane that
    /// has nothing to scroll. Measured at `NARROW` x [`ROOMY`] with this set
    /// to 1: egui paints the floating bar's trough and handle at
    /// **x = 292..298, y = 147..2382, alpha 0x66** -- a full-height bar, on a
    /// body that fits -- and fades it out over the following frames. So
    /// `the_lane_leaves_the_cards_eighteen_points_of_clear_space` would fail
    /// on its roomy half, reporting a lane intruder that the reader never
    /// sees, and `the_previous_password_rows_are_the_lane_tests_one_known_
    /// intruder` with it. Twenty frames is what lets a bar that should not
    /// be there finish going away.
    ///
    /// What makes the flush test non-vacuous is neither: it is the parked
    /// pointer and `bar_rects`'s `fill.a() > 0` clause, together. Take the
    /// pointer out of [`settled`] and the bar is allocated and drawn at
    /// alpha 0; `bar_rects` finds nothing and the test correctly reports
    /// that the pane "paints nothing at all in its scroll lane". Take the
    /// alpha clause out as well and the test passes **green on a bar that
    /// was never drawn** -- the vacuous scroll-bar test this crate has
    /// already shipped once. Both measured.
    ///
    /// Same number as `item_list.rs`'s `SETTLE_FRAMES`.
    const SETTLE_FRAMES: usize = 20;

    /// The tallest item minus its binding: at [`NARROW`] the app card's path
    /// row widens the body sideways and paints into the lane on its own
    /// account, which is the separate horizontal defect noted in
    /// `assert_visible` and not what these assertions are about.
    fn the_tallest_item_without_its_binding() -> VaultItem {
        let mut item = the_tallest_item();
        item.fields.clear();
        item
    }

    /// **Everything the frame lays ink with that BEGINS at or past `edge`** --
    /// boxes by the ink they really cover, runs by their GLYPHS, and every
    /// other shape by [`shape_ink::ink_of`].
    ///
    /// Not `Shot::rects`, which was this suite's entire notion of "painted"
    /// until the walker was fixed, and which is blind three separate ways: to
    /// a rect's own stroke, blur and rotation; to circles, lines, paths,
    /// curves and meshes; and to text.
    ///
    /// "Begins at or past" and not "overlaps", deliberately: the pane and
    /// header backgrounds span the full width and cross the edge by
    /// construction, and a run that starts left of the edge and reaches over
    /// it is a row that is too wide, which is a different report.
    fn lane_ink(shot: &Shot, edge: f32) -> Vec<(String, egui::Rect)> {
        let mut found: Vec<(String, egui::Rect)> = Vec::new();
        for (ink, fill) in &shot.rect_ink {
            if fill.a() > 0 && ink.left() >= edge - 0.5 {
                found.push((format!("a {fill:?} box"), *ink));
            }
        }
        for (label, ink) in &shot.glyphs {
            if ink.left() >= edge - 0.5 {
                found.push((format!("the run {label:?}"), *ink));
            }
        }
        for (kind, ink) in &shot.marks {
            if ink.left() >= edge - 0.5 {
                found.push(((*kind).to_string(), *ink));
            }
        }
        found
    }

    /// [`SETTLE_FRAMES`] frames with the pointer in the middle of the pane,
    /// which is what the app is doing whenever the user is looking at this
    /// pane and reaching for the bar.
    fn settled(size: egui::Vec2, item: &VaultItem) -> Shot {
        let mut pane = ShortPane::new(size);
        let over = vec![egui::Event::PointerMoved(pane.bounds().center())];
        let mut shot = pane.frame(item, over.clone());
        for _ in 1..SETTLE_FRAMES {
            shot = pane.frame(item, over.clone());
        }
        shot
    }

    /// The left edge of the lane [`BODY_PAD_X`] reserves.
    fn lane_left(pane: egui::Vec2) -> f32 {
        pane.x - f32::from(BODY_PAD_X)
    }

    /// Every VISIBLE rectangle that starts inside the reserved lane and is
    /// tall and narrow enough to be a scroll bar rather than a card spilling
    /// into it.
    ///
    /// The search and the assertions it feeds answer two different
    /// questions, and the filter is written so that it cannot answer the
    /// assertions'.
    ///
    /// * **Is this a bar?** -- the width ceiling is the LANE's width, not the
    ///   bar's, deliberately. egui's floating default is a 2pt sliver, and
    ///   a ceiling set at [`theme::SCROLLBAR_WIDTH`] would quietly accept it
    ///   as a well-formed bar. What a sliver fails is the assertion, not the
    ///   search. The height floor drops the hairlines and row separators
    ///   that the overflowing card paints across the lane at [`NARROW`].
    /// * **Is the bar where it belongs?** -- NOT asked here. In particular
    ///   there is no `rect.right() <= pane.x` clause: `detail_edit.rs` had
    ///   one and `5fc41ef` removed it, because it made the identical clause
    ///   in the assertion unfailable and made a bar OVERHANGING the pane
    ///   vanish from the search instead of failing it -- reporting "no bar
    ///   was painted, so this test is vacuous" in place of the real problem.
    ///   An overhanging bar is FOUND here and REJECTED below, by a message
    ///   that says it overhangs.
    fn bar_rects(shot: &Shot, pane: egui::Vec2) -> Vec<egui::Rect> {
        let lane = lane_left(pane);
        shot.rects
            .iter()
            .filter(|(rect, fill)| {
                fill.a() > 0
                    && rect.left() >= lane - 0.5
                    && rect.width() > 0.0
                    && rect.width() <= f32::from(BODY_PAD_X) + 0.5
                    && rect.height() > f32::from(BODY_PAD_X)
            })
            .map(|(rect, _)| *rect)
            .collect()
    }

    /// **Where the read pane's bar is, in absolute pane coordinates.**
    ///
    /// The user's report was that the read pane's right padding had shrunk:
    /// a bar centred in its 24pt lane left 9pt of clear space between the
    /// cards and the bar and 9pt of dead gap behind it, against the pane's
    /// own edge where nothing is compared to it. `72c0fea` moved the rule
    /// into `theme::scrollbar_in_gutter` -- the bar takes the OUTERMOST
    /// `SCROLLBAR_WIDTH` of the lane -- which gives this pane 18pt of clear
    /// space and nothing behind the bar. Measured on a 298pt pane: the lane
    /// is x = 274..298, the cards end at 274, and the bar's two rects (the
    /// trough and the handle) both span 292..298.
    ///
    /// **Until this test existed the read pane asserted NOTHING about that.**
    /// Its seven scroll tests locate ink anywhere in the lane and none names
    /// a position, so setting `bar_outer_margin = 18.0` here -- the bar hard
    /// against the cards with 18pt of dead gap behind it, the reported defect
    /// in its worst form -- left the whole suite green. The placement was
    /// protected only by the accident of sharing a helper with
    /// `detail_edit.rs`, which did have an absolute-x assertion.
    ///
    /// **Three checks that DIAGNOSE three defects -- not three independent
    /// detections.** Each was verified to fire, with the numbers below, on
    /// the change named beside it; what none of them is, is a defect the
    /// other two would miss. Given WIDTH (`|w - 6| <= 0.5`) and FLUSH
    /// (`|left - 292| <= 0.5`) both passing, `right <= 299.0` follows
    /// arithmetically, so OVERHANG can only fire inside that slop. The
    /// `bar_outer_margin = -3.0` case below fails FLUSH too; OVERHANG merely
    /// fires first and names it far better -- "hangs 3pt off the right edge
    /// of the pane it scrolls" against "leaves -3pt of dead gap". That is
    /// what all three are for, and it is why none should be dropped as
    /// redundant: a reader who sees one must not be told another's story.
    /// (`5fc41ef` had to correct a doc that misattributed exactly this kind
    /// of essentiality.)
    ///
    /// * dropping `scrollbar_in_gutter` altogether leaves egui's floating
    ///   default, a sliver pinned to the pane's right edge -- measured here at
    ///   296..298, 2pt wide. That fails the WIDTH check first; with the width
    ///   check neutered the FLUSH check fires on its own
    ///   (`spans x = 296..298 but the outermost 6pt ... is 292..298`), because
    ///   flushness is measured from the bar's LEFT edge and a narrow bar that
    ///   shares the right edge does not share the left one;
    /// * `bar_outer_margin = 18.0` -- the bar hard against the cards -- fails
    ///   the FLUSH check and nothing else in this test;
    /// * `bar_outer_margin = -3.0` puts the bar at 295..301, straddling the
    ///   pane edge. It is FOUND by `bar_rects` and fails the OVERHANG check,
    ///   which is the whole point of not filtering on overhang. Push it far
    ///   enough out (-10.0, 302..308) and egui clips every pixel of it before
    ///   it is ever tessellated: then there is no bar in the shot to reject,
    ///   and the empty-`bars` assertion fires instead -- which is the true
    ///   report, since the reader sees no bar at all.
    #[test]
    fn the_body_scroll_bar_is_flush_to_the_outer_edge_of_its_own_lane() {
        let pane = egui::vec2(NARROW, SHORT);
        let item = the_tallest_item_without_its_binding();
        let shot = settled(pane, &item);

        let bars = bar_rects(&shot, pane);
        assert!(
            !bars.is_empty(),
            "a body that overflows a {NARROW}x{SHORT}pt pane paints nothing at all in its \
             scroll lane, so nothing tells the reader there is more below"
        );
        // The outermost `SCROLLBAR_WIDTH` of the lane, absolute: 292..298.
        let bar_left = pane.x - theme::SCROLLBAR_WIDTH;
        for bar in &bars {
            assert!(
                (bar.width() - theme::SCROLLBAR_WIDTH).abs() <= 0.5,
                "the scroll bar is {}pt wide, not {}pt -- this is egui's floating default \
                 painted over the card, not a bar in the reserved lane. Bars: {bars:?}",
                bar.width(),
                theme::SCROLLBAR_WIDTH
            );
            // Three separate assertions, because they fail for three
            // different reasons and a reader who sees one must not be told
            // another's story.
            assert!(
                bar.right() <= pane.x + 0.5,
                "the scroll bar spans x = {}..{} and so hangs {}pt off the right edge of \
                 the {}pt pane it scrolls -- that ink is painted outside the panel \
                 altogether, where the pane clips it. Bars: {bars:?}",
                bar.left(),
                bar.right(),
                bar.right() - pane.x,
                pane.x
            );
            assert!(
                (bar.left() - bar_left).abs() <= 0.5,
                "the scroll bar spans x = {}..{} but the outermost {}pt of the \
                 {BODY_PAD_X}pt lane is {bar_left}..{} -- the bar is not flush to the \
                 pane's outer edge, so it leaves {}pt of the reader's padding as a dead \
                 gap BEHIND itself and only {}pt of clear space between it and the cards, \
                 which is the report this lane exists to answer. Bars: {bars:?}",
                bar.left(),
                bar.right(),
                theme::SCROLLBAR_WIDTH,
                pane.x,
                pane.x - bar.right(),
                bar.left() - lane_left(pane)
            );
        }
    }

    /// The other side of the same measurement: with the bar showing there are
    /// 18 of the lane's 24 points clear between the cards and the bar, and
    /// with nothing to scroll all 24 are clear.
    ///
    /// Ink that BEGINS at or past that edge is what counts: the pane and
    /// header backgrounds span the whole width and cross it by construction,
    /// and at [`NARROW`] the tall card overflows its own width and spills
    /// across it -- the separate horizontal defect noted in `assert_visible`,
    /// which every test in this module already holds apart from the lane.
    ///
    /// `the_bar_does_not_move_the_cards` already pins the cards' right edge
    /// at `NARROW - BODY_PAD_X` on both heights; this states what stands
    /// between that edge and the pane's, which is the quantity the user
    /// actually reported on. Anything VISIBLY painted in the lane counts
    /// against the clear space, whatever it is -- a box, a run's glyphs, a
    /// circle, a line, a path, a curve or a mesh, through [`lane_ink`].
    ///
    /// **That sentence used to be false.** `walk` ended `_ => {}` and this
    /// test read `Shot::rects`, so only `Shape::Rect` counted: a filled
    /// circle at x = 274..290 and a 3pt line at 274.5..298.5, both squarely
    /// inside the 18pt this test names and the line overhanging the pane's
    /// edge, left the whole suite green. A label drawn into the lane was
    /// equally invisible, since the runs were not consulted either.
    /// `the_lane_test_sees_ink_that_is_not_a_rect` is the control that says
    /// so, and it fails on a walker that discards those shapes.
    ///
    /// The item is [`the_tallest_item_without_its_binding`]: the binding's
    /// path row is still held apart for the reason given there, and the
    /// previous-password rows are NOT, because they now fit. They used to be
    /// this test's one licensed intruder -- five reveal eyes at
    /// x = 285.26..303.56, 11pt inside a lane meant to stay clear and 5.6pt
    /// off the pane -- held out through a second helper and pinned by a test
    /// of their own. `masked_row` was taught to stack and to wrap, both went
    /// away, and this test is back on the whole item minus the binding.
    #[test]
    fn the_lane_leaves_the_cards_eighteen_points_of_clear_space() {
        let item = the_tallest_item_without_its_binding();

        // Scrolling: the nearest visible ink past the cards' edge is the bar.
        let pane = egui::vec2(NARROW, SHORT);
        let edge = lane_left(pane);
        let shot = settled(pane, &item);
        let found = lane_ink(&shot, edge);
        let nearest = found.iter().map(|(_, r)| r.left()).fold(pane.x, f32::min);
        assert!(
            (nearest - edge - 18.0).abs() <= 0.5,
            "the first ink past the cards' edge on a scrolling pane is at x = {nearest}, \
             leaving {}pt of clear space and not the 18 the lane is meant to give \
             ({BODY_PAD_X}pt lane less a {}pt bar flush to the outer edge): {found:?}",
            nearest - edge,
            theme::SCROLLBAR_WIDTH
        );

        // Not scrolling: the whole lane is clear.
        let roomy = egui::vec2(NARROW, ROOMY);
        let edge = lane_left(roomy);
        let shot = settled(roomy, &item);

        // The control this half went without. `intruders.is_empty()` is what
        // a pane that painted NOTHING says too -- and a pane that painted
        // nothing is what a regression in `draw_detail_read`, in `settled`,
        // or in `walk` itself looks like from here. The first half is safe
        // without one (its `fold(pane.x, min)` seed makes an empty result
        // fail); this half is not. So: the body really is on screen, and the
        // ink really is being collected, just not in the lane.
        assert!(
            shot.sources().contains(&"Ada Lovelace"),
            "the roomy pane painted no item name, so it drew no body at all and the \
             emptiness below is vacuous: {:?}",
            shot.sources()
        );
        assert!(
            lane_ink(&shot, 0.0).len() > 20,
            "the roomy pane reports only {} pieces of ink across its whole width, so \
             `lane_ink` is not collecting and the emptiness below is vacuous",
            lane_ink(&shot, 0.0).len()
        );

        let intruders = lane_ink(&shot, edge);
        assert!(
            intruders.is_empty(),
            "a body with nothing to scroll still paints {} piece(s) of ink starting past \
             the cards' edge at x = {edge}, so the reader sees less than the \
             {BODY_PAD_X}pt of padding this pane is meant to have: {intruders:?}",
            intruders.len()
        );
    }

    /// **The lane test can see ink that is not a `Shape::Rect`.**
    ///
    /// The reviewer's two plants, exactly: a filled red circle at
    /// x = 274..290 and a 3pt red line at 274.5..298.5, both inside the 18pt
    /// of clear space and the line overhanging the pane's right edge. On the
    /// walker this suite shipped for its whole life -- `_ => {}` -- neither
    /// reached `Shot` at all and the assertion above was silent about them.
    /// A label is planted too, because the runs were not consulted either.
    ///
    /// Deliberately NOT a plant into the roomy shot followed by
    /// `assert!(!intruders.is_empty())`: that would pass on a `lane_ink` that
    /// returned everything. Each plant is named and its ink is compared to
    /// the numbers above, which are not the numbers `lane_ink` is given.
    #[test]
    fn the_lane_test_sees_ink_that_is_not_a_rect() {
        let pane = egui::vec2(NARROW, ROOMY);
        let ctx = ShortPane::new(pane).ctx;
        let edge = lane_left(pane);
        let red = egui::Color32::RED;
        let galley = ctx.fonts_mut(|f| {
            f.layout_no_wrap("HI".to_string(), egui::FontId::proportional(12.0), red)
        });

        let mut shot = Shot::default();
        for shape in [
            egui::Shape::circle_filled(egui::pos2(282.0, 100.0), 8.0, red),
            egui::Shape::LineSegment {
                points: [egui::pos2(276.0, 120.0), egui::pos2(297.0, 120.0)],
                stroke: egui::Stroke::new(3.0, red),
            },
            egui::Shape::Text(egui::epaint::TextShape::new(
                egui::pos2(280.0, 140.0),
                galley,
                red,
            )),
        ] {
            walk(&shape, &mut shot);
        }

        let found = lane_ink(&shot, edge);
        assert_eq!(
            found.len(),
            3,
            "a circle at 274..290, a line at 274.5..298.5 and a label at x = 280 were \
             planted in the {BODY_PAD_X}pt lane of a {NARROW}pt pane, all three past the \
             cards' edge at x = {edge}. The lane test found {}: {found:?}",
            found.len()
        );
        let circle = found
            .iter()
            .find(|(k, _)| k == "a circle")
            .unwrap_or_else(|| panic!("no circle among {found:?}"));
        assert!(
            (circle.1.left() - 274.0).abs() <= 0.01 && (circle.1.right() - 290.0).abs() <= 0.01,
            "the circle's ink is {}..{}, not the 274..290 it covers",
            circle.1.left(),
            circle.1.right()
        );
        let line = found
            .iter()
            .find(|(k, _)| k == "a line")
            .unwrap_or_else(|| panic!("no line among {found:?}"));
        assert!(
            (line.1.left() - 274.5).abs() <= 0.01 && (line.1.right() - 298.5).abs() <= 0.01,
            "the line's ink is {}..{}, not the 274.5..298.5 its 3pt stroke covers -- a \
             stroke's own width is half of what puts this one off the pane",
            line.1.left(),
            line.1.right()
        );
        assert!(
            found.iter().any(|(k, _)| k.starts_with("the run")),
            "the label planted at x = 280 is not in the lane's ink, so a caption drawn \
             into the lane would still be invisible here: {found:?}"
        );

        // ... and the invisible ones are still not ink. An alpha-0 fill and
        // an "invisible red" stroke are the trap this suite's sibling met.
        let mut blank = Shot::default();
        for shape in [
            egui::Shape::circle_filled(
                egui::pos2(282.0, 100.0),
                8.0,
                egui::Color32::from_rgba_premultiplied(255, 0, 0, 0),
            ),
            egui::Shape::LineSegment {
                points: [egui::pos2(276.0, 120.0), egui::pos2(297.0, 120.0)],
                stroke: egui::Stroke::new(
                    3.0,
                    egui::Color32::from_rgba_premultiplied(255, 0, 0, 0),
                ),
            },
        ] {
            walk(&shape, &mut blank);
        }
        assert!(
            lane_ink(&blank, edge).is_empty(),
            "a shape drawn at alpha 0 counts as ink, so the floating scroll bar egui \
             allocates and does not draw would fail this lane: {:?}",
            lane_ink(&blank, edge)
        );
    }

    /// **A box's ink is not its `RectShape::rect`**, and both walkers now say
    /// so. This is `5fc41ef`'s own blind spot, one level down: it routed
    /// every shape but `Rect` through `ink_of` and recorded `Rect` by the
    /// geometric rect, which excludes the stroke, the blur and the rotation.
    ///
    /// The three plants are measured, at the edge of a [`NARROW`] pane, and
    /// the recorded rect passes `<= 298` in every one of them:
    ///
    /// | planted at 280..298 | ink actually covers |
    /// |---|---|
    /// | 6pt `StrokeKind::Outside` | 274..304 |
    /// | 20pt `blur_width` | 270..308 |
    /// | rotated 0.6 rad | 275.9..302.1 |
    ///
    /// `blur_width` is not hypothetical: it is how epaint renders every
    /// `Shadow` this app paints.
    #[test]
    fn a_boxs_ink_is_not_the_rect_it_was_recorded_at() {
        use egui::epaint::{RectShape, StrokeKind};
        let at = egui::Rect::from_min_max(egui::pos2(280.0, 10.0), egui::pos2(298.0, 30.0));
        let red = egui::Color32::RED;
        let cases = [
            (
                "a 6pt outside stroke",
                egui::Shape::Rect(RectShape::new(
                    at,
                    0,
                    red,
                    egui::Stroke::new(6.0, red),
                    StrokeKind::Outside,
                )),
                (274.0_f32, 304.0_f32),
            ),
            (
                "a 20pt blur",
                egui::Shape::Rect(RectShape::filled(at, 0, red).with_blur_width(20.0)),
                (270.0, 308.0),
            ),
            (
                "a 0.6 rad rotation",
                egui::Shape::Rect(RectShape::filled(at, 0, red).with_angle(0.6)),
                (275.9, 302.1),
            ),
        ];
        for (what, shape, (left, right)) in cases {
            let mut shot = Shot::default();
            walk(&shape, &mut shot);
            assert_eq!(
                shot.rects.len(),
                1,
                "{what}: the geometric rect is still recorded, for `bar_rects`"
            );
            assert!(
                (shot.rects[0].0.right() - 298.0).abs() <= 0.01,
                "{what}: the premise is that the RECORDED rect is inside a {NARROW}pt \
                 pane, and it ends at {}",
                shot.rects[0].0.right()
            );
            assert_eq!(shot.rect_ink.len(), 1, "{what}: no ink was recorded at all");
            let ink = shot.rect_ink[0].0;
            assert!(
                (ink.left() - left).abs() <= 0.1 && (ink.right() - right).abs() <= 0.1,
                "{what}: the ink is {}..{}, not the {left}..{right} it covers -- a box \
                 recorded inside the pane whose ink is outside it is exactly what the \
                 overflow tests were blind to",
                ink.left(),
                ink.right()
            );
            assert!(
                ink.right() > NARROW,
                "{what}: the plant is supposed to leave a {NARROW}pt pane and does not, \
                 so this case proves nothing"
            );
        }

        // A box that is allocated and not drawn is still not ink -- neither
        // an alpha-0 fill nor an "invisible red" stroke.
        let mut blank = Shot::default();
        let invisible = egui::Color32::from_rgba_premultiplied(255, 0, 0, 0);
        walk(
            &egui::Shape::Rect(RectShape::new(
                at,
                0,
                invisible,
                egui::Stroke::new(6.0, invisible),
                StrokeKind::Outside,
            )),
            &mut blank,
        );
        assert!(
            blank.rect_ink.is_empty(),
            "an invisible box counts as ink, which is how the floating scroll bar egui \
             draws at alpha 0 would be certified as painted: {:?}",
            blank.rect_ink
        );
    }

    /// **The previous-password rows fit the narrowest pane the app has.**
    ///
    /// This is the test that replaced `the_previous_password_rows_are_the_
    /// lane_tests_one_known_intruder`, which pinned the opposite: at
    /// [`NARROW`] each of the five rows painted its reveal eye as a
    /// `Shape::Path` at x = 285.26..303.56 and a `Shape::Circle` at
    /// 292.01..296.81, on a pane 298pt wide whose cards end at 274. The eye
    /// sat wholly inside the reserved scroll lane and 5.6pt of it was clipped
    /// away; the row's masked run reached 280.2. See [`masked_row`] for what
    /// the row does about it now.
    ///
    /// Asserted against the CARDS' right edge rather than the pane's, because
    /// a control that has merely stopped being clipped is still one the user
    /// finds under the scroll bar.
    ///
    /// The vacuity controls matter here more than usual: "no ink past the
    /// cards' edge" is also what a pane that drew no history at all reports,
    /// and dropping the rows is a repair this test would otherwise certify.
    /// So it counts the eyes and the masked runs first.
    #[test]
    fn the_previous_password_rows_fit_the_narrowest_pane() {
        let pane = egui::vec2(NARROW, ROOMY);
        let edge = lane_left(pane);
        let shot = settled(pane, &the_tallest_item_without_its_binding());

        // The rows are really on the pane: five masked runs, and the eyes
        // that go with them. `the_tallest_item` seeds exactly five previous
        // passwords.
        let masked = "\u{2022}".repeat(MASKED_BULLETS);
        let runs = shot.runs.iter().filter(|(s, _, _)| *s == masked).count();
        assert_eq!(
            runs, 5,
            "the pane painted {runs} masked runs, not the five previous passwords this \
             item carries -- the emptiness below would be vacuous: {:?}",
            shot.sources()
        );
        // The eyes, counted by DIFFERENCE against the same pane with the
        // history taken away -- the pane's header draws a path of its own, so
        // an absolute count would be pinning a number that belongs to another
        // control.
        let paths = |shot: &Shot| shot.marks.iter().filter(|(kind, _)| *kind == "a path").count();
        let mut without = the_tallest_item_without_its_binding();
        without.other.remove("passwordHistory");
        let eyes = paths(&shot) - paths(&settled(pane, &without));
        assert_eq!(
            eyes, 5,
            "the previous-password rows contribute {eyes} reveal eyes, not five -- so the \
             rows lost their controls rather than gaining room for them"
        );

        // ... and every piece of ink any of them lays is inside the cards.
        let past: Vec<&(&str, egui::Rect)> = shot
            .marks
            .iter()
            .filter(|(_, ink)| ink.right() > edge + 0.5)
            .collect();
        assert!(
            past.is_empty(),
            "a reveal eye still reaches past the cards' right edge at x = {edge}, into the \
             {BODY_PAD_X}pt scroll lane: {past:?}"
        );
        let spilled: Vec<&(String, egui::Rect)> = shot
            .glyphs
            .iter()
            .filter(|(label, ink)| *label == masked && ink.right() > edge + 0.5)
            .collect();
        assert!(
            spilled.is_empty(),
            "a masked run still reaches past the cards' right edge at x = {edge}: {spilled:?}"
        );
    }

    /// **A previous password long enough to need the wrap, REVEALED.**
    ///
    /// 44 characters, and every row's password is distinct so that five runs
    /// really means five rows and not one counted five times.
    const LONG_HISTORY_PASSWORD: &str = "correct-horse-battery-staple-9f3c2a71bd4";

    /// [`the_tallest_item_without_its_binding`] with its five `old-secret-N`
    /// entries replaced by ones that cannot fit their line unwrapped.
    ///
    /// 13 characters was never a wrap fixture. The whole item is kept rather
    /// than a fresh minimal login, so this measures the same pane, the same
    /// cards and the same lane the two sibling tests do.
    fn the_tallest_item_with_a_long_history() -> VaultItem {
        let mut item = the_tallest_item_without_its_binding();
        let history: Vec<serde_json::Value> = (0..5)
            .map(|i| {
                serde_json::json!({
                    "lastUsedDate": "2026-07-30T09:15:00.000Z",
                    "password": format!("{LONG_HISTORY_PASSWORD}{i}"),
                })
            })
            .collect();
        item.other
            .insert("passwordHistory".to_string(), serde_json::Value::Array(history));
        item
    }

    /// [`settled`], with every previous-password row already revealed.
    ///
    /// The flag is set on the pane rather than driven through the eyes,
    /// because a click is a second thing that can go wrong and the reveal is
    /// not what this is measuring. `each_history_row_is_revealed_only_by_its_
    /// own_flag` is what pins the flag to the row.
    fn settled_revealing_the_history(size: egui::Vec2, item: &VaultItem) -> Shot {
        let mut pane = ShortPane::new(size);
        pane.reveal.password_history = [true; MAX_HISTORY_ROWS];
        let over = vec![egui::Event::PointerMoved(pane.bounds().center())];
        let mut shot = pane.frame(item, over.clone());
        for _ in 1..SETTLE_FRAMES {
            shot = pane.frame(item, over.clone());
        }
        shot
    }

    /// **The other half of [`masked_row`]'s repair: the WRAP WIDTH, which
    /// nothing pinned.**
    ///
    /// `the_previous_password_rows_fit_the_narrowest_pane` and
    /// `the_lane_leaves_the_cards_eighteen_points_of_clear_space` both render
    /// with `RevealState::default()`, so the only value either of them ever
    /// measures is the ten-bullet mask -- and the mask fits the stacked line
    /// unwrapped. Deleting `job.wrap.max_width = room;` therefore left all
    /// 1597 lib tests green while a revealed previous password ran to
    /// x = 351.2 and its reveal eye to 346.7, on a 298pt pane whose cards end
    /// at 274. That is the same defect `a9dad37` exists to fix, one click
    /// away. `a9dad37`'s message claims "Neither half of the repair is
    /// optional... Pinned by" those two tests; the stacking half was, the
    /// wrapping half was not. This is the missing half.
    ///
    /// Held to the CARDS' right edge and not the pane's, for the reason its
    /// sibling gives: a control that has merely stopped being clipped is
    /// still one the user finds under the scroll bar.
    ///
    /// The vacuity controls come first, and there are two kinds. "No ink past
    /// the cards" is also what a pane that drew no history reports, so the
    /// five revealed runs are counted. And a revealed value that happened to
    /// fit its line would measure the wrap no more than the mask does, so the
    /// runs are shown to have really wrapped.
    #[test]
    fn a_revealed_previous_password_fits_the_narrowest_pane_too() {
        let pane = egui::vec2(NARROW, ROOMY);
        let edge = lane_left(pane);
        let item = the_tallest_item_with_a_long_history();
        let shot = settled_revealing_the_history(pane, &item);

        // The rows are really on the pane, and really in the clear: five
        // distinct plaintext runs, one per history entry.
        let revealed: Vec<&(String, egui::Rect)> = shot
            .glyphs
            .iter()
            .filter(|(label, _)| label.starts_with(LONG_HISTORY_PASSWORD))
            .collect();
        assert_eq!(
            revealed.len(),
            5,
            "the pane painted {} revealed previous passwords, not the five this item \
             carries -- so the emptiness below would be vacuous: {:?}",
            revealed.len(),
            shot.sources()
        );
        // The eyes that go with them, counted by DIFFERENCE against the same
        // pane with the history taken away, exactly as the masked sibling
        // does: the card's header draws a path of its own.
        let paths = |shot: &Shot| shot.marks.iter().filter(|(kind, _)| *kind == "a path").count();
        let mut without = the_tallest_item_with_a_long_history();
        without.other.remove("passwordHistory");
        let eyes = paths(&shot) - paths(&settled_revealing_the_history(pane, &without));
        assert_eq!(
            eyes, 5,
            "the revealed previous-password rows contribute {eyes} reveal eyes, not five"
        );

        // The property. Every piece of ink the revealed pane lays -- the
        // runs' glyphs and every non-text mark, the eyes among them -- is
        // inside the cards.
        let spilled: Vec<&&(String, egui::Rect)> = revealed
            .iter()
            .filter(|(_, ink)| ink.right() > edge + 0.5)
            .collect();
        assert!(
            spilled.is_empty(),
            "a REVEALED previous password reaches past the cards' right edge at \
             x = {edge}, into the {BODY_PAD_X}pt scroll lane -- the value's wrap width is \
             gone, so the run drew its full unwrapped length: {spilled:?}"
        );
        let past: Vec<&(&str, egui::Rect)> = shot
            .marks
            .iter()
            .filter(|(_, ink)| ink.right() > edge + 0.5)
            .collect();
        assert!(
            past.is_empty(),
            "a mark on the revealed pane reaches past the cards' right edge at x = {edge} \
             -- a reveal eye pushed off the card by a value that did not fold: {past:?}"
        );

        // ... and the runs really did fold, so the assertion above is about a
        // wrap and not about a value that happened to fit. Measured against
        // the height of the masked run on the same pane, which is one line by
        // construction.
        let masked = "\u{2022}".repeat(MASKED_BULLETS);
        let one_line = settled(pane, &item)
            .glyphs
            .iter()
            .filter(|(label, _)| *label == masked)
            .map(|(_, ink)| ink.height())
            .fold(0.0_f32, f32::max);
        assert!(
            one_line > 0.0,
            "the masked pane painted no bullet run to measure a line against"
        );
        for (label, ink) in &revealed {
            assert!(
                ink.height() > one_line * 1.5,
                "the revealed run {label:?} is {}pt tall against a {one_line}pt line, so it \
                 fitted on one line and this test measures the wrap no better than the \
                 masked fixtures do -- lengthen LONG_HISTORY_PASSWORD",
                ink.height()
            );
        }
    }
}

/// **The breach badge**: the metadata strip's second run.
///
/// Its own module rather than more tests in `tests` above, because every one
/// of them has to drive `draw_detail_read` with the two new arguments under
/// its own control, and the harnesses up there deliberately pass the feature
/// off.
///
/// **Nothing here can reach the network.** `BreachCache::live` is not named
/// anywhere in this module; every cache below is `BreachCache::new` around a
/// closure that returns a constant. `no_breach_test_here_can_reach_the_real_
/// api` checks that on this module's own source rather than on this sentence.
#[cfg(test)]
mod breach_badge_tests {
    use super::shape_ink::glyph_ink;
    use super::tests::{an_item, item_type_for, EVERY_KIND};
    use super::*;
    use crate::breach::{BreachCache, BreachStatus};
    use crate::vault_bridge::{ItemKind, VaultItem};
    use std::sync::Arc;

    /// The width `tests::PANE` uses, so the numbers pinned below are about
    /// the same pane every other geometry assertion in this file is about.
    const PANE: f32 = 900.0;

    /// The narrowest the detail column can ever be -- 900 - 212 - 390 =
    /// 298pt -- spelled out of the three constants that produce it exactly as
    /// `read_pane_scroll_tests::NARROW` is. The breached segment is the
    /// longest string this strip has ever carried and this is the width it
    /// has to survive.
    const NARROW: f32 = crate::settings::MIN_VAULT_WINDOW_SIZE.0 as f32
        - crate::vault_window::SIDEBAR_WIDTH
        - crate::vault_window::LIST_WIDTH;

    const PASSWORD: &str = "hunter2-but-longer";

    /// The strip these tests expect beside the badge: `fill_count` is 3 in
    /// the harness below and the fixture carries no `revisionDate`, so
    /// `metadata_line_for` is `metadata_line(None, 3, PASSWORD)`. Named
    /// through the production function, never written out, so a reworded
    /// strip moves this with it instead of failing here.
    fn strip_text() -> String {
        metadata_line(None, 3, PASSWORD)
    }

    /// An item of `kind` carrying a login block with `password` in it.
    ///
    /// **Every kind gets the login block**, including the four that are not
    /// logins. That is the point: `draw_detail_read` reads the password out
    /// of `item.login` whatever the kind is, so a card with a password in it
    /// is exactly the item that would get a badge if the gate were the
    /// password rather than the kind.
    fn an_item_of(kind: ItemKind, password: Option<&str>) -> VaultItem {
        let mut item = an_item(item_type_for(kind));
        item.login = Some(crate::vault_bridge::LoginData {
            username: Some("u".to_string()),
            password: password.map(|p| p.to_string().into()),
            totp: None,
            uris: Vec::new(),
            other: serde_json::Map::new(),
        });
        item
    }

    /// One painted string, with the ink a reader would actually see.
    #[derive(Clone, Debug)]
    struct Run {
        /// The layout job's SOURCE string.
        text: String,
        /// The characters egui really placed glyphs for -- empty for a run
        /// that was allocated and laid out nothing.
        rendered: String,
        /// The box the GLYPHS cover, not the box the layout was given. Inside
        /// a `horizontal_wrapped` row the second is the whole wrap width; see
        /// [`glyph_ink`], whose doc is the record of that mistake.
        ink: egui::Rect,
        /// One box per glyph actually laid out **that paints something**.
        ///
        /// Whitespace is dropped: a space is allocated and advances the
        /// cursor but marks no pixel, so it cannot sit on top of anything.
        /// Keeping it produced this test's second false positive -- the
        /// segment's leading space overlapped the strip's last `g` by 0.7pt
        /// of font bearing, which is two adjacent glyphs on one line and not
        /// a collision.
        ///
        /// **`ink` is not good enough for a collision test.** It is the union
        /// over every row of a run, and a wrapped run's union covers the gap
        /// at the end of each row that the run never touches. The first
        /// version of `the_breached_segment_stays_inside_the_card` failed on
        /// exactly that: the strip wraps to two rows, the segment starts
        /// beside the strip's second row, and the two unions overlapped while
        /// no glyph of either was anywhere near a glyph of the other.
        glyphs: Vec<egui::Rect>,
        /// **The colour the tessellator will use**, resolved the way epaint
        /// resolves it: an override wins, a `PLACEHOLDER` section falls back
        /// to the shape's fallback colour, and the whole thing is scaled by
        /// the shape's opacity factor. Reading `RichText`'s colour instead
        /// would restate the argument this file passed in rather than measure
        /// what it painted -- and a run at alpha 0 has a perfectly correct
        /// rectangle.
        color: egui::Color32,
    }

    fn collect_runs(shape: &egui::Shape, out: &mut Vec<Run>) {
        match shape {
            egui::Shape::Text(text) => {
                let section = text
                    .galley
                    .job
                    .sections
                    .first()
                    .map(|s| s.format.color)
                    .unwrap_or(egui::Color32::PLACEHOLDER);
                let base = text.override_text_color.unwrap_or(
                    if section == egui::Color32::PLACEHOLDER {
                        text.fallback_color
                    } else {
                        section
                    },
                );
                let alpha = (f32::from(base.a()) * text.opacity_factor).round();
                let color = egui::Color32::from_rgba_unmultiplied(
                    base.r(),
                    base.g(),
                    base.b(),
                    alpha.clamp(0.0, 255.0) as u8,
                );
                let rendered: String = text
                    .galley
                    .rows
                    .iter()
                    .flat_map(|row| row.glyphs.iter().map(|glyph| glyph.chr))
                    .collect();
                let glyphs = text
                    .galley
                    .rows
                    .iter()
                    .flat_map(|row| {
                        row.glyphs
                            .iter()
                            .filter(|glyph| !glyph.chr.is_whitespace())
                            .map(move |glyph| {
                                egui::Rect::from_min_size(
                                    text.pos + row.pos.to_vec2() + glyph.pos.to_vec2(),
                                    glyph.size(),
                                )
                            })
                    })
                    .collect();
                out.push(Run {
                    text: text.galley.text().to_string(),
                    rendered,
                    ink: glyph_ink(text).unwrap_or(egui::Rect::NOTHING),
                    glyphs,
                    color,
                });
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_runs(shape, out);
                }
            }
            _ => {}
        }
    }

    fn collect_fills(shape: &egui::Shape, out: &mut Vec<(egui::Rect, egui::Color32)>) {
        match shape {
            egui::Shape::Rect(rect) => out.push((rect.rect, rect.fill)),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_fills(shape, out);
                }
            }
            _ => {}
        }
    }

    /// What one frame of the pane painted.
    struct Painted {
        runs: Vec<Run>,
        fills: Vec<(egui::Rect, egui::Color32)>,
    }

    impl Painted {
        /// Every run whose source text contains `needle`.
        fn matching(&self, needle: &str) -> Vec<&Run> {
            self.runs.iter().filter(|r| r.text.contains(needle)).collect()
        }

        /// The one run containing `needle`, and a named failure if there is
        /// not exactly one. **This is the count assertion the brief asks for
        /// and it comes first in every geometry test below**: egui culls
        /// shapes that fall entirely outside the screen rect, so a badge
        /// pushed past the pane's edge comes back as *nothing at all*, and
        /// "everything I found fits" is green on a strip that lost it.
        fn only(&self, needle: &str) -> &Run {
            let hits = self.matching(needle);
            assert_eq!(
                hits.len(),
                1,
                "expected exactly one painted run containing {needle:?}, found {}: {:?}",
                hits.len(),
                self.runs.iter().map(|r| &r.text).collect::<Vec<_>>()
            );
            hits[0]
        }

        /// The smallest CARD-filled rectangle that encloses `run` -- the tile
        /// the run is supposed to be inside of.
        fn card_around(&self, run: &Run) -> egui::Rect {
            self.fills
                .iter()
                .filter(|(rect, fill)| *fill == theme::CARD && rect.contains_rect(run.ink))
                .map(|(rect, _)| *rect)
                .min_by(|a, b| a.area().partial_cmp(&b.area()).expect("a NaN card"))
                .unwrap_or_else(|| {
                    panic!("no CARD tile encloses {:?} at {:?}", run.text, run.ink)
                })
        }
    }

    /// Frames of `draw_detail_read` with the breach feature in a known state,
    /// returning what the LAST one painted.
    ///
    /// `answer` is what the stub worker returns. `BreachCache::status` starts
    /// that worker and says `Pending` on the frame that asks, so a test about
    /// `Safe`, `Breached` or `Unavailable` has to run frames until the answer
    /// has been promoted -- `settle` does that, and asserts rather than
    /// silently returning the pending frame if it never arrives.
    ///
    /// The stub is a closure over a `Copy` status. It opens no socket, and
    /// the cache's live constructor is never named here -- see
    /// `no_breach_test_here_can_reach_the_real_api`, which is why that
    /// sentence does not spell the name it is about.
    fn painted(
        item: &VaultItem,
        enabled: bool,
        answer: BreachStatus,
        width: f32,
        settle: bool,
    ) -> Painted {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 900.0),
            )),
            ..Default::default()
        };
        // `theme::apply`'s font set only takes effect at the start of the
        // next frame, so a throwaway one runs first -- the same two-step
        // every other harness in this file does.
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});

        let mut cache = BreachCache::new(Arc::new(move |_, _| answer));
        let mut reveal = RevealState::default();
        let mut apps = crate::app_identity::AppIdentityCache::default();
        let want = strip_segment(answer).expect("every status has a segment");
        let mut painted = Painted { runs: Vec::new(), fills: Vec::new() };
        for _ in 0..600 {
            let output = ctx.run_ui(input(), |ui| {
                draw_detail_read(
                    ui,
                    item,
                    None,
                    3,
                    &TotpState::NoSecret,
                    false,
                    &mut reveal,
                    None,
                    &mut apps,
                    enabled,
                    false,
                    &mut cache,
                );
            });
            painted.runs.clear();
            painted.fills.clear();
            for clipped in &output.shapes {
                collect_runs(&clipped.shape, &mut painted.runs);
                collect_fills(&clipped.shape, &mut painted.fills);
            }
            if !settle || painted.runs.iter().any(|r| r.text.contains(&want)) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if settle {
            assert!(
                painted.runs.iter().any(|r| r.text.contains(&want)),
                "the stub's {answer:?} never reached the strip; painted: {:?}",
                painted.runs.iter().map(|r| &r.text).collect::<Vec<_>>()
            );
        }
        painted
    }

    /// The metadata card's WIDTH and HEIGHT on the unchanged tree, measured
    /// on `a506592` (the commit before this badge existed) for this exact
    /// fixture: a login with `PASSWORD`, `fill_count` 3, no `revisionDate`,
    /// on a 900pt pane. Card `[[24 371] - [876 412]]`.
    ///
    /// Absolute numbers taken from BEFORE the change, which is the only kind
    /// that can prove the card did not move. A number measured afterwards
    /// would be a photograph of whatever this commit happens to do.
    const BASELINE_CARD: (f32, f32) = (852.0, 41.0);

    /// The same, at the detail column's minimum width: `[[24 471] - [275 525]]`.
    const BASELINE_CARD_NARROW: (f32, f32) = (251.0, 54.0);

    /// **The "we broke nothing" test.** With the preference off -- which is
    /// how it ships -- the strip is the string `metadata_line` produces and
    /// the card is the size it was before any of this existed.
    #[test]
    fn the_strip_is_byte_identical_when_the_check_is_off() {
        let item = an_item_of(ItemKind::Login, Some(PASSWORD));
        let off = painted(&item, false, BreachStatus::Safe, PANE, false);

        // Byte-identical, compared against `metadata_line`'s own output --
        // not against a copy of the wording written out here.
        let strip = off.only(&strip_text());
        assert_eq!(strip.text, metadata_line(None, 3, PASSWORD));
        assert_eq!(strip.color, theme::TEXT_FAINT);
        assert!(strip.color.a() > 0, "the strip is painted at alpha 0");

        // And nothing else in the frame says anything about a breach, in any
        // casing. A segment that leaked in with the feature off would be a
        // request the user did not consent to as well as a string.
        let lower: Vec<String> = off.runs.iter().map(|r| r.text.to_lowercase()).collect();
        assert!(
            !lower.iter().any(|t| t.contains("breach")),
            "the feature is off and the pane painted a breach segment: {lower:?}"
        );

        // The card is the size it was on the unchanged tree.
        let card = off.card_around(strip);
        assert_eq!(
            (card.width(), card.height()),
            BASELINE_CARD,
            "the metadata card moved with the feature off; it was {BASELINE_CARD:?}"
        );

        // **The guard against a vacuous absence.** The same fixture with the
        // preference ON gains a run in the same frame, so the two assertions
        // above are about the preference and not about a renderer that
        // stopped drawing the strip's second half for everyone.
        let on = painted(&item, true, BreachStatus::Safe, PANE, false);
        assert!(
            on.matching("Breach check").len() == 1,
            "with the preference on the pane painted no segment, so the off case proves nothing"
        );
        // And the decision underneath agrees with the pixels.
        assert!(!should_check(false, ItemKind::Login, PASSWORD));
        assert!(should_check(true, ItemKind::Login, PASSWORD));
    }

    /// **The worst outcome this feature has**: a request that failed, shown
    /// as a password that was checked and cleared.
    #[test]
    fn an_unavailable_check_never_says_safe() {
        let item = an_item_of(ItemKind::Login, Some(PASSWORD));
        let frame = painted(&item, true, BreachStatus::Unavailable, PANE, true);
        let seg = frame.only("Breach check unavailable");

        let lower = seg.text.to_lowercase();
        assert!(!lower.contains("not in"), "the unavailable badge reassures: {:?}", seg.text);
        assert!(!lower.contains("safe"), "the unavailable badge reassures: {:?}", seg.text);
        // Nor is it the Safe wording under another name.
        assert_ne!(
            strip_segment(BreachStatus::Unavailable),
            strip_segment(BreachStatus::Safe)
        );
        // The pure decision says the same thing, so this is not only a fact
        // about the one string the renderer happened to reach.
        let text = strip_segment(BreachStatus::Unavailable).unwrap().to_lowercase();
        assert!(!text.contains("not in") && !text.contains("safe"), "{text:?}");
        // Painted, visible, and NOT the alarm colour -- "could not be
        // checked" is not a verdict in either direction.
        assert!(!seg.rendered.is_empty(), "the unavailable badge laid out no glyphs");
        assert!(seg.color.a() > 0, "the unavailable badge is painted at alpha 0");
        assert_eq!(seg.color, theme::TEXT_FAINT);
        assert!(!segment_is_urgent(BreachStatus::Unavailable));
    }

    /// Only a login has a password, so only a login has anything to check.
    #[test]
    fn only_logins_get_a_breach_segment() {
        // **The loop has to be worth running.** Without both of these it can
        // pass on an empty list, or on a list of six logins.
        assert!(!EVERY_KIND.is_empty());
        assert!(
            EVERY_KIND.iter().any(|k| kind_offers_fill(*k)),
            "no kind in the loop offers a fill, so the true case is never taken"
        );
        assert!(
            EVERY_KIND.iter().any(|k| !kind_offers_fill(*k)),
            "every kind in the loop is a login, so this proves nothing"
        );

        let mut checked = 0;
        for kind in EVERY_KIND {
            let item = an_item_of(kind, Some(PASSWORD));
            // One frame: the answer does not matter here, only whether the
            // pane asked at all, and the frame that asks says "checking".
            let frame = painted(&item, true, BreachStatus::Safe, PANE, false);
            let has_segment = !frame.matching("Breach check").is_empty();
            assert_eq!(
                has_segment,
                kind_offers_fill(kind),
                "{kind:?}: painted {has_segment}, `kind_offers_fill` says {}",
                kind_offers_fill(kind)
            );
            // The decision and the render, checked against each other rather
            // than each against a copy of the rule.
            assert_eq!(should_check(true, kind, PASSWORD), kind_offers_fill(kind), "{kind:?}");
            checked += 1;
        }
        assert_eq!(checked, EVERY_KIND.len());
        assert!(checked >= 6, "only {checked} kinds were checked");
    }

    /// An item with no password has no prefix worth asking about, and every
    /// one of them would share a cache entry.
    #[test]
    fn should_check_is_false_for_an_empty_password_even_when_enabled() {
        assert!(!should_check(true, ItemKind::Login, ""));
        // The other two conditions held, so the empty password is what did it.
        assert!(should_check(true, ItemKind::Login, PASSWORD));

        // And the pane obeys it, for both spellings of "no password": a login
        // whose password field is absent, and one whose password is "".
        for password in [None, Some("")] {
            let item = an_item_of(ItemKind::Login, password);
            let frame = painted(&item, true, BreachStatus::Safe, PANE, false);
            assert!(
                frame.matching("Breach check").is_empty(),
                "a login with password {password:?} was checked anyway: {:?}",
                frame.runs.iter().map(|r| &r.text).collect::<Vec<_>>()
            );
        }
        // The same fixture WITH a password does get one, so the absence above
        // is the empty password and not a broken harness.
        let item = an_item_of(ItemKind::Login, Some(PASSWORD));
        assert_eq!(
            painted(&item, true, BreachStatus::Safe, PANE, false)
                .matching("Breach check")
                .len(),
            1
        );
    }

    /// A red warning painted in faint grey is not a warning.
    #[test]
    fn a_breached_item_says_change_it_and_says_it_in_red() {
        let item = an_item_of(ItemKind::Login, Some(PASSWORD));
        let frame = painted(&item, true, BreachStatus::Breached(3), PANE, true);
        let want = strip_segment(BreachStatus::Breached(3)).unwrap();
        let seg = frame.only(&want);

        assert!(
            seg.text.to_lowercase().contains("change this password"),
            "the breached badge does not tell the user to change it: {:?}",
            seg.text
        );
        assert!(seg.text.contains('3'), "the count is not in the badge: {:?}", seg.text);
        assert!(!seg.rendered.is_empty(), "the breached badge laid out no glyphs");

        // **THE COLOUR, off the shape the tessellator is handed** -- not off
        // the `RichText` this file built.
        assert_eq!(
            seg.color,
            theme::ERROR,
            "the breach warning is not the palette's red; it is {:?}",
            seg.color
        );
        assert_eq!(seg.color.a(), 255, "an alarm at alpha {} is not an alarm", seg.color.a());
        assert_ne!(seg.color, theme::TEXT_FAINT);

        // The strip beside it stays faint, so the assertion above is about
        // the segment and not about a card that turned red wholesale.
        let strip = frame.only(&strip_text());
        assert_eq!(strip.color, theme::TEXT_FAINT);

        // The advice does not soften for a small number and does not escalate
        // for a large one -- `breach_phrase` owns that and this is the badge
        // agreeing with it.
        for count in [1_u64, 3, 40_000] {
            let text = strip_segment(BreachStatus::Breached(count)).unwrap();
            assert!(
                text.to_lowercase().contains("change this password"),
                "{count}: {text:?}"
            );
            assert!(segment_is_urgent(BreachStatus::Breached(count)));
            assert_eq!(segment_color(BreachStatus::Breached(count)), theme::ERROR);
        }
    }

    /// The longest string this strip has ever carried, at the narrowest the
    /// detail column can be.
    #[test]
    fn the_breached_segment_stays_inside_the_card() {
        let item = an_item_of(ItemKind::Login, Some(PASSWORD));
        let frame = painted(&item, true, BreachStatus::Breached(40_000), NARROW, true);
        let want = strip_segment(BreachStatus::Breached(40_000)).unwrap();

        // **The count first.** egui culls shapes entirely outside the screen
        // rect, so a segment shoved past the right edge would come back as
        // nothing and every containment assertion below would pass vacuously.
        let seg = frame.only(&want);
        let strip = frame.only(&strip_text());

        // Real ink: glyphs laid out, and a colour a reader can see.
        assert!(!seg.rendered.is_empty(), "the segment laid out no glyphs");
        assert!(seg.ink.is_positive(), "the segment covers no area: {:?}", seg.ink);
        assert!(seg.color.a() > 0, "the segment is painted at alpha 0");

        // Inside the pane at all -- the cull would have hidden this, which is
        // why the count assertion had to come first.
        assert!(
            seg.ink.min.x >= 0.0 && seg.ink.max.x <= NARROW,
            "the segment runs from {} to {} on a {NARROW}pt pane",
            seg.ink.min.x,
            seg.ink.max.x
        );

        // Inside the card the strip is in -- the same tile, not one of its
        // own. Half a point of slack for the glyph boxes' subpixel edges.
        let card = frame.card_around(strip);
        assert!(
            card.expand(0.5).contains_rect(seg.ink),
            "the segment {:?} is outside its card {card:?}",
            seg.ink
        );

        // **No neighbour's ink is under the badge's ink**, compared glyph box
        // against glyph box rather than union against union -- see
        // `Run::glyphs`. Half a point in each direction of real overlap is
        // the threshold, so two glyphs that merely share an edge are not a
        // collision.
        let collides = |a: &egui::Rect, b: &egui::Rect| {
            let hit = a.intersect(*b);
            hit.width() > 0.5 && hit.height() > 0.5
        };
        let mut compared = 0_usize;
        for other in &frame.runs {
            if std::ptr::eq(other, seg) {
                continue;
            }
            for theirs in &other.glyphs {
                for ours in &seg.glyphs {
                    compared += 1;
                    assert!(
                        !collides(ours, theirs),
                        "the segment's glyph at {ours:?} sits on {:?}'s glyph at {theirs:?}",
                        other.text
                    );
                }
            }
        }
        // The strip is one of those neighbours, and it is the one the badge
        // shares a line with -- so the loop above is not vacuous.
        assert!(!strip.glyphs.is_empty(), "the strip laid out no glyphs to collide with");
        assert!(
            compared >= strip.glyphs.len() * seg.glyphs.len(),
            "the overlap loop compared {compared} pairs, fewer than the strip alone has"
        );

        // The card grew downwards to hold it and not sideways: the width is
        // the width the unchanged tree measured at this pane, and the height
        // is greater than it was because there is now a second thing in it.
        assert_eq!(card.width(), BASELINE_CARD_NARROW.0, "the card changed width");
        assert!(
            card.height() > BASELINE_CARD_NARROW.1,
            "the card is {}pt tall against the {}pt it was with no segment, so the segment \
             is being drawn in the space the strip already occupied",
            card.height(),
            BASELINE_CARD_NARROW.1
        );
    }

    /// This module's own source, scanned to EOF: no test here builds the live
    /// cache, and none of them names the production endpoint.
    ///
    /// Written as a probe of the region rather than as a claim about it --
    /// the `#[cfg(test)]` guards elsewhere in this crate have been blind to
    /// everything below the file's first test module, and this module is
    /// below it. The needle is `concat!`-split so this test does not match
    /// its own text.
    #[test]
    fn no_breach_test_here_can_reach_the_real_api() {
        let source = include_str!("detail.rs");
        let start = source
            .find("mod breach_badge_tests {")
            .expect("this module is not in its own file");
        let mine = &source[start..];
        assert!(
            mine.contains("BreachCache::new(Arc::new(move |_, _| answer))"),
            "the module this scans is not the one with the harness in it"
        );
        assert_eq!(
            mine.matches(concat!("BreachCache::", "live")).count(),
            0,
            "a test in this module builds the live cache"
        );
        assert_eq!(
            mine.matches(concat!("pwnedpasswords", ".com")).count(),
            0,
            "a test in this module names the production endpoint"
        );
    }
}

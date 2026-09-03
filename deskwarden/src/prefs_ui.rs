//! Design 3e's sectioned preferences window.
//!
//! 3e is a left nav of seven sections (General, Autofill, Native apps,
//! Security, Shortcuts, Sync & account, About) beside a content pane, with the
//! app version pinned to the bottom of the nav. This file builds that shell,
//! and populates it **only where a setting genuinely exists** -- which is now
//! also what decides whether a section is listed at all. Five of 3e's seven
//! rows had nothing on them a user could change and have been removed; see
//! [`Section`], and `no_page_in_the_nav_is_only_prose`, which is what stops
//! another one appearing.
//!
//! What exists today is four fields on [`Settings`]: `keep_backend_running`,
//! `prompt_on_match`, `auto_lock_enabled` and `auto_lock_minutes`. All four
//! live on General -- the last two as a toggle and the number it governs, the
//! number greyed out while the toggle is off. `prompt_on_match` is the whole
//! of the automatic half of autofill, and is the one setting here that a
//! section of 3e (Autofill) would otherwise have claimed. Every other section in 3e -- its
//! five autofill toggles, the per-app table, Touch ID, the overlay-position
//! segmented control -- has no backing behaviour anywhere in this crate, so
//! those sections say so in one line rather than showing a switch that flips
//! and changes nothing. A control whose state is not connected to anything is
//! indistinguishable from a broken feature, and is this project's most-repeated
//! defect; an empty section is merely unfinished, which is the truth.
//!
//! Two caveats on the design, both deliberate:
//!
//!  * **3e is drawn as a macOS window** (traffic lights, a centred
//!    "Preferences" title, 44px bar) and is the only Preferences block in the
//!    document -- there is no Windows variant to read. Its *metrics, palette
//!    and typography* are taken verbatim; its window chrome is not, because
//!    this crate is Windows and every other window here paints
//!    [`draw_window_chrome`]'s bar instead.
//!  * **"Deskwarden 1.4.0" in 3e's nav footer is mock data.** The real version
//!    comes from `CARGO_PKG_VERSION`. So does 3e's "Bitwarden account linked" --
//!    see [`ACCOUNT_STATUS`] for why that one cannot be shown here at all yet.

use crate::login_ui::{draw_window_chrome, round_window_corners, ChromeAction};
use crate::service_keys::{Access, KeyRecord, Scope, Subject};
use crate::settings::{
    clamp_auto_lock_minutes, parse_clipboard_minutes, ClearInterval, ClipboardEntry, Settings,
};
use crate::vault_bridge::ItemKind;
use crate::theme;
use eframe::egui::{
    self, CornerRadius, FontFamily, FontId, Margin, Pos2, Rect, RichText, Sense, Stroke,
    StrokeKind, Ui, Vec2,
};
use std::cell::RefCell;
use std::rc::Rc;

const WINDOW_TITLE: &str = "Deskwarden Preferences";

/// 3e's window card measures 1000x780, and a seven-row nav plus a content pane
/// does not fit in the 520x300 this window used while it had a single toggle
/// in it. Still non-resizable, like every other fixed window here: nothing in
/// this layout reflows usefully, and `login_ui::draw_resize_handles` is
/// deliberately the vault window's alone.
const WINDOW_SIZE: [f32; 2] = [1000.0, 780.0];

// ---------------------------------------------------------------------------
// 3e's metrics. Colours are `theme` constants throughout -- every value 3e
// uses already has a name there (`#eae7e7` = `HAIRLINE`, `#f3f2f2` = `CANVAS`,
// `#eef2fc` = `BLUE_WASH`, `#14307a` = `BLUE_DEEP`, `#d7d3d3` =
// `BORDER_STRONG`, `#9b9797` = `TEXT_GHOST`, `#7d7979` = `TEXT_FAINT`), so
// nothing here re-declares a colour under a new name.
// ---------------------------------------------------------------------------

/// `grid-template-columns: 208px 1fr`.
const NAV_WIDTH: f32 = 208.0;
/// The nav column's `padding: 14px 10px`.
const NAV_PAD_X: f32 = 10.0;
const NAV_PAD_Y: f32 = 14.0;
/// A nav row's `padding: 8px 10px` around 13px text, and the column's `gap: 2px`.
const NAV_ITEM_HEIGHT: f32 = 33.0;
const NAV_ITEM_PAD_X: f32 = 10.0;
const NAV_ITEM_GAP: f32 = 2.0;
const NAV_ITEM_RADIUS: u8 = 8;
/// The footer block's own `padding: 10px`.
const NAV_FOOTER_PAD: f32 = 10.0;

/// The content pane's `padding: 24px 28px` and `gap: 16px`.
const CONTENT_PAD_X: f32 = 28.0;
const CONTENT_PAD_Y: f32 = 24.0;
const CONTENT_GAP: f32 = 16.0;
/// The heading block's own `gap: 4px`.
const HEADING_GAP: f32 = 4.0;

/// A settings card: `border-radius: 10px`, `1px solid #eae7e7`, white.
const CARD_RADIUS: u8 = 10;
/// A card row's `padding: 13px 16px` and `gap: 20px`.
const ROW_PAD_X: i8 = 16;
const ROW_PAD_Y: i8 = 13;
const ROW_GAP: f32 = 20.0;
/// A row's label/description `gap: 2px`.
const ROW_TEXT_GAP: f32 = 2.0;
/// Width reserved for a row's trailing control. 3e sizes its controls
/// intrinsically and lets the text column flex; a fixed reservation is visually
/// identical (the control is right-aligned inside it, so it still lands on the
/// row's right edge) and it lets the text column be allocated at a known width
/// instead of whatever a flex layout happens to leave. Wide enough for the
/// widest control on this window, the 112pt stepper.
const CONTROL_COLUMN_WIDTH: f32 = 160.0;
/// Floor on a row's height, so a single-line row still fits a 28pt control.
const CONTROL_MIN_HEIGHT: f32 = STEPPER_HEIGHT;

/// 3e's toggle pill is 40x22 (painted by [`theme::toggle_pill`]).
const TOGGLE_SIZE: Vec2 = Vec2::new(40.0, 22.0);

/// The stepper borrows 3e's segmented control exactly -- `border: 1px solid
/// #d7d3d3; border-radius: 7px`, 12px text, cells divided by 1px of the same
/// border -- at the 28px height 3e gives its "+ Add app" button. 3e has no
/// numeric input anywhere, so this is the one control on this window the design
/// does not contain; it is assembled from 3e's own parts rather than invented.
const STEPPER_HEIGHT: f32 = 28.0;
const STEPPER_STEP_WIDTH: f32 = 28.0;
const STEPPER_VALUE_WIDTH: f32 = 56.0;
const STEPPER_RADIUS: u8 = 7;
/// Stable across frames because a `TextEdit`'s focus and cursor live in egui's
/// memory under its id, and an id derived from layout position would lose them
/// the moment anything above the row changed height.
const STEPPER_FIELD_ID: &str = "prefs-auto-lock-minutes";
/// The clipboard interval's own stable id, for the same reason.
const INTERVAL_FIELD_ID: &str = "prefs-clipboard-interval";
/// The Reset button, wide enough for its word at 12px semibold with the
/// breathing room 3e gives its own "+ Add app".
const RESET_BUTTON_WIDTH: f32 = 72.0;

// ---------------------------------------------------------------------------
// Copy
// ---------------------------------------------------------------------------

/// The `bw serve` lifetime row's label.
///
/// **Names the CLI, because that is what a user will find in Task Manager**,
/// which its own reason for naming `bw` at all has always been. It said
/// "Bitwarden backend" while the row was drawn on both backends and had to
/// cover a case where the thing running was not `bw`. It is only ever drawn to
/// somebody who has the CLI now, so it can say so.
const BACKEND_LABEL: &str = "Keep the Bitwarden CLI running";

/// The label for [`crate::settings::Settings::keep_ui_loaded`].
const UI_LOADED_LABEL: &str = "Open the vault instantly";

/// The description under [`UI_LOADED_LABEL`].
///
/// **Names the cost as well as what it buys**, as the backend row above
/// does: this is the same trade in a different process, and somebody who
/// turns it on should not meet the memory afterwards in Task Manager.
///
/// It also names what still closes the window, because "instantly" would
/// otherwise read as a promise that a lock cannot keep -- locking,
/// switching account and editing these settings all end the process, so
/// that what they have to tell the app actually gets told.
const UI_LOADED_DESCRIPTION: &str =
    "Keeps the vault window loaded and hidden after you close it, so it opens \
     immediately next time. Holds about 76 MB while the vault is unlocked. \
     Locking, switching account or changing these settings closes it fully.";

/// The description under [`BACKEND_LABEL`].
///
/// **No longer a function of a `bool`, and that is the change.** It had a
/// second arm that read "there is nothing here to decide" -- a row whose only
/// content was that it did not apply. [`draw_backend_card`] now omits the row
/// entirely on the backend that has no subprocess, so there is no second state
/// left to describe; see
/// `docs/superpowers/specs/2026-08-30-preferences-per-backend-design.md` for
/// the hide-versus-ghost rule and why this row falls on the hide side of it.
///
/// # Why this row is a child of the one above it, and why it moved pages
///
/// It was a plain `toggle_row` on General, two pages away from the switch
/// that decides whether the thing it governs exists at all. That was not
/// merely untidy: [`crate::backend_policy::should_run`] answers `false` for
/// `DirectRest` **whatever this setting says**, so with the direct backend
/// selected this row was a live-looking switch that did nothing. A setting
/// that silently means nothing is worse than two settings that are hard to
/// tell apart, and the owner hit exactly that.
///
/// So the two are now one card, parent and child, on the page whose subtitle
/// is already "The Bitwarden account this vault comes from". Ghosted, the row
/// says what would make it available -- the same `child_toggle_row` contract
/// the Hello-gated disk cache uses -- and hands back the stored value
/// unchanged, so a click cannot set a preference about a subprocess this
/// configuration has no use for.
///
/// # It reads the CHOSEN backend, not the running one
///
/// The gate is [`crate::backend_policy::choose`] over the account's server
/// and the *live* value of the toggle above, which is what will apply on the
/// next launch -- not what this process happens to be running. Reading the
/// running backend would ghost the row a restart later than the switch that
/// caused it, so the user would turn `bw` back on and find this row still
/// grey with nothing on the page explaining the delay.
const BACKEND_DESCRIPTION: &str =
    "Faster, and uses about 118 MB while idle. Off runs it only while the vault window is \
     open; autofill is unaffected either way.";

/// The backend choice's label: the QUESTION, with the two answers beside it.
///
/// **It stopped being a switch, and this label is where that shows.** It read
/// "Use the official Bitwarden CLI" over an on/off pill, which named one side
/// and left the other as the unnamed absence of it -- a user who wanted the
/// built-in client had to work out that "off" meant a second product existed.
/// The owner's instruction was to make both sides visible as peers, so the row
/// now asks which client, and [`OFFICIAL_CHOICE`] and [`BUILT_IN_CHOICE`]
/// answer it in a two-cell [`choice_button`] picker.
///
/// So this string deliberately names NEITHER client: naming one here would put
/// it back in the privileged position the pill gave it. The naming happens in
/// the two cells, which are the same size as each other on screen and in the
/// sentence.
///
/// It is the sign-in window's own question, in the same words
/// (`bw_acquire::CliSetupState::Choosing`'s "Which client should open this
/// vault?"), because a user who meets the choice in both places is meeting one
/// decision and not two.
const BACKEND_CHOICE_LABEL: &str = "Which client opens this vault";

/// The picker's first cell: the official CLI.
///
/// **Names `bw` the way the user will meet it**, which is
/// [`BACKEND_LABEL`]'s rule and the reason both are pinned by
/// `the_two_backend_labels_name_the_cli_and_not_this_codebases_words`.
/// "bw" is a filename and "crypto" is a word about internals; the official
/// Bitwarden CLI is a thing a person can look up and find in Task Manager.
const OFFICIAL_CHOICE: &str = "The official Bitwarden CLI";

/// The picker's second cell: this app's own client.
///
/// **Named as a product rather than as "not the CLI"**, which is the whole of
/// what the two-cell control buys over the pill it replaced. It is
/// Deskwarden's, and saying so is what makes the two cells peers rather than a
/// thing and its absence.
const BUILT_IN_CHOICE: &str = "Deskwarden's built-in client";

/// The description shown under [`BACKEND_CHOICE_LABEL`], in its two states.
///
/// A pure function of one fact, for [`disk_cache_description`]'s reason
/// exactly: this is the text a user reads before deciding where their master
/// key lives, so it is asserted by tests rather than buried in an eframe
/// closure where nothing can reach it.
///
/// # It is two short paragraphs now, and it was four long ones
///
/// The owner's instruction: "four paragraphs under a settings row is too
/// much". The four were carrying the whole argument -- what each side buys,
/// what each side costs, what the switch does to `userkey.bin` -- under a row
/// in a settings list, which is not where an argument belongs. The sign-in
/// window puts the same fork to the user with room to make it
/// (`bw_acquire::CliSetupState::Choosing`), and the confirmation this row
/// raises carries the key hygiene ([`backend_switch_prompt`]). What is left
/// here is what a person needs to pick a cell: one sentence per client, and
/// one sentence saying when the pick applies.
///
/// Three properties the tests below still hold, each deliberate:
///
///  * **each side is named and costed.** The memory figures are the README's
///    measured table -- ~118 MB for the `bw serve` subprocess, ~19 MB total
///    for the built-in client -- and not a fourth number invented here. The
///    row said "about 110 MB" for years and nothing measured agrees with it;
///  * **the cost of the built-in client is stated without a euphemism**: the
///    passwords are stored in the app. Not "credentials are cached locally";
///    the key that opens the vault is kept on this PC and it does not expire,
///    which is the sentence [`crate::user_key_store`]'s own module doc opens
///    with and the one a user has to weigh. It is the one paragraph of the
///    four that could not be shortened away;
///  * **when the row is unavailable it says why in the row**, readable without
///    hovering -- the same rule the disk-cache row follows, and for the same
///    reason: a ghosted control with no explanation reads as a bug.
///
/// # "The next time you sign in", not "the next time Deskwarden starts"
///
/// This sentence was inherited and had gone stale. The choice is applied by
/// `main`'s `settle_the_vault_backend`, which used to run only at startup --
/// so "restart" was the whole truth. Since 0.15.4 a sign-in re-settles the
/// live process for the account it is about
/// (`login_ui::direct_login_for_this_sign_in` ->
/// `backend_policy::resettle_for`), and the sign-in window offers this
/// account's stored answer as the preselected cell
/// (`accounts::official_cli_after_sign_in`, which reads exactly the field this
/// row writes). A restart is one way to reach a sign-in and is no longer the
/// only one, so the row names the moment rather than one route to it.
fn official_crypto_description(self_hosted: bool) -> &'static str {
    if self_hosted {
        "The official Bitwarden CLI is the `bw` program: it holds your keys in a background \
         process of its own, and costs about 118 MB of RAM. Deskwarden's built-in client \
         talks to your server itself for about 19 MB in total — but then the key that \
         unlocks your vault is stored in the app, kept on this PC and protected by Windows, \
         and unlike a session it never expires, so anyone who can run programs as you on \
         this PC can use it.\n\n\
         Changing this takes effect the next time you sign in to this account, and the \
         sign-in window asks the same question there with this answer already offered."
    } else {
        // Not a silent no-op under the same label, and not a hidden row. The
        // owner's rule is "disabled if not self-hosted vault to avoid issues
        // with Bitwarden": the alternative to `bw` is this app speaking the
        // Bitwarden server protocol itself, which is a thing to do to your own
        // server and not to somebody else's service. A hidden row would leave
        // a self-hoster who has not finished setting up wondering where the
        // setting went; a ghosted one with this sentence under it says
        // exactly what would make it available.
        "Only available on a self-hosted server. On bitwarden.com and bitwarden.eu — and until \
         Deskwarden knows which server this account is on — the vault always goes through the \
         official Bitwarden CLI."
    }
}

/// Whether the signed-in account is on a positively self-hosted server, as
/// this page can tell.
///
/// **Reached through [`crate::backend_policy::is_self_hosted`], never
/// re-decided here.** That function is the rule the *startup* path spends, and
/// a second host test on this page is how the row comes to be enabled for an
/// account the app will not actually switch -- or, worse, ghosted for one it
/// will. `backend_policy`'s own doc makes the same point about
/// `favicon::bitwarden_cloud` having one implementation and three callers;
/// this is the fourth caller and not a fourth copy.
///
/// Everything that is not a signed-in account with a readable self-hosted URL
/// is `false`, including the moments before the status lands. Unknown counts
/// as official, which is the safe direction: the row is ghosted for an instant
/// on a self-hosted launch, rather than clickable for an account it could not
/// serve.
fn account_is_self_hosted(status: Option<AccountStatus>) -> bool {
    match status {
        Some(AccountStatus::SignedIn { server, .. }) => {
            crate::backend_policy::is_self_hosted(server.as_deref())
        }
        _ => false,
    }
}

/// Whether this page shows the rows that are only about the `bw serve`
/// subprocess.
///
/// **Reached through [`crate::backend_policy::choose`], never re-decided
/// here.** That is [`account_is_self_hosted`]'s rule one function above, and
/// it is load-bearing in the same way: a page with its own idea of "which
/// backend" is a page whose switch and whose rows can disagree by one edit.
///
/// # It reads the CHOSEN backend, not the running one
///
/// `use_official_bw_crypto` is captured once, by `main`'s `BackendSettlement`,
/// so the click does not take effect until the next launch. This still follows
/// the *live* value, which is what the backend row's gate did before it and
/// for the same reason: a row that disappeared a restart after the switch that
/// removed it would leave the user looking at a page that disagrees with the
/// click they just made.
fn cli_rows_are_shown(server: Option<&str>, use_official_bw_crypto: bool) -> bool {
    matches!(
        crate::backend_policy::choose(server, use_official_bw_crypto),
        crate::backend_policy::VaultBackendChoice::BwServe
    )
}

/// The server this page's account is on, or `None` for everything that is not
/// a signed-in account with one.
///
/// `None` is bitwarden.com **by definition** and not "not known yet" --
/// [`crate::backend_policy::is_self_hosted`] says so in as many words -- so
/// the `SignedOut` and nothing-published arms landing here is the safe
/// direction and not an oversight: unknown counts as official, and the `bw`
/// rows are the ones a user already had.
///
/// One reader for the two call sites in [`draw_backend_card`], so a mid-frame
/// status arrival cannot give the switch and the row below it different
/// answers.
fn account_server(status: &Option<AccountStatus>) -> Option<&str> {
    match status {
        Some(AccountStatus::SignedIn { server, .. }) => server.as_deref(),
        _ => None,
    }
}

/// The encrypted disk cache's label. It names the file rather than the
/// benefit, because the benefit ("opens instantly") is not the part a user
/// has to weigh.
const DISK_CACHE_LABEL: &str = "Keep an encrypted copy of your vault on this PC";

/// The read-path row's label, and the child of the one above it.
///
/// Phrased as what the user gets rather than as the mechanism: "read from
/// the copy first" is a thing to want, where "cache-first read path" is a
/// thing to look up.
const READ_THROUGH_LABEL: &str = "Read from that copy first";

/// The description under [`READ_THROUGH_LABEL`], in its two states.
///
/// # Why this row is a child
///
/// With no copy permitted there is nothing to read, so
/// `backend_policy::read_path` answers `ServiceOnly` whatever this says.
/// A live-looking switch that decides nothing is the defect this page
/// already had once, with `keep_backend_running` sitting two pages from the
/// setting that made it meaningless -- see `backend_description`. Ghosted,
/// it says what would make it available and hands back the stored value
/// unchanged.
fn read_through_description(cache_on: bool) -> &'static str {
    if cache_on {
        "On, Deskwarden answers from the encrypted copy before asking your server -- which is \
         what makes filling work when the connection to Bitwarden is stopped. Off keeps the \
         copy on disk for a fast start but always asks the server for what it fills."
    } else {
        "Only applies when there is a copy to read -- the switch above."
    }
}

/// The description shown under the disk-cache toggle, in its two states.
///
/// **The wording is the requirement here, not an implementation detail**, and
/// it is a pure function for exactly that reason: this is the text a user
/// reads before accepting a security tradeoff, so it is asserted by tests
/// rather than buried in an eframe closure where nothing can reach it.
///
/// Four properties the tests below hold, each deliberate:
///
///  * it names what is in the file -- "usernames, passwords, notes and
///    two-factor secrets" -- instead of the euphemism "vault data";
///  * it states the survives-a-lock behaviour, in the negative, because that
///    is the part a reasonable person assumes goes the other way;
///  * it names the residual attacker in plain terms rather than implying the
///    file is safe from everything;
///  * it never uses the word "secure". It describes what gates the file.
fn disk_cache_description() -> &'static str {
    {
        // **This paragraph used to promise a TPM, and the promise was the
        // reason the setting could not be used.**
        //
        // The copy was sealed with a key only Windows Hello could release,
        // so every launch asked -- and when the prompt did not appear,
        // startup waited for it forever and the app did not start at all.
        // The key is now stored beside the file and protected by Windows
        // the way `user_key_store`'s is; see `vault_disk_cache`'s own doc.
        //
        // **Saying so is not optional.** A file protected by DPAPI, offered
        // under copy that names a TPM, is exactly the misleading security
        // claim the old comment here refused to make -- and it would be
        // this app making it. The sentence changed because the file did.
        //
        // It still obeys this module's rules for such copy: it names what
        // is in the file, what gates it, and who can read it anyway,
        // without ever using the word "secure".
        "Deskwarden opens instantly after a restart and autofill works the moment it \
         starts, instead of waiting for the vault to be fetched again.\n\n\
         The copy contains your usernames, passwords, notes and two-factor secrets. It \
         is encrypted with a key kept on this PC and protected by Windows — the same \
         protection as the key that unlocks your vault. Anyone who can run programs as \
         you on this PC can read it, and so can someone who takes this disk and knows \
         your Windows password. It is not deleted when your vault locks — only when you \
         log out, or after 7 days."
    }
}

/// **"on match" is gone from the label, because the setting is no longer
/// about matches.** `Settings::prompt_on_match` governs every card the
/// overlay raises by itself now -- the fill prompt for a window the vault
/// knows, and the "no saved login for this app" card for one it does not. A
/// label that named only matches was a label a user had to disbelieve: they
/// turned it off, and the card for unmatched apps went on appearing.
const PROMPT_LABEL: &str = "Show autofill prompts";
/// **Says what OFF does, because off is the state that changes what the app
/// does on its own.** The user's own framing: "only shortcuts will work in
/// that case". Naming the hotkey here is what stops the toggle reading as
/// "switch autofill off" -- it never is; the hotkey arms for every match in
/// both states (`app::match_arms_hotkey`).
///
/// It now says *any* window rather than a matched one, for the reason on
/// `PROMPT_LABEL` directly above.
const PROMPT_DESCRIPTION: &str = "Offer to fill, or to save, when a window that wants a password \
     comes to the front. Off means nothing opens on its own and CTRL+ALT+B is the only way to \
     fill. Nothing is ever typed until you ask for it either way.";

const BREACH_LABEL: &str = "Check passwords against known breaches";
/// **Says what leaves the machine, because something does.** Off by default is
/// stated in the copy and not only in `Settings::default`: this is the one row
/// on General whose ON state makes a network request keyed on a password, and
/// a user reading the pane should not have to infer that from the pill.
/// The k-anonymity bargain -- five hex characters out, thirty-five matched
/// here -- is what makes the request safe to offer at all, so it is the
/// description rather than a footnote.
const BREACH_DESCRIPTION: &str = "Off by default. When on, Deskwarden sends the first 5 \
     characters of a SHA-1 hash of a password to Have I Been Pwned and matches the rest on this \
     machine. Your password, and the rest of its hash, never leave your PC.";

/// The scan card's own heading, in `UPDATE_SECTION_LABEL`'s idiom.
const SCAN_SECTION_LABEL: &str = "Scan the whole vault";
const SCAN_BUTTON: &str = "Scan all passwords now";
const SCAN_RUNNING_BUTTON: &str = "Scanning...";
/// **Says what a scan costs before it is started**, in requests and in what
/// leaves the machine, because the button beside it is the one control in
/// this app that makes hundreds of outbound calls from a single click.
const SCAN_IDLE_DESCRIPTION: &str = "Checks every saved password against Have I Been Pwned, one \
     request per distinct password -- so a vault full of reused passwords is far fewer requests \
     than it has items. Only the first 5 characters of each hash leave your PC.";
/// The empty state of the history list. A result, not a blank panel: see
/// `password_health`'s summary for the same argument.
const SCAN_NO_HISTORY: &str = "No scan has been run yet.";
const SCAN_HISTORY_LABEL: &str = "Previous scans";
/// What the page says when the vault has nothing to check. Its own wording,
/// not "0 found": those are different results.
const SCAN_NOTHING_DESCRIPTION: &str = "There are no saved passwords in this vault to check. \
     Cards, notes, SSH keys and logins with an empty password are not checked.";
/// The state no shipped build reaches -- rendered honestly rather than as a
/// button that does nothing.
const SCAN_UNAVAILABLE_DESCRIPTION: &str = "This build cannot scan: nothing set the scan up when \
     Deskwarden started.";

const FETCH_ICONS_LABEL: &str = "Show site icons";
/// **Says what the request discloses, and says it is the DOMAIN.** This is the
/// row for the request `PRIVACY.md` calls the one with the most privacy weight
/// in the app, and the whole reason a user would turn it off is what the
/// service on the other end gets to see. Copy that said only "downloads icons"
/// would be describing the feature and hiding the cost.
///
/// Three things are named because each is a thing a user would otherwise have
/// to guess: WHAT is sent (the domain), to WHOM (their own server's icon
/// service when they self-host), and what is NOT sent -- the credential. The
/// last is not padding: "sends the website to Bitwarden" is exactly what a
/// worried reader assumes, and it is wrong.
///
/// On by default, and stated in the copy rather than left to
/// `Settings::default`, the same way `BREACH_DESCRIPTION` states its own
/// opposite default.
const FETCH_ICONS_DESCRIPTION: &str = "On by default. Deskwarden asks the icon service for an \
     item's site icon by domain name — your own server's if you self-host. It never sends the \
     username, the password, or which account the item is in. Off shows coloured initials \
     instead and nothing leaves your PC.";

const DIRECT_ICONS_LABEL: &str = "Fetch site icons from the sites themselves";
/// **Says what ON costs, in the sentence a user would otherwise have to work
/// out for themselves.** This is the only switch in the app that makes it
/// connect to hosts it has no other relationship with, and the whole reason
/// somebody would leave it off is what those hosts get to see. Copy that said
/// "fetches icons directly" would be describing the mechanism and hiding the
/// consequence, which for this row is the entire content.
///
/// So the cost is stated as a consequence rather than as a mechanism -- each
/// site learns that you hold an entry for it, and roughly when you looked --
/// because "connects to each site" is a true sentence a reader can nod at
/// without understanding what it discloses.
///
/// **It also names the exception, because the exception is not governed by
/// this pill and a user reading only the label would assume it was.**
/// Addresses on the user's own network are fetched directly whether this is
/// on or off, and the reason is stated (the icon service cannot reach them)
/// rather than asserted, so the behaviour reads as a fact about routing
/// instead of as this switch leaking. `Settings::fetch_icons_direct` carries
/// the long form of the same argument and `PRIVACY.md` carries the longest.
///
/// Off by default, and stated in the copy rather than left to
/// `Settings::default`, the same way `BREACH_DESCRIPTION` and
/// `FETCH_ICONS_DESCRIPTION` state theirs.
const DIRECT_ICONS_DESCRIPTION: &str = "Off by default, and it decides where PUBLIC sites' icons \
     come from. An address on your own network — 192.168.x.x, 10.x.x.x, localhost — is always \
     fetched from that address itself, because an icon service out on the internet has no route \
     to your network and never will. On, Deskwarden fetches every icon that way and the icon \
     service is not asked for any of them. What that costs: each of those sites receives a \
     request from your PC, so it learns that an entry for it exists in your vault and roughly \
     when you looked at it.";

const BRAND_LOGOS_LABEL: &str = "Show card network logos";
/// **Says where the images come from, because that is the part a user cannot
/// guess and the only part they have to act on.** This row is unlike every
/// other pill on this page: turning it on does nothing whatever until the user
/// has put a file somewhere, so copy that said only "shows network logos"
/// would be describing a switch that looks broken.
///
/// So it names the folder, and it names the fallback in the same breath -- a
/// brand with no image keeps its printed name -- which is what makes a
/// half-filled folder a reasonable state to be in rather than a half-broken
/// vault.
///
/// It also says **nothing is downloaded**, because "logos" on a page whose
/// neighbouring row is about a request to an icon service is exactly where a
/// careful reader assumes a second one.
///
/// Off by default, and stated in the copy rather than left to
/// `Settings::default`, the same way `FETCH_ICONS_DESCRIPTION` and
/// `BREACH_DESCRIPTION` state theirs.
const BRAND_LOGOS_DESCRIPTION: &str = "Off by default. Draws a card's network mark as that \
     network's own logo, read from PNG files you put in the brand-marks folder beside your \
     settings — visa.png, mastercard.png, amex.png and so on. Nothing is downloaded. Any brand \
     with no image keeps its printed name.";

const TOTP_SECRET_LABEL: &str = "Show TOTP secrets on the details screen";
/// **Says what ON adds, and what it costs.** Off is the default and is stated
/// in the copy rather than left to `Settings::default`, exactly as
/// `BREACH_DESCRIPTION` states its own: these are the two rows on General
/// whose ON state gives something away, and a user reading the pane should
/// not have to infer either from the pill.
///
/// The word "masked" is in the copy because the row this turns on is masked
/// until its eye is clicked -- turning this on does not put a seed on screen,
/// it puts a row there. And the reason to leave it off is named rather than
/// implied: the six-digit code expires, the seed it comes from does not.
///
/// **It names the row the way the details screen labels it, which is "TOTP".**
/// This sentence said "under its one-time code" long after that row's own
/// label became `TOTP` (`vault_window::detail`'s `totp_row`), so the copy sent
/// the user looking for a row with that name and there is not one. A
/// description that names a label the app does not paint is worse than a
/// vague one: the user concludes the setting did not work.
const TOTP_SECRET_DESCRIPTION: &str = "Off by default. When on, an item's TOTP secret appears \
     on the details screen as an extra masked row under its TOTP code, revealed by clicking \
     the eye. The code expires in 30 seconds; the secret behind it never does.";

/// The automatic-check row's label.
///
/// **It said "Check for updates" until this row moved onto
/// [`Section::Updates`]**, where [`UPDATE_CHECK_BUTTON`] says exactly that in
/// the card directly below. Two identical strings a card apart, one a switch
/// and one a button, is the worst possible reading of a distinction the whole
/// page is built to make -- and it was invisible while they were two pages
/// apart, which is how it survived this long.
///
/// So the switch names the half it actually governs, and the button keeps the
/// words for the thing a user presses. Nothing about either behaviour
/// changed; the row simply stopped calling itself by the button's name. The
/// copy underneath it ([`UPDATE_CHECK_DESCRIPTION`]) was already written this
/// way -- "Deskwarden asks GitHub" -- so this is the label catching up to its
/// own description.
const UPDATE_CHECK_LABEL: &str = "Check for updates automatically";
/// **Says what the request discloses and, unusually, argues for leaving it
/// on.** Every other privacy row here describes a cost; this one has to
/// describe a cost that is small and a consequence of switching it off that
/// is not, because the symptom of a missed update is nothing happening and
/// the user will not attribute it to this pill.
///
/// "Nothing about you or your vault" is the accurate claim and is the same
/// one `PRIVACY.md` makes: the request names this app's own public
/// repository, not the user.
const UPDATE_CHECK_DESCRIPTION: &str = "On by default. Deskwarden asks GitHub whether a newer \
     Deskwarden has been released. The request says nothing about you or your vault. Off means \
     you will not be told about fixes, including security ones, until you look yourself.";


// --- Clipboard ------------------------------------------------------------

const CLIPBOARD_MASTER_LABEL: &str = "Take copied secrets back off the clipboard";
/// **The master switch's copy has to say what OFF means, because off is the
/// state that withdraws a protection.** Turning this off is a real reduction
/// in what the app does, freely chosen -- exactly like
/// `AUTO_LOCK_ENABLED_DESCRIPTION` -- so the sentence says so plainly rather
/// than describing the feature and leaving the cost to be inferred.
///
/// It also has to say what does **not** change, because that is the thing a
/// reader will otherwise assume this pill governs: the formats that keep a
/// copied password out of `Win+V` and off the user's other devices are
/// unconditional and are not on this page as a control. See
/// `CLIPBOARD_HISTORY_NOTE`, the row that says so in its own words.
const CLIPBOARD_MASTER_DESCRIPTION: &str = "On by default. Off means a secret you copy stays on \
     the clipboard until something else replaces it — no timer, and none of the three below. \
     Keeping copies out of clipboard history is separate and stays on either way.";

const CLIPBOARD_ON_LOCK_LABEL: &str = "Clear when the vault locks";
/// Names all four ways the vault locks, because a user who locks by idling
/// should not have to guess whether the Lock button is the only one meant.
/// The fourth -- the session being invalidated elsewhere -- is described
/// rather than named, since "needs_reauth" is not a word on any screen.
const CLIPBOARD_ON_LOCK_DESCRIPTION: &str = "Locking by hand, from the tray, after idling, or \
     because the session expired. Deskwarden has no separate sign-out: this switch covers all \
     of them.";

const CLIPBOARD_ON_ACCOUNT_LABEL: &str = "Clear when the account changes";
const CLIPBOARD_ON_ACCOUNT_DESCRIPTION: &str = "Switching to another account, adding one, or \
     removing one. A credential from the vault you have just left does not follow you to the \
     next one.";

const CLIPBOARD_ON_QUIT_LABEL: &str = "Clear when Deskwarden quits";
/// **Says what it cannot cover**, because a switch called "when Deskwarden
/// quits" reads as a promise about every way the process can end, and three of
/// those ways no in-process arrangement can catch. `PRIVACY.md` makes the same
/// admission; a page that made the stronger claim would be the one disagreeing
/// with it.
const CLIPBOARD_ON_QUIT_DESCRIPTION: &str = "Quitting from the tray, and shutting down to install \
     an update. A crash, a Task Manager kill or a power cut cannot be caught, and leave the copy \
     where it is.";

const CLIPBOARD_INTERVAL_LABEL: &str = "Clear after";
/// **States the unit, the range and the resolution**, because all three are
/// things a user would otherwise discover by being refused. The floor is
/// stated on screen and not only enforced, exactly as `AUTO_LOCK_DESCRIPTION`
/// states its own.
const CLIPBOARD_INTERVAL_DESCRIPTION: &str = "Minutes before a copied secret is taken back. \
     Decimals are fine — 0.5 is thirty seconds, which is the shortest Deskwarden will use, and \
     60 minutes the longest. One decimal place.";

/// The row that exists to say a thing is **not** a setting.
///
/// It is a plain text row with no control, like About's account line, and that
/// is the point: a disabled toggle would read as a feature that is present and
/// broken, and leaving it off the page entirely would let "everything on this
/// page is switchable" be read as a promise about the whole module. The
/// argument for it being unconditional is in `clipboard.rs`'s own header and
/// in `PRIVACY.md`; this is the one-sentence version, on the page where
/// somebody would go looking for the switch.
const CLIPBOARD_HISTORY_LABEL: &str = "Clipboard history and sync are always excluded";
const CLIPBOARD_HISTORY_NOTE: &str = "A secret you copy is kept out of Windows clipboard history \
     (Win+V) and is never synced to your other devices. This has no setting and is not affected \
     by anything above — there is no version of it worth turning off.";

const CLIPBOARD_RESET_LABEL: &str = "Reset to default";
/// Says the scope, because "reset to default" on a page inside a preferences
/// window is otherwise ambiguous between this page and the app. It also says
/// what the defaults *are*, so the button's effect is legible before it is
/// pressed rather than only after.
const CLIPBOARD_RESET_DESCRIPTION: &str = "Puts the five settings on this page back to how they \
     ship — everything on, one minute. Nothing on any other page is touched, and there is no \
     confirmation because setting them again is the same five clicks.";
const CLIPBOARD_RESET_BUTTON: &str = "Reset";

/// What the interval field says when it refuses an entry.
///
/// One sentence per refusal, rather than one shared "invalid": the whole
/// reason `settings::ClipboardEntry` has four variants is that `soon`, `0.1`,
/// `90` and `1.25` are wrong in four different ways and a user who typed one
/// of them needs to be told which.
///
/// The refused entry is **not** applied, so the value in effect is still the
/// one shown a moment ago -- which is why every one of these says what the
/// field wants rather than only what it got.
const CLIPBOARD_ENTRY_NOT_A_NUMBER: &str = "Type a number of minutes, like 1 or 0.5.";
const CLIPBOARD_ENTRY_BELOW_FLOOR: &str =
    "0.5 minutes (thirty seconds) is the shortest — below that the clipboard expires before a \
     slow sign-in page is ready.";
const CLIPBOARD_ENTRY_ABOVE_CEILING: &str =
    "60 minutes is the longest. To stop clearing altogether, use the switch at the top.";
const CLIPBOARD_ENTRY_BETWEEN_STEPS: &str = "One decimal place — 1.5, not 1.25.";

/// **One switch, two ways of leaving.** This governs the idle timeout below
/// *and* `deskwarden::away_lock` -- locking Windows (Win+L), switching user,
/// and the machine going to sleep. There is deliberately no second preference
/// for those: a user who turns this off has said not to lock the vault behind
/// their back, and Win+L is behind their back in the most literal sense
/// available. The label says "step away" rather than "idle" because the
/// timeout is now the weaker of the two things it turns on.
const AUTO_LOCK_ENABLED_LABEL: &str = "Lock the vault when you step away";
const AUTO_LOCK_ENABLED_DESCRIPTION: &str =
    "Locks after the idle time below, and immediately when you lock Windows, switch user, or the \
     machine sleeps. Off means the vault stays unlocked until you lock it yourself or quit \
     Deskwarden.";

const AUTO_LOCK_LABEL: &str = "Lock the vault after";
const AUTO_LOCK_DESCRIPTION: &str = "Minutes of no activity before the vault window locks itself. \
     One minute is the shortest Deskwarden will use.";

/// The one global shortcut this app registers, in the form the user sees it.
///
/// Hardcoded rather than derived, because `hotkey::register_fill_hotkey`
/// builds it from `global_hotkey`'s `Modifiers`/`Code` types, which have no
/// display form worth showing a user. `the_shortcuts_page_names_the_hotkey_
/// that_is_actually_registered` is a source-text guard over `hotkey.rs` so the
/// two cannot drift apart silently.
const FILL_HOTKEY: &str = "CTRL+ALT+B";
const FILL_HOTKEY_LABEL: &str = "Fill the focused app";
const FILL_HOTKEY_DESCRIPTION: &str =
    "The only shortcut Deskwarden registers. It cannot be changed yet.";
/// The label the row takes when the chord could not be registered.
///
/// It says *not working* in the label rather than only in the description,
/// because the label is the line a user scans: a row still headed "Fill the
/// focused app" over a greyed chip is a row that can be read as working. The
/// reason -- and what to do about it -- comes from
/// `hotkey::Unavailable::message`, which is authored next to the decision that
/// produced it rather than here.
const FILL_HOTKEY_UNAVAILABLE_LABEL: &str = "Fill the focused app — shortcut not working";

/// What the About page says about the account when nobody has published one.
///
/// **Kept, and it is the floor rather than the answer.** The page now shows
/// the signed-in address when the app knows it (see [`AccountStatus`]), and
/// this is what the row says when no shell has published anything at all --
/// `examples/ui_preview`, a test, and any future entry point that draws this
/// page without going through `main`. The row is never blank in any of them,
/// which is the property the original constant existed for: a label with an
/// empty right-hand column reads as a field that failed to load.
const ACCOUNT_STATUS: &str = "Open the vault window to see the signed-in account.";

/// The row's label, and the two things it can say about a known account.
///
/// **There used to be a third, "Checking...", and there is no longer a moment
/// it could be shown.** It covered the 2.8 seconds a startup `bw status`
/// spawn took to answer, during which the row had to say something other than
/// "Not signed in" -- the two being opposite claims rather than shades of one.
/// That spawn is gone: the address is read off the `Account`, or off the
/// sign-in that established it, and both answer on the frame they are asked.
const ACCOUNT_LABEL: &str = "Bitwarden account";
const ACCOUNT_SIGNED_OUT: &str = "Not signed in";
/// **One sentence for two situations, because this build cannot tell them
/// apart.** `login_ui::unknown_status_details` returns the same value for a
/// CLI that could not be spawned, one that answered nothing usable, and one
/// that answered honestly that nobody is signed in. Saying only "not signed
/// in" would be a claim this page cannot support; saying both is the honest
/// width of what is known.
///
/// It named the Bitwarden CLI, which is only one of the two things that can
/// fail to answer: on the built-in client the status comes from `rest::api`
/// and no CLI is running at all.
const ACCOUNT_SIGNED_OUT_NOTE: &str =
    "No account is signed in, or Deskwarden could not reach the vault to ask.";
/// A signed-in account whose address was not reported. Not blank, and
/// not silently "signed in" either -- the row promises to say WHICH account.
const ACCOUNT_NO_EMAIL: &str = "Signed in";
const ACCOUNT_NO_EMAIL_NOTE: &str = "The address for this account was not reported.";
/// Under a known address: which server this vault lives on.
const ACCOUNT_SERVER_PREFIX: &str = "Signed in at ";

/// What the About page knows about the signed-in account.
///
/// # Why the email is on this page at all
///
/// It is a personal identifier arriving on a screen that had none, so the
/// call is made deliberately rather than by default. It is the user's own
/// address, on their own machine, in a window they opened -- and it is the
/// one fact that answers "which account is this vault", which the row has
/// promised to answer since it was written. It goes nowhere: it is painted,
/// never logged (`vault_window` logs only whether an email was *present*),
/// never copied, and never sent. The alternative -- a row that says an
/// account is linked without saying which -- is the state the user reported
/// as "empty but we know it for sure".
///
/// # Two variants, and there is deliberately no third for "not yet"
///
/// There was one, `Checking`, and it is deleted. It existed for a single
/// publisher: `main` spawned a `bw status` at startup to learn the account's
/// address, that spawn took 2.8 seconds on the machine this row was reported
/// from, and a Preferences window opened during it had to say something other
/// than "Not signed in". The spawn is gone; the address is read off the
/// `Account`, or off the sign-in that established it, and both answer on the
/// frame they are asked. `None` -- nothing published yet -- is the only
/// before-state left, and it has its own honest sentence in
/// [`ACCOUNT_STATUS`]. Re-adding an in-flight variant would need a real
/// in-flight moment to go with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountStatus {
    /// A signed-in account. `email` is `None` when the address was not
    /// reported; `server` is `None` for Bitwarden's own cloud.
    SignedIn { email: Option<String>, server: Option<String> },
    /// Nobody is signed in -- or the vault could not be asked. See
    /// [`ACCOUNT_SIGNED_OUT_NOTE`].
    SignedOut,
}

// --- Updates: the update flow ---------------------------------------------
//
// **This is where the tray's update item went.** That item was created as
// `MenuItem::new("Update available", false, None)` -- the words present from
// startup, the *enabling* the only thing the check ever did -- so a session
// with no update (which is nearly every session) showed a permanent claim that
// one existed, on a control that then refused to be clicked. It is deleted.
//
// What replaced it is here, on the page a user comes to for exactly this
// question, and it can say all four things the tray could say none of: not
// checked yet, checked and current, checked and here is what is new, and here
// is how far the download has got. The mechanism is `update_panel`, which owns
// its own thread and channel because `run` below blocks and `main.rs`'s loop is
// not running while this page is on screen.

/// The flow card's label -- the row the description and the button sit on.
///
/// **It said "Updates" while this card lived on About**, where that was the
/// only word on the page naming the subject. On [`Section::Updates`] it is
/// also the page heading and also the nav row, so the same string was about
/// to be painted three times in one view and `ink_of` would no longer have
/// been able to say which one a test meant.
///
/// A noun phrase rather than [`SCAN_SECTION_LABEL`]'s imperative, and that is
/// the one place these two pages deliberately differ: "Scan the whole vault"
/// stays true while a scan is running, whereas an imperative here would be
/// telling the user to check for a release they are already downloading. The
/// stage is the *description's* job on both pages; this label only has to
/// name what the card is about, at every stage.
const UPDATE_SECTION_LABEL: &str = "New releases";
const UPDATE_CHECK_BUTTON: &str = "Check for updates";
const UPDATE_CHECKING_BUTTON: &str = "Checking...";
const UPDATE_DOWNLOAD_BUTTON: &str = "Download";
const UPDATE_DOWNLOADING_BUTTON: &str = "Downloading...";
const UPDATE_RESTART_BUTTON: &str = "Restart to install";
const UPDATE_RETRY_BUTTON: &str = "Try again";

const UPDATE_IDLE_DESCRIPTION: &str =
    "Ask GitHub whether a newer Deskwarden has been released. Nothing is downloaded until you \
     ask for it.";
const UPDATE_CHECKING_DESCRIPTION: &str = "Asking GitHub for the latest release.";
const UPDATE_UP_TO_DATE_DESCRIPTION: &str = "This is the latest release.";
const UPDATE_READY_DESCRIPTION: &str =
    "Downloaded and signature-checked. Deskwarden will close, install, and start again.";
const UPDATE_UNAVAILABLE_DESCRIPTION: &str =
    "This build cannot check for updates. Please report it -- it is a defect, not a setting.";

/// Shown under the button when `Settings::check_for_updates` is off.
///
/// **The button still works, and this row is why that is honest.** The setting
/// governs the check Deskwarden makes *by itself*; a click here is the user
/// making the request. Saying so on the page is the difference between a
/// button that appears to ignore a preference and one that is explicit about
/// which preference it is not governed by. `PRIVACY.md` carries the same
/// claim.
///
/// **It used to say "on the General page", and no longer has to.** The switch
/// is now the card directly above this row (see [`draw_updates`]), so the
/// sentence can point at it instead of sending the reader somewhere to check
/// -- which is the whole reason the two were put on one page. Naming a page
/// that no longer holds the switch would have been worse than vague; naming
/// no page at all, with the switch in the same glance, is better than either.
const UPDATE_AUTOMATIC_OFF_NOTE: &str =
    "Automatic checks are off — the switch above. This button still asks, because you asked it \
     to.";

const UPDATE_NOTES_LABEL: &str = "What is new";
/// Shown in place of the notes when the release has none. A release with an
/// empty body is normal; an empty box is not distinguishable from a box that
/// failed to load.
const UPDATE_NOTES_EMPTY: &str = "This release came with no notes.";

/// The floor under the notes region's height.
///
/// **This used to be the region's fixed height, and the reasoning that made
/// it fixed still holds -- it is now enforced by a different quantity.** A
/// GitHub release body is written by whoever cut the release and can be any
/// length; a region that grew *to fit its content* would push the buttons
/// below the bottom edge of a window that is not resizable, and this crate
/// has shipped a layout that put a control out of reach before. What
/// [`notes_max_height`] does instead is grow the region to the space the page
/// still HAS -- never to what the notes want -- so the ceiling is the page's
/// own remaining height, and nothing below the region can be displaced
/// because below it there is nothing left. Overflow still scrolls; it simply
/// scrolls later.
///
/// The floor bites only on a page whose remaining space is smaller than this,
/// which the shipping window sizes do not produce. Kept at the old fixed
/// height so that case is exactly today's behaviour rather than a new one,
/// and so a degenerate rect cannot collapse the region to nothing.
///
/// The character bound in `updater::release_notes_for_display` is unchanged
/// and still the second half of the guarantee, covering layout cost rather
/// than reach.
const UPDATE_NOTES_MIN_HEIGHT: f32 = 128.0;

/// Vertical distance between two lines of the notes, and between two
/// paragraphs of them.
///
/// The first is a line gap rather than the card's `ROW_TEXT_GAP`, because a
/// release body's lines are prose: a row's worth of air between every bullet
/// reads as a list of paragraphs rather than as a list. The second is what a
/// blank source line is painted as, and the reason it is a real number is
/// that the gap between "Added" and "Fixed" is information the author put
/// there.
const UPDATE_NOTES_LINE_GAP: f32 = 2.0;
const UPDATE_NOTES_PARAGRAPH_GAP: f32 = 6.0;

/// How far one level of bullet nesting insets a line, and the glyph that
/// replaces the source's `-`.
///
/// The nesting depth this is multiplied by is bounded in
/// `updater::MAX_BULLET_DEPTH`, because the leading spaces that produce it
/// are chosen by whoever wrote the release.
const UPDATE_NOTES_BULLET_STEP: f32 = 12.0;
const UPDATE_NOTES_BULLET_GLYPH: &str = "•  ";

/// The notes scrollbar's width and the gap either side of it. Named, because
/// the region's content width is this much less than the row's and the two
/// numbers have to be the same ones -- a mismatch shows as this card being a
/// few points wider than the Version card directly above it.
const UPDATE_NOTES_BAR_WIDTH: f32 = 6.0;
const UPDATE_NOTES_BAR_MARGIN: f32 = 4.0;

/// The progress bar's height and corner, matched to the stepper's radius so
/// the page keeps one vocabulary of shapes.
const UPDATE_BAR_HEIGHT: f32 = 8.0;
const UPDATE_BAR_RADIUS: u8 = 4;

/// Wide enough for the longest label above without truncating it, since a
/// button whose text is cut is a button whose action is a guess.
const UPDATE_BUTTON_WIDTH: f32 = 150.0;
/// The scan button's box. Wider than [`UPDATE_BUTTON_WIDTH`] because its
/// label is longer, and drawn on a row of its own rather than in the trailing
/// control column -- see [`draw_breaches`], where the alternative (widening
/// `CONTROL_COLUMN_WIDTH` for every row on every page) is spelled out.
const SCAN_BUTTON_WIDTH: f32 = 184.0;

// ---------------------------------------------------------------------------
// Sections
// ---------------------------------------------------------------------------

/// The pages the nav lists, in its order.
///
/// **Clipboard is the one page 3e does not contain.** 3e lists seven sections
/// and none of them is about the clipboard, because 3e was drawn before this
/// app took a copied secret back off it. It is a page of its own rather than a
/// group on an existing one, because it carries five controls -- more than any
/// neighbour-group on General, and enough to bury them under the rows already
/// there.
///
/// # Five of 3e's sections are gone, and they were the empty ones
///
/// Autofill, Native apps, Security, Shortcuts and Sync & account are not here
/// any more. All five had the same defect: **nothing on them could be
/// changed.** Three were a single sentence saying so out loud; Sync & account
/// was two sentences pointing at other places; Shortcuts was one read-only
/// chip. A nav row is a promise that there is a decision behind it, and five
/// rows out of twelve were spending the reader's attention to answer "no,
/// not here" -- which is worse than not being listed, because the reader had
/// to visit each one to learn it.
///
/// What those pages knew that nowhere else did was **the fill hotkey**, and
/// it moved rather than went: see [`fill_hotkey_row`], which General draws.
/// Everything else on them restated a fact that is either already on the page
/// the reader would look at next, or is not a preference at all (signing in
/// and syncing are done from the tray and the vault window, and always were).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    General,
    /// **Everything about checking passwords against known breaches**: the
    /// consent pill, the scan button, and what previous scans counted.
    ///
    /// Its own page rather than three more rows on General, and the toggle
    /// moved here rather than being left behind. A pill on one page and the
    /// button it does *not* govern on another is precisely the arrangement
    /// that makes a user distrust both -- see
    /// [`crate::breach_scan::SCAN_CONSENT_NOTE`], which has to be readable in
    /// the same glance as the switch it is talking about.
    ///
    /// Directly after General, because that is where this setting used to be
    /// and where a reader will look for it.
    Breaches,
    /// **Everything about where this vault lives and what is kept of it on
    /// this PC.**
    ///
    /// [`Section::Breaches`]'s argument, taken to its end on the one subject
    /// where the pages had actually scattered. This page carries all of it:
    /// which backend holds the vault (the official `bw` CLI or Deskwarden's
    /// built-in client), whether that backend is kept warm, whether an
    /// encrypted copy is kept on this PC and whether reads consult it.
    ///
    /// **What it does not carry is the local API**, which is
    /// [`Section::Api`]. Those four settings answer "where does Deskwarden
    /// get this vault from"; the API answers "who else may ask Deskwarden for
    /// it". They met here only because both had once been called "the vault
    /// service", and a page that answers two questions is a page on which
    /// neither answer is findable -- the key list, which is the longest thing
    /// in this window and has no upper bound, sat under four rows of
    /// three-paragraph copy that nobody reading about keys wanted.
    ///
    /// # Why they were gathered, and it is not tidiness
    ///
    /// The owner walked into a self-trapping state: with
    /// `keep_backend_running` off the vault window failed to load, and
    /// Preferences is a page *inside* that window -- so the only switch that
    /// could undo it was behind the thing it had broken. That was reachable
    /// because the settings that decide how the vault is served were on three
    /// different pages, each placement individually defensible. Seen
    /// together they are one decision with several parts, and a reader who
    /// can see all of them at once can see what they have just done to
    /// themselves.
    ///
    /// **Directly after Breaches**, which is as near the top as this page can
    /// get without displacing the setting a reader arrives for. It used to
    /// follow Security, and it follows Breaches now for the same reason it
    /// followed Security then: it is the first page a reader looking for
    /// "where does my vault come from" reaches.
    Vault,
    /// **The local HTTP API, and every key that opens it**: the switch that
    /// starts the endpoint on 127.0.0.1, the form that mints a key, the one
    /// showing of a key it has just made, the list of the keys that exist,
    /// and revoking one.
    ///
    /// # Why it is its own page and not the bottom of [`Section::Vault`]
    ///
    /// The half that must never be split is the switch and the keys, and it
    /// is not split: **a switch on one page and the list of who can walk
    /// through the door it opens on another is an arrangement where the
    /// owner can believe they turned something off while three keys still
    /// exist for it.** That argument is about these two, and both are here.
    ///
    /// It says nothing about the backend choice or the disk cache, which
    /// answer a different question -- see [`Section::Vault`]. Those four are
    /// settings, each a row that is read once and left alone. This page is a
    /// *workspace*: it has a form, an unbounded list, and two questions the
    /// user answers in place. Sharing a scroll region with four rows of
    /// three-paragraph copy meant the mint button moved down the page as
    /// keys were added, which is the one control here a user comes back for.
    ///
    /// **Directly after Vault**, because the endpoint serves whatever the
    /// Vault page decided is being served, and because a reader who has just
    /// read what is kept on this PC is exactly the reader who should next be
    /// asked who may reach it.
    Api,
    Clipboard,
    /// **Everything about updating this build**: the automatic-check pill, the
    /// manual check, what the release says, the download, and the restart.
    ///
    /// The same argument as [`Section::Breaches`], reached from the other
    /// side. There, the pill was on General and there was no button at all;
    /// here the pill was on General and the whole flow was on About -- the
    /// same split, with both halves already built. The switch governs what
    /// Deskwarden asks **on its own**; the button is the user asking. Those
    /// two sentences only read as honest when the switch and the control it
    /// does not govern are in one glance -- see [`UPDATE_AUTOMATIC_OFF_NOTE`],
    /// which used to have to name another page and now points at the row
    /// directly above it.
    ///
    /// **Directly before About, not after General.** Breaches went to the top
    /// because its setting lived on General and that is where its user's hand
    /// goes; nearly all of this page came off About, so that is where *this*
    /// page's user's hand goes -- one row up in the nav rather than seven.
    /// The rule is the same in both cases: the new page lands where the bulk
    /// of it came from. It also keeps the tail of the nav reading outward
    /// from the vault -- what is in it, then this build, then what this build
    /// is -- and leaves About last, which is where a page that decides
    /// nothing belongs. (It used to read "the account this vault comes from,
    /// then this build": Sync & account sat between Clipboard and here, and
    /// was removed for having no setting on it.)
    Updates,
    About,
}

impl Section {
    /// The nav, top to bottom.
    pub const ALL: [Section; 7] = [
        Section::General,
        Section::Breaches,
        Section::Vault,
        Section::Api,
        Section::Clipboard,
        Section::Updates,
        Section::About,
    ];

    /// The nav row's text, which is also the content pane's heading (3e uses
    /// the same word for both).
    pub fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Breaches => "Breaches",
            Section::Vault => "Vault",
            Section::Api => "Local API",
            Section::Clipboard => "Clipboard",
            Section::Updates => "Updates",
            Section::About => "About",
        }
    }

    /// The line under the heading.
    ///
    /// 3e only draws the Autofill page, so its subtitle was the one written
    /// by the design and every other was written to its shape. That page is
    /// gone -- it had no setting on it -- so what is left here is all
    /// shape-and-no-original, which changes nothing about how they are
    /// written: one sentence, naming what the page decides.
    /// `pub(crate)` so that tests elsewhere can CONTROL on the real text
    /// rather than on a copy of it. `send_ui`'s preferences-modal control
    /// held its own transcription of the General subtitle and went stale the
    /// day this one gained "the shortcut it answers" -- a red test about a
    /// sentence, not about the modal it was guarding.
    pub(crate) fn subtitle(self) -> &'static str {
        match self {
            // **Says "the shortcut it answers" because the fill hotkey row is
            // on this page now.** A subtitle that named only background
            // behaviour and locking would be a subtitle a user scanning for
            // CTRL+ALT+B reads straight past, on the one page that tells them
            // what it is and whether it is working.
            Section::General => {
                "How Deskwarden runs in the background, the shortcut it answers, and when it \
                 locks itself."
            }
            Section::Breaches => {
                "Whether your saved passwords are checked against public breach lists, and \
                 what the last checks found."
            }
            // Every part, in the order the page draws them: where the vault
            // is served FROM, then what is kept on this PC, then who else may
            // ask for it. The endpoint still comes before the keys, for the
            // reason it always did -- the keys mean nothing while nothing is
            // listening, and a reader who has not grasped that there IS an
            // endpoint cannot weigh them.
            // **It said "which backend holds this vault", and "backend" is
            // this codebase's word for `bw serve`.** The subtitle is painted
            // on both products, so it was one more sentence naming machinery
            // half its readers do not have -- the defect the per-backend split
            // was for, one line above the rows it was drawn over.
            // `the_built_in_vault_page_names_no_subprocess` caught it.
            Section::Vault => {
                "Where this vault comes from, and what is kept of it on this PC."
            }
            // Names the door before the keys, for the reason the page draws
            // them in that order: the keys mean nothing while nothing is
            // listening, and a reader who has not grasped that there IS an
            // endpoint cannot weigh them. Says *other programs on this PC*
            // rather than "clients", because who may walk through is the
            // decision this page asks for.
            Section::Api => {
                "The local endpoint that hands this vault to other programs on this PC, and \
                 the keys that open it."
            }
            // Says *taken back*, not "cleared", and names the copy rather than
            // the clipboard: the page is about the second half of
            // `clipboard.rs` and not about the history exclusion, which has no
            // switch and appears on this page only as a line saying so.
            Section::Clipboard => "When a secret you have copied is taken back off the clipboard.",
            // Both halves, in the order the page draws them: the switch that
            // decides what Deskwarden does unasked, then the button that does
            // not consult it.
            Section::Updates => {
                "Whether Deskwarden looks for new releases by itself, and how to install one now."
            }
            // **Unchanged, and now literally true.** It was already the
            // sentence for an identity page while the page underneath it
            // carried a check button, a download and a restart. Those went to
            // `Section::Updates`; this is what is left, and it is what this
            // line always said it would be.
            Section::About => "Which build of Deskwarden this is.",
        }
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Everything the window edits, plus the one piece of transient UI state
/// (the stepper's text buffer) that has to survive between frames.
pub struct PrefsState {
    pub settings: Settings,
    section: Section,
    /// What is currently *typed* into the minutes field, which is not the same
    /// as `settings.auto_lock_minutes`: mid-edit it may be empty, or "4" on the
    /// way to "45". It is reconciled back to the committed value the moment the
    /// field loses focus (see [`minutes_stepper`]).
    auto_lock_text: String,
    /// The same thing for the clipboard interval, and for the same reason:
    /// mid-edit the field may be empty, or `"0."` on the way to `"0.5"`.
    ///
    /// Separate from [`Self::auto_lock_text`] rather than shared, because the
    /// two fields are on different pages and both keep their own committed
    /// value; one buffer would mean typing into one and finding the other had
    /// moved.
    clipboard_interval_text: String,
    /// Why the last entry into the interval field was refused, or `None` if
    /// nothing has been refused since.
    ///
    /// **Kept in state rather than recomputed each frame** because it is about
    /// an *event* -- the moment the field lost focus with something
    /// unacceptable in it -- and not about the current text. Recomputing it
    /// would put the message on screen while the user was still typing `0.` on
    /// the way to `0.5`, which is scolding them for not having finished.
    clipboard_entry_error: Option<&'static str>,
    /// The Updates page's update flow: its stage, and the receiver its worker
    /// threads report on.
    ///
    /// **It lives on the state rather than in the draw**, because it is the
    /// one thing on this page that must survive a frame. A check takes seconds
    /// and a download takes minutes; a panel rebuilt each frame would drop its
    /// receiver every 16ms and never see an answer. That is also why the
    /// screenshot example holds its `PrefsState` for the update surfaces where
    /// it rebuilds it for the clipboard ones.
    update: crate::update_panel::UpdatePanel,
    /// Where the About page reads the account from, **as a function pointer
    /// read every frame** rather than a value captured when the window
    /// opened.
    ///
    /// Both halves matter. Every frame, because the answer can CHANGE while
    /// this window is open -- an account switch, a sign-out, a sign-in in the
    /// vault window all republish it -- so a value snapshotted in `new` would
    /// leave the row naming the account the user just left, for the whole
    /// life of the window and with no way to correct it. (It used to be worse
    /// still: the first answer arrived 2.8 seconds after the window, from a
    /// `bw status` spawn that no longer exists.) A seam, because the default
    /// reads a process-wide published value, and a test that drove that
    /// global would leave the next test's page describing an account nobody
    /// set: the tests and `examples/ui_preview` install their own `fn` and
    /// touch no shared state at all.
    account_source: fn() -> Option<AccountStatus>,
    /// The Breaches page's scan flow: its stage, and the receiver its worker
    /// threads report on.
    ///
    /// On the state rather than in the draw for [`Self::update`]'s reason
    /// exactly: a scan takes seconds to minutes, and a panel rebuilt each
    /// frame would drop its receiver every 16ms and never see an answer.
    ///
    /// A `Default` panel is `Idle` and wired to nothing, so a window opened
    /// and closed without touching this page has started no scan and holds no
    /// thread -- which is the whole "nothing runs on its own" rule, in the
    /// only place it could have been broken by accident.
    scan: crate::breach_scan::ScanPanel,
    /// What `scan_history.json` held when this window opened, refreshed
    /// whenever a scan finishes.
    ///
    /// Read once rather than every frame: it is a file, the page draws at 60
    /// frames a second, and the only thing that changes it is a scan
    /// finishing -- which this window is the one to notice.
    /// Whether Windows Hello is set up on this machine, **as a function
    /// pointer** rather than a value, for [`Self::account_source`]'s reason:
    /// the default reads the OS, and no test in this crate may.
    ///
    /// [`PrefsState::new`] -- what the paint tests build -- defaults it to
    /// "unavailable", which is the inert answer: the disk-cache row renders
    /// ghosted and nothing can be turned on. The production shells install
    /// the real probe in [`Self::with_scan_history`], exactly as they are the
    /// only things that read `scan_history.json`.
    hello_available: fn() -> bool,
    scan_history: crate::scan_history::ScanHistory,
    /// The API keys the service will honour, as the store held them when this
    /// window opened.
    ///
    /// **Empty in [`PrefsState::new`] and NOT read off disk there**, exactly
    /// as [`Self::scan_history`] is empty there and for the same reason: `new`
    /// is what the paint tests build, and a constructor that reached
    /// `%APPDATA%\Deskwarden` would make every one of them a reader of the
    /// owner's real credentials. [`Self::with_scan_history`] loads it.
    keys: Vec<KeyRecord>,
    /// Where a minted or revoked key is written.
    ///
    /// **A `fn` on the state, defaulting in `new` to one that writes nothing
    /// and succeeds** -- the inert answer, in [`Self::hello_available`]'s
    /// idiom. A draw that called [`save_service_keys`] directly would make
    /// every paint test in this module a *writer* of the owner's key store,
    /// which is a worse failure than the reading one that seam already
    /// prevents. The production shells install the real writer in
    /// [`Self::with_scan_history`].
    keys_sink: fn(&[KeyRecord]) -> Result<(), String>,
    /// What is typed into the mint form, between frames.
    key_form: KeyForm,
    /// The one key that has been minted and not yet dismissed, holding the
    /// only plaintext copy of it that exists anywhere.
    minted: Option<MintedKey>,
    /// What the page last told the owner about a mint or a revoke: a refusal,
    /// or a store that could not be written. `None` is "nothing to say".
    ///
    /// Kept in state rather than recomputed each frame for
    /// [`Self::clipboard_entry_error`]'s reason: it is about an *event* --
    /// the moment a button was pressed -- and not about what is currently in
    /// the form.
    key_message: Option<String>,
    /// The backend flip that has been clicked once and not yet confirmed.
    /// `None` means nothing is being asked.
    ///
    /// **The proposed direction, not a copy of the setting.** What the pill
    /// paints while this is `Some` is still
    /// `Settings::use_official_bw_crypto`, unchanged -- so a question that is
    /// dismissed, or abandoned by leaving the page, cannot leave the row
    /// showing a state the app is not in.
    pending_backend_switch: Option<BackendSwitch>,
    /// The name of the key whose revoke button has been pressed once and not
    /// yet confirmed. `None` means nothing is being asked.
    pending_revoke: Option<String>,
    /// The clock the expiry list and `mint` are answered against, **as a
    /// function pointer** for [`Self::account_source`]'s reason: expiry is a
    /// question about *now*, and a test that had to wait for a real second to
    /// pass to see a key expire is a test nobody writes.
    key_clock: fn() -> u64,
    /// Where a new key's 256 bits come from. A seam for
    /// `service_token::mint`'s own reason: the encoding, the length and the
    /// hashing are all worth driving from a test, and none of them needs an
    /// unpredictable value to be driven.
    key_random: fn() -> [u8; 32],
    /// Where the Copy button sends the revealed key.
    ///
    /// **A no-op in `new`**, like [`Self::keys_sink`]: the Windows clipboard
    /// is process-wide state, and a paint test that clicked Copy would put a
    /// live credential on the clipboard of whoever is running the suite. The
    /// production shells install `clipboard::copy_secret`, which is this
    /// crate's one clipboard path and the only one that clears itself.
    key_copy: fn(&str),
}

/// The wall clock, in milliseconds since the Unix epoch, UTC.
///
/// The one place this window reads it. A scan record's timestamp is "when it
/// finished", so it genuinely has to come from the clock -- and every
/// *display* of it goes through [`crate::local_time`] with the offset
/// resolved for that instant, so nothing here depends on where or when the
/// machine is.
fn now_unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The recorded scans, or an empty history where there is no resolvable
/// config directory or the file could not be read.
///
/// Empty is a state the page renders out loud ("No scan has been run yet"),
/// so there is nothing here to report as an error.
fn load_scan_history() -> crate::scan_history::ScanHistory {
    crate::scan_history::default_path()
        .map(|path| crate::scan_history::ScanHistory::load(&path))
        .unwrap_or_default()
}

/// The account the shells publish and the About page reads.
///
/// # Why a published value rather than a parameter
///
/// The same two-shells problem `update_panel::install_env` solves, and the
/// same answer for the same reason: `prefs_ui::run` is entered from `main`
/// and `PrefsState::new` from inside `vault_window::build_frame`'s closure,
/// so a parameter would have to be threaded through `run`, `PrefsState::new`
/// and that closure -- a signature change two call sites away from the row
/// that uses it, in two shells that must not disagree.
///
/// # Why an `RwLock` and not a `OnceLock`
///
/// This is where it deliberately differs from `install_env`, which was
/// flagged as the decision most worth overruling. An update environment is
/// fixed at startup; an ACCOUNT is not. This app switches accounts
/// (`accounts.rs`) and signs out. A `OnceLock` would pin whichever answer
/// landed first -- the account the app started on -- and the row would then
/// be wrong for the rest of the session with no way to correct it. So the
/// value is republished whenever it changes, and the last publisher wins.
static PUBLISHED_ACCOUNT: std::sync::RwLock<Option<AccountStatus>> =
    std::sync::RwLock::new(None);

/// Publishes what the app now knows about the signed-in account.
///
/// Called by the shells at the moments the answer changes: when `main`
/// settles the account it launched on, and when a vault window receives
/// details of its own. Cheap and idempotent -- publishing the same value
/// again is a write of a value that was already there.
///
/// A poisoned lock is ignored rather than propagated: this is a decorative
/// row on an About page, and panicking a caller mid-startup over it would be
/// a much worse failure than a row that is one publish out of date.
pub fn publish_account_status(status: AccountStatus) {
    if let Ok(mut slot) = PUBLISHED_ACCOUNT.write() {
        *slot = Some(status);
    }
}

/// What was last published, or `None` where nothing ever was -- the
/// screenshot example, and any test.
pub fn published_account_status() -> Option<AccountStatus> {
    PUBLISHED_ACCOUNT.read().ok().and_then(|slot| slot.clone())
}

/// A `bw status` answer as this page understands it.
///
/// **One mapping, here, for both publishers.** `main`'s startup drain and
/// `vault_window`'s late arrival both hold a `BwStatusDetails` and both feed
/// this row; two conversions would be two chances for the same CLI answer to
/// reach the page as two different sentences.
pub fn account_status_of(details: &crate::login_ui::BwStatusDetails) -> AccountStatus {
    match details.status {
        // `Unauthenticated` is also what a CLI that could not be spawned or
        // could not be parsed comes back as -- see
        // `login_ui::unknown_status_details` -- which is why the row's
        // wording for this case covers both.
        crate::login_ui::BwStatus::Unauthenticated => AccountStatus::SignedOut,
        _ => AccountStatus::SignedIn {
            email: details.user_email.clone(),
            server: details.server_url.clone(),
        },
    }
}

impl PrefsState {
    /// Clamps the loaded value up front, deliberately.
    ///
    /// `settings.json` can contain `auto_lock_minutes: 0` (it was, before the
    /// `auto_lock_enabled` toggle existed, the only hand-written way to say
    /// "never lock"), and `settings::auto_lock_policy` still uses 1 minute for
    /// it -- deliberately, see `MIN_AUTO_LOCK_MINUTES`'s doc: "never" is now
    /// the toggle's job and a legacy `0` is not retro-fitted to mean it.
    /// Showing the stored `0` in the field would
    /// make this window display a number that is not the number in effect --
    /// so the window opens on the value that *is* in effect. The cost is that
    /// opening Preferences on such a file makes `edited != settings` true in
    /// `main.rs` and writes the corrected value back, which is the right
    /// outcome: the file then says what the app is doing.
    /// The clipboard interval is clamped up front for the same reason and with
    /// the same cost: a hand-edited `clear_clipboard_seconds: 14400` would
    /// otherwise be displayed as four hours while the app cleared after one,
    /// and this window would be showing a number that is not the number in
    /// effect. Opening Preferences on such a file writes the corrected value
    /// back, which is the right outcome -- the file then says what the app is
    /// doing.
    pub fn new(settings: Settings) -> Self {
        let minutes = clamp_auto_lock_minutes(settings.auto_lock_minutes);
        let interval = ClearInterval::from_seconds(settings.clear_clipboard_seconds);
        Self {
            settings: Settings {
                auto_lock_minutes: minutes,
                clear_clipboard_seconds: interval.seconds(),
                ..settings
            },
            section: Section::General,
            auto_lock_text: minutes.to_string(),
            clipboard_interval_text: interval.as_minutes_text(),
            clipboard_entry_error: None,
            update: crate::update_panel::UpdatePanel::default(),
            account_source: published_account_status,
            scan: crate::breach_scan::ScanPanel::default(),
            // **Empty, and NOT read off disk here.** See
            // [`Self::with_scan_history`]: `new` is called by roughly forty
            // tests, and a constructor that reached `%APPDATA%\Deskwarden`
            // would make every one of them a test that reads -- and, one
            // careless edit later, writes -- the user's real file. The two
            // production shells load it explicitly; anything that does not
            // gets an empty history, which is a state the page renders in
            // words.
            scan_history: crate::scan_history::ScanHistory::default(),
            hello_available: || false,
            // The key store, and the two ways out of this window that could
            // touch something real, all inert here. See each field's doc:
            // `new` is what the paint tests build.
            keys: Vec::new(),
            keys_sink: |_| Ok(()),
            key_form: KeyForm::default(),
            minted: None,
            key_message: None,
            pending_backend_switch: None,
            pending_revoke: None,
            key_clock: crate::service_keys::now_unix,
            key_random: crate::service_token::os_random,
            key_copy: |_| {},
        }
    }

    /// Which page is open. Used by the screenshot job (`examples/ui_preview`),
    /// which has to be able to open a page nobody has clicked on.
    pub fn show(&mut self, section: Section) {
        self.section = section;
    }

    /// Parks the update flow in a given stage, for the same reason
    /// [`show`](Self::show) exists: `examples/ui_preview` has to draw states
    /// nobody can click their way to in a screenshot run -- a found release, a
    /// download in flight, a failure -- and must reach none of them by
    /// touching the network.
    ///
    /// The panel it installs is wired to nothing (see
    /// `update_panel::UpdatePanel::parked`), so a preview cannot start work
    /// even by accident.
    pub fn show_update_stage(&mut self, stage: crate::update_panel::UpdateStage) {
        self.update = crate::update_panel::UpdatePanel::parked(stage);
    }

    /// The same state, with the recorded scans loaded off disk.
    ///
    /// **The only thing in this module that reads `scan_history.json`**, and
    /// it is a separate constructor rather than part of [`Self::new`] for one
    /// reason: `new` is what the paint tests build, and no test in this crate
    /// may touch `%APPDATA%\Deskwarden`. A constructor that read the file
    /// would make every one of them a reader of the user's real history.
    ///
    /// Both production shells -- [`run`] and the vault window's modal -- call
    /// this. `examples/ui_preview` deliberately does not; it supplies its own
    /// entries through [`Self::show_scan_history`].
    pub fn with_scan_history(settings: Settings) -> Self {
        Self {
            scan_history: load_scan_history(),
            hello_available: crate::vault_disk_cache::hello_available,
            // The API keys are loaded here for the same reason and under the
            // same rule: this is the constructor that is allowed to read
            // `%APPDATA%\Deskwarden`, and it is the one the two production
            // shells call. The writer and the clipboard path are installed
            // here too, so a state built any other way can reach neither.
            keys: load_service_keys(),
            keys_sink: save_service_keys,
            key_copy: crate::clipboard::copy_secret,
            ..Self::new(settings)
        }
    }

    /// Parks the scan flow in a given stage, for
    /// [`show_update_stage`](Self::show_update_stage)'s reason exactly:
    /// `examples/ui_preview` has to draw a scan in flight and a scan that
    /// mostly failed, and must reach neither by touching the network.
    ///
    /// The panel it installs is wired to nothing
    /// ([`crate::breach_scan::ScanPanel::parked`]), and the flow refuses to
    /// begin any work without a process-wide `ScanEnv` that only `main.rs`
    /// installs -- so a preview cannot start a scan even by accident.
    pub fn show_scan_stage(&mut self, stage: crate::breach_scan::ScanStage) {
        self.scan = crate::breach_scan::ScanPanel::parked(stage);
    }

    /// Supplies the scan history the Breaches page lists, instead of the one
    /// that was on disk when this state was built.
    ///
    /// For the screenshot example, which **must not read**
    /// `%APPDATA%\Deskwarden`, and for the paint tests, which must not
    /// either.
    pub fn show_scan_history(&mut self, history: crate::scan_history::ScanHistory) {
        self.scan_history = history;
    }

    /// Points the About page's account row at `source` instead of at the
    /// process-wide published value.
    ///
    /// For `examples/ui_preview` and for tests, and a function pointer rather
    /// than a value for the reason [`PrefsState::account_source`] gives: the
    /// row is read every frame, and nothing here may write the global that
    /// the rest of the process -- and the next test -- reads.
    pub fn show_account_source(&mut self, source: fn() -> Option<AccountStatus>) {
        self.account_source = source;
    }

    /// Answers the disk-cache row's "is Windows Hello set up?" from `probe`
    /// instead of from the OS.
    ///
    /// For `examples/ui_preview`, which has to draw both states of a row
    /// whose real answer depends on the machine taking the screenshot, and
    /// for the tests. A function pointer for
    /// [`show_account_source`](Self::show_account_source)'s reason exactly.
    pub fn show_hello_available(&mut self, probe: fn() -> bool) {
        self.hello_available = probe;
    }

    /// Supplies the API keys the Local API page lists, instead of the
    /// ones that were on disk when this state was built.
    ///
    /// For `examples/ui_preview`, which **must not read**
    /// `%APPDATA%\Deskwarden`, and for the tests, which must not either --
    /// [`show_scan_history`](Self::show_scan_history)'s argument, over the
    /// file beside it.
    pub fn show_service_keys(&mut self, keys: Vec<KeyRecord>) {
        self.keys = keys;
    }

    /// Answers this page's "what time is it?" from `clock` instead of from
    /// the machine.
    ///
    /// Expiry is the one thing on this window that changes with no input, so
    /// a screenshot of an expired key -- and a test of one -- can only exist
    /// if the clock is a parameter.
    pub fn show_key_clock(&mut self, clock: fn() -> u64) {
        self.key_clock = clock;
    }

    /// Mints from `random` instead of from the OS.
    ///
    /// For the tests, which need to know what key came out in order to check
    /// that its plaintext is on screen and that its hash -- and not it -- is
    /// what the record kept.
    pub fn show_key_random(&mut self, random: fn() -> [u8; 32]) {
        self.key_random = random;
    }

    /// Sends a minted key somewhere other than the store.
    ///
    /// For the tests, which must not write `%APPDATA%\Deskwarden`, and which
    /// need to drive the "the key was made and the file could not be
    /// written" branch -- a state the owner has to be told about truthfully,
    /// and one that cannot be reached by breaking a real disk.
    pub fn show_keys_sink(&mut self, sink: fn(&[KeyRecord]) -> Result<(), String>) {
        self.keys_sink = sink;
    }

    /// Sends the Copy button's key somewhere other than the clipboard.
    pub fn show_key_copy(&mut self, copy: fn(&str)) {
        self.key_copy = copy;
    }
}

// ---------------------------------------------------------------------------
// The numeric control (pure parts first)
// ---------------------------------------------------------------------------

/// What a typed entry commits to: the number if it is one, otherwise the value
/// that was already there.
///
/// Every path runs through [`clamp_auto_lock_minutes`], so the committed value
/// is by construction one `Settings::auto_lock_timeout` will use unaltered --
/// this control cannot put a number on screen that the clamp then overrides.
/// A non-number (empty, mid-edit, "soon", or a value too large for `u64`)
/// leaves the previous value alone rather than resetting to a default: the
/// user pressing Escape's worth of nonsense should not silently change their
/// lock timeout.
fn parse_minutes_entry(text: &str, previous: u64) -> u64 {
    match text.trim().parse::<u64>() {
        Ok(minutes) => clamp_auto_lock_minutes(minutes),
        Err(_) => clamp_auto_lock_minutes(previous),
    }
}

/// One step down, never below the floor.
fn decrement_minutes(value: u64) -> u64 {
    clamp_auto_lock_minutes(value.saturating_sub(1))
}

/// One step up. `saturating_add` for the same reason `auto_lock_timeout`
/// saturates: `u64::MAX` is reachable from a hand-edited file, and `+ 1` on it
/// panics in a debug build.
fn increment_minutes(value: u64) -> u64 {
    clamp_auto_lock_minutes(value.saturating_add(1))
}

/// `[-] [ 15 ] [+]` in 3e's segmented-control box. Returns the value after this
/// frame; `buffer` is the caller's persistent text state.
///
/// `enabled == false` is the auto-lock toggle being off, and it is a *disabled*
/// control rather than a hidden one: the number stays on screen (greyed, so it
/// reads as inert) because it is still the number that comes back when the
/// toggle is turned on again, and a control that disappears takes its value's
/// visibility with it. Nothing in here is merely painted differently --
/// neither step button senses a click, and the text field is replaced by a
/// painted galley rather than a read-only `TextEdit`, so there is no widget
/// left to focus, click into, or type at. "Looks disabled" and "is disabled"
/// are the pair this codebase keeps having to reunite.
fn minutes_stepper(ui: &mut Ui, value: u64, buffer: &mut String, enabled: bool) -> u64 {
    let (outer, _) = ui.allocate_exact_size(
        Vec2::new(STEPPER_STEP_WIDTH * 2.0 + STEPPER_VALUE_WIDTH, STEPPER_HEIGHT),
        Sense::hover(),
    );
    // 3e has no disabled variant of its segmented control, so the greyed
    // treatment is built from 3e's own two lighter greys: the card's hairline
    // border in place of the control border, on the canvas grey in place of
    // white. No new colour is introduced for it.
    let (fill, border) = if enabled {
        (theme::CARD, theme::BORDER_STRONG)
    } else {
        (theme::CANVAS, theme::HAIRLINE)
    };
    ui.painter().rect(
        outer,
        CornerRadius::same(STEPPER_RADIUS),
        fill,
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );

    let minus = Rect::from_min_size(outer.min, Vec2::new(STEPPER_STEP_WIDTH, STEPPER_HEIGHT));
    let field = Rect::from_min_size(
        Pos2::new(minus.max.x, outer.min.y),
        Vec2::new(STEPPER_VALUE_WIDTH, STEPPER_HEIGHT),
    );
    let plus = Rect::from_min_size(
        Pos2::new(field.max.x, outer.min.y),
        Vec2::new(STEPPER_STEP_WIDTH, STEPPER_HEIGHT),
    );
    for x in [field.min.x, field.max.x] {
        ui.painter().rect_filled(
            Rect::from_min_max(Pos2::new(x, outer.min.y), Pos2::new(x + 1.0, outer.max.y)),
            CornerRadius::ZERO,
            border,
        );
    }

    // The buttons run *before* the field is drawn, and that ordering is
    // load-bearing rather than incidental: a `TextEdit` paints the string it
    // was handed, so updating `buffer` after drawing it left the field showing
    // the previous number for one whole frame -- press `-` on 16 and the value
    // was 15 while the control still read 16.
    //
    // A step operates on what the field currently *shows*, not on the last
    // committed value, so typing 45 and then pressing `+` gives 46 rather than
    // discarding the 45 and giving 16.
    let shown = parse_minutes_entry(buffer, value);
    let mut next = value;
    // The floor is shown, not merely enforced: at the minimum there is nothing
    // below to step to, and a `-` that accepts the click and refuses the change
    // is the same lie as a switch that does nothing. `enabled &&` in front of
    // both, so an off toggle disables the ends for the same reason.
    if step_button(ui, minus, "-", enabled && shown > decrement_minutes(shown)) {
        next = decrement_minutes(shown);
    }
    if step_button(ui, plus, "+", enabled && shown < increment_minutes(shown)) {
        next = increment_minutes(shown);
    }
    let stepped = next != value;
    if stepped {
        *buffer = next.to_string();
    }

    if !enabled {
        // A painted galley, not a disabled/read-only `TextEdit`: egui's
        // read-only text edit still takes focus, still shows a caret and
        // still accepts a click, which is precisely the "greyed out but
        // secretly live" state this is meant not to be. Nothing here is
        // interactive because there is no widget here at all.
        let galley = ui.painter().layout_no_wrap(
            value.to_string(),
            FontId::new(12.0, FontFamily::Proportional),
            theme::TEXT_GHOST,
        );
        ui.painter().galley(
            Pos2::new(
                field.center().x - galley.size().x / 2.0,
                field.center().y - galley.size().y / 2.0,
            ),
            galley,
            theme::TEXT_GHOST,
        );
        // Kept in step with the committed value while the control is off, so
        // turning the toggle back on hands the live field the number that has
        // been on screen all along rather than a stale mid-edit fragment.
        *buffer = value.to_string();
        return value;
    }

    // Frameless: the box around it is painted above, so `TextEdit`'s own frame
    // would draw a second, differently-rounded rectangle inside it.
    // The number is placed by hand rather than by `horizontal_align(Center)`,
    // and that is a measurement, not a preference. On egui 0.35 a singleline
    // `TextEdit` centres its galley over a region 12pt WIDER than the rect it
    // is handed: given `field.shrink(4.0)` (48pt) it centred over 60pt, which
    // put the number 6pt right of the cell while the greyed branch above --
    // which centres an explicit galley -- sat dead centre. So the live control
    // and the disabled one disagreed by 6pt horizontally and 3.5pt vertically,
    // which is what the bug report was.
    //
    // `desired_width` does NOT move it (measured: 48 and 36 give the same
    // result), so there is no width to tune. What IS exact is `Align::Min`,
    // which lands the text at precisely `rect.min.x`. The origin is therefore
    // computed here from the same `layout_no_wrap` measurement the greyed
    // branch uses, so the two branches now agree BY CONSTRUCTION rather than
    // by coincidence -- and neither depends on the 12pt discrepancy being
    // understood, which it is not.
    //
    // The rect still runs to the cell's right edge so most of the cell stays
    // clickable; only the text's left edge moves with the digit count.
    let text_width = ui
        .painter()
        .layout_no_wrap(
            buffer.clone(),
            FontId::new(12.0, FontFamily::Proportional),
            theme::INK,
        )
        .size()
        .x;
    let inner = field.shrink(4.0);
    let entry = ui.put(
        Rect::from_min_max(
            Pos2::new(field.center().x - text_width / 2.0, inner.min.y),
            inner.max,
        ),
        egui::TextEdit::singleline(buffer)
            .id(egui::Id::new(STEPPER_FIELD_ID))
            .frame(egui::Frame::new())
            .font(FontId::new(12.0, FontFamily::Proportional))
            .horizontal_align(egui::Align::Min)
            .vertical_align(egui::Align::Center)
            .margin(Margin::ZERO),
    );
    if entry.lost_focus() && !stepped {
        next = parse_minutes_entry(buffer, value);
    }

    // Reconciled only when the field is not being typed into -- otherwise every
    // keystroke would be replaced by the committed value and the field could
    // never be edited at all.
    if !entry.has_focus() {
        *buffer = next.to_string();
    }
    next
}

/// One end cell of the stepper. Inert when `enabled` is false: no click sense,
/// no hover cursor, ghosted glyph.
fn step_button(ui: &mut Ui, rect: Rect, glyph: &str, enabled: bool) -> bool {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let response = ui.interact(rect, egui::Id::new(STEPPER_FIELD_ID).with(glyph), sense);
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let color = if enabled { theme::TEXT_SECONDARY } else { theme::TEXT_GHOST };
    // ASCII `-` and `+` rather than U+2212 MINUS SIGN: the bundled Archivo
    // subset is the only face these can render in, and a glyph it lacks would
    // paint as a replacement box.
    let galley = ui.painter().layout_no_wrap(
        glyph.to_owned(),
        FontId::new(14.0, FontFamily::Name(theme::SEMIBOLD.into())),
        color,
    );
    ui.painter().galley(
        Pos2::new(
            rect.center().x - galley.size().x / 2.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
    enabled && response.clicked()
}

// ---------------------------------------------------------------------------
// Shell
// ---------------------------------------------------------------------------

/// Nav column and content pane. Split out of [`run`] so the tests can drive
/// real frames of it without opening a window.
///
/// **Public for the screenshot job** (`examples/ui_preview`), which is a
/// separate crate and has to be able to draw a page of this window without
/// opening one. A settings page nobody has looked at is exactly what that job
/// exists to catch, and six tests in this crate have been found structurally
/// blind to what they appeared to check -- a picture is the honest oracle for
/// layout.
pub fn draw_prefs_body(ui: &mut Ui, state: &mut PrefsState) {
    let full = ui.max_rect();
    let nav = Rect::from_min_max(full.min, Pos2::new(full.min.x + NAV_WIDTH, full.max.y));
    let content = Rect::from_min_max(Pos2::new(nav.max.x, full.min.y), full.max);

    draw_nav(ui, nav, state);

    let inner = content.shrink2(Vec2::new(CONTENT_PAD_X, CONTENT_PAD_Y));
    ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
        ui.spacing_mut().item_spacing = Vec2::new(0.0, CONTENT_GAP);
        draw_section(ui, state);
    });
}

fn draw_nav(ui: &mut Ui, rect: Rect, state: &mut PrefsState) {
    ui.painter().rect_filled(rect, CornerRadius::ZERO, theme::CARD);
    ui.painter().rect_filled(
        Rect::from_min_max(Pos2::new(rect.max.x - 1.0, rect.min.y), rect.max),
        CornerRadius::ZERO,
        theme::HAIRLINE,
    );

    let inner = rect.shrink2(Vec2::new(NAV_PAD_X, NAV_PAD_Y));
    ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
        ui.spacing_mut().item_spacing = Vec2::new(0.0, NAV_ITEM_GAP);
        for section in Section::ALL {
            if nav_item(ui, section.label(), state.section == section) {
                state.section = section;
            }
        }
    });

    // 3e pins the version to the bottom of the nav with a `flex: 1` spacer.
    // There is no second line: 3e's "Bitwarden account linked" is a claim this
    // window cannot make (see `ACCOUNT_STATUS`).
    let galley = ui.painter().layout_no_wrap(
        version_line(),
        FontId::new(11.0, FontFamily::Proportional),
        theme::TEXT_GHOST,
    );
    ui.painter().galley(
        Pos2::new(
            inner.min.x + NAV_FOOTER_PAD,
            inner.max.y - NAV_FOOTER_PAD - galley.size().y,
        ),
        galley,
        theme::TEXT_GHOST,
    );
}

fn nav_item(ui: &mut Ui, label: &str, selected: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), NAV_ITEM_HEIGHT),
        Sense::click(),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if selected {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(NAV_ITEM_RADIUS), theme::BLUE_WASH);
    }
    let (family, color) = if selected {
        (FontFamily::Name(theme::BOLD.into()), theme::BLUE_DEEP)
    } else {
        (FontFamily::Proportional, theme::INK)
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), FontId::new(13.0, family), color);
    ui.painter().galley(
        Pos2::new(
            rect.min.x + NAV_ITEM_PAD_X,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
    response.clicked()
}

/// `Deskwarden <version>`, from the crate's own version rather than 3e's
/// mocked "1.4.0".
fn version_line() -> String {
    format!("Deskwarden {}", env!("CARGO_PKG_VERSION"))
}

fn draw_section(ui: &mut Ui, state: &mut PrefsState) {
    section_heading(ui, state.section);
    match state.section {
        // The one read of the published status -- see `hotkey::availability`,
        // and `fill_hotkey_row` for why it is a parameter from here down. It
        // used to be read for `Section::Shortcuts`; that page was one
        // read-only chip and is gone, and the chip is a row on General now.
        Section::General => draw_general(ui, state, crate::hotkey::availability()),
        Section::Breaches => draw_breaches(ui, state),
        Section::Vault => draw_vault(ui, state),
        Section::Api => draw_api(ui, state),
        Section::Clipboard => draw_clipboard(ui, state),
        Section::Updates => draw_updates(ui, state),
        Section::About => draw_about(ui, state),
    }
}

fn section_heading(ui: &mut Ui, section: Section) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = HEADING_GAP;
        // `letter-spacing: -0.02em` at 24px is -0.48pt; `RichText` has no
        // tracking control, so this goes through `theme::letterspaced`.
        ui.label(theme::letterspaced(
            section.label(),
            24.0,
            theme::EXTRABOLD,
            -0.48,
            theme::INK,
        ));
        ui.label(
            RichText::new(section.subtitle())
                .size(13.0)
                .color(theme::TEXT_FAINT),
        );
    });
}

// ---------------------------------------------------------------------------
// Cards and rows
// ---------------------------------------------------------------------------

/// A white card with 3e's hairline border, sized to whatever `add` drew. The
/// background shape is reserved before the content so it paints underneath.
fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let bg = ui.painter().add(egui::Shape::Noop);
    let width = ui.available_width();
    let inner = ui.scope(|ui| {
        ui.set_width(width);
        // Rows carry their own padding and separators; egui's default item
        // spacing between them would show as a gap in the card.
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        add(ui);
    });
    ui.painter().set(
        bg,
        egui::epaint::RectShape::new(
            inner.response.rect,
            CornerRadius::same(CARD_RADIUS),
            theme::CARD,
            Stroke::new(1.0, theme::HAIRLINE),
            StrokeKind::Inside,
        ),
    );
}

fn card_row(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .inner_margin(Margin {
            left: ROW_PAD_X,
            right: ROW_PAD_X,
            top: ROW_PAD_Y,
            bottom: ROW_PAD_Y,
        })
        .show(ui, |ui| add(ui));
}

/// 3e's `border-bottom: 1px solid #f3f2f2` between rows -- one step lighter
/// than the card's own border, which is `HAIRLINE`.
fn row_separator(ui: &mut Ui) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, theme::CANVAS);
}

/// A row's `flex: 1` text column: 14px semibold title over a 12px faint line.
fn row_text(ui: &mut Ui, label: &str, description: &str) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = ROW_TEXT_GAP;
        ui.label(theme::semibold(label, 14.0).color(theme::INK));
        ui.label(
            RichText::new(description)
                .size(12.0)
                .color(theme::TEXT_FAINT),
        );
    });
}

/// A row whose trailing control is drawn by `control`: 3e's `flex: 1` text
/// column, the 20px gap, then the control right-aligned and vertically centred
/// on the text.
///
/// The two columns are allocated at explicit widths rather than by wrapping the
/// whole row in a `right_to_left` layout, and that is not a style preference.
/// A `Layout::right_to_left(Align::Center)` has to know its own height to
/// centre anything in it, so given an unbounded one it takes *all* the height
/// still available in the card -- the first row of a two-row card consumed
/// every remaining point and the second row was laid out at the bottom edge of
/// the window with zero height, painting its title and silently dropping its
/// description. Measuring the text column first and handing the control a rect
/// of exactly that height is what makes the centring well-defined.
fn control_row(ui: &mut Ui, label: &str, description: &str, control: impl FnOnce(&mut Ui)) {
    card_row(ui, |ui| {
        let text_width = (ui.available_width() - CONTROL_COLUMN_WIDTH - ROW_GAP).max(1.0);
        let origin = ui.cursor().min;
        let text = ui.allocate_ui_with_layout(
            Vec2::new(text_width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(text_width);
                row_text(ui, label, description);
            },
        );
        let height = text.response.rect.height().max(CONTROL_MIN_HEIGHT);
        let control_rect = Rect::from_min_size(
            Pos2::new(origin.x + text_width + ROW_GAP, origin.y),
            Vec2::new(CONTROL_COLUMN_WIDTH, height),
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(control_rect)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            control,
        );
    });
}

/// 3e's settings row: label, description, trailing 40x22 pill. Returns the new
/// value. The pill is paint-only ([`theme::toggle_pill`]), so the click sense
/// is allocated here.
fn toggle_row(ui: &mut Ui, label: &str, description: &str, value: bool) -> bool {
    let mut next = value;
    control_row(ui, label, description, |ui| {
        let (rect, response) = ui.allocate_exact_size(TOGGLE_SIZE, Sense::click());
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            theme::toggle_pill(ui, value);
        });
        if response.clicked() {
            next = !value;
        }
    });
    next
}

/// A row that reports a value rather than editing one (About).
fn value_row(ui: &mut Ui, label: &str, description: &str, value: &str) {
    control_row(ui, label, description, |ui| {
        ui.label(RichText::new(value).size(13.0).color(theme::TEXT_MUTED));
    });
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

fn draw_general(ui: &mut Ui, state: &mut PrefsState, hotkey: crate::hotkey::HotkeyStatus) {
    card(ui, |ui| {
        // **The backend row is not here any more.** It moved, whole, to
        // `Section::Vault` (by way of `Section::SyncAndAccount`), where it is
        // a CHILD of the switch that decides whether `bw serve` is used at
        // all -- see [`backend_description`] for why a live-looking switch
        // two pages from the thing that makes it meaningless was a defect and
        // not merely an untidiness. The same argument the breach row's move
        // makes below, one subsystem later.
        //
        // **The disk-cache pair is not here any more either**, and it left
        // for the third time this comment has had to be written: to sit
        // beside the thing that governs it. `cache_vault_to_disk` and its
        // child `read_through_cache` are now on `Section::Vault`, in the same
        // card as the backend rows -- see that section's own doc for the
        // self-trapping bug that gathering them was a response to. The
        // pairing this comment used to describe (the backend row above, the
        // disk cache below) is intact; both halves simply moved together.
        //
        // The one switch that governs what a matched window does. It sits on
        // General beside the other two rather than under Shortcuts, because
        // it is not about a shortcut: `PROMPT_DESCRIPTION` names the hotkey
        // only to say what is left when this is off.
        state.settings.prompt_on_match = toggle_row(
            ui,
            PROMPT_LABEL,
            PROMPT_DESCRIPTION,
            state.settings.prompt_on_match,
        );
        row_separator(ui);
        // **Directly under the prompt row, because that row's description
        // already names this chord.** `PROMPT_DESCRIPTION` ends "CTRL+ALT+B
        // is the only way to fill" -- a sentence that leaves a reader wanting
        // to know what CTRL+ALT+B is and whether it is working, and this is
        // the row that answers both. They are the pair the removed Shortcuts
        // page could never be beside.
        fill_hotkey_row(ui, hotkey);
        row_separator(ui);
        // **The breach row is not here any more.** It moved, whole, to
        // `Section::Breaches`, which owns the scan button and the history as
        // well: a consent pill on one page and the control it governs on
        // another is an arrangement a user has to hold in their head, and
        // `breach_scan::SCAN_CONSENT_NOTE` -- which says exactly what the
        // pill does and does not decide -- has to be readable in the same
        // glance as the pill itself.
        //
        // Site icons is the app's other vault-keyed network call, and used to
        // sit under the breach row for that reason. It stays on General
        // because it has no page of its own to go to, and the two default
        // OPPOSITE ways in any case -- see `Settings::fetch_icons` -- so they
        // were neighbours rather than a group with a shared rule.
        state.settings.fetch_icons = toggle_row(
            ui,
            FETCH_ICONS_LABEL,
            FETCH_ICONS_DESCRIPTION,
            state.settings.fetch_icons,
        );
        row_separator(ui);
        // **A CHILD of the row above, not a peer of it.** It answers "where
        // from", and "where from" is not a question at all when the answer to
        // "whether" is no -- so with site icons off this row greys out and
        // stops sensing clicks, exactly as the clipboard children do under
        // their master switch. It is greyed rather than removed for
        // `child_toggle_row`'s reason: three rows going grey says what the
        // switch above just did, three rows vanishing says nothing.
        //
        // **It is not the whole of the direct-fetch behaviour, deliberately.**
        // A private address is fetched directly with this pill off, because
        // the icon service cannot reach one; the copy says so, because a
        // switch whose label overstates its reach is worse than no switch.
        // See `favicon::icon_source_for` for the split and
        // `Settings::fetch_icons_direct` for why it is not this pill's to
        // decide.
        let icons_on = state.settings.fetch_icons;
        state.settings.fetch_icons_direct = child_toggle_row(
            ui,
            DIRECT_ICONS_LABEL,
            DIRECT_ICONS_DESCRIPTION,
            state.settings.fetch_icons_direct,
            icons_on,
        );
        row_separator(ui);
        // Directly under the icon row because the two are the same question
        // about the same tile column -- what a card wears -- and a user
        // looking for either is looking at this part of the page. They are
        // NOT a group: that one is a network request and this one reads two
        // folders on the user's own disk, which is why each states its own
        // default in its own copy instead of sharing a heading that would
        // imply a shared rule.
        state.settings.use_brand_logos = toggle_row(
            ui,
            BRAND_LOGOS_LABEL,
            BRAND_LOGOS_DESCRIPTION,
            state.settings.use_brand_logos,
        );
        row_separator(ui);
        // The other off-by-default row, and
        // wired exactly as it is. This pill is the only thing that decides
        // whether the read pane draws a TOTP-secret row at all -- the pane
        // skips the row outright when this is off rather than drawing it
        // disabled or invisible.
        state.settings.reveal_totp_seed = toggle_row(
            ui,
            TOTP_SECRET_LABEL,
            TOTP_SECRET_DESCRIPTION,
            state.settings.reveal_totp_seed,
        );
        row_separator(ui);
        // The toggle sits above the number it governs, in 3e's own 40x22
        // pill, and the number's row stays put below it -- greyed, not
        // removed. A row that vanished would reflow the card on every click
        // and would hide the value the toggle is about to restore.
        state.settings.auto_lock_enabled = toggle_row(
            ui,
            AUTO_LOCK_ENABLED_LABEL,
            AUTO_LOCK_ENABLED_DESCRIPTION,
            state.settings.auto_lock_enabled,
        );
        row_separator(ui);
        let enabled = state.settings.auto_lock_enabled;
        control_row(ui, AUTO_LOCK_LABEL, AUTO_LOCK_DESCRIPTION, |ui| {
            state.settings.auto_lock_minutes = minutes_stepper(
                ui,
                state.settings.auto_lock_minutes,
                &mut state.auto_lock_text,
                enabled,
            );
        });
        // **The update row is not here any more either**, and it left for
        // the same reason the breach row did. It used to sit last on this
        // card, deliberately apart from the two vault-keyed network rows
        // above -- that request is keyed on nothing but the app's own
        // version, and grouping it with them would have suggested it
        // discloses the same kind of thing. It is now on
        // `Section::Updates`, directly above the check button it does not
        // govern; see `draw_updates`.
    });
}


/// A [`toggle_row`] that can be switched off, for a child of a master switch.
///
/// **Disabled means disabled, not merely painted grey.** The pill senses no
/// click, sets no hover cursor, and is drawn at
/// [`theme::toggle_pill_disabled`]'s greyed treatment; the row's text is
/// ghosted so the whole row reads as inert rather than only its control. That
/// is the pair `minutes_stepper` already keeps together and the pair this
/// codebase keeps having to reunite -- "looks disabled" and "is disabled".
///
/// **The row is greyed, never hidden.** A child that vanished when its master
/// switch went off would reflow the card on every click, would hide the value
/// it is about to restore, and -- worst of the three -- would teach the user
/// nothing about what the master switch just did. Three rows going grey says
/// "these are what I turned off"; three rows disappearing says nothing at all.
///
/// The returned value is unchanged when `enabled` is false, so an off master
/// switch cannot have its children edited out from under it by a stray click.
fn child_toggle_row(
    ui: &mut Ui,
    label: &str,
    description: &str,
    value: bool,
    enabled: bool,
) -> bool {
    if enabled {
        return toggle_row(ui, label, description, value);
    }
    control_row_ghosted(ui, label, description, |ui| {
        let (rect, _) = ui.allocate_exact_size(TOGGLE_SIZE, Sense::hover());
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            theme::toggle_pill_disabled(ui, value);
        });
    });
    value
}

/// [`control_row`] with the text column ghosted -- the disabled twin, sharing
/// its layout exactly rather than approximating it.
fn control_row_ghosted(
    ui: &mut Ui,
    label: &str,
    description: &str,
    control: impl FnOnce(&mut Ui),
) {
    card_row(ui, |ui| {
        let text_width = (ui.available_width() - CONTROL_COLUMN_WIDTH - ROW_GAP).max(1.0);
        let origin = ui.cursor().min;
        let text = ui.allocate_ui_with_layout(
            Vec2::new(text_width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(text_width);
                row_text_ghosted(ui, label, description);
            },
        );
        let height = text.response.rect.height().max(CONTROL_MIN_HEIGHT);
        let control_rect = Rect::from_min_size(
            Pos2::new(origin.x + text_width + ROW_GAP, origin.y),
            Vec2::new(CONTROL_COLUMN_WIDTH, height),
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(control_rect)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            control,
        );
    });
}

/// [`row_text`] in the greyed treatment: `TEXT_GHOST` for both lines, which is
/// the grey `minutes_stepper` already uses for its disabled digits and
/// `step_button` for its disabled glyphs. No new colour is introduced.
fn row_text_ghosted(ui: &mut Ui, label: &str, description: &str) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = ROW_TEXT_GAP;
        ui.label(theme::semibold(label, 14.0).color(theme::TEXT_GHOST));
        ui.label(
            RichText::new(description)
                .size(12.0)
                .color(theme::TEXT_GHOST),
        );
    });
}

/// A row with no control at all: a statement, not a setting.
///
/// Used for `CLIPBOARD_HISTORY_NOTE`, and shaped exactly like About's account
/// line -- the same `card_row` plus `row_text`, no trailing column. A row with
/// an empty right-hand column would read as a field that failed to load, and a
/// *disabled* toggle would read as a feature that is present and broken. It is
/// neither: it is a thing that is always on and always will be.
fn note_row(ui: &mut Ui, label: &str, note: &str) {
    card_row(ui, |ui| row_text(ui, label, note));
}

/// The **Breaches** page: the consent pill, the scan button, and what previous
/// scans counted.
///
/// # The three things on this page are on ONE page deliberately
///
/// The pill used to be on General and there was no button at all. A user
/// weighing "should I let this app talk to Have I Been Pwned" was reading a
/// switch with no visible consequence; a user pressing a scan button on
/// another page would have been acting on a rule they could not see. Here the
/// switch, the control it does *not* govern, the sentence saying so
/// ([`crate::breach_scan::SCAN_CONSENT_NOTE`]), and the record of what has
/// actually been sent are all in one glance.
///
/// # The button is not gated on the pill, and the page says so
///
/// See `breach_scan`'s module header for the argument in full. In short:
/// pressing the button is the user initiating the request in the same breath
/// as consenting to it -- the same reasoning that settled the manual update
/// check -- and the setting governs what this app does **on its own**, which
/// is the per-item badge and nothing on this page.
fn draw_breaches(ui: &mut Ui, state: &mut PrefsState) {
    use crate::breach_scan::ScanStage;

    // `pump` is where the side effects are (the history write, the publish);
    // this is the only place it is called, once per frame, exactly as
    // `draw_update_card` calls its own.
    if state.scan.pump(now_unix_millis()) {
        // A finished run rewrote the history file. Re-read rather than
        // reconstructed, so the list on screen is what is on disk -- and so a
        // scan run from the other shell of this window shows up here.
        state.scan_history = load_scan_history();
    }
    if state.scan.is_busy() {
        // egui repaints on input. A progress line nobody is typing over would
        // otherwise advance only when the mouse moved.
        ui.ctx().request_repaint_after(crate::breach_scan::SCAN_POLL_INTERVAL);
    }

    card(ui, |ui| {
        // Off by default and left that way here: this pill is the only
        // consent that exists for the automatic per-item check, so it is set
        // by a click on it and by nothing else.
        state.settings.check_breaches = toggle_row(
            ui,
            BREACH_LABEL,
            BREACH_DESCRIPTION,
            state.settings.check_breaches,
        );
    });

    card(ui, |ui| {
        let stage = state.scan.stage().clone();
        let description = match &stage {
            ScanStage::Idle => SCAN_IDLE_DESCRIPTION.to_string(),
            ScanStage::Running { done, total, found, failed } => {
                crate::breach_scan::progress_wording(*done, *total, *found, *failed)
            }
            // **The outcome, in `breach_scan`'s own words**, so the sentence
            // on this page and the numbers in `scan_history.json` cannot
            // disagree about what just happened -- and so the failure count
            // is the last thing said, here as everywhere.
            ScanStage::Finished(record) => crate::breach_scan::outcome_wording(record),
            ScanStage::NothingToScan => SCAN_NOTHING_DESCRIPTION.to_string(),
            ScanStage::Unavailable => SCAN_UNAVAILABLE_DESCRIPTION.to_string(),
        };
        control_row(ui, SCAN_SECTION_LABEL, &description, |_ui| {});

        row_separator(ui);
        // **The button is on its own row, left-aligned**, rather than in the
        // trailing control column every other row on this window uses. Its
        // label is wider than `CONTROL_COLUMN_WIDTH`, and widening that
        // column would move the control on every row of every page to make
        // room for one button.
        //
        // A running scan still draws a button -- disabled, and labelled with
        // what is happening -- rather than drawing nothing: a control that
        // vanishes mid-action reflows the card under the cursor.
        //
        // **Driven by the STAGE, not by `is_busy`.** They agree in the
        // shipped app, and they must not be used interchangeably: `is_busy`
        // asks whether a channel is still open, which is a fact about this
        // process, while `Running` is what the page is claiming. The
        // screenshot example parks a panel in `Running` with no channel
        // behind it, and a button keyed on `is_busy` drew that surface with
        // an idle, clickable button on it -- a picture of a state the app
        // cannot be in.
        let running = matches!(stage, ScanStage::Running { .. });
        let mut clicked = false;
        card_row(ui, |ui| {
            let label = if running { SCAN_RUNNING_BUTTON } else { SCAN_BUTTON };
            clicked = scan_button(ui, label, !running);
        });
        if let ScanStage::Running { done, total, .. } = &stage {
            row_separator(ui);
            // The same bar the update card's download uses, so "something is
            // in flight and this is how far it has got" looks the same in
            // both places. The total is always known here -- it is the number
            // of distinct passwords the plan counted before anything was
            // spawned -- which is the honest half of `progress_bar`'s
            // `Option`.
            //
            // The bar alone: the sentence above it already says "Checked 61
            // of 128", in passwords, and `progress_bar`'s own caption counts
            // BYTES.
            let fraction = if *total == 0 { 0.0 } else { *done as f32 / *total as f32 };
            card_row(ui, |ui| progress_track(ui, fraction.clamp(0.0, 1.0)));
        }
        if clicked {
            // **The one call to `begin_scan` in this crate**, and it is
            // behind a click. Nothing consults `check_breaches` here; see
            // this function's doc and `SCAN_CONSENT_NOTE` below.
            state.scan.begin_scan(now_unix_millis());
        }

        row_separator(ui);
        card_row(ui, |ui| {
            ui.label(
                RichText::new(crate::breach_scan::SCAN_CONSENT_NOTE)
                    .size(12.0)
                    .color(theme::TEXT_FAINT),
            );
        });
    });

    card(ui, |ui| {
        card_row(ui, |ui| {
            ui.label(RichText::new(SCAN_HISTORY_LABEL).size(13.0).color(theme::INK));
        });
        if state.scan_history.entries.is_empty() {
            row_separator(ui);
            card_row(ui, |ui| {
                ui.label(RichText::new(SCAN_NO_HISTORY).size(12.0).color(theme::TEXT_FAINT));
            });
            return;
        }
        for record in &state.scan_history.entries {
            row_separator(ui);
            card_row(ui, |ui| {
                ui.spacing_mut().item_spacing.y = ROW_TEXT_GAP;
                ui.label(
                    RichText::new(scan_history_when(record)).size(12.0).color(theme::INK),
                );
                ui.label(
                    RichText::new(crate::breach_scan::outcome_wording(record))
                        .size(11.0)
                        // A run with a failure in it is not an ordinary
                        // result and is not painted like one. `ERROR` is what
                        // this app uses for "something is wrong" everywhere
                        // else; there is no success colour in this design, so
                        // a complete run is ordinary secondary ink.
                        .color(if record.is_complete() {
                            theme::TEXT_SECONDARY
                        } else {
                            theme::ERROR
                        }),
                );
            });
        }
    });
}

/// When a recorded scan finished, in the user's **own** timezone.
///
/// The stored instant is UTC and the label never says so; see
/// [`crate::local_time`]. The offset is resolved for that instant rather than
/// once for the process, so an entry from the far side of a daylight-saving
/// change still reads as the time the user's clock showed.
fn scan_history_when(record: &crate::scan_history::ScanRecord) -> String {
    crate::local_time::format_day_time(crate::local_time::local_parts(
        record.finished_at_unix_millis,
        &crate::local_time::SystemZone,
    ))
}

/// The scan button: [`update_button`]'s box, wide enough for its own label.
fn scan_button(ui: &mut Ui, label: &str, enabled: bool) -> bool {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(SCAN_BUTTON_WIDTH, STEPPER_HEIGHT), sense);
    let hovered = enabled && response.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().rect(
        rect,
        CornerRadius::same(STEPPER_RADIUS),
        if hovered { theme::CANVAS } else { theme::CARD },
        Stroke::new(1.0, if enabled { theme::BORDER_STRONG } else { theme::HAIRLINE }),
        StrokeKind::Inside,
    );
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::new(12.0, FontFamily::Name(theme::SEMIBOLD.into())),
        if enabled { theme::TEXT_SECONDARY } else { theme::TEXT_GHOST },
    );
    let at = Pos2::new(
        rect.center().x - galley.size().x / 2.0,
        rect.center().y - galley.size().y / 2.0,
    );
    ui.painter().galley(at, galley, theme::TEXT_SECONDARY);
    enabled && response.clicked()
}

// ---------------------------------------------------------------------------
// The local API, and the keys that open it
// ---------------------------------------------------------------------------

/// The master switch's label. It names **what happens to the vault**, not the
/// mechanism: "Run the local API" describes a process, and the thing the
/// owner is deciding about is that other programs get their items.
const SERVICE_LABEL: &str = "Serve this vault to programs on this PC";

/// The description under [`SERVICE_LABEL`], in its two states.
///
/// A pure function, and asserted on by tests, for [`disk_cache_description`]'s
/// reason exactly: this is the sentence a person reads before deciding to put
/// a door into their vault, and copy buried in an eframe closure is copy
/// nothing can reach.
///
/// Four properties the tests below hold, each deliberate:
///
///  * **The OFF copy says what turning it ON would do.** This is the one that
///    matters and the one that is easy to get wrong -- an off state that says
///    only "off" leaves the decision to the label, and the label is six
///    words. Off is also the default, and the copy says so rather than
///    leaving it to `Settings::default`, the same way `BREACH_DESCRIPTION`
///    states its own.
///  * **It names what is served, without a euphemism**: usernames, passwords,
///    notes and two-factor secrets. Not "vault data". The disk-cache row two
///    pages away already fought this argument and lost the euphemism.
///  * **It names who can use it** -- any program on this PC holding a key --
///    because "local endpoint" reads to most people as "only me", and
///    `service_token`'s own module doc is explicit that it is not.
///  * **It says a key is the whole credential.** Every other door into this
///    vault asks for the master password; this one does not, and a reader
///    who assumes it does has misjudged the entire trade.
fn service_description(enabled: bool) -> &'static str {
    if enabled {
        "On. Deskwarden is listening on 127.0.0.1 and will hand out decrypted vault items -- \
         usernames, passwords, notes and two-factor secrets -- to any program on this PC that \
         presents one of the keys below. It never asks for your master password: a key is the \
         whole credential, so anyone who can read one can use it. Turning this off stops the \
         endpoint and leaves the keys where they are."
    } else {
        "Off, which is the default. Turning it on starts a local HTTP endpoint on 127.0.0.1 \
         that hands out decrypted vault items -- usernames, passwords, notes and two-factor \
         secrets -- to any program on this PC that presents one of the keys you mint below. It \
         never asks for your master password: a key is the whole credential, so anyone who can \
         read one can use it. Nothing is listening, and no key opens anything, until you turn \
         this on."
    }
}

/// The key store's file name.
///
/// **This must be the name the service itself reads** (`service_keys_path` in
/// `main.rs`), or this screen mints keys into a file nothing consults and the
/// whole page is decoration. There is no shared constant to import -- the
/// service's copy is a private function in a binary -- so the agreement is
/// held by a source guard over both spellings instead of by the compiler.
use crate::service_keys::KEY_STORE_FILE_NAME as SERVICE_KEYS_FILE_NAME;

/// Where the key store lives: beside `settings.json`, in the directory this
/// app already owns.
fn service_keys_path() -> Option<std::path::PathBuf> {
    crate::settings::config_dir().map(|dir| dir.join(SERVICE_KEYS_FILE_NAME))
}

/// The stored keys, or none where there is no resolvable config directory.
///
/// An empty store is a state the page renders in words ("No keys have been
/// minted"), so there is nothing here to report as an error --
/// [`load_scan_history`]'s argument, on the file next to it.
fn load_service_keys() -> Vec<KeyRecord> {
    service_keys_path().map(|path| crate::service_keys::load(&path)).unwrap_or_default()
}

/// Writes the key store where the service will read it.
///
/// **The only production writer**, and it is a `fn` installed on the state
/// rather than a call in the draw, for the reason [`PrefsState::keys_sink`]
/// gives: the paint tests build [`PrefsState::new`], and a draw that saved
/// would make every one of them a writer of the owner's real key store.
fn save_service_keys(records: &[KeyRecord]) -> Result<(), String> {
    let path = service_keys_path()
        .ok_or_else(|| "Deskwarden could not work out where its own settings live".to_string())?;
    crate::service_keys::save(&path, records)
}

// -- copy -------------------------------------------------------------------

const KEYS_SECTION_LABEL: &str = "API keys";

/// The empty state. A result, not a blank panel -- and it says what the
/// emptiness *means*, because "no keys" and "the service is wide open" are
/// exactly the confusion this page has to prevent.
const KEYS_NONE: &str = "No keys have been minted, so nothing can reach the service yet. A \
                         program needs a key from here before it is answered at all.";

const MINT_SECTION_LABEL: &str = "Mint a key";
const MINT_SECTION_DESCRIPTION: &str = "A key is shown once, when it is made, and after that only \
                                        its hash is kept.";
const NAME_LABEL: &str = "Name";
const NAME_DESCRIPTION: &str = "Name it after the program that will hold it. The name is how you \
                                recognise this key in the list, and how you revoke it.";
const EXPIRY_LABEL: &str = "Expires after";
const EXPIRY_DESCRIPTION: &str = "Days from now. Leave it empty for a key that never expires \
                                  and works until you revoke it.";
const SUBJECT_LABEL: &str = "What it can reach";
const ACCESS_LABEL: &str = "What it may do";
const ITEM_ID_LABEL: &str = "Item id";
const ITEM_ID_DESCRIPTION: &str = "The id of the one item this key may reach, as it appears in \
                                   the vault's own links.";
const MINT_BUTTON: &str = "Mint the key";

/// The mint form's field ids, stable across frames for [`STEPPER_FIELD_ID`]'s
/// reason: focus and the caret live in egui's memory under the widget's id,
/// and this page changes height under the cursor whenever the item-id row
/// appears.
const NAME_FIELD_ID: &str = "prefs-key-name";
const EXPIRY_FIELD_ID: &str = "prefs-key-expiry-days";
const ITEM_FIELD_ID: &str = "prefs-key-item-id";
/// The full trailing control column, which is what a key's name needs.
const NAME_FIELD_WIDTH: f32 = CONTROL_COLUMN_WIDTH;
/// A box for a number of days, sized like the two steppers' value cells.
const EXPIRY_FIELD_WIDTH: f32 = 72.0;
/// Wide enough for their own labels at 12px semibold, with 3e's breathing
/// room around them.
const MINT_BUTTON_WIDTH: f32 = 120.0;
const REVOKE_BUTTON_WIDTH: f32 = 88.0;
const COPY_BUTTON_WIDTH: f32 = 88.0;
/// A picker cell's horizontal padding, total. Its width is measured from its
/// own label, because "Secure note" and "Card" are not the same word.
const CHOICE_PADDING: f32 = 20.0;

const EVERYTHING_CHOICE: &str = "Everything";
const ONE_ITEM_CHOICE: &str = "One item";
const READ_CHOICE: &str = "Read";
const WRITE_CHOICE: &str = "Write";

/// The heading over a key that has just been minted.
const REVEAL_LABEL: &str = "Copy this key now";

/// **The whole contract of the reveal, in the place it is shown.**
///
/// It says the key is not stored, not merely that it will not be shown again
/// -- because "shown once" reads as a policy someone could relax, and "we
/// keep only a hash of it" is the reason it cannot be relaxed. A reader who
/// believes the key is retrievable will close this card and come back for it.
const REVEAL_NOTE: &str = "This is the only time this key is shown. Deskwarden stores nothing \
                           but a hash of it, so it cannot be shown again -- not by this screen, \
                           not from the file. If you lose it, revoke this key and mint another.";

const COPY_BUTTON: &str = "Copy";
const DONE_BUTTON: &str = "Done";
const REVOKE_BUTTON: &str = "Revoke";
const REVOKE_CONFIRM_BUTTON: &str = "Revoke it";
const REVOKE_CANCEL_BUTTON: &str = "Keep it";

/// What the owner is asked before a key goes.
///
/// **It names the consequence, not the operation.** "Are you sure?" tells a
/// person nothing they did not know when they clicked; what they need is
/// that the program holding this key stops working immediately, that the
/// failure it gets says nothing about why, and that there is no undo -- which
/// is exactly the mis-click this confirmation exists for.
fn revoke_prompt(name: &str) -> String {
    format!(
        "Revoke \u{201c}{name}\u{201d}? Any program still holding this key stops being answered \
         the moment you do -- with a refusal that tells it nothing about why -- and the key \
         cannot be brought back. Replacing it means putting a new key into whatever was using \
         this one."
    )
}

/// Why a form was refused before [`crate::service_keys::mint`] ever saw it.
///
/// These three are deliberately **not** [`crate::service_keys::MintRefusal`] arms: `mint` cannot
/// see any of them. It takes an `Option<u64>` and a `Vec<Scope>`, so a box of
/// nonsense where a number should be, and a scope set that is empty because
/// neither access was chosen, have already become "no expiry" and "no
/// permissions" by the time it is called -- both of which are legitimate
/// values it must accept. Judging them here is the only place they are still
/// distinguishable from the thing the owner meant.
const EXPIRY_NOT_A_NUMBER: &str = "Enter the number of days this key should last -- 30, say -- \
                                   or leave the box empty for a key that never expires.";
const NO_ITEM_ID: &str = "Enter the id of the item this key is for. A key scoped to no item is \
                          refused every item, which is a key that looks live and is not.";
const NO_ACCESS: &str = "Choose Read, Write, or both. A key with neither is refused everything \
                         it asks for, which is a key that looks live and is not.";

/// Said after a key was minted but the store could not be written.
///
/// **The key is live in this window and dead after a restart**, which is the
/// worst of the three possible states and the one a "could not save" would
/// not convey.
const MINT_NOT_SAVED: &str = "The key was made, but the key store could not be written, so it \
                              will be gone the next time Deskwarden starts: ";

/// Said after a key was revoked but the store could not be written.
///
/// **The other direction, and the dangerous one**: the list on screen no
/// longer shows the key and the file still grants it. A message that said
/// only "could not save" would leave the owner believing they had revoked
/// it.
const REVOKE_NOT_SAVED: &str = "The key is gone from this list, but the key store could not be \
                                written -- so it still works, and will be back in this list when \
                                Deskwarden restarts: ";

// -- what a key is, in words ------------------------------------------------

/// The five kinds a key can be scoped to, in the order the picker offers
/// them.
///
/// [`ItemKind::Unknown`] is deliberately absent: it is what an item of a type
/// this build does not know reads as, and there is no sense in offering a
/// grant over "whatever we could not parse". A key wanting one of those
/// scopes to a single id instead.
const KEY_CATEGORIES: [ItemKind; 5] = [
    ItemKind::Login,
    ItemKind::SecureNote,
    ItemKind::Card,
    ItemKind::Identity,
    ItemKind::SshKey,
];

/// One grant, as a sentence.
///
/// Access first, because that is the half that decides whether a mistake here
/// costs a disclosure or an edit.
fn scope_wording(scope: &Scope) -> String {
    let access = match scope.access {
        Access::Read => "Read",
        Access::Write => "Write",
    };
    let subject = match &scope.subject {
        Subject::All => "everything in the vault".to_string(),
        Subject::Category(kind) => format!("every {}", kind.label().to_lowercase()),
        Subject::Item(id) => format!("the item {id}"),
        // **Named, and named as granting nothing**, because that is what
        // `service_keys` does with it. A row that quietly omitted an
        // unrecognised scope would be a screen claiming a key is narrower
        // than the file says, on a build that is merely older than the file.
        Subject::Unrecognised(raw) => {
            format!("\u{201c}{raw}\u{201d}, which this version does not understand and treats as \
                     permitting nothing")
        }
    };
    format!("{access} {subject}")
}

/// Every grant on a key, as one line.
///
/// The empty set is a sentence rather than a blank, and it says what the
/// default-deny rule means in practice: a key with no scopes is refused
/// everything. `service_keys`' module doc is explicit that this is a state a
/// record can genuinely be in.
fn scopes_wording(scopes: &[Scope]) -> String {
    if scopes.is_empty() {
        return "No permissions at all: this key is refused everything it asks for.".to_string();
    }
    scopes.iter().map(scope_wording).collect::<Vec<_>>().join(" \u{00b7} ")
}

/// When a key was made and when it stops working, as one line.
///
/// **Expiry is answered against `now_unix`, not read off the record**, for
/// the reason `service_keys::find` takes a clock: a key whose date has passed
/// is refused by the service, and a list that showed it as an ordinary future
/// date would be the screen disagreeing with the door.
///
/// "Never expires" is spelled out rather than left blank. A blank cell in an
/// expiry column is read as "not applicable", which is the opposite of what
/// it means here.
fn key_when_wording(
    record: &KeyRecord,
    now_unix: u64,
    zone: &dyn crate::local_time::LocalOffset,
) -> String {
    let when = |seconds: u64| {
        crate::local_time::format_day_time(crate::local_time::local_parts(
            (seconds as i64).saturating_mul(1000),
            zone,
        ))
    };
    let created = format!("Created {}", when(record.created_unix));
    match record.expires_unix {
        None => format!("{created} \u{00b7} Never expires"),
        // Inclusive, exactly as `service_keys::find` is: a key whose expiry
        // is this second is already refused, so this second is already
        // "expired" and not "expires now".
        Some(at) if at <= now_unix => {
            format!("{created} \u{00b7} Expired {} \u{2014} it is already refused", when(at))
        }
        Some(at) => format!("{created} \u{00b7} Expires {}", when(at)),
    }
}

// -- the form, as plain values ----------------------------------------------

/// What the subject picker is on.
///
/// A choice rather than a [`Subject`] because [`Subject::Item`] carries the
/// id, and the id lives in its own text buffer that must survive the user
/// clicking away to `Everything` and back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubjectChoice {
    Everything,
    Category(ItemKind),
    OneItem,
}

/// What is typed into the mint form, between frames.
///
/// Plain values with no `Ui` anywhere near them, so every decision this form
/// makes -- what it refuses, what scopes it produces, what expiry it computes
/// -- is a function call in a test rather than a click on a window.
#[derive(Debug, Clone, PartialEq, Eq)]
struct KeyForm {
    name: String,
    /// Days from now, as typed. Empty means no expiry.
    expiry_days: String,
    subject: SubjectChoice,
    /// Kept even while [`Self::subject`] is not `OneItem`, so clicking away
    /// and back does not lose a pasted id.
    item_id: String,
    read: bool,
    write: bool,
}

impl Default for KeyForm {
    /// **Read on, write off.** Not "both off" and not "both on".
    ///
    /// Both off would make the first click on `Mint` a refusal for a reason
    /// the owner has not been told about yet. Both on would make the default
    /// key one that can *change* the vault, which is not what anyone reaches
    /// for this screen to do first -- the case in the design is a backup
    /// script -- and a default that grants more than the common case is how
    /// an over-scoped key gets minted by someone who never looked at this
    /// row.
    fn default() -> Self {
        Self {
            name: String::new(),
            expiry_days: String::new(),
            subject: SubjectChoice::Everything,
            item_id: String::new(),
            read: true,
            write: false,
        }
    }
}

/// What is wrong with the form, before `mint` is asked.
fn form_refusal(form: &KeyForm) -> Option<&'static str> {
    if !form.expiry_days.trim().is_empty() && form.expiry_days.trim().parse::<u64>().is_err() {
        return Some(EXPIRY_NOT_A_NUMBER);
    }
    if form.subject == SubjectChoice::OneItem && form.item_id.trim().is_empty() {
        return Some(NO_ITEM_ID);
    }
    if !form.read && !form.write {
        return Some(NO_ACCESS);
    }
    None
}

/// The instant the form's expiry lands on, or `None` for no expiry.
///
/// `saturating_mul`/`saturating_add`, because the box takes a number and
/// `u64::MAX` days is one that can be typed into it. Saturating lands on a
/// date far past any clock, which is the honest reading of "that many days";
/// wrapping would land in the past and mint a key that is dead on arrival.
///
/// **Zero days is not special-cased here.** It computes to exactly `now`,
/// which `service_keys::mint` refuses as `ExpiryAlreadyPassed` -- and that is
/// the refusal the owner should see, in `service_keys`' own words, rather
/// than a second sentence written here that says the same thing differently.
fn form_expiry(form: &KeyForm, now_unix: u64) -> Option<u64> {
    let days: u64 = form.expiry_days.trim().parse().ok()?;
    Some(now_unix.saturating_add(days.saturating_mul(86_400)))
}

/// The grants the form describes.
///
/// Empty when neither access is ticked -- which [`form_refusal`] stops before
/// it can be minted, and which is still the right value to compute rather
/// than a panic: default deny is `service_keys`' rule and an empty set is how
/// it is spelled.
fn form_scopes(form: &KeyForm) -> Vec<Scope> {
    let subject = match form.subject {
        SubjectChoice::Everything => Subject::All,
        SubjectChoice::Category(kind) => Subject::Category(kind),
        SubjectChoice::OneItem => Subject::Item(form.item_id.trim().to_string()),
    };
    let mut scopes = Vec::new();
    if form.read {
        scopes.push(Scope { subject: subject.clone(), access: Access::Read });
    }
    if form.write {
        scopes.push(Scope { subject, access: Access::Write });
    }
    scopes
}

/// A key that has been made and not yet dismissed.
///
/// **No `Debug`, derived or otherwise.** It holds the one plaintext key that
/// exists anywhere in this process, and `crate::debug_leak_guard` is the
/// record of what happens when a type like this gets a derive.
struct MintedKey {
    name: String,
    /// Wiped when this is dropped. The store holds only a hash, so this
    /// string is the only copy in existence until the owner has pasted it
    /// somewhere.
    secret: zeroize::Zeroizing<String>,
}

/// What pressing Mint did.
///
/// A value rather than a mutation of the state, so the whole decision --
/// which refusals fire, in which order, and what the owner is told -- is
/// testable without a window.
enum MintOutcome {
    Minted(KeyRecord, MintedKey),
    /// The sentence to show. Either one of this module's three form refusals
    /// or, **verbatim**, [`crate::service_keys::MintRefusal::message`]: a refusal the owner cannot
    /// act on is a refusal that wastes their afternoon, and `service_keys`
    /// already wrote the actionable words.
    Refused(String),
}

/// The whole of pressing Mint, as a function of values.
fn attempt_mint(
    form: &KeyForm,
    now_unix: u64,
    random: fn() -> [u8; 32],
    existing: &[KeyRecord],
) -> MintOutcome {
    if let Some(reason) = form_refusal(form) {
        return MintOutcome::Refused(reason.to_string());
    }
    match crate::service_keys::mint(
        form.name.clone(),
        form_expiry(form, now_unix),
        form_scopes(form),
        now_unix,
        random,
        existing,
    ) {
        Ok((record, secret)) => {
            let minted =
                MintedKey { name: record.name.clone(), secret: zeroize::Zeroizing::new(secret) };
            MintOutcome::Minted(record, minted)
        }
        Err(refusal) => MintOutcome::Refused(refusal.message()),
    }
}

// -- the page ---------------------------------------------------------------

/// The **Vault** page: where the vault is served from, what is kept on this
/// PC, and who else may ask for it.
///
/// # Everything about the vault on one page, deliberately
///
/// [`draw_breaches`]'s argument, with more at stake, and now applied to the
/// whole subject rather than to the service alone. The backend choice decides
/// which program holds the keys; `keep_backend_running` decides whether that
/// program stays up; the disk cache decides what is left on the disk when it
/// is not; the service switch decides whether a door exists at all; the keys
/// are that door's only credential; the mint form is how another one is cut;
/// the revoke button is the only way one is taken back.
///
/// Split across pages -- which is what shipped, over General, Sync & account
/// and here -- the owner can turn the backend off on one screen and discover
/// on another that the window they would undo it from no longer opens, and
/// can turn the service "off" without ever having seen the three keys that
/// will work again the moment it is on. Both of those happened.
///
/// # The order is the order of consequence
///
/// Where the vault comes from, then what is written down, then who may ask.
/// Each card is only decidable once the one above it is: whether an encrypted
/// copy is worth keeping depends on whether a backend is being kept warm, and
/// what the service can serve depends on both.
///
/// # The plaintext key is on screen exactly once, and never on disk
///
/// [`attempt_mint`] hands back a [`MintedKey`] which lives in
/// [`PrefsState::minted`] until the owner dismisses it, and is wiped when it
/// is dropped. What is *stored* is the [`KeyRecord`], which holds
/// `SHA-256(key)` -- see `service_keys`' module doc for why that hash is fast
/// and why the file is worth nothing to whoever reads it.
///
/// The card stays up until it is dismissed, rather than clearing on a page
/// change, and that is not laziness: it is the only copy of the key in
/// existence, and a mis-click in the nav must not destroy it.
///
/// # Revoking asks first
///
/// Every other control on this window is reversible by clicking it again.
/// This one is not, and the thing it breaks is somebody's unattended script
/// -- so it is the one place in Preferences with a confirmation, and the
/// confirmation says what breaks rather than "are you sure".
/// Which way a click on the backend row would take the vault.
///
/// **Named directions rather than the `bool` itself**, because the two are
/// not the same size of decision and the whole point of the confirmation is
/// that they are not: one of them deletes a key off the user's disk and the
/// other does not. A `bool` threaded from the pill to the prompt would let
/// that asymmetry be lost to a single inverted comparison, silently, in the
/// one sentence a user reads before agreeing to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendSwitch {
    /// Off to on: back to the official `bw` CLI, which is also the arm that
    /// takes `userkey.bin` off this PC -- see `main`'s
    /// `settle_the_vault_backend`, which clears the store on every arm that
    /// is not `DirectRest`.
    ToOfficial,
    /// On to off: over to Deskwarden's own built-in direct-REST client.
    ToBuiltIn,
}

impl BackendSwitch {
    /// Whether taking this direction removes the stored vault key from the
    /// PC.
    ///
    /// Only one of the two does, and that is not a detail of the wording: the
    /// key is a non-expiring wrapped master key, so "it is deleted" is the
    /// reassurance a user needs going one way and a claim that would be
    /// simply false going the other.
    fn deletes_the_stored_vault_key(self) -> bool {
        matches!(self, Self::ToOfficial)
    }
}

/// What a click on the backend pill is proposing, or `None` where it is
/// proposing nothing.
///
/// **A pure function of the two values, decided away from the frame.** The
/// question "does this need confirming, and what does it cost" is the whole
/// of this feature, and it is answered here so the tests can put it under
/// oath without a window, a server or a vault -- the same separation
/// [`official_crypto_description`] keeps for the copy under the row.
///
/// `None` for a frame where the row handed back what it was given, which is
/// every frame nobody clicked and every click on the ghosted row: a
/// confirmation that could be raised without a flip would ask the user about
/// a change that is not being made.
fn backend_switch(stored: bool, clicked: bool) -> Option<BackendSwitch> {
    match (stored, clicked) {
        (false, true) => Some(BackendSwitch::ToOfficial),
        (true, false) => Some(BackendSwitch::ToBuiltIn),
        _ => None,
    }
}

const BACKEND_SWITCH_CONFIRM_BUTTON: &str = "Switch it";
const BACKEND_SWITCH_CANCEL_BUTTON: &str = "Leave it";

/// What the owner is asked before the backend moves.
///
/// [`revoke_prompt`]'s rule on the other control that costs something: it
/// names the consequence rather than asking "are you sure". Three facts, and
/// the third only where it is true:
///
///  * **it does not take effect now.** The choice is applied by
///    `settle_the_vault_backend`, which this click does not run -- so the
///    click changes nothing this session, and a user who is not told that
///    clicks it again;
///  * **you have to sign in again, and that is the moment it lands.** Either
///    direction re-derives the vault key from the master password, and the
///    re-settle rides along with it
///    (`login_ui::direct_login_for_this_sign_in`). A login prompt nobody was
///    warned about reads as a fault;
///  * **and, going back to `bw` only, the stored vault key is deleted from
///    this PC.** That is the good news of that direction and the reason it
///    cannot be worded generically: said in both, it would be a lie in one;
///    left out of both, the user cannot tell whether turning this back on
///    undoes what turning it off did.
///
/// **It does not promise a relaunch, and it no longer demands one.** Nothing
/// in this program restarts it -- there is no `current_exe` respawn anywhere
/// -- so where a restart is named the sentence asks the user to close and
/// reopen Deskwarden themselves. But a restart is no longer the only route:
/// since 0.15.4 a sign-in re-settles the live process for the account it is
/// about (`login_ui::direct_login_for_this_sign_in` ->
/// `backend_policy::resettle_for`), so signing out and back in reaches the
/// same place. The sentence names the moment -- the next sign-in -- and then
/// both routes to it, rather than naming one route as though it were the
/// requirement.
fn backend_switch_prompt(switch: BackendSwitch) -> &'static str {
    if switch.deletes_the_stored_vault_key() {
        "Switch back to the official Bitwarden CLI? It does not take effect until you next \
         sign in to this account -- either sign out and back in, or close Deskwarden and open \
         it again yourself. This also deletes the vault key stored on this PC, so nothing is \
         left behind by the switch."
    } else {
        "Switch to Deskwarden's built-in client? It does not take effect until you next sign \
         in to this account -- either sign out and back in, or close Deskwarden and open it \
         again yourself."
    }
}

/// The question, under the row that raised it.
///
/// Shaped exactly like [`key_row`]'s pending half -- the prompt in
/// [`theme::ERROR`], then the two buttons side by side -- because this window
/// now asks twice and two confirmations that looked different would read as
/// two different kinds of question.
fn backend_switch_row(ui: &mut Ui, switch: BackendSwitch) -> Option<RowAction> {
    let mut action = None;
    card_row(ui, |ui| {
        ui.spacing_mut().item_spacing.y = ROW_TEXT_GAP;
        ui.label(RichText::new(backend_switch_prompt(switch)).size(12.0).color(theme::ERROR));
        ui.horizontal(|ui| {
            if key_button(ui, BACKEND_SWITCH_CONFIRM_BUTTON, REVOKE_BUTTON_WIDTH) {
                action = Some(RowAction::Confirm);
            }
            if key_button(ui, BACKEND_SWITCH_CANCEL_BUTTON, REVOKE_BUTTON_WIDTH) {
                action = Some(RowAction::Cancel);
            }
        });
    });
    action
}

/// **Which backend holds the vault, and whether it is kept warm.**
///
/// The parent is `use_official_bw_crypto`, drawn as a two-cell picker: the
/// official `bw` CLI, or Deskwarden's own built-in client. The child is
/// `keep_backend_running`,
/// which is a trade about the `bw serve` subprocess and therefore means
/// nothing when there is no subprocess.
///
/// # Both gates are `backend_policy`'s, never re-decided here
///
/// The parent is available only where the choice can actually take effect:
/// [`account_is_self_hosted`], which reaches
/// [`crate::backend_policy::is_self_hosted`]. The owner's rule is that the
/// built-in client is a thing to point at your own server and not at somebody
/// else's service, and a switch that flips while
/// [`crate::backend_policy::choose`] goes on answering `BwServe` is a switch
/// that lies.
///
/// The child is gated on [`crate::backend_policy::choose`] itself, through
/// [`cli_rows_are_shown`], over the account's server and the **live** value of
/// the parent -- so the two rows agree within one frame rather than one
/// restart, and there is no second copy here of what "which backend" means.
///
/// # The parent ghosts and the child hides, two rows apart
///
/// The parent is a [`backend_choice_row`] -- a two-cell picker naming both
/// clients, where it used to be an on/off pill naming only the CLI. On an
/// account the built-in client cannot serve it goes grey under a sentence
/// naming the remedy, and the returned value is the stored one unchanged, so
/// a click writes nothing; that is [`child_toggle_row`]'s contract, kept by
/// the control that replaced it. The
/// child is not drawn at all on the backend that has no subprocess, because
/// there is no remedy for it to name -- the remedy would be the switch one row
/// above it -- and the only sentence a ghost could carry there is a confession
/// about internals. See
/// `docs/superpowers/specs/2026-08-30-preferences-per-backend-design.md`.
fn draw_backend_card(ui: &mut Ui, state: &mut PrefsState) {
    card(ui, |ui| {
        // Read once and shared by both rows: two calls could answer
        // differently mid-frame -- the status arrives on a worker thread --
        // and a parent that says "self-hosted" over a child that says
        // otherwise is the disagreement this card was rebuilt to remove.
        let status = (state.account_source)();
        let self_hosted = account_is_self_hosted(status.clone());
        // **The pill paints the stored value, including while the question
        // is up.** The owner's rule for a declined switch is that the toggle
        // visibly stays where it was, and the way to be sure of that is never
        // to have moved it: the click is a proposal, and nothing but
        // `Confirm` below writes the field.
        let stored = state.settings.use_official_bw_crypto;
        let clicked = backend_choice_row(
            ui,
            official_crypto_description(self_hosted),
            stored,
            self_hosted,
        );
        if let Some(switch) = backend_switch(stored, clicked) {
            state.pending_backend_switch = Some(switch);
        }
        if let Some(switch) = state.pending_backend_switch {
            row_separator(ui);
            match backend_switch_row(ui, switch) {
                Some(RowAction::Confirm) => {
                    // Cleared on BOTH arms, and that is the bug the revoke
                    // confirmation shipped with for a while: an answer that
                    // left `pending` set would leave the question on screen
                    // and make the buttons inert, which is a confirmation the
                    // owner cannot get out of.
                    state.pending_backend_switch = None;
                    // Spelled out rather than reusing
                    // `deletes_the_stored_vault_key`, which happens to be
                    // true on the same arm: two facts that coincide today are
                    // two facts, and collapsing them is how one of them gets
                    // changed by an edit to the other.
                    state.settings.use_official_bw_crypto =
                        matches!(switch, BackendSwitch::ToOfficial);
                }
                Some(RowAction::Cancel) => state.pending_backend_switch = None,
                Some(RowAction::Ask) | None => {}
            }
        }
        let server = account_server(&status);
        // **Hidden, not ghosted, and the two rows above show why the
        // distinction is not a preference.** The switch above is ghosted on an
        // account that cannot use the built-in client, because that is a
        // remedy the user can act on -- change the server -- and grey is the
        // promise that the row comes back. This row has no remedy to offer: on
        // the built-in client there is no subprocess, and the only sentence a
        // ghost could carry is a confession about how the app is built.
        if cli_rows_are_shown(server, state.settings.use_official_bw_crypto) {
            row_separator(ui);
            state.settings.keep_backend_running = toggle_row(
                ui,
                BACKEND_LABEL,
                BACKEND_DESCRIPTION,
                state.settings.keep_backend_running,
            );
        }
        row_separator(ui);
        // **Not gated, and never was.** This is about Deskwarden's own window
        // and is true on every backend -- see `Settings::keep_ui_loaded`. It
        // is the row most likely to be swept up by a careless split, because
        // it shares a card with the one row that goes.
        state.settings.keep_ui_loaded = toggle_row(
            ui,
            UI_LOADED_LABEL,
            UI_LOADED_DESCRIPTION,
            state.settings.keep_ui_loaded,
        );
    });
}

/// **The encrypted copy on this PC, and whether reads consult it.**
///
/// Deliberately its own card rather than two more rows under the backend
/// pair: these two are about what is written to the user's disk, and the two
/// above are about which process is running. A single card of four rows would
/// read as one group with one rule, which they are not.
///
/// **Not two independent switches, and the card must not imply they are.**
/// [`crate::backend_policy::read_path`] answers `ServiceOnly` whenever
/// `cache_vault_to_disk` is off, whatever `read_through_cache` says -- the
/// first is a veto over the second. So the second is a [`child_toggle_row`]
/// of the first: ghosted when there is no copy to read, saying so in the row,
/// and handing back the stored value unchanged.
///
/// The parent has a gate of its own that is not the child's: Windows Hello.
/// The file is encrypted with a key Hello keeps in this PC's TPM, and without
/// Hello there is no such key -- so the row is ghosted with the reason in it
/// rather than silently downgraded to something weaker under the same label.
fn draw_disk_cache_card(ui: &mut Ui, state: &mut PrefsState) {
    card(ui, |ui| {
        // **No Hello gate.** The setting used to be ghosted whenever
        // Windows Hello was unavailable, because the file could not be
        // sealed without it. It can now, so a machine without Hello is
        // simply a machine that can use this setting -- and leaving the
        // row grey would be refusing a feature for a reason that stopped
        // existing.
        state.settings.cache_vault_to_disk = child_toggle_row(
            ui,
            DISK_CACHE_LABEL,
            disk_cache_description(),
            state.settings.cache_vault_to_disk,
            // Always live now: nothing about this row depends on Hello.
            true,
        );
        row_separator(ui);
        // The child of the row above: with no copy permitted there is nothing
        // to read. Ghosted rather than hidden, for `child_toggle_row`'s usual
        // reason -- a row that vanishes is a row a user cannot find out about.
        state.settings.read_through_cache = child_toggle_row(
            ui,
            READ_THROUGH_LABEL,
            read_through_description(state.settings.cache_vault_to_disk),
            state.settings.read_through_cache,
            state.settings.cache_vault_to_disk,
        );
    });
}

/// Where the vault is served from, then what is kept of it on this PC.
///
/// **No scroll region, and that is a measurement rather than an opinion.**
/// This page had one while the key list was on it, because that list has no
/// upper bound. Four rows of long copy do not: the worst combination of
/// descriptions this page can paint -- the three-paragraph crypto copy over
/// the Windows Hello explanation -- ends 180 points above the fold of a body
/// that is a fixed 740. See
/// `the_whole_vault_page_is_readable_without_scrolling`, which drives all
/// four combinations and would fail before a user found a row they could not
/// reach.
///
/// So it follows the same rule as every other fixed page here: a scroll
/// region on content that fits reserves a lane for a bar that can never be
/// needed, and this file's bars are `AlwaysVisible` -- one here would be a
/// permanent bar on a page that never moves.
fn draw_vault(ui: &mut Ui, state: &mut PrefsState) {
    vault_cards(ui, state);
}

/// The Vault page's cards, in reading order.
///
/// Kept as its own function, though it is now two calls: it is the seam the
/// scroll-region question is asked at, and inlining it would put the answer
/// back in `draw_section`'s match arm where nothing can document it.
fn vault_cards(ui: &mut Ui, state: &mut PrefsState) {
    draw_backend_card(ui, state);
    draw_disk_cache_card(ui, state);
}

/// **The one page in this window that scrolls**, and it is not a precaution:
/// the key list has no upper bound. Every other page here is a fixed set of
/// rows that fits the fixed window by construction, so a scroll region on
/// them would reserve a lane for a bar that can never be needed. Ten keys on
/// this one runs off the bottom of a window that cannot be resized, and a
/// revoke button below the fold is a revoke button that does not exist.
///
/// **This is the half of the split that had to keep the region.** The Vault
/// page's was there for this list; with the list gone, its remaining four
/// rows fit -- see [`draw_vault`].
///
/// Always visible, for `draw_notes`'s reason exactly: content clipped with no
/// bar reads as content that failed to load, and a bar whose lane comes and
/// goes with the content makes the cards change width as keys are minted.
///
/// The `id_salt` is the one this region has always had. It is the same
/// region, moved with the content it was made for, and an id is what egui
/// keeps the scroll offset under -- changing it here would be a new region
/// wearing the old one's job for no reason.
fn draw_api(ui: &mut Ui, state: &mut PrefsState) {
    egui::ScrollArea::vertical()
        .id_salt("prefs-vault-service")
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(0.0, CONTENT_GAP);
            api_cards(ui, state);
        });
}

/// The page's cards, in reading order.
///
/// **The unbounded list is last.** Every card above it is fixed-height and
/// all of them are more urgent than the eleventh key; putting the list above
/// them would push the mint button off the bottom of a page whose length the
/// owner does not control.
///
/// **The switch is first**, because a key is meaningless while nothing is
/// listening: a reader who mints one without having seen the switch has
/// bought a credential for a door that does not exist.
fn api_cards(ui: &mut Ui, state: &mut PrefsState) {
    card(ui, |ui| {
        state.settings.service_enabled = toggle_row(
            ui,
            SERVICE_LABEL,
            service_description(state.settings.service_enabled),
            state.settings.service_enabled,
        );
    });

    draw_minted_key(ui, state);
    draw_mint_form(ui, state);

    card(ui, |ui| {
        card_row(ui, |ui| {
            ui.label(RichText::new(KEYS_SECTION_LABEL).size(13.0).color(theme::INK));
        });
        if state.keys.is_empty() {
            row_separator(ui);
            card_row(ui, |ui| {
                ui.label(RichText::new(KEYS_NONE).size(12.0).color(theme::TEXT_FAINT));
            });
            return;
        }
        let now = (state.key_clock)();
        // Collected before the loop: the rows borrow `state.keys`, and what
        // a click on one of them changes is `state`.
        let mut asked = None;
        let mut confirmed = None;
        let mut cancelled = false;
        for record in &state.keys {
            row_separator(ui);
            let pending = state.pending_revoke.as_deref() == Some(record.name.as_str());
            match key_row(ui, record, now, pending) {
                Some(RowAction::Ask) => asked = Some(record.name.clone()),
                Some(RowAction::Confirm) => confirmed = Some(record.name.clone()),
                // **A flag, not `asked = None`.** Setting the local back to
                // `None` leaves `pending_revoke` exactly where it was, so the
                // question stays up and Keep it does nothing -- which is a
                // confirmation the owner cannot get out of, on the one
                // control here that cannot be undone. The test that drives
                // Keep it is what found this.
                Some(RowAction::Cancel) => cancelled = true,
                None => {}
            }
        }
        if let Some(name) = asked {
            state.pending_revoke = Some(name);
        }
        if cancelled {
            state.pending_revoke = None;
        }
        if let Some(name) = confirmed {
            state.pending_revoke = None;
            if crate::service_keys::revoke(&mut state.keys, &name) {
                let sink = state.keys_sink;
            if let Err(error) = sink(&state.keys) {
                    state.key_message = Some(format!("{REVOKE_NOT_SAVED}{error}"));
                } else {
                    state.key_message = None;
                }
            }
        }
    });
}

/// What a key's row reported this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowAction {
    /// Revoke was pressed: ask before doing anything.
    Ask,
    Confirm,
    Cancel,
}

/// One stored key: its name, when it was made, when it dies, and what it
/// opens -- and **never the key**, because the store holds only a hash and
/// there is nothing else that could be shown.
fn key_row(ui: &mut Ui, record: &KeyRecord, now_unix: u64, pending: bool) -> Option<RowAction> {
    let mut action = None;
    card_row(ui, |ui| {
        ui.spacing_mut().item_spacing.y = ROW_TEXT_GAP;
        ui.label(theme::semibold(&record.name, 14.0).color(theme::INK));
        let when = key_when_wording(record, now_unix, &crate::local_time::SystemZone);
        // An expired key is not an ordinary row and is not painted like one:
        // `ERROR` is what this app uses for "something is wrong" everywhere
        // else, and a key the service already refuses is a script that has
        // already stopped working.
        let expired = record.expires_unix.is_some_and(|at| at <= now_unix);
        ui.label(
            RichText::new(when)
                .size(12.0)
                .color(if expired { theme::ERROR } else { theme::TEXT_FAINT }),
        );
        ui.label(
            RichText::new(scopes_wording(&record.scopes)).size(12.0).color(theme::TEXT_FAINT),
        );
        if pending {
            ui.label(RichText::new(revoke_prompt(&record.name)).size(12.0).color(theme::ERROR));
            ui.horizontal(|ui| {
                if key_button(ui, REVOKE_CONFIRM_BUTTON, REVOKE_BUTTON_WIDTH) {
                    action = Some(RowAction::Confirm);
                }
                if key_button(ui, REVOKE_CANCEL_BUTTON, REVOKE_BUTTON_WIDTH) {
                    action = Some(RowAction::Cancel);
                }
            });
        } else if key_button(ui, REVOKE_BUTTON, REVOKE_BUTTON_WIDTH) {
            action = Some(RowAction::Ask);
        }
    });
    action
}

/// The just-minted key, or nothing.
fn draw_minted_key(ui: &mut Ui, state: &mut PrefsState) {
    // Taken out and put back, so the card can be drawn from a borrow while
    // the buttons under it decide whether the state keeps it.
    let mut reveal = state.minted.take();
    let mut copy = false;
    let mut done = false;
    if let Some(minted) = reveal.as_ref() {
        card(ui, |ui| {
            card_row(ui, |ui| {
                ui.spacing_mut().item_spacing.y = ROW_TEXT_GAP;
                ui.label(theme::semibold(REVEAL_LABEL, 14.0).color(theme::INK));
                ui.label(RichText::new(&minted.name).size(12.0).color(theme::TEXT_FAINT));
                ui.label(RichText::new(REVEAL_NOTE).size(12.0).color(theme::ERROR));
            });
            row_separator(ui);
            card_row(ui, |ui| {
                // Monospace, and painted in full: this is a 64-character hex
                // string somebody is about to compare by eye or select with a
                // mouse, and an elided one is a key they cannot use.
                ui.label(
                    RichText::new(minted.secret.as_str())
                        .size(12.0)
                        .family(FontFamily::Monospace)
                        .color(theme::INK),
                );
            });
            row_separator(ui);
            card_row(ui, |ui| {
                ui.horizontal(|ui| {
                    copy = key_button(ui, COPY_BUTTON, COPY_BUTTON_WIDTH);
                    done = key_button(ui, DONE_BUTTON, COPY_BUTTON_WIDTH);
                });
            });
        });
        if copy {
            (state.key_copy)(minted.secret.as_str());
        }
    }
    if done {
        // Dropped here, and `Zeroizing` wipes it on the way out.
        reveal = None;
    }
    state.minted = reveal;
}

/// The form that makes a key, and the one button in this app that does.
fn draw_mint_form(ui: &mut Ui, state: &mut PrefsState) {
    let mut mint = false;
    card(ui, |ui| {
        card_row(ui, |ui| {
            ui.spacing_mut().item_spacing.y = ROW_TEXT_GAP;
            ui.label(RichText::new(MINT_SECTION_LABEL).size(13.0).color(theme::INK));
            ui.label(
                RichText::new(MINT_SECTION_DESCRIPTION).size(12.0).color(theme::TEXT_FAINT),
            );
        });

        row_separator(ui);
        control_row(ui, NAME_LABEL, NAME_DESCRIPTION, |ui| {
            form_field(ui, NAME_FIELD_ID, &mut state.key_form.name, NAME_FIELD_WIDTH);
        });

        row_separator(ui);
        control_row(ui, EXPIRY_LABEL, EXPIRY_DESCRIPTION, |ui| {
            form_field(ui, EXPIRY_FIELD_ID, &mut state.key_form.expiry_days, EXPIRY_FIELD_WIDTH);
        });

        row_separator(ui);
        card_row(ui, |ui| {
            ui.spacing_mut().item_spacing.y = ROW_TEXT_GAP;
            ui.label(theme::semibold(SUBJECT_LABEL, 14.0).color(theme::INK));
            ui.horizontal_wrapped(|ui| {
                let all = state.key_form.subject == SubjectChoice::Everything;
                if choice_button(ui, EVERYTHING_CHOICE, all) {
                    state.key_form.subject = SubjectChoice::Everything;
                }
                for kind in KEY_CATEGORIES {
                    let chosen = state.key_form.subject == SubjectChoice::Category(kind);
                    if choice_button(ui, &kind.label(), chosen) {
                        state.key_form.subject = SubjectChoice::Category(kind);
                    }
                }
                let one = state.key_form.subject == SubjectChoice::OneItem;
                if choice_button(ui, ONE_ITEM_CHOICE, one) {
                    state.key_form.subject = SubjectChoice::OneItem;
                }
            });
        });

        if state.key_form.subject == SubjectChoice::OneItem {
            row_separator(ui);
            control_row(ui, ITEM_ID_LABEL, ITEM_ID_DESCRIPTION, |ui| {
                form_field(ui, ITEM_FIELD_ID, &mut state.key_form.item_id, NAME_FIELD_WIDTH);
            });
        }

        row_separator(ui);
        card_row(ui, |ui| {
            ui.spacing_mut().item_spacing.y = ROW_TEXT_GAP;
            ui.label(theme::semibold(ACCESS_LABEL, 14.0).color(theme::INK));
            ui.horizontal(|ui| {
                if choice_button(ui, READ_CHOICE, state.key_form.read) {
                    state.key_form.read = !state.key_form.read;
                }
                if choice_button(ui, WRITE_CHOICE, state.key_form.write) {
                    state.key_form.write = !state.key_form.write;
                }
            });
        });

        row_separator(ui);
        card_row(ui, |ui| {
            mint = key_button(ui, MINT_BUTTON, MINT_BUTTON_WIDTH);
        });

        if let Some(message) = &state.key_message {
            row_separator(ui);
            card_row(ui, |ui| {
                ui.label(RichText::new(message).size(12.0).color(theme::ERROR));
            });
        }
    });

    if !mint {
        return;
    }
    match attempt_mint(&state.key_form, (state.key_clock)(), state.key_random, &state.keys) {
        MintOutcome::Refused(message) => {
            // **Nothing else moves.** The form keeps everything that was
            // typed into it, so a refused name is one edit away from a good
            // one rather than a form to fill in again.
            state.key_message = Some(message);
        }
        MintOutcome::Minted(record, minted) => {
            state.keys.push(record);
            let sink = state.keys_sink;
            state.key_message = match sink(&state.keys) {
                Ok(()) => None,
                Err(error) => Some(format!("{MINT_NOT_SAVED}{error}")),
            };
            // The plaintext goes here and nowhere else. It is not logged, not
            // put on the record, and not written to the store.
            state.minted = Some(minted);
            state.key_form = KeyForm::default();
        }
    }
}

/// A single-line text box in the trailing control column.
///
/// A stable `id` for [`STEPPER_FIELD_ID`]'s reason: focus and the caret live
/// in egui's memory under the widget's id, and an id derived from layout
/// position is lost the moment a row above changes height -- which on this
/// page happens whenever the item-id row appears.
fn form_field(ui: &mut Ui, id: &str, buffer: &mut String, width: f32) {
    let (outer, _) = ui.allocate_exact_size(Vec2::new(width, STEPPER_HEIGHT), Sense::hover());
    ui.painter().rect(
        outer,
        CornerRadius::same(STEPPER_RADIUS),
        theme::CARD,
        Stroke::new(1.0, theme::BORDER_STRONG),
        StrokeKind::Inside,
    );
    ui.put(
        outer.shrink(5.0),
        egui::TextEdit::singleline(buffer)
            .id(egui::Id::new(id))
            .frame(egui::Frame::new())
            .font(FontId::new(12.0, FontFamily::Proportional))
            .vertical_align(egui::Align::Center)
            .margin(Margin::ZERO),
    );
}

/// One cell of a picker: [`key_button`]'s box, filled when it is the choice
/// in force.
///
/// Selected state is the nav's own language (`BLUE_WASH` behind
/// `BLUE_DEEP`), rather than a new colour, so "this is the one that is on"
/// looks the same here as it does in the column to the left.
fn choice_button(ui: &mut Ui, label: &str, selected: bool) -> bool {
    let galley_width = ui
        .painter()
        .layout_no_wrap(
            label.to_owned(),
            FontId::new(12.0, FontFamily::Name(theme::SEMIBOLD.into())),
            theme::INK,
        )
        .size()
        .x;
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(galley_width + CHOICE_PADDING, STEPPER_HEIGHT),
        Sense::click(),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let (fill, ink) = if selected {
        (theme::BLUE_WASH, theme::BLUE_DEEP)
    } else if response.hovered() {
        (theme::CANVAS, theme::TEXT_SECONDARY)
    } else {
        (theme::CARD, theme::TEXT_SECONDARY)
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(STEPPER_RADIUS),
        fill,
        Stroke::new(1.0, if selected { theme::BLUE_EDGE } else { theme::BORDER_STRONG }),
        StrokeKind::Inside,
    );
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::new(12.0, FontFamily::Name(theme::SEMIBOLD.into())),
        ink,
    );
    ui.painter().galley(
        Pos2::new(
            rect.center().x - galley.size().x / 2.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        ink,
    );
    response.clicked()
}

/// [`choice_button`]'s disabled twin, in [`control_row_ghosted`]'s idiom.
///
/// A separate function rather than an `enabled` parameter on
/// [`choice_button`], which is exactly the shape
/// [`theme::toggle_pill_disabled`] and [`row_text_ghosted`] already take on
/// this page: the three live call sites in the mint form have no disabled
/// state and gain nothing from carrying one.
///
/// **Disabled means disabled, not merely painted grey** -- [`child_toggle_row`]'s
/// rule, on the control that replaced it here. The cell senses no click and
/// sets no hover cursor, so there is no path by which it can be pressed.
///
/// **The cell in force is still legible.** It keeps [`theme::BLUE_WASH`], the
/// same "this is the one that is on" the live control uses, while both labels
/// go to [`theme::TEXT_GHOST`]. A picker that greyed both cells identically
/// would tell a user on `bitwarden.com` that they have no client, when what
/// is true is that they have this one and cannot change it.
fn choice_button_ghosted(ui: &mut Ui, label: &str, selected: bool) {
    let galley_width = ui
        .painter()
        .layout_no_wrap(
            label.to_owned(),
            FontId::new(12.0, FontFamily::Name(theme::SEMIBOLD.into())),
            theme::INK,
        )
        .size()
        .x;
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(galley_width + CHOICE_PADDING, STEPPER_HEIGHT),
        Sense::hover(),
    );
    ui.painter().rect(
        rect,
        CornerRadius::same(STEPPER_RADIUS),
        if selected { theme::BLUE_WASH } else { theme::CARD },
        Stroke::new(1.0, theme::BORDER_STRONG),
        StrokeKind::Inside,
    );
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::new(12.0, FontFamily::Name(theme::SEMIBOLD.into())),
        theme::TEXT_GHOST,
    );
    ui.painter().galley(
        Pos2::new(
            rect.center().x - galley.size().x / 2.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        theme::TEXT_GHOST,
    );
}

/// **Which client opens this vault: a two-cell picker, not an on/off pill.**
///
/// Returns the value after this frame -- `true` for the official CLI, which
/// is `Account::use_official_bw_crypto`'s own direction, so nothing between
/// this row and [`crate::backend_policy::choose`] has to invert anything.
///
/// # Why a picker and not the switch it replaces
///
/// The owner's instruction was to name both sides. A switch labelled "Use the
/// official Bitwarden CLI" makes the built-in client the unnamed absence of
/// something: it has no label, no position of its own, and a user who wants it
/// has to infer that "off" is a second product rather than a feature being
/// declined. The two cells are the same shape as each other, and the label
/// above them ([`BACKEND_CHOICE_LABEL`]) is the question rather than one of
/// the answers.
///
/// # It reuses [`choice_button`] rather than inventing a control
///
/// This page already has an either-or control and already draws it twice, on
/// this very section's sibling card: the mint form's subject and access
/// pickers ([`draw_mint_form`]). Selected is `BLUE_WASH` behind `BLUE_DEEP`,
/// which is the nav's own language for "this is the one in force". A new
/// widget here would have been a third dialect of "selected" in one window.
///
/// The layout is that card's too -- a full-width [`card_row`] with the label
/// and copy above the cells, not [`control_row`]'s 160-point trailing column,
/// which neither cell's name fits in.
///
/// # Ghosted, it hands back what it was given
///
/// [`child_toggle_row`]'s contract, kept exactly: on an account the built-in
/// client cannot serve, both cells are inert, the row says why in the row, and
/// the returned value is the stored one -- so a click writes nothing.
fn backend_choice_row(ui: &mut Ui, description: &str, official: bool, enabled: bool) -> bool {
    let mut next = official;
    card_row(ui, |ui| {
        ui.spacing_mut().item_spacing.y = ROW_TEXT_GAP;
        let (title, body) = if enabled {
            (theme::INK, theme::TEXT_FAINT)
        } else {
            (theme::TEXT_GHOST, theme::TEXT_GHOST)
        };
        ui.label(theme::semibold(BACKEND_CHOICE_LABEL, 14.0).color(title));
        ui.label(RichText::new(description).size(12.0).color(body));
        ui.horizontal(|ui| {
            if enabled {
                // Two independent presses rather than one flip, so that
                // pressing the cell already in force is a no-op and
                // `backend_switch` sees no proposal -- which is what keeps a
                // confirmation off the screen of a user who clicked the
                // client they were already on.
                if choice_button(ui, OFFICIAL_CHOICE, official) {
                    next = true;
                }
                if choice_button(ui, BUILT_IN_CHOICE, !official) {
                    next = false;
                }
            } else {
                choice_button_ghosted(ui, OFFICIAL_CHOICE, official);
                choice_button_ghosted(ui, BUILT_IN_CHOICE, !official);
            }
        });
    });
    next
}

/// This page's button: [`scan_button`]'s box at a width its own label needs.
///
/// A separate function rather than a parameter on `scan_button`, because that
/// one carries an `enabled` state this page has no use for -- every button
/// here is live whenever it is drawn, and the states that would grey one out
/// (a form that cannot mint) are said in words instead.
fn key_button(ui: &mut Ui, label: &str, width: f32) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, STEPPER_HEIGHT), Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().rect(
        rect,
        CornerRadius::same(STEPPER_RADIUS),
        if response.hovered() { theme::CANVAS } else { theme::CARD },
        Stroke::new(1.0, theme::BORDER_STRONG),
        StrokeKind::Inside,
    );
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::new(12.0, FontFamily::Name(theme::SEMIBOLD.into())),
        theme::TEXT_SECONDARY,
    );
    ui.painter().galley(
        Pos2::new(
            rect.center().x - galley.size().x / 2.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        theme::TEXT_SECONDARY,
    );
    response.clicked()
}

fn draw_clipboard(ui: &mut Ui, state: &mut PrefsState) {
    // **The whole page's enable state is one value, read once**, and it comes
    // from `settings::clipboard_clearing` -- the same pure function the
    // clipboard module is configured from. There is no second opinion here
    // about what the master switch means: if this page and that module could
    // disagree, the page would be the thing lying.
    let live = state.settings.clipboard_clearing();
    let children_enabled = state.settings.clear_clipboard;
    debug_assert_eq!(
        children_enabled,
        live.interval_is_live(),
        "the page and `clipboard_clearing` disagree about the master switch"
    );

    card(ui, |ui| {
        // The master switch first, and always live: it is the one control on
        // this page nothing else can grey out.
        state.settings.clear_clipboard = toggle_row(
            ui,
            CLIPBOARD_MASTER_LABEL,
            CLIPBOARD_MASTER_DESCRIPTION,
            state.settings.clear_clipboard,
        );
        row_separator(ui);
        // The three triggers, in the order they appear in a session: the
        // vault locks, the account changes, the app quits. Not in order of
        // how likely they are to be turned off, which nobody knows.
        state.settings.clear_clipboard_on_lock = child_toggle_row(
            ui,
            CLIPBOARD_ON_LOCK_LABEL,
            CLIPBOARD_ON_LOCK_DESCRIPTION,
            state.settings.clear_clipboard_on_lock,
            children_enabled,
        );
        row_separator(ui);
        state.settings.clear_clipboard_on_account_change = child_toggle_row(
            ui,
            CLIPBOARD_ON_ACCOUNT_LABEL,
            CLIPBOARD_ON_ACCOUNT_DESCRIPTION,
            state.settings.clear_clipboard_on_account_change,
            children_enabled,
        );
        row_separator(ui);
        state.settings.clear_clipboard_on_quit = child_toggle_row(
            ui,
            CLIPBOARD_ON_QUIT_LABEL,
            CLIPBOARD_ON_QUIT_DESCRIPTION,
            state.settings.clear_clipboard_on_quit,
            children_enabled,
        );
        row_separator(ui);
        // The interval last of the four, because it is the weakest of them --
        // the three above are moments the user has *said* they are finished,
        // and this one is a guess. `clipboard.rs`'s own doc ranks them the
        // same way and for the same reason.
        interval_row(ui, state, children_enabled);
    });

    // **A second card, not a fifth row.** The reset button acts on the card
    // above rather than sitting in it, and a row inside it would read as a
    // fifth setting -- one that is somehow always on. The note about clipboard
    // history joins it because it is the other thing on this page that is not
    // a setting.
    card(ui, |ui| {
        note_row(ui, CLIPBOARD_HISTORY_LABEL, CLIPBOARD_HISTORY_NOTE);
        row_separator(ui);
        // Always live, including while the master switch is off: "put this
        // page back" is exactly what a user who has switched everything off
        // and changed their mind wants, and a reset button that greyed out
        // with the thing it resets would be unreachable from the state you
        // most want to leave.
        control_row(ui, CLIPBOARD_RESET_LABEL, CLIPBOARD_RESET_DESCRIPTION, |ui| {
            if reset_button(ui) {
                // The whole of the button's behaviour is one pure function on
                // `Settings`, so "this resets the section and nothing else" is
                // a property a test constructs rather than one it observes by
                // clicking. See `Settings::with_default_clipboard_clearing`.
                state.settings = state.settings.with_default_clipboard_clearing();
                // The field's buffer and any refusal message follow the value,
                // or the row would keep showing the number the user just
                // reset away from.
                state.clipboard_interval_text =
                    ClearInterval::from_seconds(state.settings.clear_clipboard_seconds)
                        .as_minutes_text();
                state.clipboard_entry_error = None;
            }
        });
    });
}

/// The interval row: the field, and under it whatever the last refused entry
/// has to be told to the user.
///
/// The message is drawn *inside* the row's text column rather than as a row of
/// its own, so it appears attached to the field it is about and the card does
/// not change height by a whole row when an entry is refused.
fn interval_row(ui: &mut Ui, state: &mut PrefsState, enabled: bool) {
    let description = state
        .clipboard_entry_error
        .map_or(CLIPBOARD_INTERVAL_DESCRIPTION, |error| error);
    // Two calls rather than a function pointer chosen up front: the two row
    // helpers take a closure, and a `let row = if .. { control_row } else
    // { control_row_ghosted }` cannot be given a type general enough over
    // the closure's lifetime.
    if enabled {
        control_row(ui, CLIPBOARD_INTERVAL_LABEL, description, |ui| {
            interval_field(ui, state, true);
        });
    } else {
        control_row_ghosted(ui, CLIPBOARD_INTERVAL_LABEL, description, |ui| {
            interval_field(ui, state, false);
        });
    }
}

/// The text field itself, in the same box `minutes_stepper` paints -- but
/// without the `-`/`+` cells.
///
/// **No stepper here, and that is a decision rather than an omission.** The
/// auto-lock control steps in whole minutes and its range is unbounded above,
/// so `-`/`+` is the natural way to move it. This range is 0.5 to 60 in tenths
/// of a minute: 596 steps, which no one is going to press their way across,
/// and a `+` from 0.5 would land on 0.6, a value almost nobody wants. Typing
/// is the operation this field is for, so typing is the whole of it.
fn interval_field(ui: &mut Ui, state: &mut PrefsState, enabled: bool) {
    let (outer, _) = ui.allocate_exact_size(
        Vec2::new(STEPPER_VALUE_WIDTH + STEPPER_STEP_WIDTH, STEPPER_HEIGHT),
        Sense::hover(),
    );
    // The same two greys `minutes_stepper` uses for its disabled state, so the
    // two numeric controls in this window are disabled in the same visual
    // language. No new colour.
    let (fill, border) = if enabled {
        (theme::CARD, theme::BORDER_STRONG)
    } else {
        (theme::CANVAS, theme::HAIRLINE)
    };
    ui.painter().rect(
        outer,
        CornerRadius::same(STEPPER_RADIUS),
        fill,
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );

    let committed = ClearInterval::from_seconds(state.settings.clear_clipboard_seconds);
    if !enabled {
        // A painted galley, not a read-only `TextEdit`, for the reason
        // `minutes_stepper` gives at length: egui's read-only text edit still
        // takes focus, still shows a caret and still accepts a click, which is
        // precisely the "greyed out but secretly live" state this must not be.
        // There is no widget here at all.
        let galley = ui.painter().layout_no_wrap(
            committed.as_minutes_text(),
            FontId::new(12.0, FontFamily::Proportional),
            theme::TEXT_GHOST,
        );
        ui.painter().galley(
            Pos2::new(
                outer.center().x - galley.size().x / 2.0,
                outer.center().y - galley.size().y / 2.0,
            ),
            galley,
            theme::TEXT_GHOST,
        );
        // Kept in step with the committed value while the control is off, so
        // turning the master switch back on hands the live field the number
        // that has been on screen all along rather than a stale fragment.
        state.clipboard_interval_text = committed.as_minutes_text();
        return;
    }

    // Placed by hand from the same `layout_no_wrap` measurement the greyed
    // branch uses, so the two branches agree BY CONSTRUCTION rather than by
    // coincidence -- see `minutes_stepper`, where the 6pt disagreement between
    // a centred `TextEdit` and a centred galley was the bug report.
    let text_width = ui
        .painter()
        .layout_no_wrap(
            state.clipboard_interval_text.clone(),
            FontId::new(12.0, FontFamily::Proportional),
            theme::INK,
        )
        .size()
        .x;
    let inner = outer.shrink(4.0);
    let entry = ui.put(
        Rect::from_min_max(
            Pos2::new(outer.center().x - text_width / 2.0, inner.min.y),
            inner.max,
        ),
        egui::TextEdit::singleline(&mut state.clipboard_interval_text)
            .id(egui::Id::new(INTERVAL_FIELD_ID))
            .frame(egui::Frame::new())
            .font(FontId::new(12.0, FontFamily::Proportional))
            .horizontal_align(egui::Align::Min)
            .vertical_align(egui::Align::Center)
            .margin(Margin::ZERO),
    );

    // **Committed on losing focus, and only then.** Judging every keystroke
    // would refuse `0.` on the way to `0.5` and refuse an empty field the
    // moment the user selected all and started again.
    if entry.lost_focus() {
        match parse_clipboard_minutes(&state.clipboard_interval_text) {
            ClipboardEntry::Accepted(interval) => {
                state.settings.clear_clipboard_seconds = interval.seconds();
                state.clipboard_entry_error = None;
                // Normalised, so `1,5` and `1.50` come back as `1.5` -- the
                // field then shows the value in the form the app stores it,
                // and the two cannot appear to disagree.
                state.clipboard_interval_text = interval.as_minutes_text();
            }
            // **A refusal leaves the committed value exactly where it was**
            // and puts the reason under the field. The text is deliberately
            // NOT reverted: the user is looking at what they typed while
            // reading why it was not taken, and silently replacing it would
            // hide the thing being explained.
            ClipboardEntry::NotANumber => {
                state.clipboard_entry_error = Some(CLIPBOARD_ENTRY_NOT_A_NUMBER);
            }
            ClipboardEntry::BelowFloor => {
                state.clipboard_entry_error = Some(CLIPBOARD_ENTRY_BELOW_FLOOR);
            }
            ClipboardEntry::AboveCeiling => {
                state.clipboard_entry_error = Some(CLIPBOARD_ENTRY_ABOVE_CEILING);
            }
            ClipboardEntry::BetweenSteps => {
                state.clipboard_entry_error = Some(CLIPBOARD_ENTRY_BETWEEN_STEPS);
            }
        }
    }
}

/// The Reset button: 3e's segmented-control box at button size, with the
/// label centred in it. Built from 3e's own parts, like the stepper, since
/// 3e's only button of this shape is its "+ Add app".
fn reset_button(ui: &mut Ui) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(RESET_BUTTON_WIDTH, STEPPER_HEIGHT), Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().rect(
        rect,
        CornerRadius::same(STEPPER_RADIUS),
        if response.hovered() { theme::CANVAS } else { theme::CARD },
        Stroke::new(1.0, theme::BORDER_STRONG),
        StrokeKind::Inside,
    );
    let galley = ui.painter().layout_no_wrap(
        CLIPBOARD_RESET_BUTTON.to_owned(),
        FontId::new(12.0, FontFamily::Name(theme::SEMIBOLD.into())),
        theme::TEXT_SECONDARY,
    );
    ui.painter().galley(
        Pos2::new(
            rect.center().x - galley.size().x / 2.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        theme::TEXT_SECONDARY,
    );
    response.clicked()
}

/// **The fill hotkey: what it is, and whether it is actually working.**
///
/// # Why it is a row on General and not a page of its own
///
/// It was the entire content of a `Shortcuts` section, and that section was
/// removed with the other four that had nothing on them a user could change.
/// A read-only chip is not a preference, and a nav row that leads to one is a
/// row that spends a click to say "here is a thing you cannot alter".
///
/// But the chip itself had to be kept, because it is the only place in the
/// app that names the chord: **a user who cannot discover CTRL+ALT+B cannot
/// use the feature at all.** General is where it went, directly under
/// `PROMPT_LABEL`'s row, whose description already ends by saying the hotkey
/// is what is left when the prompt is off. The two were always one thought
/// split across two pages; they are one card now. General is also the page
/// this window opens on, which is the difference between a discoverable
/// shortcut and a documented one.
///
/// # Why it says so here and not in a startup dialog
///
/// The difference matters. A
/// shortcut another program got to first is a degraded convenience, not a
/// failure to start: everything else Deskwarden does works. A modal at launch
/// over a keyboard chord would interrupt every single launch for as long as
/// the conflict lasted, would arrive before the user had asked anything, and
/// would be the second most annoying thing this app could do after vanishing.
///
/// But it must be said *somewhere*, because a shortcut that silently does
/// nothing is its own confusing failure -- the user presses CTRL+ALT+B, some
/// other program answers or nothing does, and Deskwarden looks broken. General
/// is the page a user comes to in order to ask that exact question, which
/// makes it the right place for the answer, and it is the same page in both
/// shells (the tray's Preferences window and the vault window's Preferences
/// modal). In the modal, opened from inside the startup vault window, the
/// status is `Unavailable(NotYetAttempted)` -- and it says so rather than
/// claiming the chord works; see [`crate::hotkey::availability`].
///
/// The status is a parameter rather than read from `hotkey::availability()` in
/// here, so that the painting can be driven from a test without a process-wide
/// value another test may be reading at the same moment; [`draw_section`] is
/// the one place that reads it.
///
/// **A row, not a card.** It draws directly into General's card between
/// [`row_separator`]s, exactly as the toggle rows around it do -- a `card` of
/// its own here would put a second white panel inside the first.
fn fill_hotkey_row(ui: &mut Ui, status: crate::hotkey::HotkeyStatus) {
    match status {
        // `kbd_chip`'s grey-on-canvas treatment, not `kbd_chip_on_card`'s: the
        // latter is a *white* chip, made for 3h's blue-washed panel, and it
        // would be invisible on this white card.
        crate::hotkey::HotkeyStatus::Armed => {
            control_row(ui, FILL_HOTKEY_LABEL, FILL_HOTKEY_DESCRIPTION, |ui| {
                theme::kbd_chip(ui, FILL_HOTKEY, false)
            });
        }
        // Ghosted, which is the treatment this file already gives a control
        // that is present and not currently doing anything -- the disabled
        // toggle and the disabled stepper. It reads as "off", which is
        // accurate, where a normal chip would read as "working" and an absent
        // row would read as "this feature does not exist".
        //
        // The reason replaces the description rather than being added under
        // it: the description says the shortcut is the only one and cannot be
        // changed, which is exactly what a user staring at a shortcut that is
        // not working does not need told.
        crate::hotkey::HotkeyStatus::Unavailable(reason) => {
            control_row_ghosted(ui, FILL_HOTKEY_UNAVAILABLE_LABEL, reason.message(), |ui| {
                theme::kbd_chip(ui, FILL_HOTKEY, false)
            });
        }
    }
}

/// The account row: which Bitwarden account this vault is, and where it
/// lives.
///
/// **Never blank in any state**, which is the property the row had when it
/// could say nothing at all and had to keep when it could say something: an
/// empty right-hand column reads as a field that failed to load, and this
/// page is read by someone checking whether their app is working.
fn account_row(ui: &mut Ui, status: Option<AccountStatus>) {
    let (description, value) = account_row_text(status.as_ref());
    match value {
        // Nothing published: the old sentence, in the old shape. A row with
        // no value is honest here because there is no value -- as opposed to
        // a row with an empty one, which claims there should be.
        None => card_row(ui, |ui| row_text(ui, ACCOUNT_LABEL, &description)),
        Some(value) => value_row(ui, ACCOUNT_LABEL, &description, &value),
    }
}

/// What the account row says, as a pure function of what is known.
///
/// Split out so every state's wording is testable without a window, and so
/// the three cases are visible in one place rather than spread through a draw.
fn account_row_text(status: Option<&AccountStatus>) -> (String, Option<String>) {
    match status {
        None => (ACCOUNT_STATUS.to_string(), None),
        Some(AccountStatus::SignedOut) => {
            (ACCOUNT_SIGNED_OUT_NOTE.to_string(), Some(ACCOUNT_SIGNED_OUT.to_string()))
        }
        Some(AccountStatus::SignedIn { email, server }) => {
            // `server_host` is `login_ui`'s, so the address here reads
            // exactly as it does in the login window's footer -- one app
            // naming one server one way -- and `None` renders as Bitwarden's
            // own cloud, which is what the CLI's default means.
            let where_it_lives =
                format!("{ACCOUNT_SERVER_PREFIX}{}.", crate::login_ui::server_host(server.as_deref()));
            match email {
                Some(email) => (where_it_lives, Some(email.clone())),
                None => (
                    format!("{where_it_lives} {ACCOUNT_NO_EMAIL_NOTE}"),
                    Some(ACCOUNT_NO_EMAIL.to_string()),
                ),
            }
        }
    }
}

/// The **About** page: which build this is, and whose account it is reading.
///
/// # Nothing on this page acts
///
/// It used to carry the whole update flow -- a check button, the release
/// notes, Download, a progress bar, Restart to install, and a failure with a
/// retry on it -- under a two-row version card. That is
/// [`Section::Updates`]'s now, entire. What is left is two statements of
/// fact, which is what an About page is for and what
/// [`Section::subtitle`] already claimed this one was.
///
/// # It keeps the version, and the version is not a button
///
/// "Which build am I running" is a fact about this build, and About is where
/// a user asks it; moving it to Updates would have left About with one row
/// and sent anyone looking for a version number to a page named after an
/// action. What did NOT stay is anything that *does* something about that
/// version -- the distinction the whole reorganisation turns on.
///
/// The Updates page deliberately does not restate it. The nav rail paints
/// [`version_line`] on every page including that one (see [`draw_nav`]), so
/// the number is already in front of a user reading the update card; a second
/// row saying it would be a third place for one fact to be stated and the
/// two that can drift.
fn draw_about(ui: &mut Ui, state: &mut PrefsState) {
    card(ui, |ui| {
        value_row(
            ui,
            "Version",
            "Unofficial, and unaffiliated with Bitwarden, Inc.",
            &version_line(),
        );
        row_separator(ui);
        account_row(ui, (state.account_source)());
    });
}

/// The **Updates** page: the automatic-check pill, then the flow it does not
/// govern.
///
/// # Both halves on one page, for [`draw_breaches`]'s reason
///
/// The pill was the last row on General and the flow was a card on About, so
/// a user weighing "should this app talk to GitHub by itself" was reading a
/// switch whose consequence was two pages away, and a user pressing *Check
/// for updates* was acting on a rule they could not see. The two cards below
/// are the same arrangement Breaches settled two days ago: the switch, the
/// control it does **not** gate, and the sentence saying so
/// ([`UPDATE_AUTOMATIC_OFF_NOTE`]) all in one glance.
///
/// # The button is not gated on the pill, and the page says so
///
/// Unchanged, and it is the point. `Settings::check_for_updates` governs what
/// Deskwarden asks **on its own**; a click on the button is the user making
/// the request in the same breath as consenting to it. That was already the
/// rule -- it is the rule `breach_scan` cites for its own button -- and it is
/// now arguable in front of the switch rather than about a switch on another
/// page.
fn draw_updates(ui: &mut Ui, state: &mut PrefsState) {
    card(ui, |ui| {
        // **Moved off General, whole.** It was the last row there,
        // deliberately apart from the two vault-keyed network rows because
        // this request is keyed on nothing but the app's own version. That
        // separation is now the page break, which says it louder.
        state.settings.check_for_updates = toggle_row(
            ui,
            UPDATE_CHECK_LABEL,
            UPDATE_CHECK_DESCRIPTION,
            state.settings.check_for_updates,
        );
    });

    draw_update_card(ui, state);
}

/// The update flow, as a card of its own below the setting.
///
/// **Every frame starts by draining the worker channel.** This is the whole
/// answer to "how does a page inside a blocking event loop hear back from a
/// background thread": it does not need `main.rs`'s loop, because it has its
/// own frames and `pump` never blocks in one.
fn draw_update_card(ui: &mut Ui, state: &mut PrefsState) {
    use crate::update_panel::UpdateStage;

    state.update.pump();
    if state.update.is_busy() {
        // egui repaints on input. A download nobody is typing over would
        // otherwise advance its bar only when the mouse moved, which is a
        // progress bar that reports the user rather than the transfer.
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }

    card(ui, |ui| {
        // The stage is cloned out before drawing, so the buttons below are
        // free to replace it without borrowing `state.update` across the
        // closure that draws from it.
        let stage = state.update.stage().clone();

        let (description, button) = match &stage {
            UpdateStage::Idle => (UPDATE_IDLE_DESCRIPTION.to_string(), Some(UPDATE_CHECK_BUTTON)),
            UpdateStage::Checking => (UPDATE_CHECKING_DESCRIPTION.to_string(), None),
            UpdateStage::UpToDate => {
                (UPDATE_UP_TO_DATE_DESCRIPTION.to_string(), Some(UPDATE_CHECK_BUTTON))
            }
            UpdateStage::Available(r) => {
                (format!("Version {} is available.", r.version), Some(UPDATE_DOWNLOAD_BUTTON))
            }
            UpdateStage::Downloading { release, .. } => {
                (format!("Downloading version {}.", release.version), None)
            }
            UpdateStage::Ready(r) => {
                (format!("Version {} is ready. {UPDATE_READY_DESCRIPTION}", r.version), Some(UPDATE_RESTART_BUTTON))
            }
            // The message comes from `updater` and is the reason as that
            // module saw it. Shown rather than reduced to "something went
            // wrong": this app has no console, and the failures here are
            // overwhelmingly network ones the user can act on.
            // The retry is offered whichever half failed. What it retries
            // differs -- see the click handler below -- but "try again" is
            // the right word for both, and a failure with no way forward is
            // a dead end on the only page that has one.
            UpdateStage::Failed { message, .. } => {
                (format!("Update failed: {message}"), Some(UPDATE_RETRY_BUTTON))
            }
            UpdateStage::Unavailable => (UPDATE_UNAVAILABLE_DESCRIPTION.to_string(), None),
        };

        // The busy stages still draw a button, disabled and labelled with what
        // is happening, rather than drawing nothing: a row whose control
        // vanishes mid-action reflows the card under the cursor.
        let busy_label = match &stage {
            UpdateStage::Checking => Some(UPDATE_CHECKING_BUTTON),
            UpdateStage::Downloading { .. } => Some(UPDATE_DOWNLOADING_BUTTON),
            _ => None,
        };

        let mut clicked = false;
        control_row(ui, UPDATE_SECTION_LABEL, &description, |ui| {
            if let Some(label) = busy_label {
                let _ = update_button(ui, label, false);
            } else if let Some(label) = button {
                clicked = update_button(ui, label, true);
            }
        });

        if clicked {
            match &stage {
                UpdateStage::Idle | UpdateStage::UpToDate => state.update.begin_check(),
                UpdateStage::Available(_) => state.update.begin_download(),
                UpdateStage::Ready(_) => state.update.install_now(),
                // Retry means "retry the thing that failed". A failure that
                // still remembers a release failed at the download, so the
                // download is what is retried; one that does not failed at the
                // check, and there is nothing to fetch yet.
                UpdateStage::Failed { release: Some(_), .. } => state.update.begin_download(),
                UpdateStage::Failed { release: None, .. } => state.update.begin_check(),
                UpdateStage::Checking
                | UpdateStage::Downloading { .. }
                | UpdateStage::Unavailable => {}
            }
        }

        // Named only when it is off, and only on the stages where the button
        // is the thing being explained. Repeating it under a progress bar
        // would be explaining a decision the user already made.
        if !state.settings.check_for_updates
            && matches!(stage, UpdateStage::Idle | UpdateStage::UpToDate)
        {
            row_separator(ui);
            card_row(ui, |ui| {
                ui.label(
                    RichText::new(UPDATE_AUTOMATIC_OFF_NOTE)
                        .size(12.0)
                        .color(theme::TEXT_FAINT),
                );
            });
        }

        if let UpdateStage::Downloading { done, total, .. } = &stage {
            row_separator(ui);
            card_row(ui, |ui| progress_bar(ui, *done, *total));
        }

        // The notes follow the release through the download and the restart
        // prompt, not just the moment it is found: a user who has started a
        // download is still entitled to read what they are installing.
        let notes_for = match &stage {
            UpdateStage::Available(r) | UpdateStage::Ready(r) => Some(r),
            UpdateStage::Downloading { release, .. } => Some(release),
            _ => None,
        };
        if let Some(release) = notes_for {
            row_separator(ui);
            card_row(ui, |ui| release_notes(ui, &release.body));
        }
    });
}

/// The notes region: a heading, then the release body rendered through the
/// bounded Markdown subset in `updater`, inside a scroll area bounded by the
/// page's remaining height.
///
/// Two things about it worth stating here:
///
/// * **The only clickable thing is an `https` link's words**, and even that
///   is a recent reversal of a rule that made this region entirely inert.
///   What the rule was, why it was lifted, and what survived the lifting are
///   recorded in `updater`'s subset header, next to the parser that now
///   decides which links are followable -- the decision is made there so a
///   refused URL is never carried as far as this function. `notes_line`
///   below is the half that turns an accepted one into a click.
/// * It does not size itself to its content. Its ceiling is
///   [`notes_max_height`] -- the space the page still has, never what the
///   notes want -- and the overflow scrolls, so no release body can push the
///   buttons above it off a window that cannot be resized.
///
/// The body has already been through `updater::release_notes_blocks`, which
/// runs `release_notes_for_display` -- control characters, bidi overrides,
/// zero-widths, and the length bound -- BEFORE it looks at a single markup
/// character, and whose spans are all slices of that cleaned string. Called
/// here rather than at parse time so the untouched body stays on
/// `ReleaseInfo`: there is exactly one place that decides what is safe to
/// paint, and this is its only caller.
fn release_notes(ui: &mut Ui, body: &str) {
    let blocks = crate::updater::release_notes_blocks(body);
    let shown = crate::updater::release_notes_for_display(body);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = ROW_TEXT_GAP;
        ui.label(theme::semibold(UPDATE_NOTES_LABEL, 14.0).color(theme::INK));
        if shown.is_empty() {
            ui.label(RichText::new(UPDATE_NOTES_EMPTY).size(12.0).color(theme::TEXT_FAINT));
            return;
        }
        // egui's default scrollbar is painted in its own light grey, which on
        // this card's white is very nearly nothing. Given a real width and
        // the page's own border colour, so the cue that there is more to read
        // is a cue someone can see.
        //
        // **Floating, with the lane allocated** -- the item list's and the
        // health pane's arrangement (`theme::scrollbar_in_gutter`), reached
        // here from the other side. This region used to pin a NON-floating
        // bar open and subtract its width from the content by hand, and that
        // hand-subtraction is what made the notes that fit and the notes that
        // scroll indistinguishable: a non-floating bar's paint cannot be
        // suppressed without also giving up its lane. `floating_allocated_
        // width` reserves the lane from `AlwaysVisible` alone, unconditionally
        // and independently of whether anything is drawn in it, which is what
        // lets `notes_fit` below change the paint and nothing else.
        ui.spacing_mut().scroll.floating = true;
        ui.spacing_mut().scroll.floating_width = UPDATE_NOTES_BAR_WIDTH;
        ui.spacing_mut().scroll.floating_allocated_width =
            UPDATE_NOTES_BAR_WIDTH + UPDATE_NOTES_BAR_MARGIN * 2.0;
        ui.spacing_mut().scroll.bar_width = UPDATE_NOTES_BAR_WIDTH;
        ui.spacing_mut().scroll.bar_inner_margin = UPDATE_NOTES_BAR_MARGIN;
        ui.spacing_mut().scroll.bar_outer_margin = 0.0;
        ui.visuals_mut().widgets.inactive.bg_fill = theme::BORDER_STRONG;
        ui.visuals_mut().widgets.hovered.bg_fill = theme::TEXT_SECONDARY;
        ui.visuals_mut().widgets.active.bg_fill = theme::TEXT_SECONDARY;
        ui.visuals_mut().extreme_bg_color = theme::CANVAS;
        if notes_fit(ui) {
            // Notes that fit paint no bar at all: a line down the side of a
            // region with nothing to scroll points at nothing. Only the PAINT
            // is suppressed -- see `notes_fit`.
            theme::hide_scrollbar(ui);
        } else {
            // **Fully opaque, not egui's dormant defaults.** A floating bar
            // is normally faint until the pointer comes near it, which is the
            // same "cue behind an action nobody takes" the visibility comment
            // below rejects, only spelled in alpha rather than in a mode.
            // Clipped notes get a bar at full strength with the pointer
            // nowhere near the card.
            let scroll = &mut ui.spacing_mut().scroll;
            scroll.dormant_background_opacity = 1.0;
            scroll.active_background_opacity = 1.0;
            scroll.interact_background_opacity = 1.0;
            scroll.dormant_handle_opacity = 1.0;
            scroll.active_handle_opacity = 1.0;
            scroll.interact_handle_opacity = 1.0;
        }
        // Measured HERE, after the heading and immediately before the region
        // that will consume it, because that is the only moment the number
        // means "what is left". Read a row earlier and it would include the
        // heading; read outside the card row it would include the card's own
        // bottom padding, which `card_row`'s frame has already taken off.
        let ceiling = notes_max_height(ui);
        let scrolled = egui::ScrollArea::vertical()
            .max_height(ceiling)
            .auto_shrink([false, true])
            // **Always visible, never on hover.** Long notes are clipped at
            // that ceiling, and clipped text with no scrollbar reads
            // as text that failed to load rather than as text that continues.
            // egui's default hides the bar until the pointer is inside the
            // region, which puts the only cue that there is more to read
            // behind an action nobody takes without the cue.
            //
            // This stays `AlwaysVisible` for the notes that fit as well, and
            // that is the load-bearing half: it is also what makes egui
            // RESERVE the bar's lane. On the default `VisibleWhenNeeded` the
            // reservation would come and go with the content, and this card
            // would change width as the notes got longer -- the defect
            // `password_health` was just fixed for, in mirror image.
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .show(ui, |ui| {
                // The lane is already reserved out of `available_width` by
                // `floating_allocated_width` above, so this is the content's
                // full width and nothing is subtracted from it a second time.
                // Subtracting again is what would leave this card's right
                // edge a few points shy of the Version card's directly above
                // it -- the mismatch the bar constants' comment warns about,
                // in the direction nobody notices until they measure.
                ui.set_width(ui.available_width());
                notes_body(ui, &blocks);
            });
        remember_notes_overflow(
            ui,
            scrolled.content_size.y > scrolled.inner_rect.height() + 0.5,
        );
    });
}

/// Paints the parsed release notes, one line per block.
///
/// **One galley per line, still, and no `TextEdit` anywhere.** The styled
/// runs go into a single `LayoutJob` rather than into per-span widgets, which
/// is what lets a line wrap as one piece of prose -- a row of widgets in a
/// horizontal layout would break at widget boundaries instead of at word
/// ones, and a bullet's continuation would no longer line up under its own
/// text. That was always the reason for the `LayoutJob`; it did not stop
/// being the reason when links became clickable. What [`notes_line`] adds is
/// a hit rectangle over the part of the already-laid-out galley that a link's
/// words occupy, which changes nothing about how the text is measured or
/// where it breaks.
///
/// The block index is passed down as the line number, because the interaction
/// ids below have to be unique across a body that can name the same URL
/// twice.
fn notes_body(ui: &mut Ui, blocks: &[crate::updater::NotesBlock]) {
    use crate::updater::NotesBlock;

    // Lines of one body sit at a line's distance from each other, not at the
    // card's row spacing: this is prose, and `ROW_TEXT_GAP` between every
    // line of a bulleted list reads as a list of paragraphs.
    ui.spacing_mut().item_spacing.y = UPDATE_NOTES_LINE_GAP;
    for (n, block) in blocks.iter().enumerate() {
        // A heading opens a section, so it gets the paragraph's air above it
        // -- except at the very top, where there is nothing to be separated
        // from and the gap would just push the notes down the region.
        let opening = n == 0;
        match block {
            // The paragraph break, painted as the space it is. Without this
            // the blank line between two sections of a release body would
            // vanish and the sections would run together -- which is half of
            // what "new lines seems like [gone]" was reporting.
            NotesBlock::Blank => ui.add_space(UPDATE_NOTES_PARAGRAPH_GAP),
            NotesBlock::Heading { level, spans } => {
                if !opening {
                    ui.add_space(UPDATE_NOTES_PARAGRAPH_GAP);
                }
                notes_line(ui, spans, 0.0, Some(*level), n);
            }
            NotesBlock::Bullet { depth, spans } => {
                // The glyph joins the line's own runs rather than being a
                // second widget beside them, so a bullet whose text wraps
                // wraps under its own text and not under the glyph.
                let mut with_glyph = vec![crate::updater::NotesSpan {
                    text: UPDATE_NOTES_BULLET_GLYPH.to_string(),
                    style: crate::updater::NotesStyle::Plain,
                    link: None,
                }];
                with_glyph.extend_from_slice(spans);
                let inset = *depth as f32 * UPDATE_NOTES_BULLET_STEP;
                notes_line(ui, &with_glyph, inset, None, n);
            }
            NotesBlock::Paragraph { spans } => notes_line(ui, spans, 0.0, None, n),
        }
    }
}

/// One line: its runs laid out into a single wrapped galley, inset for a
/// bullet's depth.
///
/// A bullet's glyph and its text share the galley rather than being two
/// widgets side by side, so a bullet whose text wraps wraps under itself the
/// way the rest of the line does. `wrap.max_width` is set from the width the
/// region actually has, minus the inset -- not from a constant, because this
/// card is drawn at two different widths by the window shell and the vault
/// modal.
///
/// # The clickable part
///
/// A span the parser accepted as an `https` link (`NotesSpan::link` is
/// `Some`) gets a hit rectangle over the glyphs its words occupy, and a click
/// there opens the URL through `vault_window::webbrowser_open`. The rectangle
/// is derived from the galley AFTER layout rather than by laying the words
/// out separately, so the link's extent is by construction the extent of the
/// painted words -- a second measurement could disagree with the first and
/// put a click target beside the thing it is meant to be on. A link that
/// wrapped across rows gets one rectangle per row for the same reason: the
/// union of two rows' boxes would cover the empty right-hand end of the first
/// row and the empty left-hand start of the second, neither of which has the
/// link's words in it.
///
/// Nothing about the LAYOUT changes: the rectangles are interacted with, not
/// allocated, so they cannot move the text they sit on.
fn notes_line(
    ui: &mut Ui,
    spans: &[crate::updater::NotesSpan],
    inset: f32,
    heading: Option<u8>,
    line: usize,
) {
    use crate::updater::NotesStyle;
    use egui::text::{LayoutJob, TextFormat};
    use egui::{FontFamily, FontId};

    if spans.is_empty() {
        return;
    }

    let mut job = LayoutJob::default();
    job.wrap.max_width = (ui.available_width() - inset).max(1.0);

    // Char ranges into the finished job's text, one per followable link, in
    // the order they were appended. Chars rather than bytes because that is
    // what a galley's rows are counted in -- one glyph per char.
    let mut links: Vec<(std::ops::Range<usize>, &str)> = Vec::new();
    let mut chars_appended = 0usize;

    if let Some(level) = heading {
        // Two sizes, not six. A release body's headings are section names
        // ("Added", "Fixed"); a scale with six steps inside a region this size
        // would be a hierarchy nobody can see.
        //
        // **A heading's runs are all painted alike, links included, and so
        // none of them is clickable here.** A heading is a section name, it
        // is painted in one weight and one colour by design, and a run inside
        // it that opened a browser while looking exactly like the words
        // either side of it would be the "looks like a link / acts like a
        // link" pair coming apart in the other direction. The destination is
        // still beside the words, as it is everywhere else.
        let size = if level <= 2 { 13.0 } else { 12.0 };
        for span in spans {
            job.append(
                &span.text,
                0.0,
                TextFormat {
                    font_id: FontId::new(size, FontFamily::Name(theme::SEMIBOLD.into())),
                    color: theme::INK,
                    ..Default::default()
                },
            );
        }
    } else {
        for span in spans {
            let format = match span.style {
                NotesStyle::Plain => TextFormat {
                    font_id: FontId::new(12.0, FontFamily::Proportional),
                    color: theme::TEXT_MUTED,
                    ..Default::default()
                },
                NotesStyle::Strong => TextFormat {
                    font_id: FontId::new(12.0, FontFamily::Name(theme::SEMIBOLD.into())),
                    color: theme::TEXT_SECONDARY,
                    ..Default::default()
                },
                // egui skews the glyphs rather than swapping the face: this
                // app's font stack is Archivo weights and carries no italic
                // one, and a real italic is not worth a fifth embedded font
                // for a release note.
                NotesStyle::Emphasis => TextFormat {
                    font_id: FontId::new(12.0, FontFamily::Proportional),
                    color: theme::TEXT_MUTED,
                    italics: true,
                    ..Default::default()
                },
                NotesStyle::Code => TextFormat {
                    font_id: FontId::new(11.0, FontFamily::Monospace),
                    color: theme::TEXT_SECONDARY,
                    background: theme::CANVAS,
                    ..Default::default()
                },
                // Underlined and in the page's blue, which is what a link
                // looks like -- and now also what it does. Only a link the
                // parser accepted is ever given this style, so the blue is a
                // promise the click below keeps. The destination beside it
                // is unchanged and is the honest half: the user can see and
                // copy where these words point BEFORE following them, which
                // a clickable link that hid its URL would not give them.
                NotesStyle::LinkText => TextFormat {
                    font_id: FontId::new(12.0, FontFamily::Proportional),
                    color: theme::BLUE,
                    underline: egui::Stroke::new(1.0, theme::BLUE),
                    ..Default::default()
                },
                NotesStyle::LinkUrl => TextFormat {
                    font_id: FontId::new(11.0, FontFamily::Proportional),
                    color: theme::TEXT_FAINT,
                    ..Default::default()
                },
            };
            if let Some(url) = span.link.as_deref() {
                let start = chars_appended;
                links.push((start..start + span.text.chars().count(), url));
            }
            chars_appended += span.text.chars().count();
            job.append(&span.text, 0.0, format);
        }
    }

    // **Laid out here rather than inside `Label`, and handed to `Label`
    // already laid out.** The widget is given the finished galley, so there
    // is exactly ONE layout of this line: what is painted and what the hit
    // rectangles below are measured from are the same object, not two
    // measurements of the same text that could disagree.
    let galley = ui.painter().layout_job(job);
    ui.horizontal(|ui| {
        ui.add_space(inset);
        let response = ui.add(egui::Label::new(galley.clone()));
        for (n, (range, url)) in links.iter().enumerate() {
            for (row, rect) in notes_link_rects(&galley, response.rect.min, range.clone()) {
                // Ids are (line, link, row) rather than anything derived from
                // the URL: a release body may name the same address twice,
                // and two widgets sharing an id is the diagnostic
                // `no_id_diagnostic_on_any_preferences_page` exists to catch.
                let hit = ui.interact(
                    rect,
                    ui.id().with(("release-note-link", line, n, row)),
                    Sense::click(),
                );
                if hit.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if hit.clicked() {
                    // The crate's one opener -- `ShellExecuteW`, not
                    // `cmd.exe` -- reused rather than reimplemented. It
                    // re-checks the scheme itself, on top of the stricter
                    // `https`-only rule `updater::https_link` applied before
                    // this URL was ever attached to a span.
                    crate::vault_window::webbrowser_open(url);
                }
            }
        }
    });
}

/// Where one link's glyphs are, one rectangle per row of the galley they
/// wrapped onto, paired with that row's index.
///
/// **Per row, not one union.** A link that wraps occupies the tail of one row
/// and the head of the next; the bounding box of the two would additionally
/// cover the blank right-hand end of the first row and the blank left-hand
/// start of the second, making a click on empty space open a URL. The row's
/// full height is used rather than the glyphs' own, because a hit target the
/// exact height of the letterforms is one a pointer slides off.
///
/// `chars` indexes the galley's text in characters. That is the unit the rows
/// are counted in -- one glyph per character -- and the line has no newlines
/// in it (`updater` emits one block per source line), so a running count
/// across rows lands exactly.
fn notes_link_rects(
    galley: &egui::Galley,
    origin: Pos2,
    chars: std::ops::Range<usize>,
) -> Vec<(usize, Rect)> {
    let mut found = Vec::new();
    let mut seen = 0usize;
    for (index, placed) in galley.rows.iter().enumerate() {
        let mut left = f32::INFINITY;
        let mut right = f32::NEG_INFINITY;
        for (n, glyph) in placed.row.glyphs.iter().enumerate() {
            if chars.contains(&(seen + n)) {
                left = left.min(glyph.pos.x);
                right = right.max(glyph.max_x());
            }
        }
        seen += placed.row.glyphs.len();
        if left <= right {
            found.push((
                index,
                Rect::from_min_max(
                    origin + Vec2::new(left, placed.pos.y),
                    origin + Vec2::new(right, placed.pos.y + placed.row.size.y),
                ),
            ));
        }
    }
    found
}

/// How tall the notes region is allowed to get: **whatever the page has
/// left**, floored at [`UPDATE_NOTES_MIN_HEIGHT`].
///
/// # Why this is safe where sizing to the content would not be
///
/// The old fixed 128 points existed to stop a long release body pushing the
/// Download button off a window that cannot be resized or scrolled. Growing
/// to `available_height()` cannot do that, and the reason is structural
/// rather than arithmetical: this region is the last row of the last card on
/// the Updates page (see [`draw_update_card`]), the page itself has no
/// scroll area of its own -- [`draw_prefs_body`] hands the content column a
/// bounded `max_rect` -- and `available_height` is the distance from the
/// region's own top to the bottom of that rect. Taking all of it displaces
/// nothing, because everything that could be displaced is already above.
/// Anything the notes need beyond it still scrolls, exactly as before; the
/// only change is where "beyond" starts.
///
/// Short notes are unaffected: `auto_shrink([false, true])` on the region
/// means this is a CEILING, and a body that needs 40 points takes 40. That
/// was true at 128 too, and it is why raising the ceiling cannot leave a
/// short body sitting in an oversized box.
///
/// # Not cached, and not a constant
///
/// The card is drawn at two widths and two heights by the window shell and
/// the vault modal, and the rows above it change with the update stage -- a
/// progress bar appears mid-download, the "automatic checks are off" note
/// comes and goes with a setting. A number computed once would be the wrong
/// number on the next stage. It is read from the live `Ui` every frame for
/// the same reason `notes_line` reads its wrap width there.
fn notes_max_height(ui: &Ui) -> f32 {
    ui.available_height().max(UPDATE_NOTES_MIN_HEIGHT)
}

/// Whether the notes are short enough that there is nothing to scroll, which
/// is when the bar is not painted.
///
/// **Read back from the LAST frame's scroll area rather than predicted from
/// the text**, for the reason `password_health::content_fits` gives at
/// length: predicting the height would mean laying the wrapped, styled notes
/// out a second time here, and a second layout that disagreed with the real
/// one would hide the bar on notes that really do continue.
///
/// **Nothing about the LAYOUT turns on this.** The bar's lane is reserved in
/// both states -- `AlwaysVisible` above reserves it, and the width
/// subtraction inside the region is unconditional -- so only the bar's six
/// opacities change. A verdict that is one frame stale therefore cannot move
/// the card's edge by a single point; it can only paint a bar one frame
/// longer than needed, and [`remember_notes_overflow`] asks for the repaint
/// that ends it.
fn notes_fit(ui: &Ui) -> bool {
    // Ties go to "can scroll", as they do in the health pane: being wrong the
    // other way hides the cue on notes the user really can move. The first
    // frame has no memory and so shows the bar.
    !ui.ctx().data(|d| d.get_temp::<bool>(notes_overflow_id()).unwrap_or(true))
}

/// Stores what [`notes_fit`] reads, and asks for one more frame when the
/// answer changed -- see there.
fn remember_notes_overflow(ui: &Ui, overflows: bool) {
    let previous = ui.ctx().data(|d| d.get_temp::<bool>(notes_overflow_id()));
    if previous != Some(overflows) {
        ui.ctx().data_mut(|d| d.insert_temp(notes_overflow_id(), overflows));
        ui.ctx().request_repaint();
    }
}

fn notes_overflow_id() -> egui::Id {
    egui::Id::new("update_notes_overflows")
}

/// The download's progress: a bar and a byte count.
///
/// The bar is drawn only when the server declared a length. Without one there
/// is no fraction to show, and an indeterminate bar animating under a number
/// that is already moving says less than the number alone.
fn progress_bar(ui: &mut Ui, done: u64, total: Option<u64>) {
    use crate::update_panel::{download_fraction, download_label};

    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = ROW_TEXT_GAP;
        if let Some(fraction) = download_fraction(done, total) {
            progress_track(ui, fraction);
        }
        ui.label(
            RichText::new(download_label(done, total))
                .size(12.0)
                .color(theme::TEXT_FAINT),
        );
    });
}

/// The bar itself, without a caption.
///
/// Split out of [`progress_bar`] because the scan needs the same bar and a
/// different sentence: `download_label` measures BYTES, and a scan measures
/// passwords. Drawing the update card's caption under a scan's bar would have
/// put "61 B of 128 B" under a whole-vault check -- a number that is true of
/// nothing.
fn progress_track(ui: &mut Ui, fraction: f32) {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), UPDATE_BAR_HEIGHT),
        Sense::hover(),
    );
    ui.painter().rect_filled(rect, CornerRadius::same(UPDATE_BAR_RADIUS), theme::CANVAS);
    let filled =
        Rect::from_min_size(rect.min, Vec2::new(rect.width() * fraction, rect.height()));
    ui.painter().rect_filled(
        filled,
        CornerRadius::same(UPDATE_BAR_RADIUS),
        theme::TEXT_SECONDARY,
    );
}

/// The update card's button: [`reset_button`]'s box at [`UPDATE_BUTTON_WIDTH`],
/// with a disabled appearance for the stages where it names what is happening
/// rather than offering an action.
///
/// A disabled button here neither hovers, nor takes a click, nor shows the
/// pointing hand -- the three things that made the old tray item read as
/// clickable when it was not.
fn update_button(ui: &mut Ui, label: &str, enabled: bool) -> bool {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(UPDATE_BUTTON_WIDTH, STEPPER_HEIGHT), sense);
    let hovered = enabled && response.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().rect(
        rect,
        CornerRadius::same(STEPPER_RADIUS),
        if hovered { theme::CANVAS } else { theme::CARD },
        Stroke::new(1.0, if enabled { theme::BORDER_STRONG } else { theme::HAIRLINE }),
        StrokeKind::Inside,
    );
    let colour = if enabled { theme::TEXT_SECONDARY } else { theme::TEXT_GHOST };
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::new(12.0, FontFamily::Name(theme::SEMIBOLD.into())),
        colour,
    );
    ui.painter().galley(
        Pos2::new(
            rect.center().x - galley.size().x / 2.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        colour,
    );
    enabled && response.clicked()
}

/// **Five pages that could not be drawn have been removed instead.**
///
/// `draw_not_yet` used to live here: a card whose whole content was one
/// sentence saying nothing on the page was configurable. It was the honest
/// treatment -- deliberately not a disabled toggle, a "coming soon" badge, or
/// a greyed-out copy of 3e'''s controls, all three of which read as a feature
/// that is present and broken -- and that argument still stands wherever a
/// surface must show an absence (`loading_ui` and `vault_disk_cache` both
/// cite it). But a *nav row* is not such a surface. It is a promise that a
/// decision lives behind it, and the honest sentence was being reached by a
/// click the row had mis-sold. Autofill, Native apps and Security are gone;
/// so are Shortcuts (one read-only chip, now [`fill_hotkey_row`] on General)
/// and Sync & account (two rows of prose, neither of them a setting).
///
/// `draw_sync_and_account`'''s second row was a signpost saying the backend
/// switch had moved to [`Section::Vault`]. **It went with the page, and that
/// is not an oversight.** A signpost only works from the place the reader is
/// standing; with the page they would have stood on gone from the nav, there
/// is nowhere left to read it from. The nav does the signpost'''s job now --
/// Vault is one row below Breaches, above the fold, named in the word the
/// sentence used.

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

/// Opens the preferences window and blocks until it closes (same shape as
/// every other window in this crate -- `run_ui_native` pumps its own event
/// loop), returning the edited settings. The caller decides whether anything
/// actually changed and persists them; this function never touches disk itself.
///
/// The returned `Settings` differs from the argument in at most
/// `keep_backend_running`, `prompt_on_match`, `auto_lock_enabled` and
/// `auto_lock_minutes` -- the four fields `Settings::persist_preferences` owns. `vault_window` is carried through
/// untouched, which is what makes `main.rs`'s stale copy of it harmless.
/// The window shell: the titlebar, and the one form directly under it.
///
/// **Split out of [`run`] so the geometry can be asserted**, which is the
/// whole reason it exists as a function. `run` opens an OS window and blocks;
/// "does the rail start where the chrome ends" is a question about a frame,
/// and a frame is what this is.
///
/// **The body is given an explicit rect rather than taking the cursor's.**
/// `draw_window_chrome` ends in `ui.advance_cursor_after_rect(bar)`, and
/// egui's `advance_cursor_after_rect` leaves the cursor at the rect's bottom
/// PLUS `item_spacing.y` -- the ambient 8 points. That was the reported gap:
/// a strip of window background between the titlebar's hairline and the top
/// of the nav rail, on a page where the rail is meant to read as continuing
/// the chrome. Deliberately fixed by naming where the body starts rather than
/// by subtracting 8 somewhere, because a negative offset cancelling a
/// positive one is two numbers that were never meant to relate, and the next
/// person to change the bar's height would be debugging both.
///
/// The rect is computed the same way [`modal_body_rect`] computes the modal's
/// -- the window's full area, minus the header's height, from the top -- so
/// the two shells now space the body by the same rule instead of one
/// measuring and one inheriting.
///
/// **Public for the screenshot job** (`examples/ui_preview`), for the same
/// reason [`draw_prefs_body`] is: the other prefs surfaces draw the body
/// alone, so the seam between the chrome and the rail -- which is what was
/// reported -- appears in no picture unless something can draw the shell.
pub fn draw_prefs_window(ui: &mut Ui, state: &mut PrefsState) -> ChromeAction {
    let full = ui.max_rect();
    let action = draw_window_chrome(ui, WINDOW_TITLE);

    let body = Rect::from_min_max(
        Pos2::new(full.min.x, (full.min.y + CHROME_BAR_HEIGHT).min(full.max.y)),
        full.max,
    );
    let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
    draw_prefs_body(&mut body_ui, state);

    action
}

/// Height of the titlebar [`draw_window_chrome`] paints.
///
/// Read off `ChromeMetrics::LOGIN`, which is the metrics that function uses,
/// rather than written out again: this number decides where the body starts,
/// and a second `40.0` here would be a value that has to agree with one over
/// there with no mechanism making it -- the defect this constant's neighbours
/// in this file keep being written to avoid.
const CHROME_BAR_HEIGHT: f32 = crate::login_ui::ChromeMetrics::LOGIN.bar_height;

pub fn run(settings: Settings) -> Settings {
    let state = Rc::new(RefCell::new(PrefsState::with_scan_history(settings)));
    let state_for_closure = state.clone();
    let mut styled = false;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(WINDOW_SIZE)
            .with_resizable(false)
            .with_decorations(false)
            .with_icon(theme::window_icon()),
        ..Default::default()
    };

    let _ = eframe::run_ui_native(WINDOW_TITLE, options, move |ui, _frame| {
        if !styled {
            // Same first-frame guard every window here uses: egui only picks
            // up a new font set at the *start* of the next frame, so drawing
            // real (Archivo-styled) content this frame would either panic on
            // a font family that doesn't exist yet or, worse, flash one
            // unpainted near-black frame before the background fill lands --
            // which reads as a console window flashing open, not a
            // preferences dialog.
            theme::paint_window_background(ui);
            theme::apply(ui.ctx());
            round_window_corners(WINDOW_TITLE);
            // The OS window exists by this first painted frame (the same
            // hook `round_window_corners` uses), and this is where it is
            // brought to the front. See `foreground`: a refusal from Windows
            // flashes the taskbar button rather than being ignored.
            crate::foreground::raise_window(WINDOW_TITLE);
            styled = true;
            ui.ctx().request_repaint();
            return;
        }

        match draw_prefs_window(ui, &mut state_for_closure.borrow_mut()) {
            ChromeAction::Close => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
            // The chrome paints a - control whether or not anyone listens for
            // it; this window used to draw it and drop the action, so the
            // button was inert. Same handling the login window gives it.
            ChromeAction::Minimize => ui
                .ctx()
                .send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            ChromeAction::None => {}
        }
    });

    let edited = state.borrow().settings.clone();
    edited
}

// ---------------------------------------------------------------------------
// The in-window modal
//
// The same form, over the vault window, instead of a window of its own.
//
// **Nothing about the settings form is duplicated here.** [`run`] above and
// [`draw_prefs_modal`] below both call the one [`draw_prefs_body`], which is
// where every section, card, row and control lives. What differs between the
// two is only what surrounds it: `run` gets its background from
// `theme::paint_window_background`, its title and its dismiss from
// `draw_window_chrome`, and its 1000x780 from the OS; the modal paints its own
// card, its own 44px header and its own scrim, because it has no window of its
// own to get any of that from. Two shells over one body -- not two forms.
//
// `run` is deliberately kept. Preferences is also reachable from the tray with
// no vault window open at all (and, in particular, with the vault LOCKED), and
// a modal needs a window to be modal over. Opening the vault window for it
// would mean demanding the master password to change a checkbox. So the tray
// keeps a real window, and the gear -- which by definition already has a
// window -- gets the modal.
// ---------------------------------------------------------------------------

/// The modal's own title bar: a touch taller than `ChromeMetrics::LOGIN`'s
/// 40px because it carries no window controls and reads as a card header.
const MODAL_HEADER_HEIGHT: f32 = 44.0;
/// Breathing room left around the card, so the dimmed vault is visible on
/// every side and the modal reads as sitting *over* it rather than replacing
/// it. That visible frame is the whole point of the feature.
const MODAL_SCREEN_MARGIN: f32 = 24.0;
const MODAL_RADIUS: u8 = 12;
const MODAL_TITLE: &str = "Preferences";
/// The scrim's alpha, taken from `folder_modal` and the launch confirmation
/// verbatim rather than picked again.
const MODAL_SCRIM_ALPHA: u8 = 90;

/// What a frame of the modal asks its host to do.
///
/// One variant besides `None`, and no `Save`/`Cancel` pair: this form commits
/// as it is edited (every control writes straight into `PrefsState::settings`),
/// exactly as it did when it was a window whose only exit was the ✕. A Cancel
/// here would have to mean "put back the settings as they were on open", which
/// nothing in `run` ever offered and nothing on disk records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefsAction {
    None,
    Close,
}

/// The card's rectangle for a given window content rect. Pure, so the "does it
/// fit, and is it inset on every side" question is answerable without a frame.
///
/// 3e's 1000x740 body is a ceiling, not a demand: the vault window is
/// resizable and its minimum is smaller than that, so on a small window the
/// card is whatever is left after [`MODAL_SCREEN_MARGIN`] on each side. It is
/// never larger than the pane it is over, which is what stops the header's ✕
/// from being pushed off-screen.
pub fn modal_card_rect(screen: Rect) -> Rect {
    let width = (screen.width() - 2.0 * MODAL_SCREEN_MARGIN)
        .clamp(0.0, WINDOW_SIZE[0]);
    let height = (screen.height() - 2.0 * MODAL_SCREEN_MARGIN)
        .clamp(0.0, WINDOW_SIZE[1] - 40.0 + MODAL_HEADER_HEIGHT);
    Rect::from_center_size(screen.center(), Vec2::new(width, height))
}

/// The body's rectangle inside a card: everything under the header. Pure, and
/// separate from [`modal_card_rect`] so a test can assert that the body and
/// the header do not overlap.
pub fn modal_body_rect(card: Rect) -> Rect {
    Rect::from_min_max(
        Pos2::new(card.min.x, (card.min.y + MODAL_HEADER_HEIGHT).min(card.max.y)),
        card.max,
    )
}

/// Draws the preferences form as a modal card over a dimmed scrim covering the
/// whole window, returning what the host should do about it.
///
/// **The scrim is a full-window click-catcher on `Order::Foreground`**, the
/// idiom `draw_folder_edit_modal` and `draw_launch_confirm_modal` already use:
/// it sits above the sidebar, list and detail panels *and* above the titlebar,
/// so nothing behind it can be clicked while this is up.
///
/// **`ui.allocate_response(screen.size(), ..)` is what makes that true, and it
/// is not an idiom.** This doc used to say the opposite -- that egui "blocks by
/// layer order rather than reserved pixels", so removing the call "does not let
/// a click through", and it was kept only as a matter of style. That was
/// measured false. On egui 0.35 `Memory::layer_id_at` hit-tests against the
/// `Area`'s **stored rect**, and an area's stored rect is what it allocated: a
/// scrim that allocates nothing has a near-zero rect and blocks nothing outside
/// the card. Deleting this one line lets a click land on a vault control out in
/// the margin while the card sits over the middle of the screen -- and did so
/// with the whole `prefs_ui::` suite green, because the test named for that
/// property was only asserting that a scrim click does not dismiss.
/// `a_click_on_the_scrim_never_reaches_the_vault_behind_it` now asserts on the
/// control behind, at the card and out in the margin, and dies when this line
/// goes.
///
/// Clicking the scrim does
/// **not** dismiss -- neither of the other two modals dismisses on a scrim
/// click either, and a form that is committed as it is typed is the last place
/// to add an accidental exit.
///
/// **Esc and the header ✕ both close**, matching those same two.
///
/// The host is still responsible for the parts a scrim cannot reach: keyboard
/// shortcuts read straight off `ctx.input` bypass hit-testing entirely, so the
/// caller must not run them while this is drawn. See
/// `vault_window`'s Ctrl+K/L/N block.
///
/// **`ctx.input` IS NOT LAYER-AWARE, and only Ctrl+K/L/N are gated.** The gate
/// is `keyboard_shortcuts_enabled`, which the host turns off for those three
/// and for nothing else. Raw text -- anything reaching a `TextEdit` behind this
/// modal, or any other `ctx.input` read the vault window grows later -- is not
/// gated by the scrim at all, because a scrim gates the pointer and nothing
/// else. It is unreachable in production today only because the one route into
/// this modal is a gear click, and clicking the gear surrenders keyboard focus
/// from whatever had it. **That is a coincidence of the current UI, not a
/// guarantee.** A second route in -- a shortcut, a menu item, a restored
/// session that reopens the modal -- would leave focus wherever it was, and the
/// next reader who assumes "the modal is up, so input is blocked" will be
/// wrong. Anything new that reads `ctx.input` must be gated explicitly.
pub fn draw_prefs_modal(ctx: &egui::Context, state: &mut PrefsState) -> PrefsAction {
    let mut action = PrefsAction::None;
    let screen = ctx.content_rect();
    let card = modal_card_rect(screen);

    // **`screen.min`, not `Pos2::ZERO`.** The area's stored rect is
    // `fixed_pos + allocated size`, and that rect is what `layer_id_at`
    // hit-tests; the *painted* rectangle just below is `screen`. Anchored at
    // `Pos2::ZERO` the two agree only while `content_rect().min` is the origin,
    // and where it is not, the scrim looks whole and blocks a `screen.size()`
    // box starting at the wrong corner -- leaving a live strip along the far
    // edges of the window. `content_rect().min` is the origin on every harness
    // and on every window this app opens today, so this is hardening rather
    // than a fix; the point is that the blocked region and the painted region
    // are now derived from the same rectangle instead of agreeing by
    // coincidence.
    egui::Area::new(egui::Id::new("prefs-modal-scrim"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            ui.allocate_response(screen.size(), Sense::click());
            ui.painter().rect_filled(
                screen,
                CornerRadius::ZERO,
                egui::Color32::from_black_alpha(MODAL_SCRIM_ALPHA),
            );
        });

    // `fixed_pos`, not `anchor`. An anchored `Area` has to measure its content
    // before it can centre it, so its first frame paints nothing at all -- and
    // this card's geometry is computed here rather than measured, so there is
    // nothing to wait for. See `an_anchored_area_paints_nothing_on_its_first_frame`.
    egui::Area::new(egui::Id::new("prefs-modal"))
        .order(egui::Order::Foreground)
        .fixed_pos(card.min)
        .show(ctx, |ui| {
            // Swallows anything aimed at the card that no control inside it
            // claims. Allocated FIRST so the widgets drawn below -- later in
            // the same layer, and therefore on top -- still win their clicks.
            ui.allocate_rect(card, Sense::click());
            ui.set_clip_rect(card);

            let header = Rect::from_min_max(
                card.min,
                Pos2::new(card.max.x, (card.min.y + MODAL_HEADER_HEIGHT).min(card.max.y)),
            );
            {
                let painter = ui.painter();
                painter.rect_filled(card, CornerRadius::same(MODAL_RADIUS), theme::WINDOW_BG);
                painter.rect_filled(header, CornerRadius::same(MODAL_RADIUS), theme::CARD);
                // Square off the header's bottom corners: the fill above
                // rounds all four, and a rounded bottom edge in the middle of
                // the card reads as two stacked cards.
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(header.min.x, header.max.y - MODAL_RADIUS as f32),
                        header.max,
                    ),
                    CornerRadius::ZERO,
                    theme::CARD,
                );
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(header.min.x, header.max.y - 1.0),
                        header.max,
                    ),
                    CornerRadius::ZERO,
                    theme::HAIRLINE,
                );
                painter.rect_stroke(
                    card,
                    CornerRadius::same(MODAL_RADIUS),
                    Stroke::new(1.0, theme::BORDER),
                    StrokeKind::Inside,
                );
            }

            let galley = ui.painter().layout_no_wrap(
                MODAL_TITLE.to_string(),
                FontId::new(13.0, FontFamily::Proportional),
                theme::INK,
            );
            ui.painter().galley(
                Pos2::new(
                    header.center().x - galley.size().x / 2.0,
                    header.center().y - galley.size().y / 2.0,
                ),
                galley,
                theme::INK,
            );

            // The ✕, in the header's right-hand end. `theme::close_glyph` is
            // the same mark `card_header_with_close` puts on the overlay --
            // drawn as two strokes, because U+2715 is a tofu box in this
            // app's face.
            let close_rect = Rect::from_center_size(
                Pos2::new(header.max.x - 22.0, header.center().y),
                Vec2::splat(16.0),
            );
            let mut close_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(close_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            if theme::close_glyph(&mut close_ui).clicked() {
                action = PrefsAction::Close;
            }

            // **The one settings form**, given the body's rect exactly as
            // `run`'s `CentralPanel` gives it the window's.
            let body = modal_body_rect(card);
            let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
            draw_prefs_body(&mut body_ui, state);
        });

    if action == PrefsAction::None && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = PrefsAction::Close;
    }

    action
}

#[cfg(test)]
mod tests {
    //! Real frames of [`draw_prefs_body`] read back through the shapes egui
    //! emitted, using the same headless technique as `vault_window`'s panes.
    //!
    //! What these can and cannot see is worth stating plainly. They can see
    //! every string painted, every rectangle's size and fill, and the state
    //! `draw_prefs_body` left behind -- so "is this section present", "is
    //! there a control here", and "which value is displayed" are all pinned.
    //! They cannot see hover cursors, focus rings, the DWM window rounding, or
    //! whether the result *looks* like 3e; those are checked by eye, and no
    //! test here pretends otherwise.
    use super::*;
    use eframe::egui::epaint::RectShape;

    /// The body's own area: 3e's card minus `ChromeMetrics::LOGIN`'s 40px bar.
    const BODY_SIZE: Vec2 = Vec2::new(WINDOW_SIZE[0], WINDOW_SIZE[1] - 40.0);

    #[derive(Default)]
    struct Painted {
        texts: Vec<(String, Rect)>,
        ink: Vec<TextInk>,
        rects: Vec<RectShape>,
    }

    /// One painted text run, with everything a geometry assertion needs that a
    /// `(String, Rect)` cannot carry: what egui actually laid out (an elided
    /// string is not the string that was asked for), how many lines it wrapped
    /// to, and the colour it was painted in. The colour is here because a
    /// control painted at alpha 0 occupies a perfectly reasonable rectangle
    /// and is not on screen, and a test reading only rectangles says so.
    #[derive(Clone)]
    struct TextInk {
        source: String,
        rendered: String,
        rect: Rect,
        rows: usize,
        color: egui::Color32,
    }

    impl Painted {
        fn strings(&self) -> Vec<&str> {
            self.texts.iter().map(|(t, _)| t.as_str()).collect()
        }

        fn contains(&self, needle: &str) -> bool {
            self.texts.iter().any(|(t, _)| t == needle)
        }

        /// [`Self::contains`]'s loose twin, for the lines that are whole
        /// sentences rather than labels: a description is painted as one
        /// string, and asserting on the whole of it would pin its wording
        /// rather than its content.
        fn any_containing(&self, needle: &str) -> bool {
            self.texts.iter().any(|(t, _)| t.contains(needle))
        }

        fn rect_of(&self, needle: &str) -> Rect {
            self.texts
                .iter()
                .find(|(t, _)| t == needle)
                .unwrap_or_else(|| panic!("{needle:?} was never painted; got {:?}", self.strings()))
                .1
        }

        /// Rectangles of exactly the given size, whatever their fill --
        /// how a control is counted without asserting on its colours.
        fn count_of_size(&self, size: Vec2) -> usize {
            self.rects
                .iter()
                .filter(|r| {
                    (r.rect.width() - size.x).abs() < 0.5
                        && (r.rect.height() - size.y).abs() < 0.5
                })
                .count()
        }

        /// The fill of the smallest rectangle painted behind a text run.
        ///
        /// **How a [`choice_button`] cell's state is read back**, and the
        /// reason the backend row's pins got stronger rather than weaker when
        /// it stopped being a pill. The helper it replaced could only say "the
        /// nth switch"; this says "the cell labelled *this*", so an assertion
        /// about which client is selected names the client instead of
        /// counting rows -- and a row reordering can no longer make a test
        /// pass by pointing it at a different control.
        ///
        /// Smallest containing rectangle, not the first: the cell sits inside
        /// the card, which sits inside the body, and all three contain the
        /// label's centre.
        fn fill_behind(&self, needle: &str) -> egui::Color32 {
            let text = self.rect_of(needle);
            let mut found: Vec<&RectShape> = self
                .rects
                .iter()
                .filter(|r| r.rect.contains(text.center()))
                .collect();
            found.sort_by(|a, b| a.rect.area().total_cmp(&b.rect.area()));
            found
                .first()
                .unwrap_or_else(|| panic!("nothing was painted behind {needle:?}"))
                .fill
        }

        // **`pill_fills` is gone, and [`Self::fill_behind`] above is what
        // replaced it.** It read the fill of the *n*th 40x22 rectangle on the
        // page, and existed for exactly one claim: which way round the
        // backend row was. That row is a two-cell `choice_button` picker now
        // and paints no pill at all, so its only caller went -- and what took
        // its place says *which cell*, by label, rather than which index.
        // Kept as a note rather than as an `allow(dead_code)` helper, because
        // an unused reader of pill colours is how the next test comes to
        // assert on a row it never located.

        /// Every rectangle of exactly this size, top to bottom -- how a
        /// control that paints no text of its own (the toggle pill) is
        /// located now that the General card holds two of them.
        ///
        /// Sorted by the painted y, not by paint order: "the pill in the
        /// second row" is a claim about where it is on screen, and a test
        /// that indexed paint order would keep passing if the rows were
        /// drawn in one order and laid out in another.
        fn rects_of_size(&self, size: Vec2) -> Vec<Rect> {
            let mut found: Vec<Rect> = self
                .rects
                .iter()
                .filter(|r| {
                    (r.rect.width() - size.x).abs() < 0.5
                        && (r.rect.height() - size.y).abs() < 0.5
                })
                .map(|r| r.rect)
                .collect();
            found.sort_by(|a, b| a.top().total_cmp(&b.top()));
            found
        }

        /// The one rectangle of exactly this size, for a control there is
        /// only ever one of.
        fn only_rect_of_size(&self, size: Vec2) -> Rect {
            let found = self.rects_of_size(size);
            assert_eq!(found.len(), 1, "expected exactly one rectangle of size {size:?}");
            found[0]
        }

        /// The stroke colour of the one rectangle of exactly this size --
        /// how "greyed out" is read back, since the stepper's box paints no
        /// text of its own.
        fn stroke_of_only_rect_of_size(&self, size: Vec2) -> egui::Color32 {
            let mut found = self.rects.iter().filter(|r| {
                (r.rect.width() - size.x).abs() < 0.5 && (r.rect.height() - size.y).abs() < 0.5
            });
            let stroke = found.next().expect("no rectangle of that size was painted").stroke;
            assert!(found.next().is_none(), "more than one rectangle of that size");
            stroke.color
        }

        /// The one painted run of exactly this text. Panics if it was never
        /// painted, and if it was painted twice -- either way the caller's
        /// "the" is wrong and a silent first-match would hide it.
        fn ink_of(&self, needle: &str) -> TextInk {
            let mut found = self.ink.iter().filter(|i| i.source == needle);
            let first = found.next().unwrap_or_else(|| {
                panic!("{needle:?} was never painted; got {:?}", self.strings())
            });
            assert!(found.next().is_none(), "{needle:?} was painted more than once");
            first.clone()
        }

        /// The one painted run of exactly this text **in the nav column**.
        ///
        /// Needed because the open section's label is painted twice -- once
        /// as its nav row and once as the content pane's heading, which use
        /// the same word deliberately -- so `ink_of` refuses it. The nav is
        /// everything left of `NAV_WIDTH`, which is a layout constant rather
        /// than a guess at where the column ends.
        fn nav_ink_of(&self, needle: &str) -> TextInk {
            let mut found = self
                .ink
                .iter()
                .filter(|i| i.source == needle && i.rect.max.x < NAV_WIDTH);
            let first = found.next().unwrap_or_else(|| {
                panic!("{needle:?} was never painted in the nav; got {:?}", self.strings())
            });
            assert!(found.next().is_none(), "{needle:?} was painted twice in the nav");
            first.clone()
        }

        fn count_filled(&self, fill: egui::Color32) -> usize {
            self.rects.iter().filter(|r| r.fill == fill).count()
        }
    }

    fn walk(shape: &egui::Shape, p: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => {
                let rect = Rect::from_min_size(text.pos, text.galley.size());
                p.texts.push((text.galley.text().to_string(), rect));
                p.ink.push(TextInk {
                    source: text.galley.text().to_string(),
                    // The glyphs actually placed, row by row -- text that was
                    // elided to fit renders fewer of them than it was given.
                    rendered: text
                        .galley
                        .rows
                        .iter()
                        .flat_map(|row| row.glyphs.iter().map(|glyph| glyph.chr))
                        .collect(),
                    rect,
                    rows: text.galley.rows.len(),
                    color: text.override_text_color.unwrap_or_else(|| {
                        text.galley
                            .job
                            .sections
                            .first()
                            .map(|section| section.format.color)
                            .unwrap_or(egui::Color32::TRANSPARENT)
                    }),
                });
            }
            egui::Shape::Rect(rect) => p.rects.push(rect.clone()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, p);
                }
            }
            // Everything else is geometry this file does not assert on. A new
            // `egui::Shape` variant carrying text would be egui's to announce,
            // not something an exhaustive match here could usefully catch.
            _ => {}
        }
    }

    /// A context with `theme::apply`'s fonts actually live. The two throwaway
    /// frames are the same ones `detail.rs`'s and `item_list.rs`'s harnesses
    /// run, for the same reason: a font set registered during a frame only
    /// becomes usable at the start of the next one.
    fn styled_context() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw_input(&[]), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(raw_input(&[]), |_ui| {});
        ctx
    }

    fn raw_input(events: &[egui::Event]) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, BODY_SIZE)),
            events: events.to_vec(),
            ..Default::default()
        }
    }

    fn frame(ctx: &egui::Context, state: &mut PrefsState, events: &[egui::Event]) -> Painted {
        let output = ctx.run_ui(raw_input(events), |ui| draw_prefs_body(ui, state));
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        painted
    }

    /// A full primary press-and-release at `pos`, which is what egui needs to
    /// report `Response::clicked` -- a `PointerButton` press alone is not a
    /// click.
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

    // ## The two id diagnostics, and why only one of them is asserted on
    //
    // egui draws two different red warnings in a debug build, and they are
    // not the same claim. Neither reaches a release build, so a user is
    // told nothing either way:
    //
    // * **`warn_on_id_clash`** (`Context::check_for_id_clash`) fires when
    //   one id is used at two different rects *within a pass*, and paints a
    //   thin outline **plus a `🔥` text label**. This is the one worth
    //   guarding: two widgets sharing an id share state, so focus lands on
    //   the wrong row and a click registers against a neighbour. In release
    //   there is no outline, no log line, and nothing else in this suite
    //   that would notice.
    // * **`warn_if_rect_changes_id`** fires when a rect keeps its geometry
    //   *between passes* while its id changes, and paints a bare 2px
    //   `Color32::RED` outline with **no text at all**. That text -- rather,
    //   its absence -- is the only thing telling the two apart on screen.
    //
    // The second one **does** fire here, and it is benign and known:
    // switching pages lands one page's [`row_separator`] on the rect the
    // previous page's separator held, and separators carry egui's
    // positional auto-ids. A separator is `Sense::hover()` decoration --
    // it holds no focus, no drag and no scroll memory, so nothing can be
    // keyed to the id it did not keep. Re-keying it would be a change to
    // shipping code made to quiet a diagnostic that is telling the truth
    // about something harmless, so it is left alone and named here instead.
    //
    // [`rect_id_changes`] therefore exists only so the positive control can
    // prove the harness would see that shape of defect at all.

    /// Every **id clash** egui drew into `shape`, appended to `out`.
    ///
    /// Shared with [`super::modal_tests`], so the modal and the body cannot
    /// come to disagree about what a clash looks like.
    pub(super) fn id_clashes(shape: &egui::Shape, out: &mut Vec<String>, at: &str) {
        match shape {
            egui::Shape::Text(text) if text.galley.text().contains('\u{1f525}') => {
                out.push(format!("{at}: ID CLASH {}", text.galley.text()));
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    id_clashes(shape, out, at);
                }
            }
            _ => {}
        }
    }

    /// Every **rect that changed id between passes**. See the note above:
    /// collected for the positive control, not asserted on by the guards.
    fn rect_id_changes(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Rect(rect)
                if rect.stroke.color == egui::Color32::RED
                    && (rect.stroke.width - 2.0).abs() < 0.01 =>
            {
                out.push(format!("RECT CHANGED ID {:?}", rect.rect));
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    rect_id_changes(shape, out);
                }
            }
            _ => {}
        }
    }

    /// **The control that stops every id guard here from passing vacuously.**
    ///
    /// Both checks are egui's, not this crate's, and both are switched on by
    /// a *default*: `warn_on_id_clash` and
    /// `style().debug.warn_if_rect_changes_id` are each
    /// `cfg!(debug_assertions)` out of the box. So the guards assert on a
    /// diagnostic nobody in this crate turns on -- which is precisely how
    /// this kind of test dies quietly. An egui release that flips a default,
    /// renames a field, moves the painting to another layer or drops the
    /// `🔥` prefix would leave every guard green while checking nothing.
    ///
    /// This hands egui one defect of each kind and insists both are
    /// reported, so that day is a failing test rather than a silent hole.
    #[test]
    fn the_id_diagnostics_are_reported_when_egui_is_handed_one_of_each() {
        let ctx = styled_context();
        let one = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::splat(20.0));
        let two = Rect::from_min_size(Pos2::new(90.0, 90.0), Vec2::splat(20.0));

        // A CLASH: one id at two rects inside a single pass.
        let output = ctx.run_ui(raw_input(&[]), |ui| {
            let _ = ui.interact(one, egui::Id::new("control-clash"), Sense::click());
            let _ = ui.interact(two, egui::Id::new("control-clash"), Sense::click());
        });
        let mut hits = Vec::new();
        for clipped in &output.shapes {
            id_clashes(&clipped.shape, &mut hits, "clash control");
        }
        assert!(
            !hits.is_empty(),
            "egui reported no clash for two widgets sharing one id"
        );

        // A RECT THAT CHANGED ID: one rect, a different id on the next pass.
        let go = |name: &'static str| {
            ctx.run_ui(raw_input(&[]), |ui| {
                let _ = ui.interact(one, egui::Id::new(name), Sense::click());
            })
        };
        let _ = go("control-a");
        let output = go("control-b");
        let mut hits = Vec::new();
        for clipped in &output.shapes {
            rect_id_changes(&clipped.shape, &mut hits);
        }
        assert!(
            !hits.is_empty(),
            "egui reported no id change for a rect that kept its geometry"
        );
    }

    /// One frame of the body as the *raw shapes* egui emitted.
    ///
    /// [`Painted`] flattens a frame into texts and rectangles and drops
    /// everything else, which is the right shape for the layout assertions
    /// and the wrong one here: [`id_clashes`] has to walk the tree as
    /// egui built it, including the nested `Shape::Vec`s a debug painter
    /// contributes.
    fn body_shapes(
        ctx: &egui::Context,
        state: &mut PrefsState,
        events: &[egui::Event],
    ) -> Vec<egui::Shape> {
        ctx.run_ui(raw_input(events), |ui| draw_prefs_body(ui, state))
            .shapes
            .into_iter()
            .map(|clipped| clipped.shape)
            .collect()
    }

    /// **No page of Preferences reports an id clash as it is drawn.**
    ///
    /// Neither check reaches a release build, which is what makes this worth
    /// running: an id clash means two widgets share state, and in release
    /// there is no outline, no log line, and nothing else in this suite that
    /// would notice a settings row answering for its neighbour.
    ///
    /// Two frames per page, not one. An `Area` that centres itself paints
    /// nothing on its first frame (see
    /// `an_anchored_area_paints_nothing_on_its_first_frame`), so a guard
    /// reading frame 1 would be asserting about a page that had not been
    /// drawn yet -- and passing for the wrong reason.
    #[test]
    fn no_id_diagnostic_on_any_preferences_page() {
        let mut hits = Vec::new();
        for section in Section::ALL {
            let ctx = styled_context();
            let mut state = PrefsState::new(Settings::default());
            state.section = section;
            let _ = body_shapes(&ctx, &mut state, &[]);
            for shape in body_shapes(&ctx, &mut state, &[]) {
                id_clashes(&shape, &mut hits, &format!("{section:?}"));
            }
        }
        assert!(hits.is_empty(), "{hits:#?}");
    }

    /// **Nor does clicking through the nav, one page to the next.**
    ///
    /// One context for the whole walk, and a settle frame after each click.
    /// A click is *reported* on the frame it completes on, but the page it
    /// selects is only drawn on the frame after, so a guard that read the
    /// click frame alone would never look at nine of the ten pages it
    /// believes it visited.
    #[test]
    fn no_id_diagnostic_while_the_nav_rows_are_clicked_through() {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        let first = frame(&ctx, &mut state, &[]);
        let targets: Vec<(String, Pos2)> = Section::ALL
            .iter()
            .map(|section| {
                let label = section.label().to_string();
                let centre = first.nav_ink_of(&label).rect.center();
                (label, centre)
            })
            .collect();
        let mut hits = Vec::new();
        for (label, pos) in targets {
            for (what, events) in [("clicking", click(pos)), ("settling after", Vec::new())] {
                for shape in body_shapes(&ctx, &mut state, &events) {
                    id_clashes(&shape, &mut hits, &format!("{what} {label:?}"));
                }
            }
        }
        assert!(hits.is_empty(), "{hits:#?}");
    }

    // -- the local API, and the keys that open it --------------------------

    /// A viewport tall enough for the whole of either scrolling page.
    ///
    /// Both scroll in the real window (see [`draw_api`] and [`draw_vault`]),
    /// and
    /// a `ScrollArea` **culls what is outside its viewport** -- so a test
    /// reading `BODY_SIZE` would find the mint button missing and could not
    /// tell that from a mint button that was never drawn. Height only:
    /// `paint_section_at`'s argument in the other axis, for the same reason
    /// it exists there.
    const TALL_BODY: Vec2 = Vec2::new(WINDOW_SIZE[0], 1600.0);

    /// The instant every test on this page is answered against. A round
    /// number in 2023, and a *parameter* rather than the clock, because
    /// expiry is the one thing on this page that changes with no input.
    const TEST_NOW: u64 = 1_700_000_000;

    /// The 32 bytes every minted key in these tests is made of, and the hex
    /// they come out as. Fixed, because a test that cannot say what key was
    /// made cannot check that the key on screen is not the one in the record.
    const TEST_KEY_BYTES: [u8; 32] = [0xab; 32];
    const TEST_KEY: &str =
        "abababababababababababababababababababababababababababababababab";

    /// Where the one test that clicks Copy puts the key, instead of on the
    /// clipboard.
    ///
    /// A `static` because the sink is a `fn` pointer with nothing to capture
    /// -- and **exactly one test writes it**, deliberately: the suite runs in
    /// parallel and a second test reading this would be reading whatever the
    /// first had just put there. Everything else on this page is driven
    /// through values.
    static COPIED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

    fn tall_input(events: &[egui::Event]) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, TALL_BODY)),
            events: events.to_vec(),
            ..Default::default()
        }
    }

    /// A Local API page with every seam pointed at a value: a fixed clock,
    /// fixed randomness, a store that writes nowhere, and a clipboard that is
    /// not the clipboard.
    fn api_state() -> PrefsState {
        let mut state = PrefsState::new(Settings::default());
        state.section = Section::Api;
        state.show_key_clock(|| TEST_NOW);
        state.show_key_random(|| TEST_KEY_BYTES);
        state
    }

    fn tall_frame(
        ctx: &egui::Context,
        state: &mut PrefsState,
        events: &[egui::Event],
    ) -> Painted {
        let output = ctx.run_ui(tall_input(events), |ui| draw_prefs_body(ui, state));
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        painted
    }

    /// A click, and then the frame that draws what it did.
    ///
    /// **Two frames, not one, and that is not a formality.** Every button on
    /// this page is read at the point it is drawn and acted on afterwards --
    /// the reveal card is above the form whose button makes it, and a row's
    /// revoke question is decided after the row has been laid out. So the
    /// frame a click is *reported* on still shows the page as it was before
    /// it. A test that asserted on the click frame would be reading the old
    /// screen, which is the same shape of mistake
    /// `no_id_diagnostic_while_the_nav_rows_are_clicked_through` documents
    /// for the nav.
    fn tall_click(ctx: &egui::Context, state: &mut PrefsState, pos: Pos2) -> Painted {
        let _ = tall_frame(ctx, state, &click(pos));
        tall_frame(ctx, state, &[])
    }

    /// A context whose fonts are live, sized for the tall viewport.
    fn tall_context() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(tall_input(&[]), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(tall_input(&[]), |_ui| {});
        ctx
    }

    /// One stored key, so the list has something in it.
    fn stored_key(name: &str, expires_unix: Option<u64>) -> KeyRecord {
        KeyRecord {
            name: name.to_string(),
            hash: crate::service_keys::hash_key(TEST_KEY),
            created_unix: TEST_NOW - 86_400,
            expires_unix,
            scopes: vec![Scope { subject: Subject::All, access: Access::Read }],
        }
    }

    /// **The built-in-client row does not claim the CLI signs you in.**
    ///
    /// It said "(the Bitwarden CLI is still used to sign in)" until the
    /// switch-over made that false, and the owner read it in the running app
    /// before anything here noticed. Copy that describes machinery goes stale
    /// the moment the machinery moves, and this row's whole job is to be the
    /// sentence somebody weighs before deciding where their master key lives.
    ///
    /// The positive control is the surrounding clause, so this fails if the
    /// paragraph is rewritten out from under it rather than passing on a
    /// needle that no longer matches anything.
    #[test]
    fn the_built_in_client_row_does_not_say_the_cli_signs_you_in() {
        let copy = official_crypto_description(true);
        // **The positive control moved with the sentence it anchors.** It
        // used to be "signing in no longer uses the CLI either", which was a
        // clause of the four-paragraph copy the owner asked to shrink. The
        // same fact is now carried by the clause below -- the built-in client
        // is what talks to the server -- and this is the clause whose removal
        // would make the assertion under it vacuous.
        assert!(
            copy.contains("talks to your server itself"),
            "control: the clause this test is about is gone from the row, so the assertion \
             below is about a paragraph that no longer exists: {copy}"
        );
        assert!(
            !copy.contains("still used to sign in"),
            "the row tells the user the Bitwarden CLI signs them in, which stopped being \
             true when the direct-REST path took the CLI out of authenticate_then_wipe"
        );
    }

    /// **The row says what it costs, not only what it buys.**
    ///
    /// The backend row beside it names its ~118 MB; this one holds about
    /// 76 MB -- in the window's own process where there is one, and in the tray
    /// process where the window is hosted there -- and a user who finds it in Task
    /// Manager rather than here has been surprised by their own settings.
    /// That surprise is the report this whole split came from.
    #[test]
    fn the_ui_loaded_row_names_both_halves_of_the_trade() {
        assert!(
            UI_LOADED_DESCRIPTION.contains("MB"),
            "the row does not say what it costs, so the memory is a surprise: \
             {UI_LOADED_DESCRIPTION}"
        );
        // **The measured figure, not a round one.** A kept-loaded vault window
        // is ~76 MB of private working set -- Task Manager's "Memory" column,
        // the counter `README.md`'s table publishes. The row said "about
        // 100 MB" and nothing in this repository measures that.
        assert!(
            UI_LOADED_DESCRIPTION.contains("76 MB"),
            "the row is off the measured vault-window figure: {UI_LOADED_DESCRIPTION}"
        );
        for stale in ["100 MB", "110 MB", "111 MB"] {
            assert!(
                !UI_LOADED_DESCRIPTION.contains(stale),
                "the row is back on {stale}, which nothing in this repository measures: \
                 {UI_LOADED_DESCRIPTION}"
            );
        }
        // **And it does not claim a second process.** Whether the window has
        // one is decided by `RealVaultOps::open_window`: the daemon hosts the
        // window ITSELF on an account served by the built-in client with no
        // stored user key, so on that configuration -- the owner's -- there is
        // no second process and the memory lands on the tray process. A row
        // that promised one sent the user looking in Task Manager for a
        // process that is not there.
        assert!(
            !UI_LOADED_DESCRIPTION.contains("second process"),
            "the row promises a second process the built-in client does not always have: \
             {UI_LOADED_DESCRIPTION}"
        );
        assert!(
            UI_LOADED_LABEL.to_lowercase().contains("open"),
            "the row does not say what it buys: {UI_LOADED_LABEL}"
        );
        assert!(
            UI_LOADED_DESCRIPTION.to_lowercase().contains("lock"),
            "the row promises instant opening without saying what still closes the \
             window, so a lock reads as the setting having failed"
        );
    }

    /// **The copy is the requirement here, not a detail of the page.**
    ///
    /// Off is the default, and a person who turns this on has decided to put
    /// a door into their vault. The sentence they read while deciding is this
    /// one, so a future edit that quietly drops the uncomfortable half of it
    /// fails a test rather than shipping -- `disk_cache_description`'s rule,
    /// on the row with more at stake.
    #[test]
    fn the_off_copy_says_what_turning_it_on_would_do() {
        let text = service_description(false);
        assert!(text.contains("default"), "the copy stopped saying off is the default");
        assert!(
            text.contains("127.0.0.1"),
            "the copy does not say an HTTP endpoint appears: {text}"
        );
        assert!(
            text.contains("usernames, passwords, notes and two-factor secrets"),
            "the copy names what is served with a euphemism: {text}"
        );
        assert!(
            text.contains("any program on this PC"),
            "the copy does not say who can use it, and \"local\" reads as \"only me\": {text}"
        );
        assert!(
            text.contains("master password"),
            "the copy does not say a key is the whole credential: {text}"
        );
        // The control: the two states are not the same sentence with a word
        // changed, so asserting on the off one is asserting on something.
        assert_ne!(text, service_description(true));
    }

    /// The ON copy describes what is happening **now**, in the present tense,
    /// and still names the reach. A person who has turned it on and come back
    /// to check is asking "what is this doing", not "what would it do".
    #[test]
    fn the_on_copy_says_the_endpoint_is_live_and_who_it_answers() {
        let text = service_description(true);
        assert!(text.contains("listening"), "got {text}");
        assert!(text.contains("127.0.0.1"), "got {text}");
        assert!(
            text.contains("usernames, passwords, notes and two-factor secrets"),
            "got {text}"
        );
        assert!(text.contains("master password"), "got {text}");
        assert!(!text.contains("Off, which is the default"), "the off copy is being shown");
    }

    /// **"Never expires" is spelled out rather than left blank.**
    ///
    /// A blank in an expiry column reads as "not applicable". Here it would
    /// mean a credential that works forever, which is the one value on this
    /// row a reader must not have to infer.
    #[test]
    fn a_key_with_no_expiry_says_so_out_loud() {
        let record = stored_key("Backup", None);
        let line = key_when_wording(&record, TEST_NOW, &zone());
        assert!(line.contains("Never expires"), "got {line}");
        assert!(line.contains("Created"), "the creation date went missing: {line}");
        // The control: a key that DOES expire says something else, so the
        // assertion above is not merely reading a constant.
        let dated = stored_key("Backup", Some(TEST_NOW + 86_400));
        let other = key_when_wording(&dated, TEST_NOW, &zone());
        assert!(!other.contains("Never expires"), "got {other}");
        assert!(other.contains("Expires"), "got {other}");
    }

    /// **Expiry is answered against the clock, and inclusively**, exactly as
    /// `service_keys::find` answers it.
    ///
    /// A key whose date has arrived is already being refused by the service.
    /// A list that showed it as an ordinary future date would be the screen
    /// disagreeing with the door -- and the owner would spend the afternoon
    /// debugging a script that is being turned away by a key this window
    /// says is fine.
    #[test]
    fn a_key_whose_expiry_has_arrived_is_shown_as_already_refused() {
        let expired = stored_key("Nightly", Some(TEST_NOW));
        let line = key_when_wording(&expired, TEST_NOW, &zone());
        assert!(line.contains("Expired"), "got {line}");
        assert!(line.contains("already refused"), "got {line}");
        // The control, one second the other side of the boundary: this is
        // the exact edge `service_keys::find` treats inclusively, and a
        // wording keyed on `<` rather than `<=` would pass every other
        // assertion here.
        let live = key_when_wording(&expired, TEST_NOW - 1, &zone());
        assert!(!live.contains("Expired"), "got {live}");
        assert!(live.contains("Expires"), "got {live}");
    }

    /// **A key with no grants is a sentence, not an empty cell.**
    ///
    /// `service_keys` permits an empty scope set and refuses everything it
    /// asks for -- default deny, and a state a hand-edited file really can be
    /// in. A blank line there would read as "unrestricted", which is the
    /// exact opposite.
    #[test]
    fn a_key_with_no_scopes_says_it_is_refused_everything() {
        let line = scopes_wording(&[]);
        assert!(line.contains("refused everything"), "got {line}");
        // The control: a key WITH a scope does not say that.
        let one = scopes_wording(&[Scope { subject: Subject::All, access: Access::Read }]);
        assert!(!one.contains("refused everything"), "got {one}");
    }

    /// Each grant reads back as what it may do and what it may do it to,
    /// in that order -- because the access is the half that decides whether a
    /// mistake here costs a disclosure or an edit.
    #[test]
    fn a_scope_reads_back_as_its_access_and_its_subject() {
        assert_eq!(
            scope_wording(&Scope { subject: Subject::All, access: Access::Read }),
            "Read everything in the vault"
        );
        assert_eq!(
            scope_wording(&Scope {
                subject: Subject::Category(ItemKind::SecureNote),
                access: Access::Write,
            }),
            "Write every secure note"
        );
        assert_eq!(
            scope_wording(&Scope {
                subject: Subject::Item("abc-123".to_string()),
                access: Access::Read,
            }),
            "Read the item abc-123"
        );
    }

    /// **A scope this build cannot parse is shown, and shown as granting
    /// nothing.**
    ///
    /// This is what an older build reading a newer file sees, and
    /// `service_keys` keeps the subject verbatim precisely so it survives the
    /// round trip. A row that silently omitted it would be this screen
    /// claiming a key is narrower than the file says -- on a build that is
    /// merely out of date, which is the one case the owner would never
    /// suspect.
    #[test]
    fn a_scope_this_build_does_not_understand_is_shown_as_permitting_nothing() {
        let line = scope_wording(&Scope {
            subject: Subject::Unrecognised("folder:work".to_string()),
            access: Access::Read,
        });
        assert!(line.contains("folder:work"), "the scope was dropped from the list: {line}");
        assert!(line.contains("permitting nothing"), "got {line}");
    }

    /// **The three refusals `service_keys::mint` cannot see.**
    ///
    /// By the time `mint` is called, a box of nonsense where a number should
    /// be has become `None` ("never expires") and an unticked Read and Write
    /// have become an empty scope set ("refused everything") -- both of which
    /// are legitimate values it must accept. Here is the only place they are
    /// still distinguishable from what the owner meant.
    #[test]
    fn the_form_refuses_what_mint_would_silently_accept() {
        let base = KeyForm { name: "Backup".to_string(), ..KeyForm::default() };

        let nonsense = KeyForm { expiry_days: "soon".to_string(), ..base.clone() };
        assert_eq!(form_refusal(&nonsense), Some(EXPIRY_NOT_A_NUMBER));

        let no_id = KeyForm { subject: SubjectChoice::OneItem, ..base.clone() };
        assert_eq!(form_refusal(&no_id), Some(NO_ITEM_ID));

        let no_access = KeyForm { read: false, write: false, ..base.clone() };
        assert_eq!(form_refusal(&no_access), Some(NO_ACCESS));

        // The controls, one per refusal: each is the same form with the one
        // fault repaired, and each must pass -- otherwise the assertions
        // above could be a function that refuses everything.
        assert_eq!(form_refusal(&base), None, "the plain form is refused");
        assert_eq!(
            form_refusal(&KeyForm { expiry_days: "30".to_string(), ..base.clone() }),
            None
        );
        assert_eq!(
            form_refusal(&KeyForm {
                subject: SubjectChoice::OneItem,
                item_id: "abc-123".to_string(),
                ..base.clone()
            }),
            None
        );
        assert_eq!(form_refusal(&KeyForm { read: false, write: true, ..base }), None);
    }

    /// **The form opens on Read, and only Read.**
    ///
    /// Both off would make the first press of Mint a refusal for a reason
    /// nobody has been told yet. Both on would make the default key one that
    /// can *change* the vault, which is not what this screen is reached for
    /// -- and a default that grants more than the common case is how an
    /// over-scoped key gets minted by someone who never read that row.
    #[test]
    fn a_new_form_grants_read_over_everything_and_no_write() {
        let form = KeyForm::default();
        assert!(form.read);
        assert!(!form.write);
        assert_eq!(form.subject, SubjectChoice::Everything);
        assert_eq!(
            form_scopes(&form),
            vec![Scope { subject: Subject::All, access: Access::Read }]
        );
    }

    /// Both boxes ticked make two grants over the same subject, because
    /// `service_keys` has two flags with no hierarchy -- neither implies the
    /// other, so neither can be left out.
    #[test]
    fn read_and_write_are_two_grants_and_not_one() {
        let form = KeyForm {
            subject: SubjectChoice::Category(ItemKind::Login),
            write: true,
            ..KeyForm::default()
        };
        assert_eq!(
            form_scopes(&form),
            vec![
                Scope { subject: Subject::Category(ItemKind::Login), access: Access::Read },
                Scope { subject: Subject::Category(ItemKind::Login), access: Access::Write },
            ]
        );
        // The control: with Write alone there is exactly one grant, and it is
        // not a Read.
        let write_only = KeyForm { read: false, write: true, ..KeyForm::default() };
        assert_eq!(
            form_scopes(&write_only),
            vec![Scope { subject: Subject::All, access: Access::Write }]
        );
    }

    /// The days box is days, and a number too large to be days lands far in
    /// the future rather than wrapping into the past.
    ///
    /// `u64::MAX` is a value that can be typed into a text box. Wrapping
    /// arithmetic on it would produce an expiry *behind* now, which
    /// `service_keys::mint` refuses -- so the owner would type an
    /// unreasonably large number and be told their expiry had already passed,
    /// which is true of the arithmetic and nonsense to the reader.
    #[test]
    fn an_expiry_in_days_lands_that_many_days_out_and_never_behind_now() {
        let form = |days: &str| KeyForm { expiry_days: days.to_string(), ..KeyForm::default() };
        assert_eq!(form_expiry(&form(""), TEST_NOW), None, "empty is no expiry");
        assert_eq!(form_expiry(&form("30"), TEST_NOW), Some(TEST_NOW + 30 * 86_400));
        let huge = form_expiry(&form("18446744073709551615"), TEST_NOW).unwrap();
        assert!(huge > TEST_NOW, "a saturating expiry landed in the past: {huge}");
    }

    /// **A refusal from `service_keys` reaches the owner in its own words.**
    ///
    /// Those sentences are written to be acted on -- "pick another name, or
    /// revoke that one first" -- and this screen re-wording them, or
    /// flattening them to "could not mint a key", is how a refusal becomes an
    /// afternoon.
    #[test]
    fn a_refusal_from_service_keys_is_passed_through_verbatim() {
        let existing = vec![stored_key("Backup", None)];
        let form = KeyForm { name: "  backup ".to_string(), ..KeyForm::default() };
        let MintOutcome::Refused(message) =
            attempt_mint(&form, TEST_NOW, || TEST_KEY_BYTES, &existing)
        else {
            panic!("a duplicate name was minted");
        };
        assert_eq!(
            message,
            crate::service_keys::MintRefusal::DuplicateName("Backup".to_string()).message(),
            "the duplicate-name refusal was reworded on the way to the screen"
        );

        // An expiry of zero days is exactly now, which `mint` refuses -- and
        // it is refused in ITS words, not in a second sentence written here
        // that says the same thing differently.
        let dead = KeyForm {
            name: "Nightly".to_string(),
            expiry_days: "0".to_string(),
            ..KeyForm::default()
        };
        let MintOutcome::Refused(message) = attempt_mint(&dead, TEST_NOW, || TEST_KEY_BYTES, &[])
        else {
            panic!("a key that was dead on arrival was minted");
        };
        assert_eq!(
            message,
            crate::service_keys::MintRefusal::ExpiryAlreadyPassed.message()
        );

        // The control: one day, and the same form mints.
        let live = KeyForm { expiry_days: "1".to_string(), ..dead };
        assert!(matches!(
            attempt_mint(&live, TEST_NOW, || TEST_KEY_BYTES, &[]),
            MintOutcome::Minted(..)
        ));
    }

    /// **The record keeps the hash and never the key.**
    ///
    /// This is the whole bargain of the store: a key file that is read --
    /// backed up, synced, copied off a disk -- must not hand over working
    /// credentials. The assertion is made over the record's *serialised*
    /// form, because that is what reaches the file, and a field added later
    /// that carried the plaintext would be invisible to an assertion that
    /// only checked `hash`.
    #[test]
    fn the_stored_record_holds_the_hash_and_the_plaintext_appears_nowhere_in_it() {
        let form = KeyForm { name: "Backup".to_string(), ..KeyForm::default() };
        let MintOutcome::Minted(record, minted) =
            attempt_mint(&form, TEST_NOW, || TEST_KEY_BYTES, &[])
        else {
            panic!("the form was refused");
        };
        // The control: there IS a key, and it is the 64 hex characters
        // `service_token::mint` promises -- so the absence asserted below is
        // the absence of something real.
        assert_eq!(minted.secret.as_str(), TEST_KEY);
        assert_eq!(minted.secret.len(), 64);

        assert_eq!(record.hash, crate::service_keys::hash_key(TEST_KEY));
        assert_ne!(record.hash, TEST_KEY, "the hash IS the key");
        let stored = serde_json::to_string(&record).expect("the record serialises");
        assert!(
            !stored.contains(TEST_KEY),
            "the plaintext key is in what would be written to the store: {stored}"
        );
    }

    // -- the page, driven at the pane --------------------------------------

    /// **The service switch, driven at the pane**: the row exists, it is
    /// wired to `service_enabled`, and it is wired to THAT field and not to a
    /// neighbour -- which on this row is worth pinning twice, because the
    /// field it moves decides whether a decrypted vault is reachable over a
    /// socket.
    #[test]
    fn clicking_the_service_toggle_changes_the_setting_it_is_wired_to() {
        let ctx = tall_context();
        let mut state = api_state();
        assert!(!state.settings.service_enabled, "the default: nothing is listening");

        let first = tall_frame(&ctx, &mut state, &[]);
        let pills = first.rects_of_size(TOGGLE_SIZE);
        assert_eq!(
            pills.len(),
            1,
            "the Local API page paints exactly one pill -- the service switch. It used to be \
             the fifth of five, under the backend and disk-cache cards; those went back to \
             the Vault page, and a test that indexed past the end, or into a pill that had \
             followed them here, would be clicking something else"
        );
        let pill = pills[0].center();

        let _ = tall_frame(&ctx, &mut state, &click(pill));
        assert!(
            state.settings.service_enabled,
            "the switch did not turn on -- the row is painted but its value is never written \
             back, so the pill is decoration"
        );
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
        assert!(state.settings.use_official_bw_crypto, "the wrong row's toggle moved");
        assert!(!state.settings.cache_vault_to_disk, "the wrong row's toggle moved");
        assert!(state.settings.read_through_cache, "the wrong row's toggle moved");

        let _ = tall_frame(&ctx, &mut state, &click(pill));
        assert!(!state.settings.service_enabled, "and back off again");
    }

    /// The page says what turning it on means **while it is off**, which is
    /// the state every install starts in and the only state in which the
    /// sentence can still change someone's mind.
    #[test]
    fn the_page_says_what_turning_the_service_on_would_do() {
        let ctx = tall_context();
        let mut state = api_state();
        let painted = tall_frame(&ctx, &mut state, &[]);
        assert!(
            painted.any_containing("usernames, passwords, notes and two-factor secrets"),
            "the page does not say what would be served: {:?}",
            painted.strings()
        );
        assert!(
            painted.any_containing("any program on this PC"),
            "the page does not say who could use it"
        );
    }

    /// **An empty store is a result, not a blank panel**, and it says what
    /// the emptiness means: nothing can reach the service. "No keys" and
    /// "no restrictions" are the confusion this page exists to prevent.
    #[test]
    fn a_store_with_no_keys_says_nothing_can_reach_the_service() {
        let ctx = tall_context();
        let mut state = api_state();
        let painted = tall_frame(&ctx, &mut state, &[]);
        assert!(painted.any_containing("No keys have been minted"), "got {:?}", painted.strings());
    }

    /// **A listed key shows its name, its dates and its scopes -- and never
    /// a key.**
    ///
    /// There is nothing else it could show: the store holds `SHA-256(key)`.
    /// So the assertion that matters is the negative one, and it is made
    /// against the *hash* as well as the plaintext -- a row that helpfully
    /// printed the hash would be publishing the one value an offline
    /// attacker wants.
    #[test]
    fn a_stored_key_is_listed_by_name_and_scope_and_never_by_its_secret() {
        let ctx = tall_context();
        let mut state = api_state();
        state.show_service_keys(vec![stored_key("Backup script", None)]);
        let painted = tall_frame(&ctx, &mut state, &[]);

        assert!(painted.contains("Backup script"), "got {:?}", painted.strings());
        assert!(painted.any_containing("Never expires"));
        assert!(painted.any_containing("Read everything in the vault"));
        // The control: the empty-state sentence is gone, so the list really
        // did draw a row.
        assert!(!painted.any_containing("No keys have been minted"));

        let hash = crate::service_keys::hash_key(TEST_KEY);
        for painted_text in painted.strings() {
            assert!(!painted_text.contains(TEST_KEY), "a key is on screen: {painted_text}");
            assert!(!painted_text.contains(&hash), "a key's hash is on screen: {painted_text}");
        }
    }

    /// **The minted key is shown, once, and is gone the moment it is
    /// dismissed.**
    ///
    /// This is the contract the whole store rests on: the plaintext exists in
    /// one place, on screen, until the owner has taken it. What is *kept* is
    /// a hash -- so a screen that could show the key again would mean the key
    /// had been stored somewhere it should not be, and a screen that lost it
    /// before the owner copied it would mean minting again.
    #[test]
    fn a_minted_key_is_shown_until_it_is_dismissed_and_then_never_again() {
        let ctx = tall_context();
        let mut state = api_state();
        state.key_form.name = "Backup".to_string();

        let first = tall_frame(&ctx, &mut state, &[]);
        // The control: it is not on screen before the button is pressed.
        assert!(
            !first.contains(TEST_KEY),
            "the key was on screen before anything was minted: {:?}",
            first.strings()
        );
        let mint = first.ink_of(MINT_BUTTON).rect.center();

        let minted = tall_click(&ctx, &mut state, mint);
        assert!(
            minted.contains(TEST_KEY),
            "the minted key is not readable anywhere: {:?}",
            minted.strings()
        );
        assert!(
            minted.any_containing("cannot be shown again"),
            "the page does not say this is the only time"
        );

        // Still there on the next frame: it is the only copy in existence,
        // and a card that cleared itself on a repaint would destroy it.
        let again = tall_frame(&ctx, &mut state, &[]);
        assert!(again.contains(TEST_KEY), "the reveal did not survive a repaint");

        let done = again.ink_of(DONE_BUTTON).rect.center();
        let dismissed = tall_click(&ctx, &mut state, done);
        assert!(!dismissed.contains(TEST_KEY), "the key is still on screen after Done");
        assert!(state.minted.is_none(), "the plaintext is still being held");

        // And it is not recoverable: the record that was kept holds the hash,
        // and nothing on a later frame can produce the key again.
        let later = tall_frame(&ctx, &mut state, &[]);
        assert!(!later.contains(TEST_KEY), "the key came back on a later frame");
        assert_eq!(state.keys.len(), 1, "the record itself was lost with the reveal");
        assert_eq!(state.keys[0].hash, crate::service_keys::hash_key(TEST_KEY));
    }

    /// **Nothing that is kept holds the plaintext.**
    ///
    /// The frame above proves the key reaches the screen; this proves it
    /// reaches nothing else. Asserted over the serialised key list, because
    /// that is exactly the bytes `service_keys::save` would write, and a
    /// field added later that carried the key would be invisible to a check
    /// that only read `hash`.
    #[test]
    fn minting_puts_the_plaintext_on_screen_and_into_nothing_that_is_stored() {
        let ctx = tall_context();
        let mut state = api_state();
        state.key_form.name = "Backup".to_string();
        let first = tall_frame(&ctx, &mut state, &[]);
        let mint = first.ink_of(MINT_BUTTON).rect.center();
        let _ = tall_click(&ctx, &mut state, mint);

        let stored = serde_json::to_string(&state.keys).expect("the store serialises");
        assert!(
            stored.contains(&crate::service_keys::hash_key(TEST_KEY)),
            "the control: the record that would be written is in this text at all"
        );
        assert!(
            !stored.contains(TEST_KEY),
            "the plaintext key is in what would be written to the store: {stored}"
        );
        // The form is empty again, so the next key cannot be minted under the
        // last one's name by someone who did not look.
        assert!(state.key_form.name.is_empty());
    }

    /// The Copy button hands over the key, and does not clear the card.
    ///
    /// Copying is not the same act as being finished with it: a paste that
    /// went to the wrong window is a second Copy away only if the key is
    /// still on screen.
    #[test]
    fn copying_hands_over_the_key_and_leaves_it_on_screen() {
        let ctx = tall_context();
        let mut state = api_state();
        state.key_form.name = "Backup".to_string();
        state.show_key_copy(|key| {
            *COPIED.lock().expect("the copy sink") = Some(key.to_string());
        });

        let first = tall_frame(&ctx, &mut state, &[]);
        let mint = first.ink_of(MINT_BUTTON).rect.center();
        let minted = tall_click(&ctx, &mut state, mint);
        // The control: nothing has been copied yet, so the assertion after
        // the click is about the click.
        assert!(COPIED.lock().expect("the copy sink").is_none());

        let copy = minted.ink_of(COPY_BUTTON).rect.center();
        let after = tall_click(&ctx, &mut state, copy);
        assert_eq!(COPIED.lock().expect("the copy sink").as_deref(), Some(TEST_KEY));
        assert!(after.contains(TEST_KEY), "the card cleared itself on a copy");
    }

    /// **One click on Revoke removes nothing.**
    ///
    /// Revocation is the only irreversible control in Preferences, and what
    /// it breaks is somebody's unattended script. So the first press asks,
    /// and the question says what breaks rather than "are you sure".
    #[test]
    fn revoking_asks_before_it_removes_anything() {
        let ctx = tall_context();
        let mut state = api_state();
        state.show_service_keys(vec![stored_key("Backup", None)]);

        let first = tall_frame(&ctx, &mut state, &[]);
        let revoke = first.ink_of(REVOKE_BUTTON).rect.center();
        let asked = tall_click(&ctx, &mut state, revoke);
        assert_eq!(state.keys.len(), 1, "one click revoked the key with no confirmation");
        assert!(
            asked.any_containing("stops being answered"),
            "the question does not say what breaks: {:?}",
            asked.strings()
        );
        assert!(asked.any_containing("cannot be brought back"), "the question hides the undo");

        // Keeping it puts the row back as it was, with the key still there.
        let keep = asked.ink_of(REVOKE_CANCEL_BUTTON).rect.center();
        let kept = tall_click(&ctx, &mut state, keep);
        assert_eq!(state.keys.len(), 1);
        assert!(!kept.any_containing("stops being answered"), "the question is still up");
        assert!(state.pending_revoke.is_none());
    }

    /// Confirming really does revoke -- the control for the test above, which
    /// would otherwise pass just as well against a button that does nothing
    /// at all.
    #[test]
    fn confirming_a_revoke_removes_the_key() {
        let ctx = tall_context();
        let mut state = api_state();
        state.show_service_keys(vec![stored_key("Backup", None), stored_key("Nightly", None)]);

        let first = tall_frame(&ctx, &mut state, &[]);
        // The first key's own Revoke button: two are painted, and the one
        // higher on the page belongs to the row that was drawn first.
        let revoke = first
            .ink
            .iter()
            .filter(|i| i.source == REVOKE_BUTTON)
            .min_by(|a, b| a.rect.top().total_cmp(&b.rect.top()))
            .expect("a revoke button")
            .rect
            .center();
        let asked = tall_click(&ctx, &mut state, revoke);
        let confirm = asked.ink_of(REVOKE_CONFIRM_BUTTON).rect.center();
        let after = tall_click(&ctx, &mut state, confirm);

        assert_eq!(state.keys.len(), 1, "the confirmation did not remove the key");
        assert_eq!(state.keys[0].name, "Nightly", "the wrong key was revoked");
        assert!(!after.contains("Backup"), "the revoked key is still listed");
        assert!(after.contains("Nightly"), "the control: the other key is still listed");
    }

    /// **A store that cannot be written is said out loud, in both
    /// directions.**
    ///
    /// The two failures are not the same failure. A mint that was not saved
    /// leaves a key that works now and is gone after a restart. A revoke that
    /// was not saved leaves a key that is off this list and still opens the
    /// vault -- and an owner who believes they have revoked it. "Could not
    /// save" would describe neither.
    #[test]
    fn a_store_that_cannot_be_written_says_which_way_it_went_wrong() {
        let ctx = tall_context();
        let mut state = api_state();
        state.show_keys_sink(|_| Err("the disk is read-only".to_string()));
        state.key_form.name = "Backup".to_string();

        let first = tall_frame(&ctx, &mut state, &[]);
        let mint = first.ink_of(MINT_BUTTON).rect.center();
        let minted = tall_click(&ctx, &mut state, mint);
        assert!(
            minted.any_containing("gone the next time Deskwarden starts"),
            "the page does not say the key will not survive a restart: {:?}",
            minted.strings()
        );
        assert!(minted.any_containing("the disk is read-only"), "the reason was swallowed");

        let done = minted.ink_of(DONE_BUTTON).rect.center();
        let listed = tall_click(&ctx, &mut state, done);
        let revoke = listed.ink_of(REVOKE_BUTTON).rect.center();
        let asked = tall_click(&ctx, &mut state, revoke);
        let confirm = asked.ink_of(REVOKE_CONFIRM_BUTTON).rect.center();
        let after = tall_click(&ctx, &mut state, confirm);
        assert!(
            after.any_containing("it still works"),
            "the page let the owner believe a key was revoked when the file still grants it: \
             {:?}",
            after.strings()
        );
    }

    /// A refusal is shown on the page, in `service_keys`' words, and the form
    /// keeps what was typed into it -- a refused name is one edit away from a
    /// good one, not a form to fill in again.
    #[test]
    fn a_refused_mint_says_why_on_the_page_and_keeps_the_form() {
        let ctx = tall_context();
        let mut state = api_state();
        state.key_form.expiry_days = "30".to_string();

        let first = tall_frame(&ctx, &mut state, &[]);
        let mint = first.ink_of(MINT_BUTTON).rect.center();
        let refused = tall_click(&ctx, &mut state, mint);

        assert!(state.keys.is_empty(), "a key with no name was minted");
        assert!(state.minted.is_none(), "a refused mint still revealed a key");
        assert!(
            refused.any_containing("Give the key a name"),
            "the refusal is not on the page: {:?}",
            refused.strings()
        );
        assert_eq!(state.key_form.expiry_days, "30", "the form was cleared by a refusal");
    }

    /// **A paint state reaches no key store, no clipboard and no clock of its
    /// own.**
    ///
    /// `PrefsState::new` is what every test in this module builds, and the
    /// three seams on this page each lead somewhere real: the owner's
    /// `service-keys.json`, the Windows clipboard, and the machine's idea of
    /// now. The defaults have to be the inert answers.
    #[test]
    fn a_paint_state_reaches_no_key_store_and_no_clipboard() {
        let state = PrefsState::new(Settings::default());
        assert!(state.keys.is_empty(), "the constructor read a key store");
        assert!(
            (state.keys_sink)(&[stored_key("Backup", None)]).is_ok(),
            "the default sink reports a failure, so the page would say so on every mint"
        );
        // The control that the sink really wrote nothing: `service_keys_path`
        // is where a real one would write, and nothing in this module may
        // resolve it -- see `no_test_here_resolves_the_real_key_store`, which
        // pins that by reading this file's own source.
        (state.key_copy)("this must not reach the clipboard");
    }

    /// **The source pin over the test half**, in
    /// `no_test_here_resolves_the_real_scan_history`'s idiom and for the same
    /// reason, over the file beside it: `%APPDATA%\Deskwarden` is off limits
    /// to the suite, and the way a test would reach it is by accident.
    ///
    /// The key store is the worse of the two to touch. Reading it discloses
    /// what a machine's credentials are named and scoped; **writing it
    /// revokes every key the owner has ever minted**, including the ones
    /// their unattended scripts are holding.
    #[test]
    fn no_test_here_resolves_the_real_key_store() {
        let source = include_str!("prefs_ui.rs");
        let tests = source
            .split_once(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file")
            .1;
        for needle in [
            concat!("load_service_", "keys("),
            concat!("save_service_", "keys"),
            concat!("service_keys_", "path("),
            concat!("copy_", "secret"),
        ] {
            assert_eq!(
                tests.matches(needle).count(),
                0,
                "a test in this module spells `{needle}`, which reaches the owner's real key \
                 store or their real clipboard"
            );
        }
        // The positive controls: production really does spell each of them,
        // so counting zero above means something.
        for needle in [
            concat!("fn load_service_", "keys()"),
            concat!("fn save_service_", "keys("),
            concat!("fn service_keys_", "path()"),
            concat!("clipboard::copy_", "secret"),
        ] {
            // Two for the clipboard path -- the field's own doc names it as
            // well as the line that installs it -- and one for each of the
            // rest. The number is the control, not the name: it is here so
            // that a needle which has drifted reads as a failure rather than
            // as a clean zero above.
            let expected =
                if needle.ends_with(concat!("copy_", "secret")) { 2 } else { 1 };
            assert_eq!(
                source.matches(needle).count(),
                expected,
                "{needle:?} is no longer spelled that way -- the needle above has drifted and \
                 its absence proves nothing"
            );
        }
    }

    /// **The file this screen writes is the file the service reads.**
    ///
    /// It used to be possible for those to differ: this screen spelled the
    /// name, and the service built its own path from a private function in
    /// `main.rs`, so the compiler could not hold the two together. Drift
    /// would have been silent in both directions -- each half working
    /// perfectly, the owner told a key exists while every request using it
    /// was refused.
    ///
    /// **The fix was a shared constant, not a better test**, so what is
    /// pinned now is that the sharing survives: neither side may go back to
    /// spelling the name itself. An earlier version of this test asserted
    /// `main.rs` *contained* the literal, and it fired the moment the
    /// duplication was removed -- correctly, and as a sign it was guarding
    /// the wrong property.
    #[test]
    fn neither_side_spells_the_key_store_name_for_itself() {
        let service = include_str!("main.rs");
        assert!(
            service.contains("service_keys::key_store_path"),
            "`main.rs` no longer routes through the shared path helper, so the service and this \
             screen can now disagree about which file holds the keys"
        );
        assert!(
            !service.contains(SERVICE_KEYS_FILE_NAME),
            "`main.rs` spells the key store's name itself again; it must come from \
             `service_keys::KEY_STORE_FILE_NAME` so the two cannot drift"
        );

        // Control: the needle really is what `main.rs` would have to contain,
        // rather than a string that could never appear either way.
        assert_eq!(SERVICE_KEYS_FILE_NAME, "service-keys.json");
        assert!(
            include_str!("service_keys.rs").contains("pub const KEY_STORE_FILE_NAME"),
            "control: the shared constant is gone, so this test is guarding nothing"
        );
    }

    /// UTC, so every painted date in these tests is exact wherever the suite
    /// runs. The page itself uses `local_time::SystemZone`.

    fn zone() -> crate::local_time::FixedOffset {
        crate::local_time::FixedOffset(0)
    }

    /// One frame of a fresh window on `section`.
    fn paint(section: Section) -> Painted {
        paint_settings(section, Settings::default())
    }

    /// The same, on the tall viewport.
    ///
    /// **For the absence assertions**, which are the ones a culling scroll
    /// region can make vacuous: on `BODY_SIZE` a page that scrolls hands back
    /// only what fits the window, so "this label is not painted here" would
    /// be satisfied by a label that is merely below the fold. On `TALL_BODY`
    /// nothing is culled, so an absence is a real absence.
    fn paint_tall(section: Section) -> Painted {
        let ctx = tall_context();
        let mut state = PrefsState::new(Settings::default());
        state.section = section;
        tall_frame(&ctx, &mut state, &[])
    }

    /// One frame of General on a pane of a given width. `frame` and its
    /// `raw_input` are pinned to `BODY_SIZE`; the wrapping assertions need
    /// more than one width, and a row that fits at 1000 points is not thereby
    /// known to fit at 652.
    /// One page, painted at an arbitrary width. The long-copy tests below run
    /// at three widths, and on the page the copy under test is actually on --
    /// which is no longer the same page for all of them, since the breach row
    /// moved. A second copy of this harness is how two of them would come to
    /// disagree about what "the pane" is.
    fn paint_section_at(section: Section, width: f32) -> Painted {
        let size = Vec2::new(width, BODY_SIZE.y);
        let input = |events: &[egui::Event]| egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            events: events.to_vec(),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(input(&[]), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(&[]), |_ui| {});
        let mut state = PrefsState::new(Settings::default());
        state.section = section;
        let output = ctx.run_ui(input(&[]), |ui| draw_prefs_body(ui, &mut state));
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        painted
    }

    /// One frame of the fill-hotkey row at a given hotkey status.
    ///
    /// Drives [`fill_hotkey_row`] directly rather than going through
    /// [`paint`]'s `draw_section`, because the status `draw_section` reads is
    /// process-wide (`hotkey::availability`) and the tests in this binary run in
    /// parallel: a test that set it would be setting it for whatever else was
    /// painting a General page at that instant. (It was a Shortcuts page until
    /// that section -- one unrebindable chip -- was removed and the row moved
    /// to General.)
    ///
    /// The row is drawn inside a [`card`], because on General it is one row of
    /// one, and a row painted onto bare canvas would sit on the wrong ground.
    fn paint_fill_hotkey_at(status: crate::hotkey::HotkeyStatus) -> Painted {
        let ctx = styled_context();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, BODY_SIZE)),
            ..Default::default()
        };
        let output = ctx.run_ui(input, |ui| card(ui, |ui| fill_hotkey_row(ui, status)));
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        painted
    }

    /// `settings.rs`'s walk, over a file with one test module.
    ///
    /// A line that is exactly a `cfg(test)` gate followed by a column-0 module
    /// opener starts a skip that runs to the next column-0 `}`; inside a
    /// module every item is indented, so that brace is the module's own.
    /// Line-ending agnostic, for the reason the original gives: `lines()`
    /// strips the carriage return, so this reads the same on a CRLF working
    /// tree and an LF checkout.
    fn production_half(source: &str) -> (String, usize) {
        let mut kept: Vec<&str> = Vec::new();
        let mut cut = 0usize;
        let mut gated = false;
        let mut skipping = false;
        for line in source.lines() {
            if skipping {
                if line == "}" {
                    skipping = false;
                }
                continue;
            }
            if gated && line.starts_with("mod ") {
                // The gate line was pushed on the previous turn; it belongs to
                // the module being cut.
                kept.pop();
                skipping = true;
                cut += 1;
                gated = false;
                continue;
            }
            gated = line.trim() == concat!("#[cfg(", "test)]");
            kept.push(line);
        }
        assert!(
            !skipping,
            "a test module was opened and never closed by a column-0 brace, so the rest of the \
             file was dropped and every needle counted over this reads nothing"
        );
        (kept.join("\n"), cut)
    }

    fn paint_settings(section: Section, settings: Settings) -> Painted {
        let ctx = styled_context();
        let mut state = PrefsState::new(settings);
        state.section = section;
        frame(&ctx, &mut state, &[])
    }

    // -- the encrypted disk cache row --------------------------------------

    /// One frame of a page whose Hello answer is `available`, since that is
    /// the only input to this row and no other harness here can supply it.
    /// One frame of the **Vault** page with Windows Hello answering
    /// `available`, on the tall viewport that page's scroll region needs.
    ///
    /// It was `paint_general_with_hello` and it moved with the rows. The
    /// viewport is `TALL_BODY` rather than `BODY_SIZE` for
    /// [`tall_frame`]'s reason exactly: a `ScrollArea` culls what is outside
    /// its viewport, so a short frame would find a row missing and could not
    /// tell that from a row that was never drawn. Reachability on the real
    /// window is a separate claim, asserted separately.
    fn paint_vault_with_hello(available: bool) -> Painted {
        let ctx = tall_context();
        let mut state = PrefsState::new(Settings::default());
        state.section = Section::Vault;
        state.show_hello_available(if available { || true } else { || false });
        tall_frame(&ctx, &mut state, &[])
    }

    /// The copy is the requirement, so these assertions exist so that a
    /// future edit which quietly drops the uncomfortable half of a sentence
    /// fails a test rather than shipping.
    #[test]
    fn the_available_disk_cache_copy_states_the_survives_a_lock_behaviour() {
        let text = disk_cache_description();
        assert!(text.contains("usernames, passwords, notes and two-factor secrets"));
        assert!(text.contains("not deleted when your vault locks"));
        assert!(text.contains("log out"));
        assert!(text.contains("7 days"));
        assert!(
            text.contains("Anyone who can run programs as you"),
            "the copy stopped naming the residual attacker"
        );
        assert!(
            !text.to_lowercase().contains("secure"),
            "the copy must describe what gates the file, not call it secure"
        );
    }

    /// **The copy does not promise a TPM, because the file no longer has
    /// one.**
    ///
    /// This replaces a test that asserted the opposite: that the row read
    /// "Unavailable -- needs Windows Hello" and promised no weaker file would
    /// be written. Both were true until the key moved to the same protection
    /// `user_key_store` uses, and the old test's subject no longer exists --
    /// keeping it would have pinned the defect.
    ///
    /// What replaces it is the stronger claim, and the one that matters: a
    /// file protected by Windows, offered under copy naming a TPM, would be
    /// this app making a false security claim to somebody deciding whether to
    /// write their vault to disk.
    #[test]
    fn the_disk_cache_copy_claims_no_protection_the_file_does_not_have() {
        let text = disk_cache_description();
        for lie in ["TPM", "Windows Hello", "Hello"] {
            assert!(
                !text.contains(lie),
                "the disk-cache copy still names `{lie}`, but the file is sealed with a key \
                 protected by Windows and nothing else -- promising more than the file has \
                 is the one thing this paragraph may never do: {text}"
            );
        }
        assert!(
            text.contains("protected by Windows"),
            "control: the copy no longer says what DOES gate the file, so the assertions \
             above would pass on a paragraph that promises nothing at all: {text}"
        );
        assert!(
            text.contains("takes this disk"),
            "the copy names the program-running-as-you attacker but not the stolen-disk one, \
             which is the protection that went when the TPM key did"
        );
        assert!(
            !text.to_lowercase().contains("secure"),
            "the copy must describe what gates the file, not call it secure"
        );
    }

    /// **A machine without Windows Hello can use the disk cache.**
    ///
    /// This replaces a test that asserted the opposite -- that the row was
    /// drawn with a reason it could not be used, pointing at Windows
    /// Settings. That reason stopped existing when the file's key stopped
    /// needing Hello to unseal, so the old test's subject is gone; what would
    /// be a loosening is deleting it and asserting nothing in its place.
    ///
    /// The claim now is the useful one: Hello's absence changes nothing about
    /// this row.
    #[test]
    fn the_disk_cache_row_is_usable_without_windows_hello() {
        let without = paint_vault_with_hello(false);
        assert!(
            without.contains(DISK_CACHE_LABEL),
            "the disk-cache row is not on the Vault page at all: {:?}",
            without.strings()
        );
        assert!(
            !without.any_containing("Sign-in options"),
            "the page still sends the user to Windows Settings to enable Hello for a file \
             that no longer needs it"
        );
        // Control: the row reads the same with Hello present, so the
        // assertion above is about a row that is live either way rather than
        // one that has quietly vanished from this page.
        let with = paint_vault_with_hello(true);
        assert!(
            with.contains(DISK_CACHE_LABEL),
            "control: the row is missing even with Hello available, so the page changed in \
             some way this test cannot see"
        );
    }

    #[test]
    fn a_frame_cannot_turn_the_disk_cache_on_while_hello_is_missing() {
        // The ghosted row returns the stored value unchanged, so there is no
        // frame in which the setting reads as on with no key to run on.
        let ctx = tall_context();
        let mut state = PrefsState::new(Settings::default());
        state.section = Section::Vault;
        state.show_hello_available(|| false);
        let _ = tall_frame(&ctx, &mut state, &[]);
        assert!(!state.settings.cache_vault_to_disk);
    }

    #[test]
    fn the_vault_page_says_what_is_in_the_file_when_hello_is_available() {
        let painted = paint_vault_with_hello(true);
        assert!(
            painted.any_containing("usernames, passwords, notes and two-factor secrets"),
            "the page does not say what is in the file"
        );
        assert!(
            painted.any_containing("not deleted when your vault locks"),
            "the page does not state the survives-a-lock behaviour"
        );
    }

    /// **The disk-cache switch, driven at the pane.** The row exists, it is
    /// wired to `cache_vault_to_disk`, and it is wired to THAT field and not
    /// to a neighbour -- which for this row is worth pinning twice over,
    /// since the neighbour above it decides whether a background process
    /// runs and this one decides whether a decrypted vault goes on the disk.
    #[test]
    fn clicking_the_disk_cache_toggle_changes_the_setting_it_is_wired_to() {
        let ctx = tall_context();
        let mut state = PrefsState::new(Settings::default());
        state.section = Section::Vault;
        state.show_hello_available(|| true);
        assert!(!state.settings.cache_vault_to_disk, "the default: nothing on disk");

        let first = tall_frame(&ctx, &mut state, &[]);
        // THIRD pill down on the Vault page: `keep_backend_running` and
        // `keep_ui_loaded` are above it, and the disk-cache card's own child
        // is directly below. Named by index rather than by position on a
        // card, because that is what `rects_of_size` returns -- and asserted
        // against the count, so an index that has drifted fails here rather
        // than quietly clicking the row above.
        let pills = first.rects_of_size(TOGGLE_SIZE);
        assert_eq!(
            pills.len(),
            4,
            "the Vault page paints four pills: `keep_backend_running`, the instant-open \
             switch beside it, and the disk copy and its child. It was FIVE while the \
             backend choice was an on/off pill; that row is a two-cell `choice_button` \
             picker now and paints no 40x22 rectangle at all, so every index below is one \
             lower than it was -- and an index that has drifted must fail here rather than \
             quietly click the row above"
        );
        let pill = pills[2].center();
        tall_frame(&ctx, &mut state, &click(pill));
        assert!(
            state.settings.cache_vault_to_disk,
            "the disk-cache toggle did not turn on -- the row is painted but its value is \
             never written back, so the pill is decoration"
        );
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
        assert!(state.settings.use_official_bw_crypto, "the wrong row's toggle moved");
        assert!(!state.settings.service_enabled, "the wrong row's toggle moved");
        assert!(state.settings.prompt_on_match, "the wrong row's toggle moved");
        assert!(state.settings.auto_lock_enabled, "the wrong row's toggle moved");

        tall_frame(&ctx, &mut state, &click(pill));
        assert!(!state.settings.cache_vault_to_disk, "and back off again");
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
    }

    /// **The disk-cache pair is one decision with a veto, and the screen
    /// says so.**
    ///
    /// [`crate::backend_policy::read_path`] answers `ServiceOnly` whenever
    /// `cache_vault_to_disk` is off, *whatever* `read_through_cache` says --
    /// the first vetoes the second. Two switches side by side, both live,
    /// would tell the user they were choosing between four configurations
    /// when there are three, and would leave "read from that copy first"
    /// switched on next to "keep a copy" switched off: a screen that says a
    /// file is being consulted which does not exist.
    ///
    /// Three claims, because any one alone is passed by the defect. The child
    /// is **inert** while the parent is off (not merely grey); it says **why**
    /// in the row, readable without hovering; and the policy really does
    /// ignore it, so the ghosting is describing the behaviour rather than
    /// inventing a rule of its own.
    #[test]
    fn the_read_through_row_is_a_child_of_the_disk_copy_and_not_a_second_switch() {
        use crate::backend_policy::{read_path, ReadPath};

        let ctx = tall_context();
        let mut state = PrefsState::new(Settings::default());
        state.section = Section::Vault;
        state.show_hello_available(|| true);
        // The premise: shipped, the copy is OFF and the read-through is ON --
        // which is precisely the pair that would read as a contradiction if
        // both rows were live.
        assert!(!state.settings.cache_vault_to_disk, "the shipped default");
        assert!(state.settings.read_through_cache, "the shipped default");
        assert_eq!(
            read_path(state.settings.cache_vault_to_disk, state.settings.read_through_cache),
            ReadPath::ServiceOnly,
            "the premise: with no copy permitted, the read-through setting decides nothing"
        );

        let first = tall_frame(&ctx, &mut state, &[]);
        // FOURTH pill: the backend card's two, then the disk copy, then this.
        // The backend choice itself paints no pill -- it is a two-cell
        // picker -- so the four are `keep_backend_running`, `keep_ui_loaded`,
        // the disk copy, and this.
        let child = first.rects_of_size(TOGGLE_SIZE)[3].center();
        tall_frame(&ctx, &mut state, &click(child));
        assert!(
            state.settings.read_through_cache,
            "the read-through pill wrote its setting while there is no copy to read, so the \
             two rows are a pair of independent switches after all"
        );
        assert!(
            first.contains(read_through_description(false)),
            "the ghosted child does not say what would make it available, so it reads as a \
             bug rather than as a consequence of the row above; got {:?}",
            first.strings()
        );

        // **The control**, and without it the assertions above are passed by
        // a row that is inert always. Turn the parent on -- through the pane,
        // so it is the same click path -- and the child is live, moves its
        // own field, and changes what the policy answers.
        let parent = first.rects_of_size(TOGGLE_SIZE)[2].center();
        tall_frame(&ctx, &mut state, &click(parent));
        assert!(state.settings.cache_vault_to_disk, "the control could not turn the parent on");
        assert_eq!(
            read_path(state.settings.cache_vault_to_disk, state.settings.read_through_cache),
            ReadPath::CacheFirst
        );

        let second = tall_frame(&ctx, &mut state, &[]);
        let child = second.rects_of_size(TOGGLE_SIZE)[3].center();
        tall_frame(&ctx, &mut state, &click(child));
        assert!(
            !state.settings.read_through_cache,
            "the child is inert even with a copy to read, so the row is decoration and the \
             refusal above proved nothing"
        );
        assert!(
            state.settings.cache_vault_to_disk,
            "clicking the child moved the parent's field"
        );
        assert_eq!(
            read_path(state.settings.cache_vault_to_disk, state.settings.read_through_cache),
            ReadPath::ServiceOnly,
            "the child's field moved but the read path did not follow it"
        );
    }

    #[test]
    fn a_paint_state_reaches_no_windows_hello_probe_of_its_own() {
        // `PrefsState::new` is what roughly forty tests build, and the real
        // probe is an OS call. The default has to be the inert answer, not
        // whatever this machine happens to say.
        let state = PrefsState::new(Settings::default());
        assert!(!(state.hello_available)());
    }

    // -- the shell ---------------------------------------------------------

    #[test]
    fn every_nav_section_design_3e_lists_is_painted() {
        let painted = paint(Section::General);
        // The seven labels, spelled out rather than looped over
        // `Section::ALL`: a test that re-derives its expectation from the
        // enum under test would still pass if a section were renamed,
        // removed, or added.
        //
        // **In order, and with the length asserted.** This used to be a bag
        // of `contains` calls, which is a shape that cannot notice a section
        // being ADDED -- adding Clipboard passed it untouched. Order matters
        // because the nav is a reading order and Clipboard belongs after
        // Security; the length matters because it is the half a `contains`
        // loop is structurally blind to.
        let expected = [
            "General",
            // Breaches sits directly after General because that is where its
            // one pill used to be, and where a reader will look for it.
            "Breaches",
            // **Five rows are gone from between Breaches and Vault, and from
            // between Clipboard and Updates, and this list is re-pinned to
            // say so rather than loosened to tolerate it.** Autofill, Native
            // apps, Security, Shortcuts and Sync & account were the five
            // pages on which a user could change nothing -- three of them a
            // single sentence saying so, one a read-only chip, one two rows
            // of prose. A nav row promises a decision; those five charged a
            // click to answer "not here". The one fact that had no other
            // home, the fill hotkey, is a row on General now
            // (`fill_hotkey_row`), and `general_names_the_fill_hotkey_and_
            // its_state` is what stops it going missing with them.
            //
            // The vault page sits directly after Breaches because that is
            // where a reader looking for "where does my vault come from"
            // looks first -- it sat after Security for exactly that reason
            // while Security existed. It was "Vault service" while the service was all it
            // carried, and the service has since gone to a page of its own;
            // what the row names now is the backend choice and the disk
            // cache, and a nav row has to name the whole page or the
            // settings on it are unfindable.
            "Vault",
            // And directly after it, the endpoint that serves what Vault
            // decided is being served. It came off the bottom of Vault, so by
            // the rule Breaches and Updates were placed by, it lands where
            // the bulk of it came from -- one row down from the page a reader
            // last saw it on.
            "Local API",
            "Clipboard",
            // Updates sits directly BEFORE About, not after General, because
            // that is where nearly all of it came from -- the check button,
            // the notes, the download and the restart were all a card on the
            // About page. Same rule as Breaches, opposite answer, because the
            // bulk of the page came from somewhere else.
            "Updates",
            "About",
        ];
        for label in expected {
            assert!(
                painted.contains(label),
                "nav row {label:?} was not painted; got {:?}",
                painted.strings()
            );
        }
        assert_eq!(
            Section::ALL.len(),
            expected.len(),
            "a section was added or removed without this list being re-pinned"
        );
        // Top to bottom, by the painted y of each label's ink.
        for pair in expected.windows(2) {
            let above = painted.nav_ink_of(pair[0]).rect;
            let below = painted.nav_ink_of(pair[1]).rect;
            assert!(
                above.top() < below.top(),
                "the nav lists {:?} above {:?}, which is not the reading order",
                pair[1],
                pair[0]
            );
        }
    }

    #[test]
    fn exactly_one_nav_row_is_highlighted() {
        // `BLUE_WASH` is 3e's selected-row fill and appears nowhere else on
        // this window, so counting it counts selections.
        // Any section but the default one, so "the selection follows
        // `state.section`" is what is being counted and not "General happens
        // to be highlighted". This was `Section::Autofill`, which no longer
        // exists; Clipboard is the nearest surviving row to where it sat.
        let painted = paint(Section::Clipboard);
        assert_eq!(painted.count_filled(theme::BLUE_WASH), 1);
    }

    #[test]
    fn clicking_a_nav_row_opens_that_section() {
        // Without this, every section could be painted and the nav still be
        // decoration -- which is the same defect as a switch that does
        // nothing, one level up.
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        assert_eq!(state.section, Section::General);

        let first = frame(&ctx, &mut state, &[]);
        let target = first.rect_of("About").center();
        let after = frame(&ctx, &mut state, &click(target));

        assert_eq!(state.section, Section::About, "the nav row did not select");
        assert!(
            after.contains(&version_line()),
            "About should now be the open page; got {:?}",
            after.strings()
        );
    }

    #[test]
    fn the_nav_footer_carries_the_real_crate_version() {
        let painted = paint(Section::General);
        assert!(
            painted.contains(&format!("Deskwarden {}", env!("CARGO_PKG_VERSION"))),
            "got {:?}",
            painted.strings()
        );
        assert!(
            !painted.strings().iter().any(|t| t.contains("1.4.0")),
            "\"1.4.0\" is the design document's mock version, not this build's"
        );
    }

    // -- General -----------------------------------------------------------

    #[test]
    fn general_paints_every_setting_that_actually_exists() {
        let painted = paint(Section::General);
        // **Not the backend row.** It is a child of the crypto switch on
        // `Section::Vault` now (it passed through Sync & account on the way,
        // and that page has since been removed for having nothing left on it);
        // `every_setting_that_decides_where_the_vault_comes_from_is_on_the_
        // vault_page` is where it is asserted, and the negative below is what
        // would catch it being drawn on both pages at once.
        assert!(
            !painted.contains(BACKEND_LABEL),
            "the backend row is painted on General as well as on its own page: two live \
             switches for one setting is worse than the confusing pair this move fixed"
        );
        assert!(painted.contains("Lock the vault when you step away"));
        assert!(painted.contains(AUTO_LOCK_ENABLED_DESCRIPTION), "got {:?}", painted.strings());
        assert!(painted.contains("Lock the vault after"));
        // The descriptions too: a row whose right-hand control squeezes the
        // text column to nothing still paints its title, so asserting only on
        // titles would not notice.
        assert!(painted.contains(PROMPT_LABEL), "got {:?}", painted.strings());
        assert!(painted.contains(PROMPT_DESCRIPTION), "got {:?}", painted.strings());
        assert!(
            PROMPT_DESCRIPTION.contains("CTRL+ALT+B"),
            "the description has to say what is left when the prompt is off -- otherwise the              toggle reads as \"switch autofill off\", which it never is"
        );
        assert!(painted.contains(AUTO_LOCK_DESCRIPTION), "got {:?}", painted.strings());
        assert!(
            AUTO_LOCK_DESCRIPTION.contains("One minute is the shortest"),
            "the floor has to be stated on screen, not only enforced"
        );
        assert!(
            painted.contains("15"),
            "the default timeout should be shown in the stepper; got {:?}",
            painted.strings()
        );
    }

    #[test]
    fn general_paints_exactly_six_toggles_and_one_stepper() {
        let painted = paint(Section::General);
        assert_eq!(
            painted.count_of_size(Vec2::new(40.0, 22.0)),
            6,
            "six 40x22 pills: `prompt_on_match`, `fetch_icons`, its child \
             `fetch_icons_direct`, `use_brand_logos`, `reveal_totp_seed` and \
             `auto_lock_enabled`, and nothing else. The sixth is the direct-fetch child, \
             and it is counted whether or not it is GHOSTED: `child_toggle_row` paints a \
             disabled pill at the same 40x22 as an enabled one, so switching site icons \
             off must not change this number. \
             FIVE settings are no longer here and all five left for the same reason -- to \
             sit beside the thing that governs them. `check_breaches` moved to Breaches, \
             `check_for_updates` moved to Updates, and `keep_backend_running`, \
             `cache_vault_to_disk` and `read_through_cache` are now on `Section::Vault` with \
             the backend choice, which is the one page where all of them can be weighed \
             against each other; see `draw_breaches`, `draw_updates` and `Section::Vault`"
        );
        assert_eq!(
            painted.count_of_size(Vec2::new(112.0, 28.0)),
            1,
            "one 112x28 stepper box: `auto_lock_minutes`"
        );
    }

    #[test]
    fn a_stored_value_below_the_floor_opens_on_the_value_actually_in_effect() {
        // `auto_lock_minutes: 0` is what a hand-written "never lock" looks
        // like, and `auto_lock_timeout` uses one minute for it. Showing "0"
        // here would be a control displaying a number that is not the number
        // in force.
        let painted =
            paint_settings(Section::General, Settings { auto_lock_minutes: 0, ..Settings::default() });
        assert!(painted.contains("1"), "got {:?}", painted.strings());
        assert!(!painted.contains("0"), "got {:?}", painted.strings());
    }

    #[test]
    fn clicking_the_toggle_changes_the_setting_it_is_wired_to() {
        // The whole point of not shipping the other sections' switches: this
        // is what a switch is supposed to do, and it is asserted rather than
        // assumed.
        //
        // **On the Vault page, not General and no longer on Sync & account.**
        // The row is a child of the crypto switch and moved with it; this
        // test moved with them both rather than being repointed at whatever
        // now sits first on General, which would have left
        // `keep_backend_running` with no click coverage at all while still
        // passing.
        //
        // This is the row the owner trapped themselves with -- turning it off
        // stopped the vault window opening, and Preferences is inside that
        // window -- so "there is a place you can click this" is the claim
        // that matters, and this test is where it is made.
        let ctx = tall_context();
        let mut state = PrefsState::new(Settings::default());
        state.section = Section::Vault;
        // A signed-in account on Bitwarden's own cloud, so `choose` answers
        // `BwServe` and the child is LIVE. Ghosted, `child_toggle_row` hands
        // back the stored value and the click below would do nothing --
        // which is asserted separately, and is a different claim from this
        // one.
        state.show_account_source(|| {
            Some(AccountStatus::SignedIn { email: Some("me@example.com".to_string()), server: None })
        });
        assert!(state.settings.keep_backend_running, "the default");

        let first = tall_frame(&ctx, &mut state, &[]);
        // FIRST pill: the parent above it is a two-cell `choice_button`
        // picker and paints no 40x22 rectangle, so this child is the topmost
        // pill on the page. Clicking it must not move the parent, which is
        // what the neighbouring assertion here pins -- a child wired to its
        // parent's field is exactly the mix-up this card's rebuild could have
        // introduced.
        let pill = first.rects_of_size(TOGGLE_SIZE)[0].center();
        tall_frame(&ctx, &mut state, &click(pill));
        assert!(!state.settings.keep_backend_running);
        assert!(state.settings.use_official_bw_crypto, "the parent's toggle moved");
        assert!(!state.settings.cache_vault_to_disk, "a neighbouring card's toggle moved");
        assert!(!state.settings.service_enabled, "a neighbouring card's toggle moved");

        tall_frame(&ctx, &mut state, &click(pill));
        assert!(state.settings.keep_backend_running, "and back again");
        assert!(state.settings.use_official_bw_crypto, "the parent's toggle moved");
    }

    /// The ghosted child is inert, not merely grey.
    ///
    /// `child_toggle_row`'s contract is that it hands back the stored value
    /// unchanged when unavailable, and this is that contract at the pane for
    /// this row: on a self-hosted account with `bw` crypto off, clicking
    /// where the pill is must not write a preference about a subprocess this
    /// configuration does not start.
    #[test]
    fn the_ghosted_backend_pill_does_not_change_the_setting_when_clicked() {
        let ctx = tall_context();
        let mut settings = Settings::default();
        settings.use_official_bw_crypto = false;
        let mut state = PrefsState::new(settings);
        state.section = Section::Vault;
        state.show_account_source(|| {
            Some(AccountStatus::SignedIn {
                email: Some("me@example.com".to_string()),
                server: Some("https://vault.example.com".to_string()),
            })
        });
        assert!(state.settings.keep_backend_running, "the default");

        let first = tall_frame(&ctx, &mut state, &[]);
        let pill = first.rects_of_size(TOGGLE_SIZE)[0].center();
        tall_frame(&ctx, &mut state, &click(pill));
        assert!(
            state.settings.keep_backend_running,
            "a ghosted row wrote its setting anyway, so the pill is live and only looks dead"
        );
    }

    /// **The switch this whole change is about, driven at the pane.**
    ///
    /// The row exists, it is wired to `prompt_on_match`, and it is wired to
    /// THAT field and not to a neighbour -- which is the defect this file has
    /// three rows to make possible. The two neighbours are asserted unmoved
    /// in both directions.
    #[test]
    fn clicking_the_prompt_toggle_turns_the_match_prompt_off_and_on_again() {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        assert!(state.settings.prompt_on_match, "the default: a match prompts");

        let first = frame(&ctx, &mut state, &[]);
        // FIRST pill down now: the disk-cache pair followed the backend row
        // off this page, to `Section::Vault`, so the prompt row is the top
        // of General's card.
        let pill = first.rects_of_size(Vec2::new(40.0, 22.0))[0].center();
        frame(&ctx, &mut state, &click(pill));
        assert!(
            !state.settings.prompt_on_match,
            "the prompt toggle did not turn off, so the one control that governs what a              matched window does is inert"
        );
        // What the toggle is FOR, asserted on the value the dispatch actually
        // consumes rather than on the flag alone: a field that flips without
        // reaching `match_disposition` is a switch that does nothing.
        assert_eq!(
            crate::app::match_disposition(state.settings.prompt_on_match),
            crate::app::MatchDisposition::Nothing
        );
        // ... and the hotkey is still armed, which is the whole reason this
        // is a prompt switch and not an autofill switch.
        assert!(
            crate::app::match_arms_hotkey(state.settings.prompt_on_match),
            "turning the prompt off has turned autofill off entirely"
        );
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
        assert!(state.settings.auto_lock_enabled, "the wrong row's toggle moved");

        frame(&ctx, &mut state, &click(pill));
        assert!(state.settings.prompt_on_match, "and back on again");
        assert_eq!(
            crate::app::match_disposition(state.settings.prompt_on_match),
            crate::app::MatchDisposition::Prompt
        );
    }

    /// **The breach switch, driven at the pane.**
    ///
    /// The counter-assertions are the test: a row wired to `prompt_on_match`
    /// or to `keep_backend_running` would still flip *a* setting on this
    /// click, and an assertion that only read `check_breaches` after the fact
    /// would be satisfied by a row wired to nothing at all if the field
    /// happened to move. All three neighbours start `true` and are asserted
    /// so before the click, which is what makes "unmoved" a claim that can
    /// fail.
    #[test]
    fn clicking_the_breach_toggle_changes_the_setting_it_is_wired_to() {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        // **On the Breaches page now, not General.** The pill moved to sit
        // beside the scan button it does not govern and the sentence that
        // says so.
        state.section = Section::Breaches;
        assert!(
            !state.settings.check_breaches,
            "the default: nothing about a password leaves the machine until this is clicked"
        );
        assert!(state.settings.keep_backend_running, "the neighbour starts true");
        assert!(state.settings.prompt_on_match, "the neighbour starts true");
        assert!(state.settings.auto_lock_enabled, "the neighbour starts true");

        let first = frame(&ctx, &mut state, &[]);
        let pills = first.rects_of_size(Vec2::new(40.0, 22.0));
        assert_eq!(
            pills.len(),
            1,
            "the Breaches page paints exactly one pill -- the consent switch -- and the scan \
             button is not a pill"
        );
        let pill = pills[0].center();

        frame(&ctx, &mut state, &click(pill));
        assert!(
            state.settings.check_breaches,
            "the breach toggle did not turn on -- the row is painted but its value is never              written back, so the pill is decoration"
        );
        assert!(state.settings.prompt_on_match, "the wrong row's toggle moved");
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
        assert!(state.settings.auto_lock_enabled, "the wrong row's toggle moved");
        assert!(!state.settings.reveal_totp_seed, "the wrong row's toggle moved");

        frame(&ctx, &mut state, &click(pill));
        assert!(!state.settings.check_breaches, "and back off again");
        assert!(state.settings.prompt_on_match, "the wrong row's toggle moved");
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
        assert!(state.settings.auto_lock_enabled, "the wrong row's toggle moved");
        assert!(!state.settings.reveal_totp_seed, "the wrong row's toggle moved");
    }

    /// Where the row is, read off the paint rather than off the source order:
    /// `draw_general` could call the rows in any order and lay them out in
    /// another, and "directly under the prompt row" is a claim about the
    /// screen.
    #[test]
    fn the_breach_row_is_the_first_thing_on_the_breaches_page() {
        let painted = paint(Section::Breaches);
        let breach = painted.ink_of(BREACH_LABEL).rect;
        let scan = painted.ink_of(SCAN_SECTION_LABEL).rect;
        let consent = painted.ink_of(crate::breach_scan::SCAN_CONSENT_NOTE).rect;
        let history = painted.ink_of(SCAN_HISTORY_LABEL).rect;
        // The instrument first: four labels at four distinct, non-empty
        // heights, so `top()` is telling them apart rather than reading one
        // number four times.
        for rect in [breach, scan, consent, history] {
            assert!(rect.height() > 0.0, "a label has no box: {rect:?}");
        }
        assert!(
            breach.top() < scan.top(),
            "the consent pill is not above the scan button it does not govern: breach at \
             {breach:?}, scan at {scan:?}"
        );
        assert!(
            scan.top() < consent.top(),
            "the sentence explaining the button is not under the button"
        );
        assert!(
            consent.top() < history.top(),
            "the history is not last: it is a record of what has been sent, and it belongs \
             under the thing that sends it"
        );
        assert!(
            scan.top() - breach.top() > 1.0,
            "the pill and the scan heading are painted at the same height, so the ordering \
             assertions above cannot fail"
        );

        // The pill follows the label, so it is the row that moved and not
        // just its text -- and there is exactly ONE pill on this page.
        let pills = painted.rects_of_size(Vec2::new(40.0, 22.0));
        assert_eq!(pills.len(), 1);
        assert!(
            pills[0].bottom() < scan.top(),
            "the consent pill overhangs the scan row"
        );

        // And it is no longer on General, which is the half a test that only
        // looked at this page would be blind to.
        let general = paint(Section::General);
        assert!(
            !general.contains(BREACH_LABEL),
            "the breach row is still painted on General as well, so there are two of it: {:?}",
            general.strings()
        );
    }

    // -- Breaches ----------------------------------------------------------

    /// **No test in this module reads the real scan history.**
    ///
    /// `%APPDATA%\\Deskwarden` is off limits to the suite, and the way a test
    /// would reach it is by accident: `PrefsState::new` used to load the file
    /// itself, which would have made every paint test in here a reader of the
    /// user's own record. The loading constructor is separate and named, and
    /// this is a source pin over the test half saying it is never used there.
    #[test]
    fn no_test_here_resolves_the_real_scan_history() {
        let source = include_str!("prefs_ui.rs");
        let tests = source
            .split_once(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file")
            .1;
        for needle in [concat!("with_scan_", "history("), concat!("load_scan_", "history(")] {
            assert_eq!(
                tests.matches(needle).count(),
                0,
                "a test in this module spells `{needle}`, which reads the real \
                 %APPDATA%\\Deskwarden scan history"
            );
        }
        // The positive control: production really does spell both, so
        // counting zero above means something.
        assert_eq!(
            source.matches(concat!("pub fn with_scan_", "history(")).count(),
            1,
            "the loading constructor is no longer spelled that way -- the needle above has \
             drifted and its absence proves nothing"
        );
        assert_eq!(
            source.matches(concat!("fn load_scan_", "history()")).count(),
            1,
            "the loader is no longer spelled that way -- see above"
        );
    }

    /// A Breaches page parked in `stage`, with `history` under the button.
    ///
    /// **Nothing here can reach the network, and that is structural rather
    /// than careful**: `ScanPanel::parked` has no receiver, and
    /// `begin_scan` refuses outright without a process-wide `ScanEnv` that
    /// only `main.rs` installs and no test does. So a frame here makes no
    /// request and spawns no thread even if one of these clicks landed on the
    /// button.
    fn paint_scan(
        stage: crate::breach_scan::ScanStage,
        history: Vec<crate::scan_history::ScanRecord>,
    ) -> Painted {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        state.section = Section::Breaches;
        state.scan = crate::breach_scan::ScanPanel::parked(stage);
        state.scan_history = crate::scan_history::ScanHistory { entries: history };
        frame(&ctx, &mut state, &[])
    }

    fn record(finished_at: i64, found: u32, failed: u32) -> crate::scan_history::ScanRecord {
        crate::scan_history::ScanRecord {
            finished_at_unix_millis: finished_at,
            passwords_checked: 128,
            items_covered: 1_600,
            found,
            failed,
        }
    }

    /// The idle page: the switch, the button, the sentence that says the
    /// button ignores the switch, and an empty history that says so out loud.
    #[test]
    fn the_idle_scan_page_offers_the_button_and_claims_nothing() {
        let painted = paint_scan(crate::breach_scan::ScanStage::Idle, vec![]);
        for text in [
            BREACH_LABEL,
            SCAN_SECTION_LABEL,
            SCAN_IDLE_DESCRIPTION,
            SCAN_BUTTON,
            crate::breach_scan::SCAN_CONSENT_NOTE,
            SCAN_HISTORY_LABEL,
            SCAN_NO_HISTORY,
        ] {
            assert!(painted.contains(text), "{text:?} is not on the page: {:?}", painted.strings());
        }
        assert!(
            !painted.contains(SCAN_RUNNING_BUTTON),
            "an idle page paints the running button as well as the idle one"
        );
    }

    /// **The failure count is on screen WHILE the scan runs**, not only at the
    /// end. A run that will finish with forty failures must not look clean for
    /// the first thirty seconds of it.
    #[test]
    fn a_running_scan_reports_its_failures_as_they_happen() {
        let painted = paint_scan(
            crate::breach_scan::ScanStage::Running { done: 60, total: 128, found: 3, failed: 40 },
            vec![],
        );
        let expected = crate::breach_scan::progress_wording(60, 128, 3, 40);
        assert!(expected.contains("40 could not be checked"), "control: {expected:?}");
        assert!(painted.contains(&expected), "got {:?}", painted.strings());
        // The button is still there, disabled and saying what is happening --
        // a control that vanishes mid-action reflows the card under the
        // cursor.
        assert!(
            painted.contains(SCAN_RUNNING_BUTTON) || painted.contains(SCAN_BUTTON),
            "the button disappeared while the scan ran: {:?}",
            painted.strings()
        );
    }

    /// A finished run says both numbers and ends on the failures, in
    /// `breach_scan`'s own words -- so the page and `scan_history.json`
    /// cannot disagree about what happened.
    #[test]
    fn a_finished_scan_reports_the_failures_last() {
        let done = record(1_787_013_000_000, 3, 40);
        let painted = paint_scan(crate::breach_scan::ScanStage::Finished(done), vec![]);
        let expected = crate::breach_scan::outcome_wording(&done);
        assert!(painted.contains(&expected), "got {:?}", painted.strings());
        assert!(
            expected.ends_with("40 could not be checked, so nothing is known about them."),
            "control: {expected:?}"
        );
    }

    /// "Nothing to check" is its own wording and not a run that found zero.
    /// Those are different results and drawing them alike is the mistake
    /// `password_health::Summary::NothingToCheck` exists to avoid.
    #[test]
    fn a_vault_with_no_passwords_says_so_rather_than_reporting_a_clean_run() {
        let painted = paint_scan(crate::breach_scan::ScanStage::NothingToScan, vec![]);
        assert!(painted.contains(SCAN_NOTHING_DESCRIPTION), "got {:?}", painted.strings());
        assert!(
            !painted.contains("None was found in a breach."),
            "an unscannable vault was reported as a clean scan"
        );
    }

    /// A build with no environment says so rather than offering a button that
    /// does nothing.
    #[test]
    fn a_build_that_cannot_scan_says_so_on_the_page() {
        let painted = paint_scan(crate::breach_scan::ScanStage::Unavailable, vec![]);
        assert!(painted.contains(SCAN_UNAVAILABLE_DESCRIPTION), "got {:?}", painted.strings());
    }

    /// **The history, newest first, with each entry's own outcome under it.**
    #[test]
    fn the_history_lists_each_scan_newest_first_with_what_it_found() {
        // 2026-08-18T00:30:00Z and a day earlier. Stated instants, never the
        // clock: see `local_time`.
        let newest = record(1_787_013_000_000, 3, 0);
        let older = record(1_787_013_000_000 - 86_400_000, 0, 7);
        let painted = paint_scan(
            crate::breach_scan::ScanStage::Idle,
            vec![newest, older],
        );
        assert!(!painted.contains(SCAN_NO_HISTORY), "the empty state is drawn over real entries");
        let top = painted.ink_of(&scan_history_when(&newest)).rect;
        let bottom = painted.ink_of(&scan_history_when(&older)).rect;
        assert!(top.height() > 0.0 && bottom.height() > 0.0, "an entry has no box");
        assert!(
            top.top() < bottom.top(),
            "the history is not newest-first: newest at {top:?}, older at {bottom:?}"
        );
        assert!(
            painted.contains(&crate::breach_scan::outcome_wording(&older)),
            "an entry's outcome is not under it: {:?}",
            painted.strings()
        );
    }

    /// **A recorded scan's time is the user's own, and never says "UTC".**
    ///
    /// The stored instant is UTC; the label is not. This asserts the rule
    /// rather than a particular offset, because the offset is the machine's
    /// and no test in this crate may depend on which machine that is.
    #[test]
    fn a_history_entry_names_no_timezone() {
        let when = scan_history_when(&record(1_787_013_000_000, 0, 0));
        assert!(!when.contains("UTC") && !when.contains("GMT"), "{when:?}");
        assert!(when.contains("Aug 2026") || when.contains("Aug 2026"), "{when:?}");
    }

    /// **The one call to `begin_scan` on this page is behind the button**, and
    /// a parked panel proves the click reaches it without anything being able
    /// to run: `begin_scan` with no installed environment lands on
    /// `Unavailable`, which is a state change only a real click can produce.
    #[test]
    fn clicking_the_scan_button_asks_for_a_scan() {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        state.section = Section::Breaches;
        assert!(!state.settings.check_breaches, "the premise: the setting is OFF");

        let first = frame(&ctx, &mut state, &[]);
        let button = first.rect_of(SCAN_BUTTON).center();
        frame(&ctx, &mut state, &click(button));

        // **The decision, driven rather than described.** With no `ScanEnv`
        // installed the request cannot be carried out, and the page says so
        // -- but it was ATTEMPTED, which is the whole point: the button is
        // not gated on the pill above it.
        assert_eq!(
            *state.scan.stage(),
            crate::breach_scan::ScanStage::Unavailable,
            "the button did nothing with the setting off, which is the control that refuses to \
             be clicked this design exists to delete"
        );
        assert!(
            !state.settings.check_breaches,
            "pressing the button silently turned the automatic setting on as well, which is \
             consent the user did not give"
        );
    }

    /// **The long copy, at every width this module paints at.**
    ///
    /// `BREACH_DESCRIPTION` is longer than any other row's, so it is the one
    /// that can wrap out of the card or into the row below. Asserted on the
    /// painted galley -- its placed glyphs, its line count and its colour --
    /// rather than on the layout rect the row was allocated, because a row
    /// allocated 570 points and painted 900 wide has a perfectly correct
    /// rect.
    #[test]
    fn the_breach_description_stays_inside_the_pane() {
        assert!(
            BREACH_DESCRIPTION.len() > 200,
            "the copy under test is not the long one, so this test is measuring nothing: {}",
            BREACH_DESCRIPTION.len()
        );
        // Every width the module already paints or measures at: the body's
        // own, and the modal card at both pane sizes `modal_card_rect`'s
        // tests use.
        let widths = [
            BODY_SIZE.x,
            modal_card_rect(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 820.0))).width(),
            modal_card_rect(Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 500.0))).width(),
        ];
        let mut visited = 0;
        for width in widths {
            let pane = Rect::from_min_size(Pos2::ZERO, Vec2::new(width, BODY_SIZE.y));
            // The positive control, per width: `contains_rect` has to be able
            // to say no here, or every assertion below is vacuous.
            assert!(
                !pane.contains_rect(Rect::from_min_size(
                    Pos2::new(pane.max.x - 1.0, 0.0),
                    Vec2::new(50.0, 10.0)
                )),
                "`contains_rect` cannot fail at width {width}"
            );

            let painted = paint_section_at(Section::Breaches, width);
            let ink = painted.ink_of(BREACH_DESCRIPTION);
            assert_eq!(
                ink.rendered.split_whitespace().collect::<Vec<_>>(),
                BREACH_DESCRIPTION.split_whitespace().collect::<Vec<_>>(),
                "the description was elided to fit at width {width}; egui laid {:?}",
                ink.rendered
            );
            assert!(
                ink.color.a() > 0,
                "the description is painted at alpha 0 at width {width}, so every geometry                  assertion here is reading a shape that is not on screen"
            );
            assert!(
                ink.rows >= 2,
                "the long copy laid out in {} line(s) at width {width} -- either it did not                  wrap, or the copy under test is not the long one",
                ink.rows
            );
            assert!(
                pane.contains_rect(ink.rect),
                "the description is painted at {:?}, outside the {width}-wide pane {pane:?}",
                ink.rect
            );
            // **The rows either side of it, and re-pinned deliberately.**
            // This list used to name the auto-lock rows as "the row below",
            // which they were until the site-icons row was inserted between
            // them. They are no longer adjacent to this description AND, at
            // the narrowest width here, the taller card now pushes them out
            // of the painted body altogether -- so `ink_of` panics on them
            // rather than measuring anything.
            //
            // Naming the site-icons row instead is a strengthening rather
            // than a relaxation: an overlap can only happen between rows that
            // are actually next to each other, and `FETCH_ICONS_DESCRIPTION`
            // is the other long one, so this is now the hardest pair on the
            // card rather than a pair separated by two rows.
            // **The rows either side of it, re-pinned for the page the row
            // is actually on now.** Its neighbours used to be the prompt row
            // above and the site-icons row below; on Breaches it is the first
            // thing on the page, and what sits under it is the scan card --
            // whose own description is the other long copy here, so this is
            // still the hardest pair on the page rather than a pair separated
            // by two rows.
            for neighbour in [
                BREACH_LABEL,
                SCAN_SECTION_LABEL,
                SCAN_IDLE_DESCRIPTION,
                crate::breach_scan::SCAN_CONSENT_NOTE,
            ] {
                let other = painted.ink_of(neighbour).rect;
                assert!(
                    !ink.rect.intersects(other),
                    "the description at {:?} overlaps {neighbour:?} at {other:?} at width                      {width}",
                    ink.rect
                );
            }
            visited += 1;
        }
        assert_eq!(visited, widths.len(), "a width was skipped");
        assert!(visited >= 3, "fewer widths than the module tests");
    }

    /// **The site-icons switch, driven at the pane**, with the same
    /// counter-assertions the breach row carries and for the same reason: a
    /// row wired to a neighbour would still flip *a* setting on this click.
    ///
    /// This one starts `true`, so the first click turns it OFF -- which is
    /// the direction that matters, and the direction a copy-pasted test
    /// written for an off-by-default row would have got backwards.
    #[test]
    fn clicking_the_site_icons_toggle_changes_the_setting_it_is_wired_to() {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        assert!(
            state.settings.fetch_icons,
            "the default: icons are shown until this is clicked"
        );
        assert!(!state.settings.check_breaches, "the neighbour starts false");
        assert!(!state.settings.reveal_totp_seed, "the neighbour starts false");
        assert!(state.settings.keep_backend_running, "the neighbour starts true");
        assert!(state.settings.prompt_on_match, "the neighbour starts true");
        assert!(state.settings.auto_lock_enabled, "the neighbour starts true");

        let first = frame(&ctx, &mut state, &[]);
        let pills = first.rects_of_size(Vec2::new(40.0, 22.0));
        assert_eq!(pills.len(), 6, "the General card no longer paints six pills");
        // SECOND pill down now: prompt, site icons, network logos, TOTP
        // secret, auto-lock. Five rows have left this page -- the breach row
        // to Breaches, the update row to Updates, and the backend row plus
        // the disk-cache pair to `Section::Vault`, where they are read
        // together with the service switch.
        let pill = pills[1].center();

        frame(&ctx, &mut state, &click(pill));
        assert!(
            !state.settings.fetch_icons,
            "the site-icons toggle did not turn off -- the row is painted but its value is \
             never written back, so the pill is decoration and the domains keep going out"
        );
        assert!(!state.settings.check_breaches, "the wrong row's toggle moved");
        assert!(!state.settings.reveal_totp_seed, "the wrong row's toggle moved");
        assert!(state.settings.prompt_on_match, "the wrong row's toggle moved");
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
        assert!(state.settings.auto_lock_enabled, "the wrong row's toggle moved");

        frame(&ctx, &mut state, &click(pill));
        assert!(state.settings.fetch_icons, "and back on again");
        assert!(!state.settings.check_breaches, "the wrong row's toggle moved");
        assert!(!state.settings.reveal_totp_seed, "the wrong row's toggle moved");
    }

    /// **The direct-fetch switch, driven at the pane.** Starts `false`, so
    /// the first click turns it ON -- the direction that matters for a row
    /// whose ON state is what discloses something.
    #[test]
    fn clicking_the_direct_icons_toggle_changes_the_setting_it_is_wired_to() {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        assert!(
            !state.settings.fetch_icons_direct,
            "the default: icons come from the icon service until this is clicked"
        );
        assert!(state.settings.fetch_icons, "its master switch starts on, so the row is live");
        assert!(!state.settings.use_brand_logos, "the neighbour below starts false");
        assert!(!state.settings.reveal_totp_seed, "the neighbour starts false");
        assert!(state.settings.prompt_on_match, "the neighbour starts true");
        assert!(state.settings.auto_lock_enabled, "the neighbour starts true");

        let first = frame(&ctx, &mut state, &[]);
        let pills = first.rects_of_size(Vec2::new(40.0, 22.0));
        assert_eq!(pills.len(), 6, "the General card no longer paints six pills");
        // THIRD pill down: prompt, site icons, THIS, network logos, TOTP
        // secret, auto-lock. It sits directly under its master switch.
        let pill = pills[2].center();

        frame(&ctx, &mut state, &click(pill));
        assert!(
            state.settings.fetch_icons_direct,
            "the direct-fetch toggle did not turn on -- the row is painted but its value is \
             never written back, so the pill is decoration"
        );
        assert!(state.settings.fetch_icons, "the master switch above moved instead");
        assert!(!state.settings.use_brand_logos, "the wrong row's toggle moved");
        assert!(!state.settings.reveal_totp_seed, "the wrong row's toggle moved");
        assert!(state.settings.prompt_on_match, "the wrong row's toggle moved");
        assert!(!state.settings.check_breaches, "the wrong row's toggle moved");
        assert!(state.settings.auto_lock_enabled, "the wrong row's toggle moved");

        frame(&ctx, &mut state, &click(pill));
        assert!(!state.settings.fetch_icons_direct, "and back off again");
        assert!(state.settings.fetch_icons, "the wrong row's toggle moved");
    }

    /// **It is a child, and a disabled child is disabled and not merely
    /// grey.** With site icons off, "where from" is not a question, so the
    /// row stops sensing clicks -- and it still PAINTS ITS PILL, which is why
    /// the count pin says six either way.
    #[test]
    fn with_site_icons_off_the_direct_fetch_row_is_inert_but_still_painted() {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings { fetch_icons: false, ..Settings::default() });
        assert!(!state.settings.fetch_icons_direct, "the default");

        let first = frame(&ctx, &mut state, &[]);
        let pills = first.rects_of_size(Vec2::new(40.0, 22.0));
        assert_eq!(
            pills.len(),
            6,
            "a ghosted child row still paints a 40x22 pill, so switching the master off must \
             not change the count -- and every index below it must not shift either"
        );
        frame(&ctx, &mut state, &click(pills[2].center()));
        assert!(
            !state.settings.fetch_icons_direct,
            "a click on the ghosted row still edited the setting, so 'disabled' is only a \
             colour here"
        );

        // **The positive control, and it is the whole test.** The identical
        // click with the master switch ON does move it -- so the assertion
        // above is about the disabled state and not about the click having
        // landed nowhere.
        let mut live = PrefsState::new(Settings::default());
        let live_first = frame(&ctx, &mut live, &[]);
        let live_pills = live_first.rects_of_size(Vec2::new(40.0, 22.0));
        assert_eq!(live_pills.len(), 6);
        assert_eq!(
            live_pills[2].center(),
            pills[2].center(),
            "the row is at a different place in the two states, so the click above was not \
             aimed at the same row"
        );
        frame(&ctx, &mut live, &click(live_pills[2].center()));
        assert!(live.settings.fetch_icons_direct, "the control failed: the row is inert either way");
    }

    /// The copy is on screen, and it says the two things a user cannot infer
    /// from the label: what turning it on costs them, and that addresses on
    /// their own network are not what this pill decides.
    #[test]
    fn the_direct_icons_row_says_what_it_costs_and_what_it_does_not_govern() {
        let painted = paint(Section::General);
        assert!(painted.contains(DIRECT_ICONS_LABEL), "got {:?}", painted.strings());
        assert!(painted.contains(DIRECT_ICONS_DESCRIPTION), "got {:?}", painted.strings());
        assert!(
            DIRECT_ICONS_DESCRIPTION.contains("Off by default"),
            "the default is stated in the copy, as every other consent row on this page states \
             its own"
        );
        // The cost, as a consequence rather than as a mechanism. "connects to
        // each site" is a true sentence a reader nods at without
        // understanding what it discloses; this is the sentence that says it.
        assert!(
            DIRECT_ICONS_DESCRIPTION.contains("an entry for it exists"),
            "the copy does not say what ON discloses to each site: {DIRECT_ICONS_DESCRIPTION:?}"
        );
        assert!(
            DIRECT_ICONS_DESCRIPTION.contains("192.168"),
            "the copy does not name the addresses this pill does NOT govern, so a user reading \
             the label would think it did: {DIRECT_ICONS_DESCRIPTION:?}"
        );
        // The house rule, as a test: this page never calls anything secure.
        assert!(
            !DIRECT_ICONS_DESCRIPTION.to_ascii_lowercase().contains("secure"),
            "the copy calls something secure: {DIRECT_ICONS_DESCRIPTION:?}"
        );
        // The instrument: an ink lookup that panics on a double paint, with a
        // real rect, so `contains` above is not reading a zero-size ghost.
        let ink = painted.ink_of(DIRECT_ICONS_LABEL);
        assert!(ink.rect.height() > 0.0 && ink.rect.width() > 0.0, "the label has no box: {:?}", ink.rect);
        assert!(ink.color.a() > 0, "the label is painted at alpha {}", ink.color.a());
        let desc = painted.ink_of(DIRECT_ICONS_DESCRIPTION);
        assert!(desc.rows >= 2, "a description this long should wrap; it took {} row(s)", desc.rows);
        // And it sits under the switch it is a child of, read off the paint.
        let master = painted.ink_of(FETCH_ICONS_LABEL).rect;
        assert!(master.height() > 0.0 && ink.rect.height() > 0.0);
        assert!(
            master.top() < ink.rect.top(),
            "the child row is painted above its master switch: master at {master:?}, child at \
             {:?}",
            ink.rect
        );
        assert!(ink.rect.top() - master.top() > 1.0, "the two labels are at the same height");
    }

    /// **The network-logos switch, driven at the pane**, with the same
    /// counter-assertions its neighbours carry: a row wired to `fetch_icons`
    /// -- the row directly above it, and the one it is most likely to be
    /// confused with -- would still flip *a* setting on this click.
    ///
    /// This one starts `false`, so the first click turns it ON, which is the
    /// direction that matters for an off-by-default row.
    #[test]
    fn clicking_the_network_logos_toggle_changes_the_setting_it_is_wired_to() {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        assert!(
            !state.settings.use_brand_logos,
            "the default: cards wear their printed network name until this is clicked"
        );
        assert!(state.settings.fetch_icons, "the neighbour above starts true");
        assert!(!state.settings.reveal_totp_seed, "the neighbour below starts false");
        assert!(state.settings.keep_backend_running, "the neighbour starts true");
        assert!(state.settings.prompt_on_match, "the neighbour starts true");
        assert!(state.settings.auto_lock_enabled, "the neighbour starts true");

        let first = frame(&ctx, &mut state, &[]);
        let pills = first.rects_of_size(Vec2::new(40.0, 22.0));
        assert_eq!(pills.len(), 6, "the General card no longer paints six pills");
        // FOURTH pill down: prompt, site icons, the direct-fetch child of
        // site icons, network logos, TOTP secret, auto-lock. It was the
        // third until that child row was inserted above it.
        let pill = pills[3].center();

        frame(&ctx, &mut state, &click(pill));
        assert!(
            state.settings.use_brand_logos,
            "the network-logos toggle did not turn on -- the row is painted but its value is \
             never written back, so the pill is decoration"
        );
        assert!(state.settings.fetch_icons, "the wrong row's toggle moved");
        assert!(!state.settings.reveal_totp_seed, "the wrong row's toggle moved");
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
        assert!(state.settings.prompt_on_match, "the wrong row's toggle moved");
        assert!(state.settings.auto_lock_enabled, "the wrong row's toggle moved");

        frame(&ctx, &mut state, &click(pill));
        assert!(!state.settings.use_brand_logos, "and back off again");
        assert!(state.settings.fetch_icons, "the wrong row's toggle moved");
    }

    /// The copy is on screen, and it says the two things a user cannot infer
    /// from the pill: that they have to supply the images themselves, and that
    /// a brand with no image is not a blank -- it keeps its word.
    #[test]
    fn the_network_logos_row_says_where_the_images_come_from_and_what_happens_without_them() {
        let painted = paint(Section::General);
        assert!(painted.contains(BRAND_LOGOS_LABEL), "got {:?}", painted.strings());
        assert!(painted.contains(BRAND_LOGOS_DESCRIPTION), "got {:?}", painted.strings());
        assert!(
            BRAND_LOGOS_DESCRIPTION.contains("brand-marks"),
            "the copy has to name the folder: with none named, a user turns this on, sees no \
             change whatever, and has nothing to act on"
        );
        assert!(
            BRAND_LOGOS_DESCRIPTION.contains("Off by default"),
            "the default is stated in the copy, as every other row's is"
        );
        assert!(
            BRAND_LOGOS_DESCRIPTION.contains("Nothing is downloaded"),
            "on a page whose neighbouring row IS a network request, silence here reads as a \
             second one"
        );
    }

    /// **The update-check switch, driven at the pane.** Starts `true`, so the
    /// first click turns it OFF, with the same counter-assertions its
    /// neighbours carry.
    ///
    /// **On the Updates page now, not General.** The pill moved to sit above
    /// the check button it does not govern, exactly as `check_breaches`
    /// moved to sit above the scan button -- so it is the ONLY pill on its
    /// page, and this test finds it by being the only one rather than by
    /// counting six neighbours it no longer has.
    #[test]
    fn clicking_the_update_check_toggle_changes_the_setting_it_is_wired_to() {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        state.section = Section::Updates;
        assert!(
            state.settings.check_for_updates,
            "the default: Deskwarden tells you about releases until this is clicked"
        );
        assert!(state.settings.fetch_icons, "the neighbour starts true");
        assert!(state.settings.auto_lock_enabled, "the neighbour starts true");
        assert!(state.settings.keep_backend_running, "the neighbour starts true");
        assert!(state.settings.prompt_on_match, "the neighbour starts true");
        assert!(!state.settings.check_breaches, "the neighbour starts false");
        assert!(!state.settings.reveal_totp_seed, "the neighbour starts false");

        let first = frame(&ctx, &mut state, &[]);
        let pills = first.rects_of_size(Vec2::new(40.0, 22.0));
        assert_eq!(
            pills.len(),
            1,
            "the Updates page paints exactly one pill -- the consent switch -- and the check \
             button beside it is not one"
        );
        let pill = pills[0].center();

        frame(&ctx, &mut state, &click(pill));
        assert!(
            !state.settings.check_for_updates,
            "the update-check toggle did not turn off -- the row is painted but its value is \
             never written back, so the pill is decoration"
        );
        assert!(state.settings.auto_lock_enabled, "the wrong row's toggle moved");
        assert!(state.settings.fetch_icons, "the wrong row's toggle moved");
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
        assert!(state.settings.prompt_on_match, "the wrong row's toggle moved");
        assert!(!state.settings.check_breaches, "the wrong row's toggle moved");
        assert!(!state.settings.reveal_totp_seed, "the wrong row's toggle moved");
        // Nothing on General moved either, which is the half that would fail
        // if the pill had been copied to this page rather than moved to it.
        assert_eq!(state.settings.auto_lock_minutes, 15);

        frame(&ctx, &mut state, &click(pill));
        assert!(state.settings.check_for_updates, "and back on again");
    }

    /// **The update row is not on General at all any more**, and this is the
    /// test that used to say it was at the foot of that card. It asserts the
    /// move instead of being deleted: a page that quietly regrew the row
    /// would put the switch back on one page and the button it does not
    /// govern on another, which is the arrangement the reorganisation
    /// existed to end.
    ///
    /// The twin of `breaches_owns_the_consent_pill_and_general_does_not`,
    /// down to the second half -- the row is somewhere, not merely gone.
    #[test]
    fn updates_owns_the_check_pill_and_general_does_not() {
        let general = paint(Section::General);
        assert!(
            !general.contains(UPDATE_CHECK_LABEL),
            "the automatic-check row is still on General; it belongs above the button it does \
             not govern. Got {:?}",
            general.strings()
        );
        assert!(
            !general.contains(UPDATE_CHECK_DESCRIPTION),
            "General still carries the update row's copy: {:?}",
            general.strings()
        );
        // The rows that stayed, so this cannot pass by General having been
        // emptied.
        assert!(general.contains(AUTO_LOCK_LABEL));
        assert!(general.contains(FETCH_ICONS_LABEL));

        // And it is on Updates, above the check button rather than below it:
        // the pill is the rule and the button is the exception to it, and a
        // rule stated after its exception explains nothing.
        let updates = paint(Section::Updates);
        let pill = updates.ink_of(UPDATE_CHECK_LABEL).rect;
        let flow = updates.ink_of(UPDATE_SECTION_LABEL).rect;
        assert!(pill.height() > 0.0 && flow.height() > 0.0);
        assert!(
            pill.top() < flow.top(),
            "the setting is painted below the flow it governs the automatic half of: pill at \
             {pill:?}, flow at {flow:?}"
        );
        assert!(
            flow.top() - pill.top() > 1.0,
            "the two labels are painted at the same height, so the ordering assertion above \
             cannot fail"
        );
    }

    /// The copy is on screen, not merely declared, and it says the one thing
    /// this row has to say that the others do not: what switching it OFF
    /// costs. A missed security fix has no symptom.
    #[test]
    fn the_update_check_row_says_what_turning_it_off_costs() {
        let painted = paint(Section::Updates);
        assert!(painted.contains(UPDATE_CHECK_LABEL), "got {:?}", painted.strings());
        // The label is not the button's words. Both are on this page now, a
        // card apart, and two identical strings -- one a switch and one a
        // control it does not gate -- would erase the distinction this page
        // was built to make.
        assert_ne!(
            UPDATE_CHECK_LABEL, UPDATE_CHECK_BUTTON,
            "the automatic-check switch and the manual-check button are painted on one page and \
             must not read as the same thing twice"
        );
        assert!(painted.contains(UPDATE_CHECK_DESCRIPTION), "got {:?}", painted.strings());
        assert!(
            UPDATE_CHECK_DESCRIPTION.contains("security"),
            "the copy has to name what is lost by turning this off; a missed security fix is \
             invisible, so the pane is the only place the user can learn it"
        );
        assert!(
            UPDATE_CHECK_DESCRIPTION.contains("On by default"),
            "on-by-default is stated in `Settings::default` and has to be stated on screen too"
        );
        let ink = painted.ink_of(UPDATE_CHECK_LABEL);
        assert!(
            ink.rect.height() > 0.0 && ink.rect.width() > 0.0,
            "the label has no box: {:?}",
            ink.rect
        );
        assert!(ink.color.a() > 0, "the label is painted at alpha {}", ink.color.a());
        let desc = painted.ink_of(UPDATE_CHECK_DESCRIPTION);
        assert!(desc.color.a() > 0, "the description is painted at alpha {}", desc.color.a());
        assert!(desc.rows >= 2, "a description this long should wrap; it took {} row(s)", desc.rows);
    }

    /// The copy is on screen, not merely declared -- and it says the thing
    /// the row exists for: WHAT is disclosed (the domain) and what is not
    /// (the credential).
    #[test]
    fn the_site_icons_row_says_the_domain_is_what_is_sent() {
        let painted = paint(Section::General);
        assert!(painted.contains(FETCH_ICONS_LABEL), "got {:?}", painted.strings());
        assert!(painted.contains(FETCH_ICONS_DESCRIPTION), "got {:?}", painted.strings());
        assert!(
            FETCH_ICONS_DESCRIPTION.contains("domain"),
            "the copy has to name what is actually sent; \"downloads icons\" describes the \
             feature and hides the cost"
        );
        assert!(
            FETCH_ICONS_DESCRIPTION.contains("password"),
            "the copy has to say what is NOT sent -- \"it sends the website to Bitwarden\" is \
             what a worried reader assumes, and it is wrong"
        );
        assert!(
            FETCH_ICONS_DESCRIPTION.contains("On by default"),
            "on-by-default is stated in `Settings::default` and has to be stated on screen too \
             -- this is the one network row here that is on unless it is turned off"
        );
        // The instrument: an ink lookup that panics on a double paint, with a
        // real rect, so `contains` above is not reading a zero-size ghost.
        let ink = painted.ink_of(FETCH_ICONS_LABEL);
        assert!(
            ink.rect.height() > 0.0 && ink.rect.width() > 0.0,
            "the label has no box: {:?}",
            ink.rect
        );
        assert!(ink.color.a() > 0, "the label is painted at alpha {}", ink.color.a());
        let desc = painted.ink_of(FETCH_ICONS_DESCRIPTION);
        assert!(desc.color.a() > 0, "the description is painted at alpha {}", desc.color.a());
        assert!(desc.rows >= 2, "a description this long should wrap; it took {} row(s)", desc.rows);
    }

    /// The counter-assertions are the test, exactly as they are for the
    /// breach row above it: a row wired to `check_breaches` or to
    /// `prompt_on_match` would still flip *a* setting on this click, and an
    /// assertion that only read `reveal_totp_seed` afterwards would be
    /// satisfied by that. Every neighbour is asserted at its starting value
    /// BEFORE the click, which is what makes "unmoved" a claim that can fail.
    #[test]
    fn clicking_the_totp_secret_toggle_changes_the_setting_it_is_wired_to() {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        assert!(
            !state.settings.reveal_totp_seed,
            "the default: no TOTP seed is offered on the details screen until this is clicked"
        );
        assert!(!state.settings.check_breaches, "the neighbour starts false");
        assert!(state.settings.keep_backend_running, "the neighbour starts true");
        assert!(state.settings.prompt_on_match, "the neighbour starts true");
        assert!(state.settings.auto_lock_enabled, "the neighbour starts true");

        let first = frame(&ctx, &mut state, &[]);
        let pills = first.rects_of_size(Vec2::new(40.0, 22.0));
        assert_eq!(pills.len(), 6, "the General card no longer paints six pills");
        // FIFTH pill down now: prompt, site icons, the direct-fetch child
        // of site icons, network logos, TOTP secret, auto-lock. It was the
        // fourth until that child row was inserted.
        let pill = pills[4].center();

        frame(&ctx, &mut state, &click(pill));
        assert!(
            state.settings.reveal_totp_seed,
            "the TOTP-secret toggle did not turn on -- the row is painted but its value is never written back, so the pill is decoration"
        );
        assert!(!state.settings.check_breaches, "the wrong row's toggle moved");
        assert!(state.settings.prompt_on_match, "the wrong row's toggle moved");
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
        assert!(state.settings.auto_lock_enabled, "the wrong row's toggle moved");

        frame(&ctx, &mut state, &click(pill));
        assert!(!state.settings.reveal_totp_seed, "and back off again");
        assert!(!state.settings.check_breaches, "the wrong row's toggle moved");
        assert!(state.settings.prompt_on_match, "the wrong row's toggle moved");
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
        assert!(state.settings.auto_lock_enabled, "the wrong row's toggle moved");
    }

    /// Where the row is, read off the paint rather than off the source order,
    /// for the reason `the_breach_row_sits_under_the_prompt_row` gives.
    #[test]
    fn the_totp_secret_row_sits_between_the_icon_row_and_the_auto_lock_row() {
        let painted = paint(Section::General);
        let breach = painted.ink_of(FETCH_ICONS_LABEL).rect;
        let secret = painted.ink_of(TOTP_SECRET_LABEL).rect;
        let auto_lock = painted.ink_of(AUTO_LOCK_ENABLED_LABEL).rect;
        // The instrument first: three labels at three distinct, non-empty
        // heights, so `top()` is telling them apart rather than reading one
        // number three times.
        assert!(breach.height() > 0.0 && secret.height() > 0.0 && auto_lock.height() > 0.0);
        assert!(
            breach.top() < secret.top(),
            "the TOTP-secret row is not under the site-icons row: icons at {breach:?}, secret at {secret:?}"
        );
        assert!(
            secret.top() < auto_lock.top(),
            "the TOTP-secret row is not above the auto-lock row"
        );
        // The positive control: the tops differ by a real amount, so the
        // comparisons above are telling rows apart and not comparing one
        // number with itself.
        assert!(secret.top() - breach.top() > 1.0);
        assert!(auto_lock.top() - secret.top() > 1.0);

        // ... and the pills follow the labels, so it is the ROW that moved
        // and not just its text.
        //
        // **The indices are named, because they moved.** General now paints
        // prompt(0), site icons(1), the direct-fetch child of site icons(2),
        // network logos(3), TOTP secret(4), auto-lock(5) -- the disk-cache
        // pair that used to occupy 0 and 1 is on `Section::Vault`. The pill
        // under test is 4, and its neighbours are 3 and 5.
        let pills = painted.rects_of_size(Vec2::new(40.0, 22.0));
        assert_eq!(pills.len(), 6);
        assert!(pills[3].top() < pills[4].top(), "the TOTP-secret pill is not below the network-logos pill");
        assert!(pills[4].top() < pills[5].top(), "the TOTP-secret pill is not above the auto-lock pill");
        assert!(
            pills[4].top() > breach.bottom(),
            "the TOTP-secret pill is level with the site-icons row's text, so the pills and the labels disagree about which row is which"
        );
        assert!(
            pills[4].bottom() < auto_lock.top(),
            "the TOTP-secret pill overhangs the auto-lock row"
        );
    }

    /// The copy is on screen, not merely declared -- and it says the two
    /// things a user has to know before clicking: that it is off unless they
    /// turn it on, and that what it adds is a MASKED row rather than a seed
    /// painted in the clear.
    #[test]
    fn the_totp_secret_row_says_what_it_turns_on_and_that_it_is_off_by_default() {
        let painted = paint(Section::General);
        assert!(painted.contains(TOTP_SECRET_LABEL), "got {:?}", painted.strings());
        assert!(painted.contains(TOTP_SECRET_DESCRIPTION), "got {:?}", painted.strings());
        assert!(
            TOTP_SECRET_LABEL.contains("TOTP secret"),
            "the label has to name the thing it reveals, not just say \"secret\""
        );
        assert!(
            TOTP_SECRET_DESCRIPTION.contains("Off by default"),
            "off-by-default is stated in `Settings::default` and has to be stated on screen too"
        );
        assert!(
            TOTP_SECRET_DESCRIPTION.contains("masked"),
            "turning this on adds a MASKED row; copy that implied a seed appears in the clear would be wrong"
        );
        assert!(
            TOTP_SECRET_DESCRIPTION.contains("details screen"),
            "the copy has to say WHERE the row appears"
        );
        // **The row is labelled `TOTP` on that screen, and the copy said
        // "one-time code" for as long as the two disagreed.** The `||` that
        // used to be on the assertion above accepted either wording, so the
        // stale one passed. Naming a label the app does not paint is the
        // failure this pins: the user goes looking for a row called "one-time
        // code", does not find one, and concludes the pill did nothing.
        assert!(
            !TOTP_SECRET_DESCRIPTION.to_ascii_lowercase().contains("one-time code"),
            "the details screen labels that row \"TOTP\", not \"one-time code\": {TOTP_SECRET_DESCRIPTION:?}"
        );
        assert!(
            TOTP_SECRET_DESCRIPTION.contains("TOTP code"),
            "the copy has to name the row it appears under, as that row is labelled"
        );
        // The instrument: an ink lookup that panics on a double paint, with a
        // real rect, so "contains" above is not reading a zero-size ghost.
        let ink = painted.ink_of(TOTP_SECRET_LABEL);
        assert!(ink.rect.height() > 0.0 && ink.rect.width() > 0.0, "the label has no box: {:?}", ink.rect);
        assert!(ink.color.a() > 0, "the label is painted at alpha {}", ink.color.a());
        let desc = painted.ink_of(TOTP_SECRET_DESCRIPTION);
        assert!(desc.color.a() > 0, "the description is painted at alpha {}", desc.color.a());
        assert!(desc.rows >= 2, "a description this long should wrap; it took {} row(s)", desc.rows);
    }

    #[test]
    fn clicking_the_auto_lock_toggle_turns_auto_lock_off_and_on_again() {
        // The user's actual request. `auto_lock_enabled` starts true, and it
        // is the LAST pill on the page -- `prompt_on_match`, `fetch_icons`,
        // its `fetch_icons_direct` child, `use_brand_logos` and
        // `reveal_totp_seed` sit above it. It was the
        // fifth until the network-logos row was inserted, the sixth before
        // `check_breaches` moved to its own page, the seventh while the
        // encrypted disk cache sat under the backend row, and the fifth
        // again now that the disk-cache pair has followed the backend row to
        // `Section::Vault`.
        //
        // **Read as the last index rather than written as `4`**, because
        // this index has now been wrong four times. What the test means is
        // "the bottom row of General", and that is a thing the paint can be
        // asked directly.
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        assert!(state.settings.auto_lock_enabled, "the default");

        let first = frame(&ctx, &mut state, &[]);
        let pills = first.rects_of_size(Vec2::new(40.0, 22.0));
        assert_eq!(pills.len(), 6, "the General card no longer paints six pills");
        let pill = pills[pills.len() - 1].center();
        frame(&ctx, &mut state, &click(pill));
        assert!(!state.settings.auto_lock_enabled, "the auto-lock toggle did not turn off");
        assert!(!state.settings.cache_vault_to_disk, "the wrong row's toggle moved");
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
        assert!(state.settings.prompt_on_match, "the wrong row's toggle moved");
        assert!(!state.settings.check_breaches, "the wrong row's toggle moved");
        assert!(state.settings.fetch_icons, "the wrong row's toggle moved");
        assert!(!state.settings.reveal_totp_seed, "the wrong row's toggle moved");
        // What the toggle is FOR, asserted on the value the vault window
        // actually consumes rather than on the flag: a field that flips
        // without reaching `auto_lock` is a switch that does nothing.
        assert_eq!(state.settings.auto_lock(), crate::settings::AutoLock::Never);

        frame(&ctx, &mut state, &click(pill));
        assert!(state.settings.auto_lock_enabled, "and back on again");
        assert_eq!(
            state.settings.auto_lock(),
            crate::settings::AutoLock::After(std::time::Duration::from_secs(15 * 60)),
            "turning it back on must restore the minutes that were on screen the whole time"
        );
    }

    #[test]
    fn the_minutes_stepper_is_greyed_while_auto_lock_is_off() {
        // "Greyed" is the visible half; the click tests below are the half
        // that matters. Read back off the stepper box's own stroke, with the
        // enabled case as the positive control -- without it, a stepper that
        // painted `HAIRLINE` in both states would pass.
        let off = paint_settings(
            Section::General,
            Settings { auto_lock_enabled: false, ..Settings::default() },
        );
        let on = paint_settings(Section::General, Settings::default());
        let stepper = Vec2::new(112.0, 28.0);
        assert_eq!(
            on.stroke_of_only_rect_of_size(stepper),
            theme::BORDER_STRONG,
            "with auto-lock on the stepper is 3e's ordinary segmented control"
        );
        assert_eq!(
            off.stroke_of_only_rect_of_size(stepper),
            theme::HAIRLINE,
            "with auto-lock off the stepper must read as disabled"
        );
        assert_ne!(theme::BORDER_STRONG, theme::HAIRLINE, "the two greys have to differ at all");
    }

    /// The number sits in the middle of its cell in BOTH states.
    ///
    /// Both halves are load-bearing and neither is redundant. The greyed
    /// branch paints an explicit galley and was always centred; the live one
    /// hands the placement to a `TextEdit` and was measured 6.0pt right and
    /// 3.5pt high of the cell centre -- visible as the number jumping when
    /// the toggle is flipped. Asserting only the live branch would let a
    /// future edit break the greyed one silently, and asserting only that the
    /// two AGREE would be satisfied by both being wrong together, so each is
    /// checked against the cell it is drawn in.
    #[test]
    fn the_minutes_number_is_centred_in_its_cell_in_both_states() {
        let minutes = clamp_auto_lock_minutes(Settings::default().auto_lock_minutes).to_string();
        for (state, painted) in [
            ("live", paint_settings(Section::General, Settings::default())),
            (
                "greyed",
                paint_settings(
                    Section::General,
                    Settings { auto_lock_enabled: false, ..Settings::default() },
                ),
            ),
        ] {
            let outer = painted.only_rect_of_size(Vec2::new(
                STEPPER_STEP_WIDTH * 2.0 + STEPPER_VALUE_WIDTH,
                STEPPER_HEIGHT,
            ));
            // The value cell is the middle segment, between the two end
            // buttons -- derived from the same constants `minutes_stepper`
            // lays the control out with, so this cannot drift from it.
            let cell = Rect::from_min_size(
                Pos2::new(outer.min.x + STEPPER_STEP_WIDTH, outer.min.y),
                Vec2::new(STEPPER_VALUE_WIDTH, STEPPER_HEIGHT),
            );
            let number = painted.rect_of(&minutes);
            // Half a point: the two branches lay the glyphs out by different
            // routes and their widths differ by ~0.1pt, so an exact equality
            // would be measuring rounding rather than centring. The defect
            // this catches was 6.0 and 3.5.
            assert!(
                (number.center().x - cell.center().x).abs() < 0.5,
                "{state}: the minutes number is not horizontally centred in its cell -- \
                 number centre {:?}, cell centre {:?}",
                number.center(),
                cell.center()
            );
            assert!(
                (number.center().y - cell.center().y).abs() < 0.5,
                "{state}: the minutes number is not vertically centred in its cell -- \
                 number centre {:?}, cell centre {:?}",
                number.center(),
                cell.center()
            );
        }
    }

    #[test]
    fn the_minutes_stepper_still_shows_its_value_while_auto_lock_is_off() {
        // Greyed, not hidden: the number the toggle will restore has to stay
        // legible, so this is not satisfied by a row that disappears.
        let painted = paint_settings(
            Section::General,
            Settings { auto_lock_enabled: false, auto_lock_minutes: 42, ..Settings::default() },
        );
        assert!(painted.contains("Lock the vault after"), "got {:?}", painted.strings());
        assert!(painted.contains("42"), "got {:?}", painted.strings());
        assert_eq!(
            painted.count_of_size(Vec2::new(112.0, 28.0)),
            1,
            "the stepper box is still drawn"
        );
    }

    #[test]
    fn the_steppers_buttons_are_inert_while_auto_lock_is_off() {
        // A click test, not a colour check: a control that is painted grey
        // and still responds is the exact defect this repo keeps re-writing.
        // Every assertion here is paired with the same click on an enabled
        // stepper, so "the stepper never works" cannot pass it.
        let ctx = styled_context();
        let mut off =
            PrefsState::new(Settings { auto_lock_enabled: false, ..Settings::default() });
        let painted = frame(&ctx, &mut off, &[]);
        let plus = painted.rect_of("+").center();
        let minus = painted.rect_of("-").center();

        frame(&ctx, &mut off, &click(plus));
        assert_eq!(off.settings.auto_lock_minutes, 15, "the disabled + stepped the value");
        frame(&ctx, &mut off, &click(minus));
        assert_eq!(off.settings.auto_lock_minutes, 15, "the disabled - stepped the value");

        let mut on = PrefsState::new(Settings::default());
        let painted = frame(&ctx, &mut on, &[]);
        assert_eq!(
            (painted.rect_of("+").center(), painted.rect_of("-").center()),
            (plus, minus),
            "the two states must put their buttons in the same place, or the clicks above \
             missed rather than being refused"
        );
        frame(&ctx, &mut on, &click(plus));
        assert_eq!(on.settings.auto_lock_minutes, 16, "positive control: the + does work");
        frame(&ctx, &mut on, &click(minus));
        assert_eq!(on.settings.auto_lock_minutes, 15, "positive control: the - does work");
    }

    #[test]
    fn the_minutes_field_cannot_be_typed_into_while_auto_lock_is_off() {
        // The other half of "non-interactive": the buttons are inert above,
        // and there is no text widget left in the middle either -- clicking
        // it takes no focus, so the keystrokes go nowhere.
        let ctx = styled_context();
        let mut off =
            PrefsState::new(Settings { auto_lock_enabled: false, ..Settings::default() });
        let painted = frame(&ctx, &mut off, &[]);
        // The middle cell of the 112x28 box, i.e. where the value sits.
        let field = painted.only_rect_of_size(Vec2::new(112.0, 28.0)).center();
        frame(&ctx, &mut off, &click(field));
        frame(&ctx, &mut off, &[egui::Event::Text("7".into())]);
        frame(&ctx, &mut off, &[]);
        assert_eq!(off.settings.auto_lock_minutes, 15, "the disabled field accepted typing");
        assert_eq!(off.auto_lock_text, "15", "and its buffer must not drift either");

        let mut on = PrefsState::new(Settings::default());
        let painted = frame(&ctx, &mut on, &[]);
        let field = painted.only_rect_of_size(Vec2::new(112.0, 28.0)).center();
        frame(&ctx, &mut on, &click(field));
        frame(&ctx, &mut on, &[egui::Event::Text("7".into())]);
        frame(&ctx, &mut on, &[]);
        assert!(
            on.auto_lock_text.contains('7') && on.auto_lock_text != "15",
            "positive control: with auto-lock on the same click and keystroke DO reach the \
             field (so the assertion above is about the disabled state, not about the harness \
             being unable to type at all); the buffer is {:?}",
            on.auto_lock_text
        );
    }

    #[test]
    fn the_steppers_buttons_move_the_stored_timeout() {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        assert_eq!(state.settings.auto_lock_minutes, 15, "the default");

        let first = frame(&ctx, &mut state, &[]);
        let plus = first.rect_of("+").center();
        let minus = first.rect_of("-").center();

        frame(&ctx, &mut state, &click(plus));
        assert_eq!(state.settings.auto_lock_minutes, 16);
        let after = frame(&ctx, &mut state, &click(minus));
        assert_eq!(state.settings.auto_lock_minutes, 15);
        assert!(
            after.contains("15"),
            "the field has to follow the buttons; got {:?}",
            after.strings()
        );
    }

    #[test]
    fn the_steppers_minus_is_inert_at_the_floor() {
        // Not merely clamped afterwards: at one minute there is nothing below,
        // and a button that accepts the click and refuses the change is the
        // same lie as a switch that does nothing.
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings { auto_lock_minutes: 1, ..Settings::default() });
        let first = frame(&ctx, &mut state, &[]);
        let minus = first.rect_of("-").center();
        frame(&ctx, &mut state, &click(minus));
        assert_eq!(state.settings.auto_lock_minutes, 1);
    }


    // -- Clipboard ---------------------------------------------------------

    /// One clipboard preference, read off a `Settings`. A named type because
    /// the tables below hold arrays of these and the inline form is a
    /// `clippy::type_complexity` warning rather than something anyone reads.
    type ReadClipboardField = fn(&Settings) -> bool;

    /// A clipboard page over given settings, and the state that drew it, so a
    /// test can click and then read the value back.
    fn clipboard_state(settings: Settings) -> PrefsState {
        let mut state = PrefsState::new(settings);
        state.show(Section::Clipboard);
        state
    }

    #[test]
    fn the_clipboard_page_paints_every_control_and_its_reasoning() {
        let painted = paint(Section::Clipboard);
        for text in [
            CLIPBOARD_MASTER_LABEL,
            CLIPBOARD_MASTER_DESCRIPTION,
            CLIPBOARD_ON_LOCK_LABEL,
            CLIPBOARD_ON_LOCK_DESCRIPTION,
            CLIPBOARD_ON_ACCOUNT_LABEL,
            CLIPBOARD_ON_ACCOUNT_DESCRIPTION,
            CLIPBOARD_ON_QUIT_LABEL,
            CLIPBOARD_ON_QUIT_DESCRIPTION,
            CLIPBOARD_INTERVAL_LABEL,
            CLIPBOARD_INTERVAL_DESCRIPTION,
            CLIPBOARD_HISTORY_LABEL,
            CLIPBOARD_HISTORY_NOTE,
            CLIPBOARD_RESET_LABEL,
            CLIPBOARD_RESET_DESCRIPTION,
            CLIPBOARD_RESET_BUTTON,
        ] {
            assert!(
                painted.contains(text),
                "{text:?} was not painted; got {:?}",
                painted.strings()
            );
        }
        // The default interval, in minutes, is on screen -- so the field shows
        // the value in effect rather than an empty box.
        assert!(painted.contains("1"), "got {:?}", painted.strings());

        // Four pills and no more: the master switch and the three triggers.
        // The interval is a field and the reset is a button, so neither may
        // paint a pill.
        assert_eq!(
            painted.count_of_size(TOGGLE_SIZE),
            4,
            "the clipboard card no longer paints exactly four pills"
        );
    }

    /// **The copy has to say the three things a reader would otherwise have to
    /// guess**, so these are pinned as claims rather than left to whoever next
    /// edits a string.
    #[test]
    fn the_clipboard_copy_states_the_floor_the_ceiling_and_what_is_not_switchable() {
        assert!(
            CLIPBOARD_INTERVAL_DESCRIPTION.contains("0.5")
                && CLIPBOARD_INTERVAL_DESCRIPTION.contains("thirty seconds"),
            "the floor has to be stated on screen, in both forms, not only enforced"
        );
        assert!(
            CLIPBOARD_INTERVAL_DESCRIPTION.contains("60 minutes"),
            "the ceiling has to be stated on screen, or a user meets it by being refused"
        );
        assert!(
            CLIPBOARD_INTERVAL_DESCRIPTION.contains("One decimal place"),
            "the resolution has to be stated, or `1.25` is refused for an unstated reason"
        );
        assert!(
            CLIPBOARD_MASTER_DESCRIPTION.contains("Off means"),
            "the master switch's copy has to say what OFF does -- off is the state that \
             withdraws a protection"
        );
        assert!(
            CLIPBOARD_MASTER_DESCRIPTION.contains("clipboard history")
                && CLIPBOARD_HISTORY_NOTE.contains("no setting"),
            "the history exclusion has to be named as always-on in both places, or this page \
             reads as governing it"
        );
        assert!(
            CLIPBOARD_ON_QUIT_DESCRIPTION.contains("crash")
                && CLIPBOARD_ON_QUIT_DESCRIPTION.contains("power cut"),
            "the quit switch has to say what it cannot cover, or it promises more than \
             PRIVACY.md does"
        );
        assert!(
            CLIPBOARD_ON_LOCK_DESCRIPTION.contains("no separate sign-out"),
            "the lock switch has to say it is also the logout case, since there is no logout \
             switch and a reader will look for one"
        );
        assert!(
            CLIPBOARD_RESET_DESCRIPTION.contains("Nothing on any other page"),
            "the reset button has to state its scope, or `Reset to default` inside a \
             preferences window is ambiguous between this page and the app"
        );
    }

    /// **Each of the four pills is wired to its own field**, top to bottom.
    /// A pill that is painted but whose value is never written back is
    /// decoration, and that is this codebase's most-repeated defect.
    #[test]
    fn every_clipboard_pill_writes_its_own_setting_back() {
        // Master, lock, account change, quit -- in the order they are painted.
        let fields: [(&str, ReadClipboardField); 4] = [
            ("the master switch", |s| s.clear_clipboard),
            ("the lock trigger", |s| s.clear_clipboard_on_lock),
            ("the account-change trigger", |s| s.clear_clipboard_on_account_change),
            ("the quit trigger", |s| s.clear_clipboard_on_quit),
        ];
        // The master switch is index 0 and turning it off greys the other
        // three, so each index is exercised from a fresh state.
        for (index, (what, read)) in fields.iter().enumerate() {
            let ctx = styled_context();
            let mut state = clipboard_state(Settings::default());
            let first = frame(&ctx, &mut state, &[]);
            let pills = first.rects_of_size(TOGGLE_SIZE);
            assert_eq!(pills.len(), 4, "the clipboard card no longer paints four pills");
            assert!(read(&state.settings), "{what}: the premise is that it starts on");

            frame(&ctx, &mut state, &click(pills[index].center()));
            assert!(!read(&state.settings), "{what} was not turned off -- the pill is decoration");
            // ...and back, so the pill toggles rather than only ever clearing.
            let after = frame(&ctx, &mut state, &[]);
            let pills = after.rects_of_size(TOGGLE_SIZE);
            frame(&ctx, &mut state, &click(pills[index].center()));
            assert!(read(&state.settings), "{what} could not be turned back on");

            // The control that makes the index meaningful: the OTHER three
            // fields are untouched by this click, so the pills are not all
            // wired to one field.
            let mut state = clipboard_state(Settings::default());
            let first = frame(&ctx, &mut state, &[]);
            let pills = first.rects_of_size(TOGGLE_SIZE);
            frame(&ctx, &mut state, &click(pills[index].center()));
            for (other_index, (other, other_read)) in fields.iter().enumerate() {
                if other_index != index {
                    assert!(
                        other_read(&state.settings),
                        "clicking {what} also changed {other}"
                    );
                }
            }
        }
    }

    /// **The master switch off greys the three triggers and the interval, and
    /// they do not respond to clicks.** "Looks disabled" and "is disabled" are
    /// the pair this codebase keeps having to reunite, so both halves are
    /// asserted.
    #[test]
    fn the_master_switch_off_disables_the_children_without_hiding_them() {
        let ctx = styled_context();
        let mut state = clipboard_state(Settings { clear_clipboard: false, ..Settings::default() });
        let painted = frame(&ctx, &mut state, &[]);

        // Still there, all four rows -- greyed, not removed. A row that
        // vanished would reflow the card and hide the value it is about to
        // restore.
        for text in [
            CLIPBOARD_ON_LOCK_LABEL,
            CLIPBOARD_ON_ACCOUNT_LABEL,
            CLIPBOARD_ON_QUIT_LABEL,
            CLIPBOARD_INTERVAL_LABEL,
        ] {
            assert!(painted.contains(text), "{text:?} vanished when the master switch went off");
        }
        assert_eq!(
            painted.count_of_size(TOGGLE_SIZE),
            4,
            "a pill was removed rather than disabled"
        );

        // And they are inert: a click on each child pill changes nothing.
        let pills = painted.rects_of_size(TOGGLE_SIZE);
        let children: [(usize, &str, ReadClipboardField); 3] = [
            (1, "the lock trigger", |s| s.clear_clipboard_on_lock),
            (2, "the account-change trigger", |s| s.clear_clipboard_on_account_change),
            (3, "the quit trigger", |s| s.clear_clipboard_on_quit),
        ];
        for (index, what, read) in children {
            frame(&ctx, &mut state, &click(pills[index].center()));
            assert!(
                read(&state.settings),
                "{what} responded to a click while the master switch was off"
            );
        }

        // The pair: the master switch itself is NOT disabled, so the page is
        // not simply inert.
        frame(&ctx, &mut state, &click(pills[0].center()));
        assert!(
            state.settings.clear_clipboard,
            "the master switch disabled itself, leaving the page unreachable"
        );
    }

    /// **Typing a fractional number of minutes commits the right whole number
    /// of seconds**, and the field then shows the value back in its normal
    /// form. This is the seam between the parser and the stored value.
    #[test]
    fn the_interval_field_commits_what_was_typed_as_seconds() {
        for (typed, seconds, shown) in [
            ("0.5", 30_u64, "0.5"),
            ("1,5", 90, "1.5"),
            ("2", 120, "2"),
            ("60", 3600, "60"),
        ] {
            let ctx = styled_context();
            let mut state = clipboard_state(Settings::default());
            let first = frame(&ctx, &mut state, &[]);
            let field = first.rect_of("1").center();
            // Click into it, select all, type, then click away so it commits.
            frame(&ctx, &mut state, &click(field));
            state.clipboard_interval_text = typed.to_owned();
            frame(&ctx, &mut state, &[]);
            let away = Pos2::new(NAV_WIDTH / 2.0, 4.0);
            frame(&ctx, &mut state, &click(away));

            assert_eq!(
                state.settings.clear_clipboard_seconds, seconds,
                "typing {typed:?} did not commit {seconds} seconds"
            );
            assert_eq!(
                state.clipboard_interval_text, shown,
                "after committing {typed:?} the field does not show the stored value"
            );
            assert_eq!(state.clipboard_entry_error, None, "{typed:?} was refused");
        }
    }

    /// **A refused entry leaves the stored value alone and says why**, with a
    /// different sentence for each way of being wrong.
    #[test]
    fn a_refused_interval_entry_keeps_the_old_value_and_names_the_reason() {
        for (typed, reason) in [
            ("soon", CLIPBOARD_ENTRY_NOT_A_NUMBER),
            ("0.1", CLIPBOARD_ENTRY_BELOW_FLOOR),
            ("90", CLIPBOARD_ENTRY_ABOVE_CEILING),
            ("1.25", CLIPBOARD_ENTRY_BETWEEN_STEPS),
        ] {
            let ctx = styled_context();
            let mut state = clipboard_state(Settings::default());
            let first = frame(&ctx, &mut state, &[]);
            frame(&ctx, &mut state, &click(first.rect_of("1").center()));
            state.clipboard_interval_text = typed.to_owned();
            frame(&ctx, &mut state, &[]);
            frame(&ctx, &mut state, &click(Pos2::new(NAV_WIDTH / 2.0, 4.0)));
            // The row's text is laid out before the field is drawn, so the
            // refusal lands one frame after the click that caused it. egui
            // repaints on a focus change, so this is a frame the user never
            // sees -- but it is a frame, and the test has to draw it rather
            // than pretend the message is synchronous.
            let painted = frame(&ctx, &mut state, &[]);

            assert_eq!(
                state.settings.clear_clipboard_seconds, 60,
                "{typed:?} was refused but the stored interval moved anyway"
            );
            assert_eq!(state.clipboard_entry_error, Some(reason), "{typed:?}");
            assert!(
                painted.contains(reason),
                "{typed:?} was refused but the reason is not on screen; got {:?}",
                painted.strings()
            );
            // The refused text is left in the field, so the user can see the
            // thing being explained.
            assert_eq!(state.clipboard_interval_text, typed);
        }
    }

    /// **Reset puts this page back and touches nothing else**, driven through
    /// a real click so the button is wired rather than merely painted.
    #[test]
    fn the_reset_button_restores_this_page_and_no_other_setting() {
        let ctx = styled_context();
        let mut state = clipboard_state(Settings {
            clear_clipboard: false,
            clear_clipboard_on_lock: false,
            clear_clipboard_on_account_change: false,
            clear_clipboard_on_quit: false,
            clear_clipboard_seconds: 300,
            // Two settings from other pages, both away from their defaults.
            check_breaches: true,
            auto_lock_minutes: 42,
            ..Settings::default()
        });
        let first = frame(&ctx, &mut state, &[]);
        // The premise, so the assertions below are about the click.
        assert!(!state.settings.clear_clipboard);
        assert_eq!(state.settings.clear_clipboard_seconds, 300);

        frame(&ctx, &mut state, &click(first.rect_of(CLIPBOARD_RESET_BUTTON).center()));

        assert!(state.settings.clear_clipboard, "the button is decoration");
        assert!(state.settings.clear_clipboard_on_lock);
        assert!(state.settings.clear_clipboard_on_account_change);
        assert!(state.settings.clear_clipboard_on_quit);
        assert_eq!(state.settings.clear_clipboard_seconds, 60);
        // The field's buffer followed the value, or the row would still show
        // the number the user just reset away from.
        assert_eq!(state.clipboard_interval_text, "1");

        // Scope: the other pages are untouched.
        assert!(state.settings.check_breaches, "Reset reached on to the General page");
        assert_eq!(state.settings.auto_lock_minutes, 42, "Reset reached the auto-lock stepper");

        // ...and the reset really is visible on the next frame, rather than
        // living only in the struct.
        let after = frame(&ctx, &mut state, &[]);
        assert!(after.contains("1"), "got {:?}", after.strings());
    }

    /// **The Reset button stays live while the master switch is off.** It is
    /// the way back from the state a user is most likely to want to leave, so
    /// greying it out with the thing it resets would make that state a
    /// one-way door.
    #[test]
    fn the_reset_button_works_with_the_master_switch_off() {
        let ctx = styled_context();
        let mut state = clipboard_state(Settings {
            clear_clipboard: false,
            clear_clipboard_seconds: 600,
            ..Settings::default()
        });
        let first = frame(&ctx, &mut state, &[]);
        frame(&ctx, &mut state, &click(first.rect_of(CLIPBOARD_RESET_BUTTON).center()));
        assert!(state.settings.clear_clipboard, "Reset was inert while the page was switched off");
        assert_eq!(state.settings.clear_clipboard_seconds, 60);
    }

    /// **A hand-edited out-of-range interval is corrected on the way in**, so
    /// the window never displays a number that is not the number in effect.
    #[test]
    fn opening_the_page_on_an_impossible_stored_interval_shows_the_one_in_effect() {
        // Far above the ceiling, and off a step.
        let state = clipboard_state(Settings {
            clear_clipboard_seconds: 14_401,
            ..Settings::default()
        });
        assert_eq!(state.settings.clear_clipboard_seconds, 3600);
        assert_eq!(state.clipboard_interval_text, "60");
        // Below the floor.
        let state = clipboard_state(Settings { clear_clipboard_seconds: 1, ..Settings::default() });
        assert_eq!(state.settings.clear_clipboard_seconds, 30);
        assert_eq!(state.clipboard_interval_text, "0.5");
        // The control: a value already in range and on a step is untouched.
        let state =
            clipboard_state(Settings { clear_clipboard_seconds: 150, ..Settings::default() });
        assert_eq!(state.settings.clear_clipboard_seconds, 150);
        assert_eq!(state.clipboard_interval_text, "2.5");
    }

    /// **Clicking the Clipboard nav row opens the Clipboard page**, which is
    /// what stops the new nav row being decoration.
    #[test]
    fn clicking_the_clipboard_nav_row_opens_the_clipboard_page() {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        assert_eq!(state.section, Section::General);
        let first = frame(&ctx, &mut state, &[]);
        let after = frame(&ctx, &mut state, &click(first.nav_ink_of("Clipboard").rect.center()));
        assert_eq!(state.section, Section::Clipboard, "the nav row did not select");
        assert!(
            after.contains(CLIPBOARD_MASTER_LABEL),
            "Clipboard should now be the open page; got {:?}",
            after.strings()
        );
    }

    // -- the backend rows, on the Vault page -------------------------------

    /// A signed-in status for a given server, as the shells publish one.
    fn signed_in_on(server: Option<&str>) -> AccountStatus {
        AccountStatus::SignedIn {
            email: Some("someone@example.com".to_string()),
            server: server.map(str::to_string),
        }
    }

    /// A state whose account row answers "self-hosted".
    ///
    /// Through [`PrefsState::show_account_source`] and **never through
    /// [`publish_account_status`]**: that writes the process-wide value the
    /// rest of this suite -- which runs in parallel in one process -- reads,
    /// and its own doc says so. A `fn` pointer per server rather than a
    /// closure, because that is the seam's shape.
    fn on_a_self_hosted_server() -> PrefsState {
        fn source() -> Option<AccountStatus> {
            Some(AccountStatus::SignedIn {
                email: Some("someone@example.com".to_string()),
                server: Some("https://vault.example.com".to_string()),
            })
        }
        let mut state = PrefsState::new(Settings::default());
        state.show(Section::Vault);
        state.show_account_source(source);
        state
    }

    /// The same, on an official cloud.
    fn on_an_official_cloud() -> PrefsState {
        fn source() -> Option<AccountStatus> {
            Some(AccountStatus::SignedIn {
                email: Some("someone@example.com".to_string()),
                server: Some("https://vault.bitwarden.com".to_string()),
            })
        }
        let mut state = PrefsState::new(Settings::default());
        state.show(Section::Vault);
        state.show_account_source(source);
        state
    }

    /// **The whole point of the gathering: one screen carries every setting
    /// that decides where this vault comes from.**
    ///
    /// Asserted as four labels on one painted page, and -- the half that can
    /// actually fail -- as four labels absent from every other page in the
    /// window. A test that only checked presence would pass a change that
    /// *copied* the rows, which is this file's most-feared defect: a control
    /// fixed in one place and left broken in the other.
    ///
    /// **`Section::Api` is in the absent list, and that is not padding.** The
    /// service switch was gathered onto Vault alongside these four and has
    /// since moved on to its own page; the mirror-image test below pins that
    /// it left. Two pages that were briefly one are exactly the pair where a
    /// half-finished split leaves a duplicate.
    ///
    /// **This is the `bw` page.** `paint_vault_with_hello` publishes no
    /// account status, so `is_self_hosted` answers false and `choose` answers
    /// `BwServe`. On the built-in client `BACKEND_LABEL` is deliberately
    /// absent; see
    /// `the_row_about_the_subprocess_is_absent_where_there_is_no_subprocess`.
    #[test]
    fn every_setting_that_decides_where_the_vault_comes_from_is_on_the_vault_page() {
        let vault = paint_vault_with_hello(true);
        let gathered =
            [BACKEND_CHOICE_LABEL, BACKEND_LABEL, DISK_CACHE_LABEL, READ_THROUGH_LABEL];
        for label in gathered {
            assert!(
                vault.contains(label),
                "{label:?} is not on the Vault page; got {:?}",
                vault.strings()
            );
        }

        // And nowhere else. General is the page these rows were taken off
        // (they went by way of Sync & account, which no longer exists -- it
        // was removed once these had left it with nothing a user could
        // change); the Local API page is the one they could most plausibly be
        // copied onto. A row left behind on either is two switches over one
        // field.
        for section in [Section::General, Section::Api] {
            let painted = paint_tall(section);
            for label in gathered {
                assert!(
                    !painted.contains(label),
                    "{label:?} is still painted on {section:?} as well as on the Vault page, \
                     so one field now has two switches"
                );
            }
        }

        // The control for the loop above: those pages are not simply blank,
        // so "not painted" is a claim about these rows rather than about a
        // paint that produced nothing. The API page's control is the switch
        // it does own, which is also the assertion the next test starts from.
        assert!(paint(Section::General).contains(PROMPT_LABEL));
        assert!(paint_tall(Section::Api).contains(SERVICE_LABEL));
    }

    /// **The mirror image, and the half a move gets wrong: the API page
    /// carries the endpoint and every key control, and the Vault page it came
    /// off carries none of them.**
    ///
    /// The switch and the key list are the one pair that must never be
    /// separated -- a switch on one page and the list of who can walk through
    /// the door it opens on another is an arrangement where the owner can
    /// believe they turned something off while three keys still exist for it.
    /// So they are asserted together, on one painted page.
    ///
    /// The absence half is asserted against Vault specifically. A row that
    /// was *copied* rather than moved would leave the Vault page painting a
    /// second `service_enabled` pill, and the owner could turn the endpoint
    /// off on one page and find it on from the other.
    #[test]
    fn the_endpoint_and_every_key_control_are_on_the_api_page_and_not_on_vault() {
        let ctx = tall_context();
        let mut state = api_state();
        state.show_service_keys(vec![stored_key("Backup script", None)]);
        let api = tall_frame(&ctx, &mut state, &[]);
        // The switch, the empty-or-not list with its heading, the form that
        // mints, and a listed key's revoke button: every control the split
        // had to carry across, and each one a separate way to lose half.
        for label in [SERVICE_LABEL, KEYS_SECTION_LABEL, MINT_SECTION_LABEL, MINT_BUTTON] {
            assert!(
                api.contains(label),
                "{label:?} is not on the Local API page; got {:?}",
                api.strings()
            );
        }
        assert!(api.contains(REVOKE_BUTTON), "a listed key has no revoke button");
        assert!(api.contains("Backup script"), "control: the list really drew a row");

        // And none of it on Vault -- driven with the same key in the store,
        // so a Vault page that still drew the list would be caught rather
        // than merely being handed nothing to draw.
        let mut vault = PrefsState::new(Settings::default());
        vault.section = Section::Vault;
        vault.show_hello_available(|| true);
        vault.show_service_keys(vec![stored_key("Backup script", None)]);
        let painted = tall_frame(&ctx, &mut vault, &[]);
        for label in
            [SERVICE_LABEL, KEYS_SECTION_LABEL, MINT_SECTION_LABEL, MINT_BUTTON, REVOKE_BUTTON]
        {
            assert!(
                !painted.contains(label),
                "{label:?} is still painted on the Vault page as well as on the Local API \
                 page; got {:?}",
                painted.strings()
            );
        }
        assert!(!painted.contains("Backup script"), "the key list is still drawn on Vault");
        // The control: the Vault page is not blank, and the store it was
        // handed is the one the API page listed a row from.
        assert!(painted.contains(DISK_CACHE_LABEL), "control: the Vault page drew its own rows");
    }

    // **`sync_and_account_says_where_the_backend_switch_went` is gone with the
    // page it guarded**, and this note is here instead of a weakened version
    // of it. It pinned a signpost row: Sync & account told a reader that the
    // backend switch now lives on Vault, in `Section::Vault`'s own label so a
    // rename could not orphan the sentence. That was right while the page
    // existed. The page has been removed -- two rows of prose, no setting --
    // and a signpost is only read by someone standing where it is planted, so
    // there is now nobody to read it. What replaces it is the nav itself:
    // Vault is the third row, above the fold, named in the same word the
    // sentence used, and `every_nav_section_design_3e_lists_is_painted` pins
    // both the name and the position. The settings the signpost pointed AT
    // are still pinned in place by
    // `every_setting_that_decides_where_the_vault_comes_from_is_on_the_vault_page`,
    // which is the assertion that actually protected the user here.

    /// One frame of the Vault page for an account on `server`, with the
    /// crypto toggle at `use_official`. Both inputs are needed together:
    /// [`cli_rows_are_shown`] is `backend_policy::choose` over the pair, and
    /// neither alone decides it.
    fn paint_vault_for(server: Option<&'static str>, use_official: bool) -> Painted {
        let ctx = tall_context();
        let mut settings = Settings::default();
        settings.use_official_bw_crypto = use_official;
        let mut state = PrefsState::new(settings);
        state.section = Section::Vault;
        state.show_account_source(match server {
            Some("self") => || {
                Some(AccountStatus::SignedIn {
                    email: Some("me@example.com".to_string()),
                    server: Some("https://vault.example.com".to_string()),
                })
            },
            _ => || {
                Some(AccountStatus::SignedIn {
                    email: Some("me@example.com".to_string()),
                    server: None,
                })
            },
        });
        tall_frame(&ctx, &mut state, &[])
    }

    /// **The defect this task removes, asserted from the other side.**
    ///
    /// `backend_policy::should_run` answers `false` for `DirectRest` whatever
    /// `keep_backend_running` says, so on a self-hosted account with the
    /// crypto toggle off this row governs nothing. This test used to say the
    /// row went *grey* there, under a sentence ending "there is nothing here
    /// to decide" -- a row whose only content was that it did not apply. It
    /// now says the row is not there, and the three combinations where it IS a
    /// real decision are unchanged from the version this replaces.
    #[test]
    fn the_row_about_the_subprocess_is_absent_where_there_is_no_subprocess() {
        let direct = paint_vault_for(Some("self"), false);
        assert!(
            !direct.contains(BACKEND_LABEL),
            "a self-hosted account with the built-in client is still offered a row about a \
             subprocess it does not have; got {:?}",
            direct.strings()
        );
        // The positive control, and it is the whole reason this is not a
        // vacuous pass: the page is not blank. The two rows that are true on
        // every backend are still on it.
        assert!(
            direct.contains(UI_LOADED_LABEL) && direct.contains(DISK_CACHE_LABEL),
            "the built-in page painted nothing, so the absence above is about a failed paint \
             and not about the row; got {:?}",
            direct.strings()
        );
        // And the row's own copy is gone with it -- a hidden row whose
        // paragraph was still painted somewhere would be the same lie with no
        // label on it.
        assert!(
            !direct.contains(BACKEND_DESCRIPTION),
            "the row is hidden but its description is still on the page"
        );

        // Every other combination is `bw serve`, so the row decides something
        // and is drawn with the copy that says what the trade is. All three,
        // because a gate that read only the toggle -- or only the server --
        // would pass one of them.
        for (server, use_official, why) in [
            (Some("self"), true, "self-hosted, official CLI chosen"),
            (None, false, "bitwarden.com, opted out -- but it cannot opt out"),
            (None, true, "bitwarden.com, official CLI chosen"),
        ] {
            let painted = paint_vault_for(server, use_official);
            assert!(
                painted.contains(BACKEND_LABEL) && painted.contains(BACKEND_DESCRIPTION),
                "{why} is served by `bw serve`, so this row decides something and must be \
                 drawn saying so; got {:?}",
                painted.strings()
            );
        }
    }

    /// **The count, which is the half a `contains` loop is structurally blind
    /// to.** Four pills on the `bw` page and three on the built-in one, and
    /// both numbers spelled out rather than derived, so a row added to either
    /// page has to be re-pinned here deliberately.
    ///
    /// **A ghosted row still paints its pill**, which is the whole of
    /// [`child_toggle_row`]'s argument made visible: grey says "this is a
    /// control you do not have here", where absence says nothing at all. So
    /// `read_through_cache` counts even though `Settings::default` leaves the
    /// disk copy off, and the difference between these two numbers is exactly
    /// the one row this split hides rather than ghosts.
    ///
    /// # Both numbers went down by one, and the row that left is still counted
    ///
    /// It was five and four while the backend choice was an on/off pill.
    /// [`backend_choice_row`] draws a two-cell [`choice_button`] picker
    /// instead, which paints no 40x22 rectangle -- so a count alone would now
    /// be satisfied by a page that had simply dropped the choice. The second
    /// half of this test is the replacement: **both cells are painted on both
    /// pages**, and exactly one of them is the one in force. Without it the
    /// counts above would be a weaker pin than the ones they replaced.
    #[test]
    fn the_vault_page_paints_one_fewer_pill_on_the_built_in_client() {
        let bw = paint_vault_for(Some("self"), true);
        let built_in = paint_vault_for(Some("self"), false);
        assert_eq!(
            bw.count_of_size(TOGGLE_SIZE),
            4,
            "the `bw` Vault page's four pills: `keep_backend_running`, `keep_ui_loaded`, \
             `cache_vault_to_disk`, and `read_through_cache` ghosted under it. The backend \
             choice is not among them -- it is a picker, and it is counted below"
        );
        assert_eq!(
            built_in.count_of_size(TOGGLE_SIZE),
            3,
            "the built-in Vault page must lose exactly one pill -- `keep_backend_running` -- \
             and keep the other three: `keep_ui_loaded`, the disk copy and its ghosted child"
        );

        // **The picker is on both pages, both cells, and the selected one is
        // the client that page is on.** This is what the two counts above
        // stopped being able to say when the pill became a picker.
        for (what, painted, official) in
            [("the `bw` page", &bw, true), ("the built-in page", &built_in, false)]
        {
            assert!(
                painted.contains(OFFICIAL_CHOICE) && painted.contains(BUILT_IN_CHOICE),
                "{what} does not offer both clients as peers; got {:?}",
                painted.strings()
            );
            let (selected, other) = if official {
                (OFFICIAL_CHOICE, BUILT_IN_CHOICE)
            } else {
                (BUILT_IN_CHOICE, OFFICIAL_CHOICE)
            };
            assert_eq!(
                painted.fill_behind(selected),
                theme::BLUE_WASH,
                "{what} does not show {selected:?} as the client in force"
            );
            assert_ne!(
                painted.fill_behind(other),
                theme::BLUE_WASH,
                "{what} shows both clients as selected"
            );
        }
    }

    // **`the_ghosted_backend_copy_names_the_switch_that_disabled_it` is gone
    // with the sentence it guarded**, and this note is here instead of a
    // weakened version of it. It pinned that the ghosted row said "the switch
    // above" so a user who found it grey had somewhere to go. The row is not
    // ghosted any more -- it is absent on the backend that has no subprocess,
    // because the only thing a ghost could have said there was that the row
    // did not apply. What replaces it is
    // `the_built_in_vault_page_names_no_subprocess`, which is a stronger claim
    // about the same page: no row on it names `bw serve` at all, the switch's
    // own row excepted by name.

    /// **The rule the owner asked for, as a scan rather than as an
    /// understanding: on the built-in client, nothing on the Vault page names
    /// the machinery.**
    ///
    /// One exemption, and it is named here rather than left implicit: the
    /// backend picker itself. It has to name both backends, because naming
    /// them is the choice it is asking the user to make -- a control that hid
    /// what it was choosing between would be unusable. That is the whole of
    /// the exemption, and confining it to one row is what makes it affordable.
    ///
    /// **The exemption grew from two strings to four, and not by one inch
    /// more than the control did.** The row used to paint a label and a
    /// description; it now paints a label, a description and two cells,
    /// because naming both clients as peers is the change. Every one is still
    /// matched by equality against a named constant, so a new row still
    /// cannot claim the exemption by accident -- which is the property that
    /// made this scan worth writing.
    #[test]
    fn the_built_in_vault_page_names_no_subprocess() {
        let painted = paint_vault_for(Some("self"), false);
        let switch_copy = official_crypto_description(true);
        let exempt = [BACKEND_CHOICE_LABEL, OFFICIAL_CHOICE, BUILT_IN_CHOICE, switch_copy];
        let offending: Vec<&str> = painted
            .strings()
            .into_iter()
            // The exemption, matched on the picker's own strings rather than
            // on a substring, so a new row cannot claim it by accident.
            .filter(|s| !exempt.contains(s))
            .filter(|s| {
                s.contains("bw serve") || s.contains("Bitwarden CLI") || s.contains("backend")
            })
            .collect();
        assert!(
            offending.is_empty(),
            "the built-in client's Vault page names machinery it does not have, outside the \
             one row that is allowed to: {offending:?}"
        );

        // **Two controls, and both are needed.**
        //
        // The page really painted something...
        assert!(
            painted.contains(DISK_CACHE_LABEL),
            "the scan above found nothing because the page drew nothing"
        );
        // ...and the exempt row really is on it, so the filter is excusing a
        // string that is actually there rather than one that never was.
        for label in [BACKEND_CHOICE_LABEL, OFFICIAL_CHOICE, BUILT_IN_CHOICE] {
            assert!(
                painted.contains(label),
                "the exempted {label:?} is not on the page, so the exemption is excusing \
                 nothing"
            );
        }
        // ...and the needles find something on the OTHER page, so they are
        // needles that can match.
        let bw_page = paint_vault_for(Some("self"), true);
        assert!(
            bw_page.strings().iter().any(|s| s.contains("Bitwarden CLI")),
            "the `bw` page does not name the CLI either, so this scan cannot tell the two \
             pages apart"
        );
    }

    /// **The two labels name a program the user can find, not a concept from
    /// this codebase.**
    ///
    /// "backend" is this file's word for `bw serve`; "bw" is a filename;
    /// "crypto" is a word about internals. The rows are read by somebody
    /// deciding where their vault comes from, and the design's rule is that
    /// this switch is the ONE place in the window where the machinery is
    /// named -- so it had better name it the way the user will meet it.
    #[test]
    fn the_two_backend_labels_name_the_cli_and_not_this_codebases_words() {
        // The needle is unchanged; what carries it moved. It was the backend
        // row's own label, which named the CLI because the pill's ON meant
        // the CLI. The picker's cell is where the CLI is named now, and the
        // rule is the same rule.
        for label in [BACKEND_LABEL, OFFICIAL_CHOICE] {
            assert!(
                label.contains("Bitwarden CLI"),
                "{label:?} does not name the program the user will see in Task Manager"
            );
        }
        // ...and it applies to every string this control paints, including
        // the two that are new.
        for label in [BACKEND_LABEL, BACKEND_CHOICE_LABEL, OFFICIAL_CHOICE, BUILT_IN_CHOICE] {
            assert!(
                !label.to_lowercase().contains("crypto"),
                "{label:?} names an internal concept the user is not choosing between"
            );
            assert!(
                !label.to_lowercase().contains("backend"),
                "{label:?} uses this file's own word for `bw serve`"
            );
        }
        // **The other side is named too, and that is the change.** An on/off
        // pill made the built-in client the unnamed absence of the CLI; the
        // second cell is what stops it being that.
        assert!(
            BUILT_IN_CHOICE.contains("built-in client"),
            "{BUILT_IN_CHOICE:?} does not name the second client, so the choice has one \
             named side and one blank"
        );
        // **And the question names neither**, so the two cells are peers
        // rather than a default with an alternative beside it. This is the
        // assertion that fails if somebody folds one client's name back into
        // the row label.
        assert!(
            !BACKEND_CHOICE_LABEL.contains("Bitwarden CLI")
                && !BACKEND_CHOICE_LABEL.contains("built-in"),
            "{BACKEND_CHOICE_LABEL:?} names one of the two answers, which puts it back in \
             the privileged position the on/off pill gave it"
        );
        // The control: these are still four different strings, and the one
        // about keeping it running still says so. Without this, they could
        // collapse and pass every assertion above.
        for pair in [
            (BACKEND_LABEL, BACKEND_CHOICE_LABEL),
            (BACKEND_CHOICE_LABEL, OFFICIAL_CHOICE),
            (OFFICIAL_CHOICE, BUILT_IN_CHOICE),
            (BACKEND_LABEL, OFFICIAL_CHOICE),
        ] {
            assert_ne!(pair.0, pair.1);
        }
        assert!(BACKEND_LABEL.contains("running"), "the row no longer says what it decides");
    }


    /// **Every row on this page, and every paragraph under it, is readable on
    /// the real window without scrolling.**
    ///
    /// This test used to be about General's ceiling, and then about the
    /// backend row landing on the first card of a Vault page that scrolled.
    /// The page does not scroll any more -- the key list it scrolled for is
    /// on `Section::Api` -- so the claim goes back to its strongest form: a
    /// row painted past `BODY_SIZE.y` on a page with no scroll region is a
    /// row nobody can reach, and one of these four rows is the switch the
    /// owner was trapped by.
    ///
    /// **All the copy combinations**, because the descriptions are what
    /// make this page long and each is a different length: the crypto
    /// row's three-paragraph copy appears only on a self-hosted account, and
    /// the disk-cache row's Hello explanation only where Hello is missing.
    /// A test that measured the default state alone would be measuring the
    /// shortest page this screen can draw.
    ///
    /// **`use_official` is the third axis, and it is new.** Self-hosted with
    /// the switch off is the shortest the page can be -- the row about the
    /// `bw serve` subprocess is not drawn there at all -- and a page that lost
    /// a row is a page whose layout changed. `(self_hosted = false,
    /// use_official = false)` is the same page as `(self_hosted = false,
    /// use_official = true)`, because bitwarden.com cannot opt out; it is
    /// driven anyway, as the control that says so.
    ///
    /// Measured on `TALL_BODY` and compared against `BODY_SIZE.y`, which is
    /// not a fiction here but the only way to see an overflow: a page whose
    /// content ran past the window would simply be clipped on `BODY_SIZE`,
    /// and the measurement would report the fold back to itself.
    #[test]
    fn the_whole_vault_page_is_readable_without_scrolling() {
        for hello in [false, true] {
            for self_hosted in [false, true] {
                for use_official in [false, true] {
                    let painted = paint_vault_copy(hello, self_hosted, use_official);
                    // The premise: this really is the long copy. Without it a
                    // future edit that shortened the descriptions to nothing
                    // would pass this test while making it meaningless.
                    assert!(
                        painted.contains(official_crypto_description(self_hosted))
                            && painted.contains(disk_cache_description()),
                        "hello={hello} self_hosted={self_hosted} use_official={use_official}: \
                         the page did not paint the copy under test; got {:?}",
                        painted.strings()
                    );
                    let bottom = content_bottom(&painted);
                    assert!(
                        bottom < BODY_SIZE.y,
                        "hello={hello} self_hosted={self_hosted} use_official={use_official}: \
                         the Vault page runs to y={bottom} on a body {} tall and has no scroll \
                         region, so its last row is one nobody can reach",
                        BODY_SIZE.y
                    );
                }
            }
        }

        // **The control, and it is the reason this measurement is worth
        // making**: the same yardstick over the page that DOES scroll says
        // the opposite. Without it, `content_bottom` could be reading
        // something that is short whatever is drawn, and every assertion
        // above would pass on an empty page.
        let ctx = tall_context();
        let mut api = api_state();
        api.show_service_keys(vec![
            stored_key("one", None),
            stored_key("two", None),
            stored_key("three", None),
        ]);
        let bottom = content_bottom(&tall_frame(&ctx, &mut api, &[]));
        assert!(
            bottom > BODY_SIZE.y,
            "control: the Local API page with three keys measures y={bottom}, inside a body \
             {} tall, so this yardstick cannot tell a long page from a short one",
            BODY_SIZE.y
        );
    }

    /// The bottom of the lowest thing painted in the **content column**.
    ///
    /// Right of `NAV_WIDTH`, because the nav rail is drawn full-height on
    /// every page and its version footer sits at the bottom of whatever
    /// viewport it is given -- a measurement that included it would report
    /// the viewport's height on every page and could never fail.
    fn content_bottom(painted: &Painted) -> f32 {
        painted
            .ink
            .iter()
            .filter(|i| i.rect.min.x > NAV_WIDTH)
            .map(|i| i.rect.max.y)
            .fold(0.0f32, f32::max)
    }

    /// The Vault page with the copy each of its two ghostable rows shows in
    /// the named state: Hello present or missing, on a self-hosted server or
    /// on the official cloud, with the backend switch on or off.
    ///
    /// The third input is what makes the shortest page reachable: with a
    /// self-hosted server and the switch off, the row about the `bw serve`
    /// subprocess is not drawn.
    fn paint_vault_copy(hello: bool, self_hosted: bool, use_official: bool) -> Painted {
        let ctx = tall_context();
        let mut settings = Settings::default();
        settings.use_official_bw_crypto = use_official;
        let mut state = PrefsState::new(settings);
        state.section = Section::Vault;
        state.show_hello_available(if hello { || true } else { || false });
        state.show_account_source(if self_hosted {
            || {
                Some(AccountStatus::SignedIn {
                    email: Some("me@example.com".to_string()),
                    server: Some("https://vault.example.com".to_string()),
                })
            }
        } else {
            || {
                Some(AccountStatus::SignedIn {
                    email: Some("me@example.com".to_string()),
                    server: Some("https://vault.bitwarden.com".to_string()),
                })
            }
        });
        tall_frame(&ctx, &mut state, &[])
    }

    /// Clicks the backend pill at `pill` and then says yes to the question
    /// it raises, leaving the switch actually taken.
    ///
    /// The tests that predate the confirmation are about where the row is
    /// wired, not about the question, and they say what they always said
    /// through this helper: a click on this row now costs two presses, and
    /// spelling both out at each of their call sites would bury the
    /// assertion they exist for.
    ///
    /// **It takes the cell's LABEL, not a rectangle.** It used to take the
    /// backend pill's `Pos2`, found by index among the page's 40x22 pills,
    /// because there was one pill per side of a switch and the switch's side
    /// was its index. There are two named cells now, so the caller says which
    /// client it is pressing -- and a test that pressed the wrong control
    /// cannot pass by having pressed a control that happened to be there.
    fn take_the_backend_switch(ctx: &egui::Context, state: &mut PrefsState, cell: &str) {
        let shown = tall_frame(ctx, state, &[]);
        let asked = tall_click(ctx, state, shown.rect_of(cell).center());
        let yes = asked.ink_of(BACKEND_SWITCH_CONFIRM_BUTTON).rect.center();
        let _ = tall_click(ctx, state, yes);
    }

    /// **One click on the backend pill moves nothing.**
    ///
    /// This row is not a preference the user can try out and undo: the choice
    /// is captured at startup, so the click costs a restart and a fresh sign
    /// in whichever way it goes, and one of the two directions deletes a key
    /// off the disk. A mis-click on it is expensive in a way a mis-click on
    /// every other toggle in this window is not, which is the same reason the
    /// revoke button asks -- so this one asks in the same shape.
    ///
    /// **And the question is escapable**, which is the half worth a test of
    /// its own. The revoke confirmation shipped for a while with a Keep it
    /// that cleared a local and not the pending state, so the question stayed
    /// up and neither button did anything. The last assertions here are that
    /// answering no both leaves the setting alone AND puts the row back, so a
    /// second attempt can still be made.
    #[test]
    fn switching_the_backend_asks_before_it_takes_effect() {
        let ctx = tall_context();
        let mut state = on_a_self_hosted_server();
        assert!(state.settings.use_official_bw_crypto, "the shipped default");

        let first = tall_frame(&ctx, &mut state, &[]);
        let other_client = first.rect_of(BUILT_IN_CHOICE).center();
        let asked = tall_click(&ctx, &mut state, other_client);

        assert!(
            state.settings.use_official_bw_crypto,
            "one click moved the backend with no confirmation"
        );
        assert!(
            asked.any_containing("open it again yourself"),
            "the question leaves who restarts the app unsaid, and nothing here restarts it: \
             {:?}",
            asked.strings()
        );
        // **The needle is one only the question carries.** The row's own
        // description also says the switch applies at the next sign-in, so
        // "sign in" alone would be satisfied by the page that was already on
        // screen before anything was clicked -- a vacuous assertion, and one
        // that would then hold just as well after the question had gone.
        assert!(
            asked.any_containing("either sign out and back in"),
            "the question does not say the user has to sign in again: {:?}",
            asked.strings()
        );
        // The picker is still showing what it showed. A control that moved on
        // the click and moved back on the refusal would satisfy every value
        // assertion here and still show the user a choice that flipped
        // itself. Both cells, because a picker that showed BOTH as selected
        // would pass an assertion about either one alone.
        assert_eq!(
            asked.fill_behind(OFFICIAL_CHOICE),
            theme::BLUE_WASH,
            "the picker moved before the question was answered"
        );
        assert_ne!(
            asked.fill_behind(BUILT_IN_CHOICE),
            theme::BLUE_WASH,
            "the cell that was merely proposed is painted as the one in force"
        );

        let no = asked.ink_of(BACKEND_SWITCH_CANCEL_BUTTON).rect.center();
        let left = tall_click(&ctx, &mut state, no);
        assert!(state.settings.use_official_bw_crypto, "saying no switched the backend anyway");
        assert_eq!(
            left.fill_behind(OFFICIAL_CHOICE),
            theme::BLUE_WASH,
            "the picker moved on a refusal"
        );
        assert!(
            state.pending_backend_switch.is_none(),
            "the question is still pending, so the row is stuck on it"
        );
        assert!(
            !left.any_containing("either sign out and back in"),
            "the question is still on screen after it was answered: {:?}",
            left.strings()
        );

        // And the whole thing can be attempted again -- the assertion the
        // never-cleared pending state would fail, and the one that makes the
        // clearing above mean something.
        let again = tall_click(&ctx, &mut state, left.rect_of(BUILT_IN_CHOICE).center());
        assert!(
            again.any_containing("either sign out and back in"),
            "a second press of the other client raises nothing, so the row can only be \
             refused once: {:?}",
            again.strings()
        );

        // **Pressing the client already in force proposes nothing.** The
        // pill this replaced had no such state -- every click on it was a
        // flip -- so this is a question the picker newly makes it possible to
        // ask, and a confirmation raised by it would be one the user never
        // asked for. Cleared first, so what is measured is the new press and
        // not the question left standing above it.
        let cleared = tall_click(
            &ctx,
            &mut state,
            again.ink_of(BACKEND_SWITCH_CANCEL_BUTTON).rect.center(),
        );
        let same_client =
            tall_click(&ctx, &mut state, cleared.rect_of(OFFICIAL_CHOICE).center());
        assert!(
            !same_client.any_containing("either sign out and back in"),
            "pressing the client this account is already on raised a confirmation for a \
             change nobody asked for: {:?}",
            same_client.strings()
        );
        assert!(state.settings.use_official_bw_crypto, "and it changed nothing");
    }

    /// Saying yes really does switch -- the control for the test above, which
    /// a pair of buttons that did nothing at all would otherwise pass.
    ///
    /// Asserted through [`crate::backend_policy::choose`] rather than the
    /// field, for `the_backend_row_is_on_when_bw_is_the_backend`'s reason:
    /// what a yes has to change is which backend serves the vault.
    #[test]
    fn confirming_a_backend_switch_applies_it() {
        use crate::backend_policy::{choose, VaultBackendChoice};
        const SERVER: Option<&str> = Some("https://vault.example.com");

        let ctx = tall_context();
        let mut state = on_a_self_hosted_server();
        assert_eq!(
            choose(SERVER, state.settings.use_official_bw_crypto),
            VaultBackendChoice::BwServe,
            "control: the page did not start on the official CLI, so a change to the built-in \
             client would prove nothing"
        );

        let first = tall_frame(&ctx, &mut state, &[]);
        let asked = tall_click(&ctx, &mut state, first.rect_of(BUILT_IN_CHOICE).center());
        let yes = asked.ink_of(BACKEND_SWITCH_CONFIRM_BUTTON).rect.center();
        let after = tall_click(&ctx, &mut state, yes);

        assert_eq!(
            choose(SERVER, state.settings.use_official_bw_crypto),
            VaultBackendChoice::DirectRest,
            "the confirmation did not take the switch"
        );
        assert!(state.pending_backend_switch.is_none(), "the question outlived its answer");
        assert_eq!(
            after.fill_behind(BUILT_IN_CHOICE),
            theme::BLUE_WASH,
            "the switch was taken and the picker still shows the old client"
        );
        assert_ne!(
            after.fill_behind(OFFICIAL_CHOICE),
            theme::BLUE_WASH,
            "the picker shows both clients as the one in force"
        );
        assert!(
            !after.any_containing("either sign out and back in"),
            "the question is still on screen after it was taken: {:?}",
            after.strings()
        );
    }

    /// **The two directions do not cost the same thing, and the question says
    /// so.**
    ///
    /// Turning `bw` back on deletes `userkey.bin` -- `main`'s
    /// `settle_the_vault_backend` clears the store on every arm that is not
    /// `DirectRest`, and its own
    /// `settling_off_direct_rest_deletes_the_stored_vault_key` pins it.
    /// Turning it off does not delete anything; it creates the file. A single
    /// generic sentence would therefore either promise a deletion that does
    /// not happen or hide the one that does, and the second is the worse of
    /// the two: it is the only reassurance that switching back undoes what
    /// switching away did.
    ///
    /// Asserted on the pure function rather than on a painted frame, because
    /// the asymmetry is a fact about the decision and not about the layout.
    #[test]
    fn only_the_switch_back_to_bw_mentions_the_deleted_vault_key() {
        let back = backend_switch_prompt(BackendSwitch::ToOfficial);
        assert!(
            back.contains("deletes the vault key stored on this PC"),
            "going back to the official CLI does not say the stored key is removed, so the \
             user cannot tell whether it undoes what the switch did: {back:?}"
        );

        let away = backend_switch_prompt(BackendSwitch::ToBuiltIn);
        assert!(
            !away.contains("delete"),
            "the switch TO the built-in client claims to delete a key -- it writes one: {away:?}"
        );

        // Both halves the two directions really do share, so the asymmetry
        // above is the only difference and not an excuse for a thinner
        // sentence on one side.
        for (name, prompt) in [("back to bw", back), ("to the built-in client", away)] {
            // **The moment, not one route to it.** This used to demand the
            // word "restarted", which was the whole truth while
            // `settle_the_vault_backend` ran only at startup. Since 0.15.4 a
            // sign-in re-settles the live process
            // (`login_ui::direct_login_for_this_sign_in` ->
            // `backend_policy::resettle_for`), so a restart is one way in and
            // no longer the requirement. The question names the moment.
            assert!(
                prompt.contains("next sign in to this account"),
                "the {name} question does not say when the switch lands: {prompt:?}"
            );
            // **And BOTH routes to it**, because naming only one of two is
            // how the stale sentence got here: a user told "restart" will
            // restart, and a user told only "sign out" will not think of the
            // restart that also works.
            assert!(
                prompt.contains("sign out and back in"),
                "the {name} question does not offer the sign-out route: {prompt:?}"
            );
            // **It does not promise a relaunch it cannot perform.** Nothing
            // in this program respawns it, so a Yes that said "restarting
            // now" would be a lie told at the moment the user is agreeing to
            // something.
            assert!(
                prompt.contains("open it again yourself"),
                "the {name} question leaves who restarts the app unsaid, and nothing here \
                 restarts it: {prompt:?}"
            );
            // **And it does not claim a restart is REQUIRED**, which is the
            // sentence 0.15.4 made false and which this row inherited for two
            // releases.
            assert!(
                !prompt.contains("until Deskwarden is restarted"),
                "the {name} question still demands a restart the app no longer needs: \
                 {prompt:?}"
            );
        }
    }

    /// **A frame nobody clicked proposes nothing.**
    ///
    /// [`backend_switch`] is asked on every frame, so an arm that answered
    /// `Some` for the value the row was already showing would put a
    /// confirmation in front of a user who did nothing -- and, worse, one
    /// whose Yes would be a change they never asked for. The two flips are
    /// the control: without them this would pass against a function that
    /// answered `None` always.
    #[test]
    fn a_backend_row_that_did_not_move_asks_nothing() {
        assert_eq!(backend_switch(true, true), None, "the row was left on and raised a question");
        assert_eq!(
            backend_switch(false, false),
            None,
            "the row was left off and raised a question"
        );
        assert_eq!(backend_switch(true, false), Some(BackendSwitch::ToBuiltIn));
        assert_eq!(backend_switch(false, true), Some(BackendSwitch::ToOfficial));
        assert!(
            BackendSwitch::ToOfficial.deletes_the_stored_vault_key(),
            "the direction that clears `userkey.bin` says it does not"
        );
        assert!(
            !BackendSwitch::ToBuiltIn.deletes_the_stored_vault_key(),
            "the direction that WRITES the stored key claims to delete it"
        );
    }

    /// **Ghosted off a self-hosted server, and a click cannot select it.**
    ///
    /// The owner's rule, measured on the value rather than on the paint: a
    /// stray click on `bitwarden.com` must not put this app on a backend it
    /// will not run there.
    #[test]
    fn the_backend_row_is_disabled_and_inert_on_an_official_cloud() {
        let ctx = tall_context();
        let mut state = on_an_official_cloud();
        assert!(state.settings.use_official_bw_crypto, "the shipped default");

        let first = tall_frame(&ctx, &mut state, &[]);
        // **Both cells, pressed by name.** The ghosted picker still paints
        // the client in force, so a test that pressed only the other one
        // would miss a control that had gone live on the selected side.
        for cell in [BUILT_IN_CHOICE, OFFICIAL_CHOICE] {
            tall_frame(&ctx, &mut state, &click(first.rect_of(cell).center()));
            assert!(
                state.settings.use_official_bw_crypto,
                "a click on the ghosted {cell:?} changed the setting anyway"
            );
        }
        assert!(
            first
                .strings()
                .iter()
                .any(|t| t.contains("Only available on a self-hosted server")),
            "the ghosted row does not say why, so it reads as a bug; got {:?}",
            first.strings()
        );
        // **Ghosted, it still shows which client this account is on.** A
        // picker that greyed both cells identically would tell a
        // `bitwarden.com` user they have no client at all, when what is true
        // is that they have this one and cannot change it.
        assert_eq!(
            first.fill_behind(OFFICIAL_CHOICE),
            theme::BLUE_WASH,
            "the ghosted picker does not say which client is serving this account"
        );
        assert_ne!(
            first.fill_behind(BUILT_IN_CHOICE),
            theme::BLUE_WASH,
            "the ghosted picker shows the client this account cannot use as the one in force"
        );
    }

    /// **And on a self-hosted server it is live, and wired to that field.**
    ///
    /// The positive control for the test above -- without it, a row that was
    /// inert for every server would pass that one -- and the wiring assertion
    /// for this one: it is `use_official_bw_crypto` that moves and nothing
    /// else on the page.
    #[test]
    fn the_backend_row_toggles_on_a_self_hosted_server_and_moves_only_its_own_field() {
        let ctx = tall_context();
        let mut state = on_a_self_hosted_server();

        take_the_backend_switch(&ctx, &mut state, BUILT_IN_CHOICE);
        assert!(
            !state.settings.use_official_bw_crypto,
            "the row did not move to the built-in client on a self-hosted server"
        );
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
        assert!(!state.settings.cache_vault_to_disk, "the wrong row's toggle moved");
        assert!(!state.settings.service_enabled, "the wrong row's toggle moved");

        take_the_backend_switch(&ctx, &mut state, OFFICIAL_CHOICE);
        assert!(state.settings.use_official_bw_crypto, "and back to the official CLI again");
    }

    /// **The one guarantee on this page that cannot be seen by looking: the
    /// cell showing as chosen is the client that will actually serve the
    /// vault.**
    ///
    /// The name is kept from when this row was a pill and the claim was "the
    /// pill is ON when `bw` is the backend". It is the same claim about the
    /// same inversion, read off a control that now names both sides -- and it
    /// got easier to state, not harder: the assertions below say *which
    /// client* is shown as chosen, where they used to say which colour a
    /// nameless rectangle was painted.
    ///
    /// An inverted row paints, clicks and persists perfectly. Nothing about
    /// it looks wrong -- the control moves, the setting is written back, the
    /// counter-assertions on the neighbouring tests all hold -- and the only
    /// symptoms are that a fresh install shows the built-in client as chosen
    /// while running the official CLI, and that a user who presses "The
    /// official Bitwarden CLI" has opted in to the built-in client and stored
    /// a non-expiring master key on their PC in the belief they had done the
    /// opposite.
    ///
    /// So this is asserted **through
    /// [`crate::backend_policy::choose`]** -- the function that actually
    /// decides -- rather than through the field. The field could be read
    /// backwards by the row, or the row could be right and the decision
    /// inverted; going pill to decision covers both, and covers any third
    /// place an inversion could be introduced between them.
    ///
    /// A self-hosted server throughout, because that is the only account
    /// where the choice is live: the assertions below are about a control the
    /// user can actually move.
    #[test]
    fn the_backend_row_is_on_when_bw_is_the_backend() {
        use crate::backend_policy::{choose, VaultBackendChoice};
        const SERVER: Option<&str> = Some("https://vault.example.com");

        let ctx = tall_context();
        let mut state = on_a_self_hosted_server();

        // The shipped default: the pill is ON, and `bw serve` is what the
        // policy answers for it. Both halves, because either alone is
        // satisfied by an inversion of the other.
        assert!(state.settings.use_official_bw_crypto, "the shipped default");
        assert_eq!(
            choose(SERVER, state.settings.use_official_bw_crypto),
            VaultBackendChoice::BwServe,
            "the default configuration does not select the official `bw` CLI"
        );
        let first = tall_frame(&ctx, &mut state, &[]);
        assert_eq!(
            first.fill_behind(OFFICIAL_CHOICE),
            theme::BLUE_WASH,
            "the shipped default does not show the official CLI as chosen, so a fresh \
             install offers the built-in client while it is running the official CLI"
        );
        assert_ne!(
            first.fill_behind(BUILT_IN_CHOICE),
            theme::BLUE_WASH,
            "both cells are painted as chosen, so the control says nothing"
        );

        // Pressing the other cell moves to the built-in direct-REST client --
        // the state whose whole cost is the copy under this row.
        take_the_backend_switch(&ctx, &mut state, BUILT_IN_CHOICE);
        assert!(!state.settings.use_official_bw_crypto);
        assert_eq!(
            choose(SERVER, state.settings.use_official_bw_crypto),
            VaultBackendChoice::DirectRest,
            "pressing the built-in client's cell did not select the built-in client, so the \
             two cells do not mean what they say they mean"
        );
        let second = tall_frame(&ctx, &mut state, &[]);
        assert_eq!(
            second.fill_behind(BUILT_IN_CHOICE),
            theme::BLUE_WASH,
            "a configuration served by the built-in client does not show it as chosen"
        );
        assert_ne!(
            second.fill_behind(OFFICIAL_CHOICE),
            theme::BLUE_WASH,
            "the official CLI is still painted as chosen on a vault the built-in client serves"
        );

        // ...and back, so the two paints above are telling the states apart
        // rather than reporting one constant twice.
        take_the_backend_switch(&ctx, &mut state, OFFICIAL_CHOICE);
        assert!(state.settings.use_official_bw_crypto);
        assert_eq!(
            choose(SERVER, state.settings.use_official_bw_crypto),
            VaultBackendChoice::BwServe
        );

        // And the copy reads the same way round as the control does. A row
        // whose control is right and whose sentence is backwards is the same
        // defect delivered in prose.
        //
        // **This used to pin an ON paragraph and an OFF paragraph**, in that
        // order, because the row was a pill and its copy had to name both of
        // its states. There are no states to name now -- the cells carry the
        // names -- so what is pinned is that the copy describes the two
        // clients in the order the cells are drawn in, which is the same
        // claim about the same possible inversion: a reader who matches the
        // first sentence to the first cell must be matching the right pair.
        let copy = official_crypto_description(true);
        let official = copy
            .find("The official Bitwarden CLI")
            .expect("the copy no longer describes the official CLI");
        let built_in = copy
            .find("Deskwarden's built-in client")
            .expect("the copy no longer describes the built-in client");
        assert!(
            official < built_in,
            "the copy describes the built-in client first, under a picker whose first cell \
             is the official CLI: {copy:?}"
        );
        // ...and the cells are drawn in that order, so the comparison above
        // is against the layout rather than against a memory of it. `bw`'s
        // cell is to the LEFT of the built-in client's.
        let painted = tall_frame(&ctx, &mut state, &[]);
        assert!(
            painted.rect_of(OFFICIAL_CHOICE).center().x
                < painted.rect_of(BUILT_IN_CHOICE).center().x,
            "the cells are drawn in the opposite order to the copy above them"
        );
    }

    /// **What the enabled copy has to say**, asserted as text rather than
    /// trusted to a reviewer: this is what a user reads before deciding where
    /// their master key lives.
    ///
    /// # Two needles left this test, and neither lost its cover
    ///
    /// The four-paragraph copy this row used to carry also said that the
    /// switch re-authenticates and that going back deletes `userkey.bin`.
    /// Those are consequences of *taking* the switch, not of choosing between
    /// two clients, and they are said where they are acted on: the
    /// confirmation this row raises
    /// ([`backend_switch_prompt`]), which
    /// `only_the_switch_back_to_bw_mentions_the_deleted_vault_key` pins on
    /// both directions and which no user can take the switch without reading.
    /// Moving them there is what let the row shrink from four paragraphs to
    /// two, which is what the owner asked for.
    ///
    /// # The needles that grew
    ///
    /// It used to check that the copy said what the built-in client buys in
    /// the owner's own words -- "much lighter and faster" -- and said nothing
    /// at all about how much lighter. It now checks the two measured numbers
    /// from the README's table, ~118 MB and ~19 MB, which is a claim a stale
    /// figure fails: the row said "about 110 MB" for two releases and nothing
    /// measured anywhere in this repository agrees with it.
    ///
    /// And it checks that the copy does NOT demand a restart, which is the
    /// sentence 0.15.4 made false. See [`official_crypto_description`].
    #[test]
    fn the_backend_rows_copy_states_the_gain_the_cost_and_the_restart() {
        let copy = official_crypto_description(true);
        // Both clients named, in the copy as well as on the cells. A
        // description that named only one would put the other back in the
        // unnamed position the on/off pill gave it.
        assert!(
            copy.contains("official Bitwarden CLI") && copy.contains("built-in client"),
            "the copy does not name both clients: {copy:?}"
        );
        // **What each side costs, from the README's measured table.** ~118 MB
        // for the `bw serve` subprocess, ~19 MB for the whole app on the
        // built-in client -- the same two figures `README.md`'s memory table
        // publishes, so a reader who compares them finds one answer.
        assert!(
            copy.contains("118 MB"),
            "the copy does not say what the official CLI costs in memory: {copy:?}"
        );
        assert!(
            copy.contains("19 MB"),
            "the copy does not say what the built-in client costs in memory, so \
             \"lighter\" is a claim with no number behind it: {copy:?}"
        );
        // The stale figure, named so it cannot come back.
        // The stale figures, named so they cannot come back. "110 MB" was
        // the row's own for two releases; "111 MB" was `backend_policy`'s
        // fourth number for the same subprocess; "21 MB" was the built-in
        // client's figure before it was re-measured on 0.15.6 (~19 MB).
        for stale in ["110 MB", "111 MB", "21 MB"] {
            assert!(
                !copy.contains(stale),
                "the copy is back on {stale}, which nothing in this repository measures: \
                 {copy:?}"
            );
        }
        assert!(
            copy.contains("stored in the app"),
            "the copy does not say what the built-in client costs, without a euphemism"
        );
        assert!(
            copy.contains("never expires"),
            "the copy does not say the stored key outlives a session, which is the one \
             property that makes it different from the session token this app already keeps"
        );
        // **When it applies.** A setting that appears to do nothing when
        // clicked is a setting the user clicks again, and the thing they
        // would be clicking is the one that decides where their master key
        // lives.
        assert!(
            copy.contains("next time you sign in to this account"),
            "the copy does not say when the change takes effect, so a user who presses a \
             cell and sees nothing happen presses it again: {copy:?}"
        );
        // **And it no longer says the wrong moment.** `settle_the_vault_backend`
        // used to run only at startup; since 0.15.4 a sign-in re-settles the
        // live process, so "the next time Deskwarden starts" names a route
        // rather than the requirement -- and names the slower of the two.
        assert!(
            !copy.contains("next time Deskwarden starts"),
            "the copy still tells the user to restart, which stopped being the requirement \
             when `direct_login_for_this_sign_in` began re-settling the backend: {copy:?}"
        );
        // The disabled twin says what would make it available, and does not
        // repeat the trade -- there is nothing to weigh on a server this
        // backend will not serve.
        let ghosted = official_crypto_description(false);
        assert!(ghosted.contains("Only available on a self-hosted server"));
        assert!(
            !ghosted.contains("19 MB") && !ghosted.contains("118 MB"),
            "the unavailable row offers a trade the user cannot take: {ghosted:?}"
        );
    }

    /// `account_is_self_hosted` answers through `backend_policy`, so the two
    /// halves of this feature cannot disagree about which servers qualify.
    #[test]
    fn the_row_asks_backend_policy_which_servers_are_self_hosted() {
        assert!(account_is_self_hosted(Some(signed_in_on(Some(
            "https://vault.example.com"
        )))));
        // The substring trap, from this side too.
        assert!(account_is_self_hosted(Some(signed_in_on(Some(
            "https://vault.bitwarden.community"
        )))));
        assert!(!account_is_self_hosted(Some(signed_in_on(Some(
            "https://vault.bitwarden.com"
        )))));
        assert!(!account_is_self_hosted(Some(signed_in_on(Some("")))));
        // `None` server is bitwarden.com by definition, not "not known yet".
        assert!(!account_is_self_hosted(Some(signed_in_on(None))));
        assert!(!account_is_self_hosted(Some(AccountStatus::SignedOut)));
        // And the moments before anything has been published: unknown counts
        // as official, which is the direction that leaves the row ghosted
        // rather than clickable for an account it could not serve.
        assert!(!account_is_self_hosted(None));
    }

    /// **The whole partition, as one table**: eight combinations, and the
    /// single `DirectRest` row is the only one that hides anything.
    ///
    /// Driven through this page's own predicate rather than through `choose`
    /// directly, because the mutation this guards is a page that re-decides
    /// "which backend" for itself and drifts from the switch above the row.
    #[test]
    fn the_cli_rows_are_shown_for_every_account_except_the_built_in_one() {
        let self_hosted = Some("https://vault.example.com");
        let official = Some("https://vault.bitwarden.com");
        let unknown = Some("");

        // The one arm that hides: positively self-hosted AND opted out.
        assert!(
            !cli_rows_are_shown(self_hosted, false),
            "the built-in client has no `bw serve`, so the rows about it must not be drawn"
        );

        // Every other combination is `bw serve`, so the rows decide something
        // and are drawn. All seven, because a predicate that read only the
        // toggle -- or only the server -- would pass some of them.
        for (server, use_official, why) in [
            (self_hosted, true, "self-hosted, official CLI chosen"),
            (official, true, "bitwarden.com, official CLI chosen"),
            (official, false, "bitwarden.com, opted out -- but it cannot opt out"),
            (unknown, true, "unknown server, official CLI chosen"),
            (unknown, false, "unknown server counts as official"),
            (None, true, "no server URL is bitwarden.com by definition"),
            (None, false, "no server URL is bitwarden.com by definition"),
        ] {
            assert!(
                cli_rows_are_shown(server, use_official),
                "{why}: this account is served by `bw serve`, so the rows about it are real"
            );
        }
    }

    /// **The predicate is `backend_policy`'s answer and not a second one.**
    ///
    /// Without this, `cli_rows_are_shown` could be written as
    /// `is_self_hosted(server) == false || use_official` -- which happens to
    /// agree today and would drift the first time `choose` gained an input.
    #[test]
    fn the_predicate_is_exactly_the_backend_policy_decision() {
        use crate::backend_policy::{choose, VaultBackendChoice};
        for server in
            [None, Some(""), Some("https://vault.example.com"), Some("https://bitwarden.eu")]
        {
            for use_official in [true, false] {
                assert_eq!(
                    cli_rows_are_shown(server, use_official),
                    choose(server, use_official) == VaultBackendChoice::BwServe,
                    "the page disagrees with `backend_policy::choose` for \
                     server={server:?} use_official={use_official}"
                );
            }
        }
    }

    /// The server reader, including the two states that are not a signed-in
    /// account. Both must read as "unknown", which `is_self_hosted` already
    /// treats as official: `SignedOut` because there is no vault to name a
    /// server for, and nothing-published because the row is drawn before any
    /// shell has spoken -- `examples/ui_preview` never publishes at all.
    #[test]
    fn the_page_reads_the_server_off_the_status_and_nothing_else() {
        assert_eq!(
            account_server(&Some(AccountStatus::SignedIn {
                email: None,
                server: Some("https://vault.example.com".to_string()),
            })),
            Some("https://vault.example.com")
        );
        // `None` for bitwarden.com by definition -- `backend_policy`'s rule,
        // not this page's.
        assert_eq!(
            account_server(&Some(AccountStatus::SignedIn { email: None, server: None })),
            None
        );
        assert_eq!(account_server(&Some(AccountStatus::SignedOut)), None);
        assert_eq!(account_server(&None), None);
    }

    // -- no page of prose ---------------------------------------------------

    /// Body rectangles that are a control rather than the card they sit in.
    ///
    /// **Three things are not controls and everything else is.** A rectangle
    /// that paints no ink at all -- egui emits one per `Frame`, purely to
    /// carry a layout -- is invisible to the reader; the card itself (`card`,
    /// a `CARD` fill inside a `HAIRLINE` stroke) is the ground the rows sit
    /// on; and a row separator (`row_separator`) is a fill one point high.
    /// What is left is a toggle pill, a stepper box, a button, or a keyboard
    /// chip: something the reader can act on, or read a state off.
    ///
    /// The invisible-`Frame` filter is the one that matters. Without it every
    /// page in the window counts several, About included, and the rule below
    /// passes for every page forever -- which is why About's zero is asserted
    /// rather than assumed.
    ///
    /// Counted right of `NAV_WIDTH` only, so the selected nav row's wash is
    /// not mistaken for content.
    fn body_control_rects(painted: &Painted) -> usize {
        painted
            .rects
            .iter()
            .filter(|r| r.rect.min.x >= NAV_WIDTH)
            .filter(|r| r.fill.a() > 0 || r.stroke.width > 0.0)
            .filter(|r| r.rect.height() > 1.0)
            .filter(|r| !(r.fill == theme::CARD && r.stroke.color == theme::HAIRLINE))
            .count()
    }

    /// **No page in the nav is a page of prose.**
    ///
    /// # The defect this exists to stop coming back
    ///
    /// Five of the twelve sections were removed at once -- Autofill, Native
    /// apps, Security, Shortcuts and Sync & account -- and they all had the
    /// same thing wrong with them: **there was nothing on them a user could
    /// change.** Three were a single sentence saying exactly that; Shortcuts
    /// was a chip that could not be rebound; Sync & account was two rows
    /// pointing at other places. Each was individually defensible when it was
    /// written (3e lists them; the sentence was honest) and the fault only
    /// showed in aggregate: a reader spending five clicks out of twelve to be
    /// told "not here" stops believing the nav.
    ///
    /// The condition is a slow one -- a page loses its last control to a move
    /// and nobody notices the row is now a signpost -- so it needs a standing
    /// guard rather than a memory. This is it.
    ///
    /// # About is the exception, and it is asserted rather than skipped
    ///
    /// About genuinely paints no control, and it is right that it does: it is
    /// the identity page, and what it shows is not prose but **values read at
    /// runtime** -- this build's version, and the account actually signed in.
    /// A page that reports state has a reason to be visited; a page that
    /// recites a sentence the file hardcodes does not. So About is pinned to
    /// zero controls *and* to painting the real version line, which is what
    /// makes the exemption a description rather than a hole -- and which
    /// doubles as this test's control: if `body_control_rects` counted card
    /// backgrounds or separators, About would not be zero and the `> 0`
    /// assertions below would be passing on scaffolding.
    #[test]
    fn no_page_in_the_nav_is_only_prose() {
        for section in Section::ALL {
            // Tall, so nothing is culled: a control below the fold is still a
            // control, and a page whose only one scrolled out of view would
            // otherwise read here as prose.
            let painted = paint_tall(section);
            let controls = body_control_rects(&painted);
            if section == Section::About {
                assert_eq!(
                    controls, 0,
                    "About has grown a control, so it is no longer this rule's exception -- \r
                     either it belongs under the rule now, or `body_control_rects` has \r
                     started counting scaffolding and every assertion below is vacuous"
                );
                assert!(
                    painted.contains(&version_line()),
                    "About paints no control AND does not report this build's version, which \r
                     is the whole of what earns it a nav row; got {:?}",
                    painted.strings()
                );
                continue;
            }
            assert!(
                controls > 0,
                "{section:?} paints no control at all: its body is prose, which is what all \r
                 five removed sections had in common. A page that only tells the reader \r
                 where to go somewhere else belongs in the nav row it would send them to, \r
                 not in a row of its own; got {:?}",
                painted.strings()
            );
        }
    }

    #[test]
    fn the_hotkey_row_reports_the_one_shortcut_that_exists() {
        // At `Armed`, supplied, rather than through `paint(Section::General)`
        // and the process-wide `hotkey::availability()` it reads: the tests in
        // this binary run in parallel, and a page whose content depends on a
        // static another test can write is a page whose test fails on somebody
        // else's schedule.
        let painted = paint_fill_hotkey_at(crate::hotkey::HotkeyStatus::Armed);
        assert!(painted.contains("Fill the focused app"));
        assert!(painted.contains("CTRL+ALT+B"), "got {:?}", painted.strings());
        assert_eq!(
            painted.count_of_size(Vec2::new(40.0, 22.0)),
            0,
            "a shortcut is reported here, not rebound"
        );
    }

    /// **The General page is where the user finds out the shortcut is not
    /// working.**
    ///
    /// The crash this replaced was a process that vanished; the fix that
    /// replaced it must not be a shortcut that silently does nothing, which
    /// is a second invisible failure wearing the first one's clothes. So the
    /// unavailable row has to name the state and the way out of it. It says it
    /// on General now, one row under the toggle whose own description ends
    /// "CTRL+ALT+B is the only way to fill" -- which is the sentence that
    /// makes a silently dead chord actively misleading.
    #[test]
    fn a_shortcut_another_program_took_is_reported_on_the_page() {
        let painted = paint_fill_hotkey_at(crate::hotkey::HotkeyStatus::Unavailable(
            crate::hotkey::Unavailable::TakenByAnotherProgram,
        ));
        assert!(
            painted.any_containing("shortcut not working"),
            "the hotkey row paints an unavailable hotkey as though it worked: {:?}",
            painted.strings()
        );
        assert!(
            painted.any_containing("Another program on this PC is already using CTRL+ALT+B"),
            "the page says the shortcut is off without saying what to do about it: {:?}",
            painted.strings()
        );
        assert!(
            painted.any_containing("Close that program"),
            "the page names no way out of the conflict: {:?}",
            painted.strings()
        );
        // Still names the chord, so a user can tell WHICH shortcut is gone --
        // and see what to stop another program from using.
        assert!(painted.contains("CTRL+ALT+B"), "got {:?}", painted.strings());
    }

    /// **The chord is discoverable from the page the window opens on.**
    ///
    /// The three tests above drive [`fill_hotkey_row`] directly, which proves
    /// the row is right and says nothing about whether anything draws it. That
    /// gap is exactly how the fill hotkey could have been lost: the Shortcuts
    /// section was removed for having nothing on it a user could change, and
    /// the one thing it *did* carry -- the chord itself -- would have gone
    /// with it silently. `PROMPT_DESCRIPTION` does say CTRL+ALT+B, but inside
    /// a sentence arguing for a toggle, and it says nothing about whether the
    /// chord is currently registered; a user who cannot discover the shortcut,
    /// or cannot find out that something else has taken it, cannot use the
    /// feature at all.
    ///
    /// Painted through `paint`, so it is `draw_section` -> `draw_general` that
    /// is under test and not a harness. The status is whatever this process
    /// has published (`NotYetAttempted` in a test, which registers nothing),
    /// so the assertion is on the pair of labels rather than on one of them:
    /// either wording is a row that is really there.
    #[test]
    fn general_names_the_fill_hotkey_and_its_state() {
        let painted = paint(Section::General);
        // The chip, as its own painted run -- not the mention inside
        // `PROMPT_DESCRIPTION`, which is a longer galley and would not match.
        assert!(
            painted.contains(FILL_HOTKEY),
            "General does not name the fill hotkey as a chord of its own. \
             `PROMPT_DESCRIPTION` mentions it mid-sentence, and that is not discovery: it \
             is the argument for a toggle, read by someone already deciding about \
             prompts. Got {:?}",
            painted.strings()
        );
        let armed = painted.contains(FILL_HOTKEY_LABEL);
        let unavailable = painted.contains(FILL_HOTKEY_UNAVAILABLE_LABEL);
        assert!(
            armed != unavailable,
            "General paints the chord with neither of the row's two labels, or with \r
             both: a bare chip says what the key is and not whether pressing it does \r
             anything. Got {:?}",
            painted.strings()
        );
        // The control on the two above: this really is General, and the row
        // really is next to the toggle whose description promises the chord.
        assert!(
            painted.contains(PROMPT_DESCRIPTION),
            "this is not the General card; got {:?}",
            painted.strings()
        );
    }

    /// And the ordinary row says none of that. Without this, a row that
    /// warned unconditionally would pass the test above.
    #[test]
    fn a_working_shortcut_is_reported_without_a_warning() {
        let painted = paint_fill_hotkey_at(crate::hotkey::HotkeyStatus::Armed);
        assert!(painted.contains(FILL_HOTKEY_LABEL));
        assert!(painted.contains("CTRL+ALT+B"));
        assert!(
            !painted.any_containing("shortcut not working"),
            "a registered shortcut is being reported as broken: {:?}",
            painted.strings()
        );
        assert!(
            !painted.any_containing("Another program"),
            "a registered shortcut is being blamed on another program: {:?}",
            painted.strings()
        );
    }

    #[test]
    fn the_hotkey_row_names_the_hotkey_that_is_actually_registered() {
        // A source-text guard, the same device as `settings.rs`'s
        // `the_config_path_still_matches_the_one_main_resolves`: `FILL_HOTKEY`
        // is a display string with no compile-time link to
        // `hotkey::register_fill_hotkey`, so changing the registered chord
        // would otherwise leave this window confidently naming the old one.
        assert_eq!(FILL_HOTKEY, "CTRL+ALT+B");
        // **Read over the production half, not the whole file**, because
        // `settings.rs` and `item_list.rs` count their cross-file needles that
        // way for a reason that has now arrived here: a fixture in another
        // module's test code can satisfy a presence pin that production has
        // stopped satisfying. When this pin was written `hotkey.rs` had no
        // test code at all and said so; it has since grown a test module whose
        // fixtures build the very chord these two needles look for, so a
        // whole-file read would go green over a `register_fill_hotkey` that
        // had stopped registering anything.
        let (hotkey_rs, cut) = production_half(include_str!("hotkey.rs"));
        assert_eq!(
            cut, 1,
            "`hotkey.rs` no longer has exactly the one test module this walk was measured \
             against, so what it cut is not what it thinks it cut"
        );
        assert_eq!(
            hotkey_rs.matches(concat!("cfg(", "test)")).count(),
            0,
            "a `cfg(test)` gate survived the cut, so the needles below can be satisfied by \
             test code instead of by the registration they guard"
        );
        assert!(
            hotkey_rs.contains("Modifiers::CONTROL | Modifiers::ALT"),
            "hotkey.rs no longer registers Ctrl+Alt -- `FILL_HOTKEY` says it does"
        );
        assert!(
            hotkey_rs.contains("Code::KeyB"),
            "hotkey.rs no longer registers B -- `FILL_HOTKEY` says it does"
        );
    }

    // -- About -------------------------------------------------------------

    #[test]
    fn about_paints_the_real_crate_version_and_not_the_designs_mock_one() {
        let painted = paint(Section::About);
        assert!(painted.contains("Version"));
        assert!(
            painted.contains(&format!("Deskwarden {}", env!("CARGO_PKG_VERSION"))),
            "got {:?}",
            painted.strings()
        );
        assert!(
            !painted.strings().iter().any(|t| t.contains("1.4.0")),
            "got {:?}",
            painted.strings()
        );
    }

    #[test]
    fn about_does_not_claim_an_account_is_linked() {
        // 3e's "Bitwarden account linked" is a claim this window has no data
        // for: `main.rs` holds the status and does not pass it in. Asserting
        // it were true would be a lie on screen for an unauthenticated user.
        let painted = paint(Section::About);
        assert!(painted.contains("Bitwarden account"));
        assert!(
            !painted.strings().iter().any(|t| t.contains("linked")),
            "got {:?}",
            painted.strings()
        );
        assert!(painted.contains(ACCOUNT_STATUS));
    }

    // -- Updates: the update card ------------------------------------------
    //
    // One frame of the real `draw_prefs_body` per stage, read back through
    // what egui painted. Nothing here can reach the network: no
    // `update_panel::UpdateEnv` is installed in a test process (the panel's
    // own suite asserts that), and every stage below is parked rather than
    // arrived at.
    //
    // **These used to open `Section::About`.** The card moved to
    // `Section::Updates` and every one of them moved with it, rather than
    // About being kept open so the old expectations would still find their
    // needles -- which would have left the suite asserting that a page which
    // no longer draws the card still draws it.

    /// One frame of Updates with the update panel parked in `stage`.
    fn paint_updates(stage: crate::update_panel::UpdateStage, settings: Settings) -> Painted {
        let ctx = styled_context();
        let mut state = PrefsState::new(settings);
        state.section = Section::Updates;
        state.show_update_stage(stage);
        frame(&ctx, &mut state, &[])
    }

    fn a_release() -> crate::updater::ReleaseInfo {
        crate::updater::ReleaseInfo {
            version: semver::Version::parse("9.9.9").unwrap(),
            installer_download_url: "https://example.invalid/x-installer.exe".to_string(),
            installer_sha256: crate::updater::parse_asset_digest(&format!(
                "sha256:{}",
                "9".repeat(64)
            ))
            .unwrap(),
            body: "Fixed the thing".to_string(),
        }
    }

    // -- where the body starts, in both shells -------------------------------

    /// The top edge of the nav rail, found by its own fill.
    ///
    /// The rail is the full-height `theme::CARD` rectangle at the left of the
    /// body, `NAV_WIDTH` wide -- located by shape rather than by index
    /// because this page paints several white rectangles and their order is
    /// not this test's business.
    fn rail_top(painted: &Painted) -> f32 {
        painted
            .rects
            .iter()
            .filter(|r| {
                r.fill == theme::CARD && (r.rect.width() - NAV_WIDTH).abs() < 0.5
            })
            .map(|r| r.rect.top())
            .fold(f32::INFINITY, f32::min)
    }

    /// **The window shell: the rail starts where the titlebar ends.**
    ///
    /// The reported defect -- "there is a gap between window title panel and
    /// left nav panel on settings screen" -- was a strip of window background
    /// between the chrome's hairline and the top of the rail.
    /// `draw_window_chrome` ends in `advance_cursor_after_rect`, which leaves
    /// the cursor a whole `item_spacing.y` BELOW the bar, and the body used
    /// to start from that cursor.
    ///
    /// Its own test, separate from the modal's below, because a single
    /// assertion over "the page looks right" would pass while one of the two
    /// shells drifted.
    #[test]
    fn the_nav_rail_starts_exactly_where_the_window_chrome_ends() {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        let output = ctx.run_ui(raw_input(&[]), |ui| {
            draw_prefs_window(ui, &mut state);
        });
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }

        assert!(
            (rail_top(&painted) - CHROME_BAR_HEIGHT).abs() < 0.5,
            "the rail starts at {} and the titlebar ends at {CHROME_BAR_HEIGHT}: the strip \
             between them is the reported gap",
            rail_top(&painted)
        );
    }

    /// **The modal shell: the same claim, asserted separately.**
    ///
    /// This shell computes its body rect rather than inheriting a cursor, so
    /// it was already right -- which is what identified the window shell as
    /// the one at fault. Pinned anyway: the fix made the two shells space the
    /// body by the same rule, and a rule that holds in one place and not the
    /// other is how they came to differ in the first place.
    #[test]
    fn the_modal_body_starts_exactly_where_its_header_ends() {
        let card = Rect::from_min_size(Pos2::new(40.0, 30.0), Vec2::new(900.0, 700.0));

        let body = modal_body_rect(card);

        assert!(
            (body.top() - (card.top() + MODAL_HEADER_HEIGHT)).abs() < 0.5,
            "the modal's body starts at {} and its header ends at {}",
            body.top(),
            card.top() + MODAL_HEADER_HEIGHT
        );
    }

    // -- the account row ----------------------------------------------------

    /// The About page with the account row pointed at `source`.
    ///
    /// **Never at the published global.** A test that published would leave
    /// the next test's About page describing an account nobody set, and these
    /// run in one process in parallel; `show_account_source` is the seam that
    /// exists so no test has to.
    fn paint_about_account(source: fn() -> Option<AccountStatus>) -> Painted {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        state.section = Section::About;
        state.show_account_source(source);
        frame(&ctx, &mut state, &[])
    }

    fn a_signed_in_account() -> Option<AccountStatus> {
        Some(AccountStatus::SignedIn {
            email: Some("someone@example.invalid".to_string()),
            server: Some("https://vault.example.invalid/api".to_string()),
        })
    }

    /// **The reported defect: "Bitwarden account is empty but we know it for
    /// sure".**
    ///
    /// The app had the address -- `vault_window` logs its arrival -- and this
    /// page said to go and look somewhere else for it.
    #[test]
    fn the_about_page_names_the_signed_in_account_and_its_server() {
        let painted = paint_about_account(a_signed_in_account);

        assert!(painted.contains(ACCOUNT_LABEL));
        assert!(
            painted.contains("someone@example.invalid"),
            "the page knows the address and still will not say it: {:?}",
            painted.strings()
        );
        assert!(
            painted.any_containing("vault.example.invalid"),
            "which server this vault lives on is half of which account it is: {:?}",
            painted.strings()
        );
        assert!(
            !painted.contains(ACCOUNT_STATUS),
            "the page is still pointing at the vault window for an answer it has: {:?}",
            painted.strings()
        );
    }

    /// **Not-asked and signed-out must not look alike.**
    ///
    /// This replaces `checking_and_signed_out_say_different_things`, which
    /// drove the deleted `AccountStatus::Checking`. The claim it was written
    /// for outlives that variant: "nobody has told this page anything" and
    /// "nobody is signed in" are opposite facts, and a page that showed the
    /// same words for both would assert the second one for every window that
    /// draws About without a shell behind it -- `examples/ui_preview`, and
    /// any future entry point.
    #[test]
    fn nothing_published_and_signed_out_say_different_things() {
        let unpublished = paint_about_account(|| None);
        let signed_out = paint_about_account(|| Some(AccountStatus::SignedOut));

        assert!(unpublished.contains(ACCOUNT_STATUS), "got {:?}", unpublished.strings());
        assert!(signed_out.contains(ACCOUNT_SIGNED_OUT), "got {:?}", signed_out.strings());
        assert!(
            !unpublished.contains(ACCOUNT_SIGNED_OUT),
            "a page nobody has published to must not assert that nobody is signed in: {:?}",
            unpublished.strings()
        );
        // The control in the other direction: the signed-out page really has
        // stopped saying the not-asked sentence, so the two assertions above
        // are two different paints rather than one string that never appears.
        assert!(
            !signed_out.contains(ACCOUNT_STATUS),
            "the signed-out row still points at the vault window for an answer it has: {:?}",
            signed_out.strings()
        );
    }

    /// **About's account row named the Bitwarden CLI, unconditionally, on a
    /// page a built-in-client user reads too.**
    ///
    /// Not ghosted and not hedged: it asserted a program was asked. These are
    /// the sentences, and the fix is to name what the app did rather than
    /// what it did it with -- which is the altitude the whole split is about.
    /// On the built-in client the status comes from `rest::api` and no CLI is
    /// running at all.
    ///
    /// It covered a third note, `ACCOUNT_CHECKING_NOTE`, until the state it
    /// belonged to stopped being reachable and was deleted with it.
    #[test]
    fn the_account_row_says_what_happened_and_not_which_program_did_it() {
        for note in [ACCOUNT_STATUS, ACCOUNT_NO_EMAIL_NOTE, ACCOUNT_SIGNED_OUT_NOTE] {
            assert!(
                !note.contains("Bitwarden CLI"),
                "{note:?} names a program that is not running on the built-in client"
            );
            // The control: each note still says something. A note emptied to
            // pass the assertion above would leave the row reading as a field
            // that failed to load, which is the defect these constants were
            // written for.
            assert!(note.len() > 20, "{note:?} was emptied rather than reworded");
        }
        // And they are still three different sentences -- "nobody has told
        // this page anything", "nobody is signed in" and "signed in, address
        // not reported" are three different facts, and a row that said the
        // same thing for any two of them would assert one of them falsely.
        assert_ne!(ACCOUNT_STATUS, ACCOUNT_SIGNED_OUT_NOTE);
        assert_ne!(ACCOUNT_STATUS, ACCOUNT_NO_EMAIL_NOTE);
        assert_ne!(ACCOUNT_NO_EMAIL_NOTE, ACCOUNT_SIGNED_OUT_NOTE);
    }

    /// **The row is never blank, in any state.**
    ///
    /// The property the old constant existed for and the one this change had
    /// to keep: an empty right-hand column reads as a field that failed to
    /// load, on the page someone opens to check whether their app is working.
    ///
    /// "In any state" is enforced rather than asserted: the `match` below has
    /// no wildcard arm, so a variant added to [`AccountStatus`] and left out
    /// of `states` stops this file compiling. It is written that way because
    /// the list previously carried `Checking`, and a hand-maintained list is
    /// exactly what a deleted or added variant leaves quietly wrong.
    #[test]
    fn the_account_row_always_says_something() {
        // The exhaustiveness control. It paints nothing; it fails to build if
        // `AccountStatus` grows an arm the list below does not carry.
        fn every_variant_is_in_the_list_below(status: &AccountStatus) {
            match status {
                AccountStatus::SignedOut | AccountStatus::SignedIn { .. } => {}
            }
        }
        every_variant_is_in_the_list_below(&AccountStatus::SignedOut);

        let states = [
            None,
            Some(AccountStatus::SignedOut),
            Some(AccountStatus::SignedIn { email: None, server: None }),
            Some(AccountStatus::SignedIn {
                email: Some("a@b.invalid".to_string()),
                server: None,
            }),
        ];

        for state in states {
            let (description, value) = account_row_text(state.as_ref());
            assert!(!description.trim().is_empty(), "{state:?} paints no description");
            assert!(
                value.as_ref().map(|v| !v.trim().is_empty()).unwrap_or(true),
                "{state:?} paints an EMPTY value, which reads as a field that failed to load; \
                 a state with nothing to say must carry no value at all"
            );
        }
    }

    /// A signed-in account whose address the CLI did not report still says
    /// which of the two facts is missing, rather than silently reading as an
    /// account with no name.
    #[test]
    fn a_signed_in_account_with_no_address_says_so() {
        let (description, value) = account_row_text(Some(&AccountStatus::SignedIn {
            email: None,
            server: None,
        }));

        assert_eq!(value.as_deref(), Some(ACCOUNT_NO_EMAIL));
        assert!(description.contains(ACCOUNT_NO_EMAIL_NOTE), "got {description:?}");
    }

    /// **A `bw status` answer becomes one sentence, in one place.**
    ///
    /// Two publishers feed this row -- `main`'s startup drain and
    /// `vault_window`'s late arrival -- and both go through
    /// `account_status_of`, so one CLI answer cannot reach the page as two
    /// different claims.
    #[test]
    fn a_cli_answer_maps_to_the_state_it_describes() {
        use crate::login_ui::{BwStatus, BwStatusDetails};

        let unlocked = account_status_of(&BwStatusDetails {
            status: BwStatus::Unlocked,
            user_email: Some("who@example.invalid".to_string()),
            server_url: None,
        });
        assert_eq!(
            unlocked,
            AccountStatus::SignedIn {
                email: Some("who@example.invalid".to_string()),
                server: None
            }
        );

        // A locked vault is still a signed-in account: the row answers "which
        // account", not "is it open".
        let locked = account_status_of(&BwStatusDetails {
            status: BwStatus::Locked,
            user_email: Some("who@example.invalid".to_string()),
            server_url: None,
        });
        assert!(matches!(locked, AccountStatus::SignedIn { .. }));

        // And the value a failed or unparseable `bw status` comes back as.
        assert_eq!(
            account_status_of(&crate::login_ui::unknown_status_details()),
            AccountStatus::SignedOut
        );
    }

    /// Two frames of the Updates page in one `Context`, and the SECOND one's
    /// paint.
    ///
    /// The notes region's `notes_fit` reads the previous frame's overflow
    /// (see there), so the first frame of a fresh context is deliberately the
    /// pessimistic one -- it shows the bar. Anything asserting on the settled
    /// verdict has to run the frame that has a memory to read, and this is
    /// that helper rather than an extra `frame` call copied into each test.
    fn paint_updates_settled(
        stage: crate::update_panel::UpdateStage,
        settings: Settings,
    ) -> Painted {
        let ctx = styled_context();
        let mut state = PrefsState::new(settings);
        state.section = Section::Updates;
        state.show_update_stage(stage);
        let _first = frame(&ctx, &mut state, &[]);
        frame(&ctx, &mut state, &[])
    }

    /// A release body far longer than [`notes_max_height`] can show.
    ///
    /// Sixty lines, which is far past the whole page's height and so past the
    /// region's ceiling however much of the page it is given. Sized this way
    /// on purpose: the number is not tuned to the old fixed 128 points, so it
    /// keeps meaning "overflows" now that the ceiling is the page's own
    /// remaining space.
    fn notes_that_overflow() -> String {
        (0..60)
            .map(|n| format!("line {n} of a release note that keeps going"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The white card the notes heading is painted on, found by containment
    /// rather than by index -- the Updates page has several cards and their
    /// order is not this test's business.
    fn notes_card_rect(painted: &Painted) -> Rect {
        let heading = painted.rect_of(UPDATE_NOTES_LABEL);
        painted
            .rects
            .iter()
            .map(|r| r.rect)
            .filter(|r| r.contains(heading.center()) && r.width() > heading.width())
            .min_by(|a, b| a.width().partial_cmp(&b.width()).unwrap())
            .expect("the notes heading is painted on no card at all")
    }

    /// Is a scrollbar-width rectangle painted in ink anyone can see?
    ///
    /// Colour, not geometry, is the question: `theme::hide_scrollbar` works by
    /// taking the bar's opacities to zero, and a bar at alpha 0 still occupies
    /// a perfectly reasonable rectangle. A test reading only rectangles would
    /// pass whether the bar was suppressed or not.
    fn paints_a_visible_scrollbar(painted: &Painted) -> bool {
        painted.rects.iter().any(|r| {
            (r.rect.width() - UPDATE_NOTES_BAR_WIDTH).abs() < 0.5
                && r.rect.height() > UPDATE_NOTES_BAR_WIDTH
                && r.fill.a() > 0
        })
    }

    /// **The bar appears only when something is actually clipped.**
    ///
    /// The reported defect: a pinned-open scrollbar down the side of three
    /// lines of notes, pointing at nothing. The cue is worth its space when
    /// the text continues past the region and is noise when it does not.
    #[test]
    fn short_notes_paint_no_scrollbar_and_long_notes_do() {
        let short = paint_updates_settled(
            crate::update_panel::UpdateStage::Available(a_release()),
            Settings::default(),
        );
        let long = paint_updates_settled(
            crate::update_panel::UpdateStage::Available(crate::updater::ReleaseInfo {
                body: notes_that_overflow(),
                ..a_release()
            }),
            Settings::default(),
        );

        assert!(
            !paints_a_visible_scrollbar(&short),
            "notes that fit inside the region are not clipped, so a scrollbar beside them \
             is a cue pointing at nothing. Bar-width rectangles painted: {:?}",
            short
                .rects
                .iter()
                .filter(|r| (r.rect.width() - UPDATE_NOTES_BAR_WIDTH).abs() < 0.5)
                .map(|r| (r.rect, r.fill))
                .collect::<Vec<_>>()
        );
        assert!(
            paints_a_visible_scrollbar(&long),
            "notes clipped at the region's ceiling with no bar read as text that failed to \
             load rather than as text that continues -- and the bar must be there WITHOUT \
             the pointer, which this frame has nowhere near the region"
        );
    }

    /// **The bar's lane is reserved whether or not the bar is painted.**
    ///
    /// This is the half that `password_health` was just fixed for in mirror
    /// image: `AlwaysVisible` is what makes egui lay the gutter out, and the
    /// width subtracted inside the region is unconditional, so suppressing
    /// the PAINT cannot move the card's edge. If the reservation were made to
    /// follow the bar's visibility instead, this card would grow and shrink
    /// as release notes got longer -- next to a Version card that does not.
    #[test]
    fn the_notes_card_is_the_same_width_short_or_long() {
        let short = paint_updates_settled(
            crate::update_panel::UpdateStage::Available(a_release()),
            Settings::default(),
        );
        let long = paint_updates_settled(
            crate::update_panel::UpdateStage::Available(crate::updater::ReleaseInfo {
                body: notes_that_overflow(),
                ..a_release()
            }),
            Settings::default(),
        );

        let short_card = notes_card_rect(&short);
        let long_card = notes_card_rect(&long);
        assert!(
            (short_card.width() - long_card.width()).abs() < 0.5
                && (short_card.left() - long_card.left()).abs() < 0.5,
            "the card moved with the length of its notes: short {short_card:?}, long \
             {long_card:?}"
        );
        // **The RIGHT edge, stated separately.** Equal widths and equal left
        // edges already imply it arithmetically, but the right edge is the
        // one a person sees against the Version card above, and it is the
        // edge a re-subtracted gutter would move. Asserted in its own name so
        // a future change to how the width is measured cannot quietly drop
        // the property this test exists for.
        assert!(
            (short_card.right() - long_card.right()).abs() < 0.5,
            "the card's right edge moved with the length of its notes: short {} long {}",
            short_card.right(),
            long_card.right()
        );
    }

    /// **The region takes the page's remaining height, then scrolls.**
    ///
    /// The reported request: "that area should expand until the end of the
    /// screen and only then scroll if needed". Both halves are here, and the
    /// second half is the one that used to be a fixed 128 points.
    ///
    /// Read off the CARD rather than off the scroll area, because the card is
    /// drawn around whatever its rows came to and is therefore the honest
    /// witness to how tall the region actually got. A body that overflows
    /// must reach the bottom of the page's content area; a short one must not
    /// -- it must still stop at its own last line, which is what stops a
    /// three-line release note sitting in a box of empty white.
    #[test]
    fn long_notes_fill_the_page_and_short_notes_only_take_what_they_need() {
        let short = paint_updates_settled(
            crate::update_panel::UpdateStage::Available(a_release()),
            Settings::default(),
        );
        let long = paint_updates_settled(
            crate::update_panel::UpdateStage::Available(crate::updater::ReleaseInfo {
                body: notes_that_overflow(),
                ..a_release()
            }),
            Settings::default(),
        );

        let short_card = notes_card_rect(&short);
        let long_card = notes_card_rect(&long);

        // Down to the page's own edge, give or take the content padding the
        // column was shrunk by. Not asserted to the point, because the exact
        // value is `CONTENT_PAD_Y` plus a card's own inner padding and that
        // arithmetic is the layout's business, not this test's.
        assert!(
            long_card.bottom() > BODY_SIZE.y - CONTENT_PAD_Y - 2.0,
            "long notes stopped {} points short of the page's bottom edge ({}), so the region \
             is still capped at something other than the space available",
            BODY_SIZE.y - long_card.bottom(),
            BODY_SIZE.y
        );
        // ...and never past it, which is the guarantee the old fixed height
        // was protecting and that this must not have spent.
        assert!(
            long_card.bottom() <= BODY_SIZE.y,
            "the notes card ran off the bottom of a page that cannot scroll or resize: {}",
            long_card.bottom()
        );

        // The short body is a single line. Anything close to the page's height
        // here would mean the region had grown to its ceiling rather than to
        // its content -- the "128pt box with empty space" defect, moved down
        // the page rather than fixed.
        assert!(
            short_card.bottom() < long_card.bottom() - UPDATE_NOTES_MIN_HEIGHT,
            "a short release note took the whole page anyway: short card ends at {}, long at \
             {}",
            short_card.bottom(),
            long_card.bottom()
        );
    }

    // -- Updates: a link in the notes ---------------------------------------
    //
    // These are the render half of the reversal recorded in `updater`'s
    // subset header: links used to be inert here by design and now are not.
    // The parse half -- which URLs become followable at all -- is asserted in
    // `updater`, at the one place that decides.
    //
    // **Nothing below clicks a link.** A click opens a browser, which is not
    // a thing a test may do, so what is asserted here is that the click
    // TARGET exists and sits on the link's words: egui reports a pointing
    // hand only from a `Sense::click` widget under the pointer, so the cursor
    // is the honest witness to a hit rectangle a test must not press.

    /// The pointing-hand cursor a hover at `pos` produces, over notes made of
    /// `body`.
    ///
    /// Two frames, like [`paint_updates_settled`]: the region's own overflow
    /// verdict settles on the second, and a hit rectangle laid over a line
    /// that moved between frames would be read at the wrong place.
    fn cursor_over_notes(body: &str, pos: Pos2) -> egui::CursorIcon {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        state.section = Section::Updates;
        state.show_update_stage(crate::update_panel::UpdateStage::Available(
            crate::updater::ReleaseInfo { body: body.to_string(), ..a_release() },
        ));
        let hover = [egui::Event::PointerMoved(pos)];
        let mut icon = egui::CursorIcon::Default;
        // A loop rather than two spelled-out frames, so this harness is ONE
        // `draw_prefs_body` site: `exactly_one_place_in_this_program_draws_
        // the_settings_form` counts them textually, and a helper that says
        // the name twice spends two of the budget it is guarding.
        for _ in 0..2 {
            icon = ctx
                .run_ui(raw_input(&hover), |ui| draw_prefs_body(ui, &mut state))
                .platform_output
                .cursor_icon;
        }
        icon
    }

    /// The rect of the one painted line containing `needle`.
    ///
    /// A line is one galley, so its whole text comes back as a single run and
    /// the link's words are inside it rather than beside it -- which is
    /// exactly the property that makes the hit rectangles worth testing.
    fn notes_line_rect(painted: &Painted, needle: &str) -> Rect {
        let mut found = painted.texts.iter().filter(|(t, _)| t.contains(needle));
        let first = found
            .next()
            .unwrap_or_else(|| panic!("no line contains {needle:?}; got {:?}", painted.strings()));
        assert!(found.next().is_none(), "{needle:?} appears on more than one line");
        first.1
    }

    /// **An `https` link's words are a click target, and the rest of the line
    /// is not.**
    ///
    /// The link is put at the START of the line on purpose, so the assertion
    /// can name a point on its words and a point off them without measuring
    /// glyphs: the words begin at the line's left edge, and the trailing
    /// prose ends at its right one. A hit rectangle covering the whole line
    /// -- the defect a union of wrapped rows would produce -- fails the
    /// second half.
    #[test]
    fn an_https_links_words_are_the_click_target_and_the_rest_of_the_line_is_not() {
        let body = "[the notes](https://example.invalid/x) and then some ordinary trailing prose";
        let painted = paint_updates_settled(
            crate::update_panel::UpdateStage::Available(crate::updater::ReleaseInfo {
                body: body.to_string(),
                ..a_release()
            }),
            Settings::default(),
        );
        let line = notes_line_rect(&painted, "the notes");

        assert_eq!(
            cursor_over_notes(body, Pos2::new(line.left() + 4.0, line.center().y)),
            egui::CursorIcon::PointingHand,
            "the words of an https link are not offering to be followed"
        );
        assert_ne!(
            cursor_over_notes(body, Pos2::new(line.right() - 4.0, line.center().y)),
            egui::CursorIcon::PointingHand,
            "the whole line became a link: ordinary prose beside a link must not open it"
        );
    }

    /// **A link the subset refused offers nothing to click, anywhere on its
    /// line.**
    ///
    /// The same line, one scheme different. Swept across the line rather than
    /// sampled at one point, because a refusal that merely moved the hit
    /// rectangle would pass a single sample and is the same defect.
    #[test]
    fn a_link_the_subset_refused_is_not_a_click_target_anywhere() {
        let body = "[the notes](file:///C:/Windows/System32/calc.exe) and then some prose";
        let painted = paint_updates_settled(
            crate::update_panel::UpdateStage::Available(crate::updater::ReleaseInfo {
                body: body.to_string(),
                ..a_release()
            }),
            Settings::default(),
        );
        let line = notes_line_rect(&painted, "the notes");

        let mut x = line.left() + 1.0;
        while x < line.right() {
            assert_ne!(
                cursor_over_notes(body, Pos2::new(x, line.center().y)),
                egui::CursorIcon::PointingHand,
                "a refused scheme was clickable at x={x} on the line {line:?}"
            );
            // Sixteen points, which is a third of the width "the notes" is
            // painted at: coarse enough to keep this test's frame count
            // sane, fine enough that the link cannot fall between samples.
            x += 16.0;
        }
        // Control: the same sweep on the same sentence with an https scheme
        // DOES find a target, so the silence above is a refusal rather than a
        // test that cannot see a link at all.
        let allowed = "[the notes](https://example.invalid/x) and then some prose";
        let allowed_line = notes_line_rect(
            &paint_updates_settled(
                crate::update_panel::UpdateStage::Available(crate::updater::ReleaseInfo {
                    body: allowed.to_string(),
                    ..a_release()
                }),
                Settings::default(),
            ),
            "the notes",
        );
        let mut x = allowed_line.left() + 1.0;
        let mut hit = false;
        while x < allowed_line.right() {
            hit |= cursor_over_notes(allowed, Pos2::new(x, allowed_line.center().y))
                == egui::CursorIcon::PointingHand;
            // Sixteen points, which is a third of the width "the notes" is
            // painted at: coarse enough to keep this test's frame count
            // sane, fine enough that the link cannot fall between samples.
            x += 16.0;
        }
        assert!(hit, "the sweep cannot see a link at all, so its silence above means nothing");
    }

    /// **The destination is still painted beside the words.**
    ///
    /// The half of the old rule that survived it becoming clickable: a reader
    /// can see where a link goes without going there. Asserted for an
    /// accepted link and a refused one, because the refused case is where a
    /// missing URL would read as the release note having lost a word.
    #[test]
    fn a_links_destination_is_visible_whether_or_not_it_can_be_followed() {
        for (body, url) in [
            ("see [the notes](https://example.invalid/x)", "https://example.invalid/x"),
            ("see [the notes](ms-settings:windowsupdate)", "ms-settings:windowsupdate"),
        ] {
            let painted = paint_updates_settled(
                crate::update_panel::UpdateStage::Available(crate::updater::ReleaseInfo {
                    body: body.to_string(),
                    ..a_release()
                }),
                Settings::default(),
            );
            assert!(
                painted.any_containing(url),
                "{url:?} was not painted anywhere: {:?}",
                painted.strings()
            );
        }
    }

    /// **A link that wraps gets a rectangle per row, not one box around
    /// both.**
    ///
    /// The union of a wrapped link's two rows covers the blank end of the
    /// first row and the blank start of the second, so a click on empty space
    /// would open a URL. Checked on the geometry directly, because the two
    /// arrangements are indistinguishable from any one sample point.
    #[test]
    fn a_wrapped_link_is_hit_tested_row_by_row() {
        use egui::text::{LayoutJob, TextFormat};

        let ctx = styled_context();
        let words = "a link whose words are long enough to have to wrap somewhere";
        let mut job = LayoutJob::default();
        job.wrap.max_width = 120.0;
        job.append(words, 0.0, TextFormat::default());
        let galley = ctx.fonts_mut(|fonts| fonts.layout_job(job));
        assert!(galley.rows.len() > 1, "the fixture did not wrap, so it tests nothing");

        let rects = notes_link_rects(&galley, Pos2::ZERO, 0..words.chars().count());
        assert_eq!(
            rects.len(),
            galley.rows.len(),
            "one rectangle per row of the wrapped link, got {rects:?}"
        );
        // No rectangle may reach into another row's band, which is what a
        // union would do.
        for (index, rect) in &rects {
            let row = &galley.rows[*index];
            assert!(
                rect.top() >= row.pos.y - 0.5 && rect.bottom() <= row.pos.y + row.row.size.y + 0.5,
                "row {index}'s hit area {rect:?} leaves the row it belongs to"
            );
        }
    }

    /// **The state the tray could not express.**
    ///
    /// The control this page replaced was `MenuItem::new("Update available",
    /// false, None)`: the words present from startup, only the enabling
    /// waiting on the check. So the answer to "am I up to date" was a
    /// permanent claim that you were not, on an item that would not open. The
    /// page has to say the opposite, in words, and offer a live button beside
    /// it.
    #[test]
    fn about_says_you_are_current_and_still_offers_the_check() {
        let painted = paint_updates(
            crate::update_panel::UpdateStage::UpToDate,
            Settings::default(),
        );

        assert!(painted.contains(UPDATE_SECTION_LABEL));
        assert!(
            painted.contains(UPDATE_UP_TO_DATE_DESCRIPTION),
            "the page must SAY there is no update, not merely fail to claim one: {:?}",
            painted.strings()
        );
        assert!(painted.contains(UPDATE_CHECK_BUTTON), "got {:?}", painted.strings());
        assert!(
            !painted.strings().iter().any(|t| t.contains(concat!("Update", " available"))),
            "the tray's words are back on the page: {:?}",
            painted.strings()
        );
    }

    /// A found release shows its version, its notes, and the way to get it.
    #[test]
    fn about_shows_the_release_notes_beside_the_download() {
        let painted = paint_updates(
            crate::update_panel::UpdateStage::Available(a_release()),
            Settings::default(),
        );

        assert!(painted.contains("Version 9.9.9 is available."), "got {:?}", painted.strings());
        assert!(painted.contains(UPDATE_NOTES_LABEL));
        assert!(painted.contains("Fixed the thing"), "got {:?}", painted.strings());
        assert!(painted.contains(UPDATE_DOWNLOAD_BUTTON));
    }

    /// Every run of ink the notes region painted, joined.
    fn painted_notes(body: &str) -> String {
        let painted = paint_updates(
            crate::update_panel::UpdateStage::Available(crate::updater::ReleaseInfo {
                body: body.to_string(),
                ..a_release()
            }),
            Settings::default(),
        );
        painted
            .ink
            .iter()
            .map(|i| i.rendered.clone())
            .collect::<Vec<_>>()
            .join("\u{1}")
    }

    /// **Markup is rendered; the characters that made it are not painted.**
    ///
    /// This asserts on the rendered galley -- `TextInk::rendered` is what
    /// egui actually laid out -- so the old behaviour, `**` on screen as two
    /// asterisks, fails it.
    #[test]
    fn release_notes_render_the_bounded_markdown_subset() {
        let painted = painted_notes(
            "# Fixed\n- a **bold** word, an *italic* one and some `code`\n",
        );

        assert!(painted.contains("Fixed"), "got {painted:?}");
        assert!(painted.contains("bold"), "got {painted:?}");
        assert!(painted.contains("italic"), "got {painted:?}");
        assert!(painted.contains("code"), "got {painted:?}");
        assert!(
            !painted.contains('*') && !painted.contains('#') && !painted.contains('`'),
            "the markup characters are still on screen, so nothing was rendered: {painted:?}"
        );
    }

    /// **What is outside the subset is painted as the characters it is.**
    ///
    /// The floor the parser falls through to, and the reason a malformed
    /// body is not a defect: raw HTML is not markup here, and an unclosed
    /// emphasis is two asterisks somebody typed.
    #[test]
    fn what_the_subset_excludes_is_painted_literally() {
        let painted = painted_notes("<b>bold?</b> and **unclosed and #1234 issues");

        assert!(painted.contains("<b>bold?</b>"), "got {painted:?}");
        assert!(painted.contains("**unclosed"), "got {painted:?}");
        assert!(
            painted.contains("#1234"),
            "a hash with no space after it is an issue number, not a heading: {painted:?}"
        );
    }

    /// **A link's words and its destination are painted, and nothing is
    /// clickable.**
    ///
    /// The click is the exclusion that matters: this text arrives over the
    /// network onto the page that says what is about to be installed, so a
    /// link there is the one element that could turn misleading styling into
    /// a place the user can be sent. A `Label` is not egui's link widget and
    /// takes no action; showing the URL keeps the information without the
    /// path.
    #[test]
    fn a_link_shows_its_words_and_its_destination_and_opens_nothing() {
        let painted = painted_notes("See [the release page](https://example.invalid/r) for more.");

        assert!(painted.contains("the release page"), "got {painted:?}");
        assert!(
            painted.contains("https://example.invalid/r"),
            "the destination must be readable, so a user can decide about it: {painted:?}"
        );
        assert!(
            !painted.contains("[the release page]"),
            "the brackets are still on screen: {painted:?}"
        );
        // A source-text guard beside the paint one: a `Label` cannot navigate
        // anywhere, and these are the two ways one could be replaced by
        // something that can. Split so this assertion does not match itself.
        let source = include_str!("prefs_ui.rs");
        for forbidden in [concat!("Hyper", "link"), concat!("open_", "url")] {
            assert!(
                !source.contains(forbidden),
                "{forbidden:?} is in this page: nothing here may navigate anywhere a \
                 release author chose"
            );
        }
    }

    /// An image is stripped to its alt text: rendering it would mean fetching
    /// it, and this page makes no request it was not asked for.
    #[test]
    fn an_image_is_reduced_to_its_alt_text() {
        let painted = painted_notes("Look: ![a screenshot](https://example.invalid/x.png) here.");

        assert!(painted.contains("a screenshot"), "got {painted:?}");
        assert!(
            !painted.contains("example.invalid"),
            "an image URL is a fetch this page must not invite: {painted:?}"
        );
    }

    /// **The sanitisation survives the parser.**
    ///
    /// A bidi override can make a painted line read backwards from its bytes,
    /// on the page whose job is saying what is about to be installed. The
    /// markdown work must not have moved the parse in front of the strip.
    #[test]
    fn the_invisible_characters_are_still_stripped_under_markdown() {
        let painted = painted_notes("- **safe\u{202e}txet**\u{200b} here");

        assert!(
            !painted.contains('\u{202e}') && !painted.contains('\u{200b}'),
            "an invisible formatting character reached the screen: {painted:?}"
        );
    }

    /// **Blank lines are space, not nothing.**
    ///
    /// Reported as "new lines seems like [gone]": a body whose sections ran
    /// together into a wall. Two paragraphs of the same characters must be
    /// taller than one, which is a claim about the gap rather than about any
    /// particular number of points.
    #[test]
    fn a_body_with_a_paragraph_break_paints_taller_than_one_without() {
        let flowing = paint_updates(
            crate::update_panel::UpdateStage::Available(crate::updater::ReleaseInfo {
                body: "alpha\nbeta".to_string(),
                ..a_release()
            }),
            Settings::default(),
        );
        let broken = paint_updates(
            crate::update_panel::UpdateStage::Available(crate::updater::ReleaseInfo {
                body: "alpha\n\nbeta".to_string(),
                ..a_release()
            }),
            Settings::default(),
        );

        let bottom = |p: &Painted| p.rect_of("beta").bottom();
        assert!(
            bottom(&broken) > bottom(&flowing) + 1.0,
            "the blank line between two sections was painted as nothing: with break {}, \
             without {}",
            bottom(&broken),
            bottom(&flowing)
        );
    }

    /// **A release body cannot push the buttons off the page.**
    ///
    /// The window is 1000x780 and not resizable, so a notes region that grew
    /// to fit its content would put the Download button below the bottom
    /// edge with no way to reach it -- a defect this crate has shipped
    /// before. The region is fixed and scrolls, which is checked here the
    /// only way it can be: by giving it a body far longer than the window and
    /// asserting that nothing the page paints leaves the window.
    #[test]
    fn a_release_body_longer_than_the_window_does_not_push_anything_off_it() {
        let enormous = (0..400)
            .map(|n| format!("line {n} of a release note nobody bounded"))
            .collect::<Vec<_>>()
            .join("\n");
        let painted = paint_updates(
            crate::update_panel::UpdateStage::Available(crate::updater::ReleaseInfo {
                body: enormous,
                ..a_release()
            }),
            Settings::default(),
        );

        let button = painted.rect_of(UPDATE_DOWNLOAD_BUTTON);
        assert!(
            button.max.y <= BODY_SIZE.y,
            "the download button was pushed to y={} on a {}-point page that cannot scroll or \
             resize",
            button.max.y,
            BODY_SIZE.y
        );
        assert!(
            painted.texts.iter().all(|(_, r)| r.min.y <= BODY_SIZE.y),
            "the page painted content below its own bottom edge"
        );
    }

    /// The progress bar and its byte count, which are the whole reason the
    /// download reports here rather than into a tray tooltip.
    #[test]
    fn a_download_in_flight_reports_its_progress_on_the_page() {
        let painted = paint_updates(
            crate::update_panel::UpdateStage::Downloading {
                release: a_release(),
                done: 3_355_443,
                total: Some(6_291_456),
            },
            Settings::default(),
        );

        assert!(painted.contains("Downloading version 9.9.9."), "got {:?}", painted.strings());
        assert!(painted.contains("3.2 MB of 6.0 MB"), "got {:?}", painted.strings());
        assert!(
            painted.contains(UPDATE_DOWNLOADING_BUTTON),
            "the button must stay put and say what is happening rather than vanishing and \
             reflowing the card under the cursor: {:?}",
            painted.strings()
        );
    }

    #[test]
    fn a_finished_download_offers_the_restart_rather_than_taking_it() {
        let painted = paint_updates(
            crate::update_panel::UpdateStage::Ready(a_release()),
            Settings::default(),
        );

        assert!(painted.contains(UPDATE_RESTART_BUTTON), "got {:?}", painted.strings());
        assert!(
            painted.strings().iter().any(|t| t.contains(UPDATE_READY_DESCRIPTION)),
            "the restart prompt must say what the restart will do: {:?}",
            painted.strings()
        );
    }

    /// A failure says why, on the page, with the way forward beside it. The
    /// old flow put this in a tray tooltip -- visible only to someone already
    /// resting a pointer on a 16px icon.
    #[test]
    fn a_failure_says_why_on_the_page_and_offers_the_retry() {
        let painted = paint_updates(
            crate::update_panel::UpdateStage::Failed {
                message: "failed to reach GitHub releases API".to_string(),
                release: Some(a_release()),
            },
            Settings::default(),
        );

        assert!(
            painted.contains("Update failed: failed to reach GitHub releases API"),
            "got {:?}",
            painted.strings()
        );
        assert!(painted.contains(UPDATE_RETRY_BUTTON));
    }

    /// **The manual button is not governed by the automatic setting, and the
    /// page says so where the button is.**
    ///
    /// `Settings::check_for_updates` is about Deskwarden contacting GitHub on
    /// its own; a click here is the user asking it to. Leaving that unsaid
    /// would make the button look like it was ignoring a preference. The same
    /// claim is in `PRIVACY.md`, which is the other half of this decision --
    /// a policy that described fewer requests than the software makes is a
    /// defect this repository has had to fix once already.
    #[test]
    fn with_automatic_checks_off_the_button_stays_and_the_page_explains_why() {
        let off = Settings { check_for_updates: false, ..Settings::default() };
        let painted = paint_updates(crate::update_panel::UpdateStage::UpToDate, off);

        assert!(
            painted.contains(UPDATE_CHECK_BUTTON),
            "the button must still be offered: an explicit click is the user initiating the \
             request. Painted: {:?}",
            painted.strings()
        );
        assert!(
            painted.contains(UPDATE_AUTOMATIC_OFF_NOTE),
            "with automatic checks off, the page must say why the button still works: {:?}",
            painted.strings()
        );
    }

    /// The note is about the button, so it appears only where the button is
    /// the thing needing explaining. Under a progress bar it would be
    /// explaining a decision the user has already made.
    #[test]
    fn the_automatic_checks_note_is_absent_when_it_would_explain_nothing() {
        let on = paint_updates(crate::update_panel::UpdateStage::UpToDate, Settings::default());
        assert!(!on.contains(UPDATE_AUTOMATIC_OFF_NOTE), "shown with the setting ON");

        let off = Settings { check_for_updates: false, ..Settings::default() };
        let downloading = paint_updates(
            crate::update_panel::UpdateStage::Downloading {
                release: a_release(),
                done: 1,
                total: Some(2),
            },
            off,
        );
        assert!(
            !downloading.contains(UPDATE_AUTOMATIC_OFF_NOTE),
            "shown under a download already in flight"
        );
    }

    /// A release with no notes is a real and ordinary state. An empty box
    /// would be indistinguishable from a box that failed to load.
    #[test]
    fn a_release_with_no_notes_says_so_rather_than_showing_an_empty_box() {
        let painted = paint_updates(
            crate::update_panel::UpdateStage::Available(crate::updater::ReleaseInfo {
                body: String::new(),
                ..a_release()
            }),
            Settings::default(),
        );

        assert!(painted.contains(UPDATE_NOTES_EMPTY), "got {:?}", painted.strings());
    }

    // -- the numeric control, as pure functions ----------------------------

    #[test]
    fn a_typed_entry_below_the_floor_commits_as_the_floor() {
        // Absolute values throughout: re-deriving these from
        // `clamp_auto_lock_minutes` would make the test pass for any floor,
        // including a broken one.
        assert_eq!(parse_minutes_entry("0", 15), 1);
        assert_eq!(parse_minutes_entry("1", 15), 1);
        assert_eq!(parse_minutes_entry("45", 15), 45);
        assert_eq!(parse_minutes_entry("  30  ", 15), 30);
    }

    #[test]
    fn a_typed_entry_that_is_not_a_number_leaves_the_value_alone() {
        assert_eq!(parse_minutes_entry("", 15), 15);
        assert_eq!(parse_minutes_entry("soon", 15), 15);
        assert_eq!(parse_minutes_entry("-5", 15), 15, "u64 cannot be negative");
        assert_eq!(parse_minutes_entry("7.5", 15), 15);
        assert_eq!(
            parse_minutes_entry("99999999999999999999999", 15),
            15,
            "too large for u64: the previous value stands rather than saturating \
             the user into a century-long timeout they did not ask for"
        );
        // ...and a previous value that was itself out of range is still
        // repaired on the way through.
        assert_eq!(parse_minutes_entry("", 0), 1);
    }

    #[test]
    fn the_steppers_arithmetic_stops_at_both_ends() {
        assert_eq!(decrement_minutes(15), 14);
        assert_eq!(decrement_minutes(2), 1);
        assert_eq!(decrement_minutes(1), 1, "the floor, not zero");
        assert_eq!(decrement_minutes(0), 1);
        assert_eq!(increment_minutes(1), 2);
        assert_eq!(increment_minutes(15), 16);
        assert_eq!(increment_minutes(u64::MAX), u64::MAX, "saturating, not panicking");
    }
}

/// Real frames of [`draw_prefs_modal`] -- the shell around the form, not the
/// form itself, which [`tests`] above already reads shape by shape.
///
/// What is worth pinning here is everything the shell is *for*: the card sits
/// inside the pane with the dimmed vault visible around it, the header's title
/// and dismiss control do not collide, the two dismiss routes work and the
/// third (a scrim click) deliberately does not -- and, above all, that a
/// control behind the scrim cannot be clicked. That last one is the defect
/// this whole feature exists to prevent: a modal that merely *covers* the
/// vault, with its buttons still live underneath, is worse than no modal.
#[cfg(test)]
mod modal_tests {
    use super::*;
    use eframe::egui::Color32;

    /// A vault-window-sized pane: larger than the card's ceiling on one axis
    /// and not the other, so the clamp and the margin are both exercised.
    const PANE: Vec2 = Vec2::new(1200.0, 820.0);

    /// The stand-in vault control, in the dead centre of the pane -- i.e.
    /// under the card, not merely under the scrim.
    const BEHIND: Rect = Rect {
        min: Pos2::new(560.0, 400.0),
        max: Pos2::new(680.0, 428.0),
    };

    /// A second stand-in, out in the margin the card does not cover. This one
    /// is the SCRIM's job and nothing else's: the card's own area cannot
    /// shield it, so a test that only ever clicked `BEHIND` would pass with no
    /// scrim at all. `the_card_alone_does_not_cover_the_margin` keeps that
    /// distinction honest.
    const BEHIND_IN_MARGIN: Rect = Rect {
        min: Pos2::new(2.0, 2.0),
        max: Pos2::new(20.0, 18.0),
    };

    /// A third stand-in, in the margin at the FAR corner of the pane.
    ///
    /// `BEHIND_IN_MARGIN` is at the top left, which every rectangle anchored at
    /// `Pos2::ZERO` covers -- including a scrim that allocated only the card's
    /// size instead of the screen's. That mutation passed every assertion in
    /// this module. This fixture is the other end: a scrim has to have
    /// allocated the whole pane to shield it.
    const BEHIND_IN_FAR_MARGIN: Rect = Rect {
        min: Pos2::new(PANE.x - 22.0, PANE.y - 20.0),
        max: Pos2::new(PANE.x - 4.0, PANE.y - 4.0),
    };

    // -----------------------------------------------------------------------
    // The pure geometry, asked directly. No frame, no fonts, no harness.
    // -----------------------------------------------------------------------

    #[test]
    fn the_card_is_inset_on_every_side_so_the_vault_stays_visible_around_it() {
        // A pane small enough that the ceiling does not bite: the card is
        // margin-bound on both axes.
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 500.0));
        let card = modal_card_rect(screen);
        assert_eq!(card.min.x - screen.min.x, MODAL_SCREEN_MARGIN);
        assert_eq!(screen.max.x - card.max.x, MODAL_SCREEN_MARGIN);
        assert_eq!(card.min.y - screen.min.y, MODAL_SCREEN_MARGIN);
        assert_eq!(screen.max.y - card.max.y, MODAL_SCREEN_MARGIN);
    }

    #[test]
    fn the_card_never_grows_past_the_designs_own_size_on_a_huge_window() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(3840.0, 2160.0));
        let card = modal_card_rect(screen);
        assert_eq!(card.width(), WINDOW_SIZE[0]);
        assert_eq!(card.height(), WINDOW_SIZE[1] - 40.0 + MODAL_HEADER_HEIGHT);
        // Still centred, which is the "do not make me hunt across a big
        // screen for the same window" half of the request.
        assert_eq!(card.center(), screen.center());
    }

    /// The vault window is resizable and its minimum is well under 3e's
    /// 1000x740. A card that kept that size on a small window would put its
    /// header -- and therefore its only mouse dismiss -- off the edge.
    #[test]
    fn the_card_never_spills_out_of_a_window_smaller_than_the_design() {
        for size in [Vec2::new(760.0, 520.0), Vec2::new(400.0, 300.0), Vec2::new(60.0, 40.0)] {
            let screen = Rect::from_min_size(Pos2::new(17.0, 23.0), size);
            let card = modal_card_rect(screen);
            assert!(
                screen.contains_rect(card),
                "a {size:?} window puts the card at {card:?}, outside the pane {screen:?}"
            );
        }
    }

    #[test]
    fn the_body_starts_below_the_header_and_never_overlaps_it() {
        let card = modal_card_rect(Rect::from_min_size(Pos2::ZERO, PANE));
        let body = modal_body_rect(card);
        let header = Rect::from_min_max(
            card.min,
            Pos2::new(card.max.x, card.min.y + MODAL_HEADER_HEIGHT),
        );
        assert!(card.contains_rect(body));
        assert_eq!(body.min.y, header.max.y);
        assert!(
            !body.intersects(Rect::from_min_max(
                header.min,
                Pos2::new(header.max.x, header.max.y - 0.01)
            )),
            "the form is drawn under the title bar it is supposed to sit beneath"
        );
    }

    // -----------------------------------------------------------------------
    // Frames
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct Shot {
        /// `(source, rendered, rect)` -- the second is not the first: see
        /// `detail.rs`'s `collect_rendered_text`. `Galley::text()` is the
        /// string that was HANDED to egui and is blind to truncation.
        texts: Vec<(String, String, Rect)>,
        fills: Vec<(Rect, Color32)>,
    }

    impl Shot {
        fn find(&self, source: &str) -> Option<&(String, String, Rect)> {
            self.texts.iter().find(|(s, _, _)| s == source)
        }

        fn sources(&self) -> Vec<&str> {
            self.texts.iter().map(|(s, _, _)| s.as_str()).collect()
        }

        fn rect_of(&self, source: &str) -> Rect {
            self.find(source)
                .unwrap_or_else(|| {
                    panic!("{source:?} was never painted; got {:?}", self.sources())
                })
                .2
        }
    }

    /// The `aae9429` contract, kept: a label counts as visible only if its
    /// rect is INSIDE the pane **and** the glyphs egui really laid are the
    /// glyphs it was handed. Either half alone passes a label that has been
    /// ellipsised to fit, or one drawn in full off the edge.
    fn assert_visible(shot: &Shot, source: &str, pane: Rect) {
        let (_, rendered, rect) = shot
            .find(source)
            .unwrap_or_else(|| panic!("{source:?} was never painted; got {:?}", shot.sources()));
        assert!(
            pane.contains_rect(*rect),
            "{source:?} is painted at {rect:?}, outside {pane:?}"
        );
        assert_eq!(
            rendered, source,
            "{source:?} was elided to fit -- egui laid {rendered:?}"
        );
    }

    fn walk(shape: &egui::Shape, out: &mut Shot) {
        match shape {
            egui::Shape::Text(text) => {
                let rendered: String = text
                    .galley
                    .rows
                    .iter()
                    .flat_map(|row| row.glyphs.iter().map(|glyph| glyph.chr))
                    .collect();
                out.texts.push((
                    text.galley.text().to_string(),
                    rendered,
                    Rect::from_min_size(text.pos, text.galley.size()),
                ));
            }
            egui::Shape::Rect(rect) => out.fills.push((rect.rect, rect.fill)),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            _ => {}
        }
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

    fn a_state() -> PrefsState {
        PrefsState::new(Settings::default())
    }

    /// One frame of the modal, drawn over a stand-in for the vault: a button
    /// in the middle of the pane, added BEFORE the modal exactly as the real
    /// window's panels are. Returns what was painted, what the modal asked
    /// for, and whether that button registered a click.
    fn frame(
        ctx: &egui::Context,
        state: &mut PrefsState,
        events: &[egui::Event],
        with_modal: bool,
    ) -> (Shot, PrefsAction, Behind) {
        let mut action = PrefsAction::None;
        let mut behind = Behind::default();
        let output = ctx.run_ui(raw_input(events), |ui| {
            behind.under_card = ui
                .put(BEHIND, egui::Button::new("a vault control"))
                .clicked();
            behind.in_margin = ui
                .put(BEHIND_IN_MARGIN, egui::Button::new("another"))
                .clicked();
            behind.in_far_margin = ui
                .put(BEHIND_IN_FAR_MARGIN, egui::Button::new("a third"))
                .clicked();
            if with_modal {
                action = draw_prefs_modal(ui.ctx(), state);
            }
        });
        let mut shot = Shot::default();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut shot);
        }
        (shot, action, behind)
    }

    /// **The modal, over a live vault control, reports no id clash.**
    ///
    /// The body's own guards (`no_id_diagnostic_on_any_preferences_page` and
    /// its nav walk) draw the pages alone. This is the arrangement the user
    /// actually gets: the window behind, the scrim, the card, and the nav
    /// inside it -- three surfaces in one pass, which is the composition an
    /// id clash between them would need in order to happen at all.
    ///
    /// A nav walk rather than a click sweep. A sweep across the whole pane
    /// costs tens of seconds here for no more coverage than this: the rows
    /// are what change the pane behind them, and a guard slow enough to be
    /// resented is a guard that gets deleted.
    #[test]
    fn no_id_diagnostic_while_the_modal_is_open_and_walked() {
        let mut hits: Vec<String> = Vec::new();
        let ctx = styled_context();
        let mut state = a_state();
        // Not [`frame`]: `Shot` flattens a pass into texts and fills, and
        // `id_clashes` has to walk the shapes as egui emitted them,
        // nested `Shape::Vec`s and all. The stand-in vault control is drawn
        // first, exactly as `frame` draws it and as the real window's panels
        // are, so the modal is measured over something rather than alone.
        let mut run = |events: &[egui::Event], at: String, hits: &mut Vec<String>| -> Shot {
            let output = ctx.run_ui(raw_input(events), |ui| {
                let _ = ui.put(BEHIND, egui::Button::new("a vault control"));
                let _ = draw_prefs_modal(ui.ctx(), &mut state);
            });
            let mut shot = Shot::default();
            for clipped in &output.shapes {
                super::tests::id_clashes(&clipped.shape, hits, &at);
                walk(&clipped.shape, &mut shot);
            }
            shot
        };
        let _ = run(&[], "warm-up".into(), &mut hits);
        let opened = run(&[], "open".into(), &mut hits);
        // The nav rows, located by name. Each section's label is painted
        // twice -- once in the nav and once as the page heading -- so the
        // leftmost of the two is the row, which is also what `nav_ink_of`
        // means by "in the nav" for the body's own harness.
        let rows: Vec<(String, Pos2)> = Section::ALL
            .iter()
            .map(|section| {
                let label = section.label().to_string();
                let mut found: Vec<Rect> = opened
                    .texts
                    .iter()
                    .filter(|(text, _, _)| *text == label)
                    .map(|(_, _, rect)| *rect)
                    .collect();
                found.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
                let row = *found
                    .first()
                    .unwrap_or_else(|| panic!("{label:?} was never painted in the modal"));
                (label, row.center())
            })
            .collect();
        for (label, pos) in rows {
            let _ = run(&click(pos), format!("clicking {label:?}"), &mut hits);
            let _ = run(&[], format!("settling after {label:?}"), &mut hits);
        }
        assert!(hits.is_empty(), "{hits:#?}");
    }

    /// Whether each stand-in vault control took a click on this frame.
    #[derive(Default)]
    struct Behind {
        under_card: bool,
        in_margin: bool,
        in_far_margin: bool,
    }

    fn click(pos: Pos2) -> Vec<egui::Event> {
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

    /// Where the header's dismiss mark is. It is drawn as two strokes rather
    /// than the character U+2715 (a tofu box in this app's face), so it cannot
    /// be found by name and its reserved space is computed instead -- the same
    /// arithmetic `draw_prefs_modal` uses.
    fn close_rect(card: Rect) -> Rect {
        Rect::from_center_size(
            Pos2::new(card.max.x - 22.0, card.min.y + MODAL_HEADER_HEIGHT / 2.0),
            Vec2::splat(16.0),
        )
    }

    // -----------------------------------------------------------------------
    // The warm-up, and why the card is not an anchored `Area`
    // -----------------------------------------------------------------------

    /// **The control this whole harness rests on.** An `Area` that has to
    /// CENTRE itself cannot place anything until it has measured its content,
    /// so its first frame emits nothing but `Shape::Noop`. A test that read
    /// frame 1 of such an area would be asserting about a blank screen, and
    /// every "does not contain" check in this module would pass for the wrong
    /// reason. Pinned here so the day it stops being true, this says so.
    #[test]
    fn an_anchored_area_paints_nothing_on_its_first_frame() {
        let ctx = styled_context();
        let output = ctx.run_ui(raw_input(&[]), |ui| {
            egui::Area::new(egui::Id::new("anchored-control"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ui.ctx(), |ui| {
                    ui.label("nothing here on frame one");
                });
        });
        let mut shot = Shot::default();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut shot);
        }
        assert!(
            shot.find("nothing here on frame one").is_none(),
            "an anchored area painted on its first frame after all -- every first-frame \
             assertion in this module needs revisiting"
        );
    }

    /// **And this modal is no exception, `fixed_pos` or not.** Measured, not
    /// assumed: an `Area` egui has never seen before paints nothing at all on
    /// the frame it is created, and asks for another. So EVERY frame test
    /// below runs a warm-up first -- and this one pins that the warm-up is
    /// really necessary, so that none of them is quietly asserting about a
    /// blank screen.
    ///
    /// The cost in the real window is one frame between the gear's click and
    /// the card appearing, which egui has already requested a repaint for.
    #[test]
    fn the_modal_is_blank_on_its_first_frame_and_complete_on_its_second() {
        let ctx = styled_context();
        let mut state = a_state();
        let (warm_up, _, _) = frame(&ctx, &mut state, &[], true);
        assert!(
            warm_up.find(MODAL_TITLE).is_none(),
            "the modal painted on its first frame after all -- the warm-up every frame test \
             below runs is no longer needed, and each of them should say so instead: {:?}",
            warm_up.sources()
        );

        let (shot, _, _) = frame(&ctx, &mut state, &[], true);
        let card = modal_card_rect(Rect::from_min_size(Pos2::ZERO, PANE));
        assert_visible(&shot, MODAL_TITLE, card);
        // And the form inside it, not just the shell.
        assert_visible(&shot, Section::General.label(), card);
        assert!(
            shot.find(AUTO_LOCK_LABEL).is_some(),
            "the first frame drew the shell but not the settings form; got {:?}",
            shot.sources()
        );
    }

    // -----------------------------------------------------------------------
    // The shell
    // -----------------------------------------------------------------------

    /// A title long enough to reach the dismiss mark would overprint the only
    /// mouse way out this modal has. Asserted as non-intersection, explicitly,
    /// because both are in the same 44px strip.
    #[test]
    fn the_title_does_not_reach_the_dismiss_control() {
        let ctx = styled_context();
        let mut state = a_state();
        let _ = frame(&ctx, &mut state, &[], true);
        let (shot, _, _) = frame(&ctx, &mut state, &[], true);
        let card = modal_card_rect(Rect::from_min_size(Pos2::ZERO, PANE));
        let title = shot.rect_of(MODAL_TITLE);
        let close = close_rect(card);
        assert!(
            !title.intersects(close),
            "the title {title:?} runs into the dismiss mark at {close:?}"
        );
        assert!(card.contains_rect(close), "the dismiss mark is outside the card");
    }

    /// The dim itself. Without it the vault behind reads as live, which is the
    /// visual half of the same claim the click tests make mechanically.
    #[test]
    fn the_whole_pane_is_dimmed_behind_the_card() {
        let ctx = styled_context();
        let mut state = a_state();
        // More than the two-frame warm-up the other tests need: egui fades a
        // new `Area` in over `Style::animation_time`, so an early frame's
        // scrim is a fraction of its final alpha. Read once it has settled --
        // this is an assertion about the colour that was chosen, not about the
        // fade, which is egui's and is fine.
        let mut shot = Shot::default();
        for _ in 0..24 {
            shot = frame(&ctx, &mut state, &[], true).0;
        }
        let pane = Rect::from_min_size(Pos2::ZERO, PANE);
        let scrim = shot
            .fills
            .iter()
            .find(|(rect, _)| *rect == pane)
            .unwrap_or_else(|| {
                panic!(
                    "no full-pane rectangle was painted at all; got {:?}",
                    shot.fills.iter().map(|(r, _)| *r).collect::<Vec<_>>()
                )
            })
            .1;
        assert_eq!(
            (scrim.r(), scrim.g(), scrim.b()),
            (0, 0, 0),
            "the scrim is not black, so it tints the vault rather than dimming it"
        );
        assert_eq!(
            scrim.a(),
            MODAL_SCRIM_ALPHA,
            "the scrim's alpha is not the one `folder_modal` and the launch confirmation use"
        );
        assert!(
            scrim.a() < 255,
            "the scrim is opaque, so the vault is hidden rather than dimmed -- the whole              point is that the window the user came from stays visible where it was"
        );
    }

    // -----------------------------------------------------------------------
    // Inertness -- the reason this feature exists
    // -----------------------------------------------------------------------

    /// **The control, first.** Without it every assertion below would pass
    /// against a fixture whose button was never clickable in the first place.
    #[test]
    fn the_control_behind_the_modal_is_clickable_when_the_modal_is_not_there() {
        let ctx = styled_context();
        let mut state = a_state();
        let _ = frame(&ctx, &mut state, &[], false);
        let (_, _, behind) = frame(&ctx, &mut state, &click(BEHIND.center()), false);
        assert!(
            behind.under_card,
            "the stand-in vault control under the card never registered a click at all"
        );
        let _ = frame(&ctx, &mut state, &[], false);
        let (_, _, behind) = frame(&ctx, &mut state, &click(BEHIND_IN_MARGIN.center()), false);
        assert!(
            behind.in_margin,
            "the stand-in vault control in the margin never registered a click at all"
        );
        let _ = frame(&ctx, &mut state, &[], false);
        let (_, _, behind) = frame(&ctx, &mut state, &click(BEHIND_IN_FAR_MARGIN.center()), false);
        assert!(
            behind.in_far_margin,
            "the stand-in vault control in the FAR margin never registered a click at all, so \
             the assertion that the scrim shields it proves nothing"
        );
    }

    /// **The other control, and the one that keeps the scrim from being dead
    /// code.** The card's own area covers `BEHIND`, so a click there would be
    /// blocked by the card whether or not a scrim existed. `BEHIND_IN_MARGIN`
    /// is deliberately outside it -- measured here rather than assumed.
    #[test]
    fn the_card_alone_does_not_cover_the_margin() {
        let card = modal_card_rect(Rect::from_min_size(Pos2::ZERO, PANE));
        assert!(card.contains_rect(BEHIND));
        assert!(
            !card.intersects(BEHIND_IN_MARGIN),
            "the margin fixture is under the card, so the scrim test below would pass              against no scrim at all"
        );
        assert!(
            !card.intersects(BEHIND_IN_FAR_MARGIN),
            "the far-margin fixture is under the card, so the scrim test below would pass \
             against no scrim at all"
        );
        // And the two margin fixtures are on opposite sides of the card, which
        // is the whole reason there are two: a scrim anchored at `Pos2::ZERO`
        // that under-allocates covers the near one and not the far one.
        assert!(
            BEHIND_IN_MARGIN.max.x < card.min.x && BEHIND_IN_MARGIN.max.y < card.min.y,
            "the near margin fixture is not before the card on both axes"
        );
        assert!(
            BEHIND_IN_FAR_MARGIN.min.x > card.max.x && BEHIND_IN_FAR_MARGIN.min.y > card.max.y,
            "the far margin fixture is not past the card on both axes, so it does not catch a \
             scrim that allocated too little"
        );
    }

    /// **The defect this feature exists to prevent.** A click that lands on a
    /// vault control behind a scrim is worse than no modal: the user believes
    /// they are editing preferences and is in fact driving the vault.
    #[test]
    fn a_click_over_the_card_never_reaches_the_vault_behind_it() {
        let ctx = styled_context();
        let mut state = a_state();
        let _ = frame(&ctx, &mut state, &[], true);
        let _ = frame(&ctx, &mut state, &[], true);
        let (_, _, behind) = frame(&ctx, &mut state, &click(BEHIND.center()), true);
        assert!(
            !behind.under_card,
            "a click over the preferences card reached the vault control underneath it"
        );
    }

    /// And the same in the margin: the scrim is a click-catcher over the whole
    /// pane, not only under the card.
    ///
    /// **This test used to assert only that a scrim click does not DISMISS.**
    /// It bound `(_, action, _)`, threw the `Behind` away, and never looked at
    /// the one property its own name promises -- so `BEHIND_IN_MARGIN`, set up
    /// for exactly this and kept honest by `the_card_alone_does_not_cover_the_
    /// margin`, was exercised by the positive control and by nothing else.
    /// Deleting the scrim's `allocate_response` let a margin click through to
    /// the vault with the entire shipped `prefs_ui::` suite green (41 passed).
    /// Both halves are asserted now, and the dismissal claim is kept alongside
    /// them rather than instead of them.
    ///
    /// **The margin is the load-bearing half.** A click over the card is
    /// blocked by the card's own area whether or not a scrim exists, so it is
    /// asserted here only as the near half of "no click anywhere reaches the
    /// vault"; `a_click_over_the_card_never_reaches_the_vault_behind_it` is
    /// that claim on its own.
    #[test]
    fn a_click_on_the_scrim_never_reaches_the_vault_behind_it() {
        let ctx = styled_context();
        let mut state = a_state();
        let card = modal_card_rect(Rect::from_min_size(Pos2::ZERO, PANE));
        // The stand-in control out in the margin, where the scrim alone covers.
        // Clicked at its centre rather than at some nearby point, so the click
        // is on the control and a failure cannot be a near miss.
        let corner = BEHIND_IN_MARGIN.center();
        assert!(
            !card.contains(corner),
            "the fixture point is under the card, so this would not be testing the scrim"
        );
        assert!(
            BEHIND_IN_MARGIN.contains(corner),
            "positive control: the click lands on the margin stand-in, not merely near it"
        );

        // Two warm-ups, as the card-click test takes: one for egui to create
        // the scrim's `Area`, one for it to have a laid-out rect to hit-test.
        let _ = frame(&ctx, &mut state, &[], true);
        let _ = frame(&ctx, &mut state, &[], true);
        let (_, action, behind) = frame(&ctx, &mut state, &click(corner), true);
        assert!(
            !behind.in_margin,
            "a click in the margin reached the vault control behind the scrim. The user \
             believes they are editing preferences and is in fact driving the vault -- which \
             is worse than having no modal at all, because it looks safe"
        );

        // The other end of the pane, past the card on both axes. A scrim that
        // allocated the CARD's size rather than the screen's still covers the
        // near corner, because both are anchored at `Pos2::ZERO`.
        let far = BEHIND_IN_FAR_MARGIN.center();
        assert!(
            !card.contains(far) && BEHIND_IN_FAR_MARGIN.contains(far),
            "positive control: the far click is outside the card and on its stand-in"
        );
        let _ = frame(&ctx, &mut state, &[], true);
        let (_, _, behind_far) = frame(&ctx, &mut state, &click(far), true);
        assert!(
            !behind_far.in_far_margin,
            "a click in the far margin reached the vault control behind the scrim, so the \
             scrim does not cover the whole pane -- only the part of it the card happens to \
             sit over"
        );

        assert_eq!(
            action,
            PrefsAction::None,
            "a scrim click dismissed the form -- neither `draw_folder_edit_modal` nor \
             `draw_launch_confirm_modal` does that, and this form commits as it is typed"
        );

        // The near half, in the same test and on its own frame: no click
        // anywhere over this pane reaches the vault.
        let _ = frame(&ctx, &mut state, &[], true);
        let (_, _, behind) = frame(&ctx, &mut state, &click(BEHIND.center()), true);
        assert!(
            !behind.under_card,
            "a click over the card reached the vault control underneath it"
        );
    }

    // -----------------------------------------------------------------------
    // Dismissal
    // -----------------------------------------------------------------------

    #[test]
    fn escape_closes_the_modal() {
        let ctx = styled_context();
        let mut state = a_state();
        let _ = frame(&ctx, &mut state, &[], true);
        let (_, action, _) = frame(
            &ctx,
            &mut state,
            &[egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
            true,
        );
        assert_eq!(action, PrefsAction::Close);
    }

    #[test]
    fn the_header_cross_closes_the_modal() {
        let ctx = styled_context();
        let mut state = a_state();
        let card = modal_card_rect(Rect::from_min_size(Pos2::ZERO, PANE));
        // TWO warm-ups: one for egui to create the area, one for it to lay
        // the dismiss mark out where a click can find it.
        let _ = frame(&ctx, &mut state, &[], true);
        let _ = frame(&ctx, &mut state, &[], true);
        let (_, action, _) = frame(&ctx, &mut state, &click(close_rect(card).center()), true);
        assert_eq!(
            action,
            PrefsAction::Close,
            "the dismiss mark did not close the modal, which leaves Esc as the only way out"
        );
    }

    /// An idle frame answers `None`. Trivially true today, and the thing that
    /// would break first if the dismiss mark's hit rect or the Esc check ever
    /// drifted onto something that fires every frame -- a modal that closes on
    /// its own is indistinguishable from a click that missed the gear.
    #[test]
    fn an_untouched_modal_stays_up() {
        let ctx = styled_context();
        let mut state = a_state();
        let _ = frame(&ctx, &mut state, &[], true);
        let (_, action, _) = frame(&ctx, &mut state, &[], true);
        assert_eq!(action, PrefsAction::None);
    }

    /// The form is live inside the modal -- a nav click changes section. The
    /// counterpart to the inertness tests above: the scrim must stop clicks
    /// reaching the vault and must NOT stop them reaching the card.
    #[test]
    fn the_form_inside_the_modal_is_live() {
        let ctx = styled_context();
        let mut state = a_state();
        let card = modal_card_rect(Rect::from_min_size(Pos2::ZERO, PANE));
        let body = modal_body_rect(card);
        // The second nav row, by the same arithmetic `draw_nav` lays out with.
        let second = Pos2::new(
            body.min.x + NAV_PAD_X + 40.0,
            body.min.y + NAV_PAD_Y + NAV_ITEM_HEIGHT + NAV_ITEM_GAP + NAV_ITEM_HEIGHT / 2.0,
        );
        let _ = frame(&ctx, &mut state, &[], true);
        let _ = frame(&ctx, &mut state, &[], true);
        assert_eq!(state.section, Section::General);
        let _ = frame(&ctx, &mut state, &click(second), true);
        assert_eq!(
            state.section,
            Section::ALL[1],
            "the nav row under the modal's own card did not take the click"
        );
    }

    // -----------------------------------------------------------------------
    // One form, two shells
    // -----------------------------------------------------------------------

    /// **The duplication guard.** Two places draw the preferences *shell*
    /// (`run`'s window and `draw_prefs_modal`'s card) and exactly one draws
    /// the form. A second copy of the body is how this project's recurring
    /// defects start: a control fixed in one and left broken in the other.
    #[test]
    fn exactly_one_place_in_this_program_draws_the_settings_form() {
        let source = include_str!("prefs_ui.rs");
        let body_calls = concat!("draw_prefs_", "body(");
        // The definition, `run`'s call, the modal's call, and `tests`' four
        // harnesses -- `frame`; `paint_general_at`, which is the same frame
        // on a pane of a chosen width; `body_shapes`, which is the same frame
        // again returned as raw shapes for the id-clash guards; and
        // `cursor_over_notes`, which is the same frame again read for its
        // cursor rather than its shapes, because a hit rectangle a test may
        // not click can only be witnessed by hovering it -- and `tall_frame`,
        // which is the same frame once more on a viewport tall enough that
        // the Local API page's scroll region culls nothing. All five are
        // tests; the production callers are still the two shells.
        assert_eq!(
            source.match_indices(body_calls).count(),
            8,
            "the number of `draw_prefs_body` sites changed; if a THIRD production caller \
             was added, confirm it is a shell and not a second form"
        );
        for forbidden in [concat!("fn draw_", "section("), concat!("fn draw_", "nav(")] {
            assert_eq!(
                source.match_indices(forbidden).count(),
                1,
                "{forbidden:?} is defined more than once"
            );
        }
    }

}

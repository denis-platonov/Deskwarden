//! **The account picker's decision: which item, then which field.**
//!
//! `CTRL+ALT+B` on an app with no configured binding offers a small card of
//! plausible accounts (`crate::app_candidates::Candidate`, Task 1) painted in
//! bare Win32 (`crate::win32_draw`, Tasks 2-3). This module is the *decision*
//! that sits between them: which candidate was picked, and once picked, which
//! field of it should be typed. No window is created here -- a later task
//! writes the Win32 half and calls [`run_with`].
//!
//! # The seam
//!
//! Mirrors `crate::unlock_prompt::run_with` exactly: `open`, then `protect`
//! **before** the first `next`, then a loop over `next`, with `close` on
//! every exit path including the failures. That ordering is security-relevant
//! there and stays security-relevant here for the same reason -- a window
//! that can be typed into (or, here, clicked into to pick a private account
//! name) before it is excluded from screen capture is one a recorder can
//! catch.
//!
//! # No secret ever rides on these types
//!
//! [`crate::app_candidates::Candidate`] already carries only an id, a name and
//! a username -- never a password. [`Outcome`] and [`Event`] keep that
//! property: `Outcome::Fill` carries the item's id and *which* field to type,
//! never the field's value. The value is fetched at dispatch, by the
//! component that already holds it, exactly as the module doc for
//! `Candidate` requires.

use std::sync::atomic::{AtomicBool, AtomicIsize};

use crate::app_candidates::Candidate;
use crate::key_sequence::FieldRef;

/// The window handle [`run_with`] deals in.
///
/// A bare `isize` newtype, not an `HWND`, for the same reason
/// `unlock_prompt::PromptWindow` is: a decision layer a test can drive must
/// not name a type that only exists behind a Win32 feature gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PickerWindow(pub isize);

/// Which field of the chosen item to type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Send {
    /// One field, by reference -- a username, a password, a TOTP code, or a
    /// custom field, exactly as offered by
    /// `crate::key_sequence::field_palette`.
    Field(FieldRef),
    /// Username, then Tab, then password. See [`tokens_for`] for why there is
    /// no trailing Enter.
    All,
    /// The item's own stored sequence, interpreted by
    /// `crate::key_sequence::parse` -- never a second reading of the string.
    Sequence,
}

/// How [`run_with`] finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A candidate and a field were chosen. `id` is the item's id, never a
    /// secret.
    Fill { id: String, send: Send },
    /// The user asked to create a new login for this app.
    NewLogin,
    /// The user asked to edit the chosen candidate's binding.
    Edit(String),
    /// The user declined. Nothing is armed.
    Cancelled,
    /// The window could not be put on screen at all.
    Unavailable,
}

/// What the user did with the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Cancel, Escape, or the close glyph.
    Cancel,
    /// The window went away underneath us. Treated exactly as `Cancel`.
    Closed,
    /// Picked the row at this index: a candidate of the slice `run_with` was
    /// given while the card is showing its list, or a **search result** while
    /// it is in search mode. `run_with` knows which, and it is the only thing
    /// that does.
    Chose(usize),
    /// Asked to search the vault: the *Search the vault* row under every
    /// populated card, and the *Search vault* row of the empty card (see
    /// [`empty_rows`]).
    ///
    /// **This no longer leaves the card.** It switches the same window into
    /// [`Outcome`]-less search mode -- a focused text box over the same list --
    /// rather than answering an outcome that opened the ~100 MB vault window
    /// to search a vault the daemon already holds in memory.
    Search,
    /// The search box's contents changed. Carries what is in it now, exactly
    /// as the control has it.
    ///
    /// **Never a secret**: this is a filter the user typed, over item names,
    /// on a card that shows names and usernames and nothing else.
    Typed(String),
    /// Asked to create a new login for this app.
    NewLogin,
    /// Asked to edit the previously chosen candidate's binding.
    EditSelected,
    /// Picked which field of the previously chosen candidate to type.
    Sends(Send),
}

/// The fields a candidate offers, and whether it has a stored sequence worth
/// offering as [`Send::Sequence`].
///
/// A named struct rather than a bare `(Vec<FieldRef>, bool)` so call sites
/// say what the `bool` means instead of `show_palette(window, &fields,
/// false)` and `|_| (vec![], false)` -- neither of which tells a reader
/// anything on its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    /// The fields offered, same shape as
    /// `crate::key_sequence::field_palette`.
    pub fields: Vec<FieldRef>,
    /// Whether the item has a stored sequence worth offering as
    /// [`Send::Sequence`].
    pub has_sequence: bool,
}

/// **What one keystroke in the search box found**, capped, and how many there
/// really were.
///
/// `offers` is at most the cap the searcher was given; `total` is the count
/// **before** the cap. The two are separate because a card that hides matches
/// without saying so is a defect this project treats as serious, and this card
/// neither scrolls nor resizes -- so the difference between them is the only
/// thing that can be said honestly about the rows that are not on screen. See
/// [`search_overflow_label`].
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SearchResults {
    /// The matches this card has room for, in the order they will be drawn.
    pub offers: Vec<Offer>,
    /// How many items matched in the whole vault, capped by nothing.
    pub total: usize,
}

/// **The seam the card's search mode reaches the vault through.**
///
/// A bare `fn` pointer, this crate's idiom for a seam a test must be able to
/// stand in for, and deliberately not a closure over a `VaultCache`: it is
/// called from [`run_with`], which is drivable with no window and no vault.
///
/// # Nothing that crosses this is a secret
///
/// It answers in [`Offer`]s -- a [`Candidate`] (id, name, username), a
/// [`Palette`] (presence of a field, never its value) and an icon -- which is
/// the same type the candidate list is built from, and which the module doc
/// already records as carrying no secret. The vault items themselves stay on
/// the far side: `crate::app`'s implementation takes its snapshot, filters it
/// through `crate::picker_ui::name_matches_filter` and drops the snapshot
/// before it returns, so no plaintext password is alive while the card is up.
///
/// # Arguments
///
/// The query as the user typed it (this function lowercases, the card does
/// not), and the cap -- how many matches the caller has room to draw. Capping
/// *inside* the searcher is what keeps a keystroke against a 1,666-item vault
/// from building 1,666 `Offer`s the card will throw all but five of away.
pub type Searcher = fn(query: &str, cap: usize) -> SearchResults;

/// The Win32 half, as `fn` pointers so [`run_with`] can be driven without a
/// desktop. Nothing here decides anything; every decision lives in
/// [`run_with`].
pub struct PickerCalls {
    /// Lays out and shows the card of candidates, for the app named by the
    /// second argument. `None` if it could not be put on screen.
    ///
    /// **The name is not decoration.** An empty candidate slice is the card's
    /// third mode -- see [`empty_rows`] -- whose heading is
    /// [`empty_text`]'s, and that heading names the app the user is in front
    /// of. It travels the seam rather than being read from a static so that
    /// a test driving these pointers sees exactly what the window would be
    /// told.
    pub open: fn(&[Candidate], &str) -> Option<PickerWindow>,
    /// `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on the top-level
    /// window, called before the first `next` -- see the module doc.
    pub protect: fn(PickerWindow) -> bool,
    /// Pumps until the user does something.
    pub next: fn(PickerWindow) -> Event,
    /// Shows the field palette for the chosen candidate.
    pub show_palette: fn(PickerWindow, &Palette),
    /// **Puts the card into search mode, or refreshes the rows it is already
    /// showing.**
    ///
    /// The first call swaps the card in place -- a focused text box above the
    /// same list, drawn by the same row painter -- and every later call is one
    /// keystroke's worth of new rows. One pointer for both, rather than an
    /// `enter_search` and a `set_results`, because the window does the same
    /// thing either way and two pointers would be two chances for the mode and
    /// its rows to disagree.
    pub show_search: fn(PickerWindow, &SearchResults),
    /// Destroys the window and releases its resources.
    pub close: fn(PickerWindow),
}

/// **The whole decision, and the only part of this module a test can run.**
///
/// `palette` maps a chosen candidate's id to the fields it offers and whether
/// it has a sequence worth offering -- the same shape
/// `crate::key_sequence::field_palette` produces, wrapped by the caller
/// because this layer works in ids and that one works in `VaultItem`s.
///
/// 1. `protect` runs immediately after `open` and before the first `next`.
/// 2. Choosing a row (`Event::Chose`) looks up that candidate's palette and
///    shows it; it does not by itself produce an `Outcome`.
/// 3. A field choice (`Event::Sends`) answers `Outcome::Fill` for the
///    most-recently-chosen candidate.
/// 4. `close` runs on every exit path, including `Unavailable`'s
///    predecessor -- there is no window to close there, which is exactly why
///    `open` returning `None` returns before ever calling it.
///
/// **An empty `candidates` slice is not a refusal.** It is the card's empty
/// mode -- design 3a's content, drawn by this card rather than by a second
/// window (see [`empty_rows`]) -- and it needs no arm of its own here: its
/// two rows answer [`Event::NewLogin`] and [`Event::Search`], which are the
/// same two events the populated card's *New login* button and its *Search the
/// vault* row answer, and they mean the same two things.
///
/// # The third mode: search, on this card
///
/// [`Event::Search`] no longer produces an outcome. It switches the same
/// window into search mode -- `search("", SEARCH_CAP)` for the first rows, then
/// one call per [`Event::Typed`] -- and the rows it draws are answered by the
/// same [`Event::Chose`] the candidate list is, leading to the same
/// `show_palette` step and the same [`Outcome::Fill`]. **One dispatch path**,
/// which is the whole reason the mode lives here rather than in a second
/// window: the alternative was `Outcome::SearchVault`, which opened the ~100 MB
/// egui vault window to search a vault the daemon already holds in memory, and
/// which never gave that memory back to the process.
///
/// Search mode is entered and not left. [`Event::Cancel`] closes the card from
/// it exactly as it does from every other mode -- see the module's `ESC`
/// discussion in `win32::next`, which answers `VK_ESCAPE` ahead of
/// `IsDialogMessageW` and has always meant "this card is done".
pub fn run_with(
    calls: &PickerCalls,
    candidates: &[Candidate],
    app_name: &str,
    palette: fn(&str) -> Palette,
    search: Searcher,
) -> Outcome {
    let Some(window) = (calls.open)(candidates, app_name) else {
        log::warn!("the account picker could not be put on screen");
        return Outcome::Unavailable;
    };

    // Before the first pump, so nothing in the card can be clicked while the
    // window is still capturable.
    if !(calls.protect)(window) {
        log::warn!(
            "SetWindowDisplayAffinity was refused for the account picker; its contents are \
             visible to screen capture on this machine"
        );
    }

    // **The chosen item's id, not its row index.** A row index means one thing
    // in the candidate list and another in the search results, and the card can
    // move between them; an id means the same thing in both, and it is the only
    // part of the choice `Outcome::Fill` and `Outcome::Edit` carry anyway.
    let mut chosen: Option<String> = None;
    // Set once, by `Event::Search`, and never cleared: search mode is entered
    // and not left. While it is set, `Event::Chose` indexes `results` rather
    // than `candidates`.
    let mut searching = false;
    let mut results = SearchResults::default();

    loop {
        match (calls.next)(window) {
            Event::Cancel | Event::Closed => {
                (calls.close)(window);
                return Outcome::Cancelled;
            }
            Event::Search => {
                if searching {
                    // The row that enters the mode is gone once it is entered,
                    // so this cannot honestly happen -- and re-entering would
                    // throw away whatever the user had typed.
                    continue;
                }
                searching = true;
                // The empty query, so the mode opens showing something rather
                // than an empty card the user has to type at to believe.
                results = search("", SEARCH_CAP);
                (calls.show_search)(window, &results);
            }
            Event::Typed(query) => {
                if !searching {
                    log::warn!(
                        "the account picker was told the search box changed while it was not in \
                         search mode; ignoring it"
                    );
                    continue;
                }
                results = search(&query, SEARCH_CAP);
                (calls.show_search)(window, &results);
            }
            Event::NewLogin => {
                (calls.close)(window);
                return Outcome::NewLogin;
            }
            Event::Chose(index) => {
                // The one place the two lists differ, and it is a lookup rather
                // than a branch on behaviour: from here down a chosen row is a
                // chosen row, whichever list it came out of.
                let picked: Option<(String, Palette)> = if searching {
                    results
                        .offers
                        .get(index)
                        .map(|offer| (offer.candidate.id.clone(), offer.palette.clone()))
                } else {
                    candidates.get(index).map(|c| (c.id.clone(), palette(&c.id)))
                };
                let Some((id, palette)) = picked else {
                    log::warn!(
                        "the account picker chose row {index} but only {len} rows were offered; \
                         the Win32 row list and the list behind it have disagreed, which would \
                         otherwise surface later as the picker typing the wrong account's \
                         password -- ignoring the choice",
                        len = if searching { results.offers.len() } else { candidates.len() }
                    );
                    continue;
                };
                chosen = Some(id);
                (calls.show_palette)(window, &palette);
            }
            Event::EditSelected => {
                if let Some(id) = chosen.clone() {
                    (calls.close)(window);
                    return Outcome::Edit(id);
                }
                log::warn!(
                    "the account picker got EditSelected with nothing chosen yet; ignoring it"
                );
            }
            Event::Sends(send) => {
                if let Some(id) = chosen.clone() {
                    (calls.close)(window);
                    return Outcome::Fill { id, send };
                }
                // **`describe_send`, not `{send:?}`.** A `Debug` of this value
                // prints `Field(Custom("Recovery PIN"))` -- a name out of the
                // user's own vault item, in a diagnostic that lands in a log
                // file on disk. See that function for why the crate's one
                // precedent for spelling such a name does not extend here.
                log::warn!(
                    "the account picker got a field choice ({}) with nothing chosen yet; \
                     ignoring it",
                    describe_send(&send)
                );
            }
        }
    }
}

/// What each choice types.
///
/// `All` is `{USERNAME}{TAB}{PASSWORD}` **with no trailing Enter** -- see the
/// test, which carries the reasoning: a trailing Enter submits, and if the
/// target's field order differs from this assumption it submits the wrong
/// content. Typing without submitting fails visibly; submitting fails
/// invisibly. `Sequence` goes through [`crate::key_sequence::parse`] rather
/// than a second reading of the string, so the picker and the sequence editor
/// can never disagree about what a sequence means.
pub fn tokens_for(send: &Send, sequence: Option<&str>) -> Vec<crate::key_sequence::Token> {
    use crate::key_sequence::Token;
    match send {
        Send::Field(field) => vec![Token::Field(field.clone())],
        Send::All => {
            // A half-sequence here is worse than none: username with no Tab
            // between it and password types the password straight into the
            // username box, in plaintext, in whatever app is focused. If
            // "TAB" is ever not a known key that is a bug in the key table,
            // not a reason to degrade -- refuse loudly instead.
            let tab = crate::key_sequence::key_named("TAB").expect("TAB is a known key");
            vec![
                Token::Field(FieldRef::Username),
                Token::Key(tab),
                Token::Field(FieldRef::Password),
            ]
        }
        Send::Sequence => sequence.map(crate::key_sequence::parse).unwrap_or_default(),
    }
}


// ---------------------------------------------------------------------------
// The window, and everything the caller must gather before there can be one.
// ---------------------------------------------------------------------------

/// The window's title.
///
/// **Unique across this process**, for the reason
/// [`crate::unlock_prompt::UNLOCK_PROMPT_TITLE`] is: `foreground::pick` finds
/// a window by title and takes the FIRST match in `EnumWindows` order, and
/// this card is provably alive alongside the tray icon's and the hotkey
/// listener's helper windows. `foreground`'s
/// `only_one_window_of_this_process_can_exist_at_a_time` asserts it differs
/// from every other title this crate opens under.
pub const PICKER_PROMPT_TITLE: &str = "Deskwarden account picker";

/// One row's worth of everything the card needs, gathered by the caller.
///
/// **The caller reads the icon off disk; the paint path never does.** The
/// favicon cache directory is `main`'s
/// (`project_dirs.cache_dir().join("icons")`), and threading it down into a
/// window procedure would mean either a second derivation of that path or a
/// static holding it. Handing the bytes in is also what keeps the promise the
/// paint path has to make: no file read, and above all no network fetch,
/// between a repaint and the pixels.
///
/// **Still no secret.** [`Candidate`] carries display strings and an id;
/// [`Palette`] is presence-only (`crate::key_sequence::field_palette` asks
/// whether a value is there, never what it is); and `icon` is a picture of a
/// website. `Outcome::Fill` still names a field rather than carrying its
/// value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub candidate: Candidate,
    /// What this candidate offers once chosen -- the second step's rows.
    pub palette: Palette,
    /// The cached favicon as PNG bytes, if the on-disk cache had one.
    /// `None` draws the row without an icon.
    pub icon: Option<Vec<u8>>,
}

/// What each [`Send`] is called on screen, and what it says it will type.
///
/// A function rather than literals at the paint site, so both steps of this
/// card and any later reader agree about what a choice is called. The field
/// names defer to [`crate::key_sequence::FieldRef::label`] rather than
/// restating it, exactly as `crate::app::FillChoice::label` does.
pub fn send_label(send: &Send) -> (String, &'static str) {
    match send {
        Send::All => ("Username + Tab + Password".to_string(), "Types both fields in order"),
        Send::Sequence => ("Saved sequence".to_string(), "Runs this item's own sequence"),
        Send::Field(field) => (
            field.label(),
            match field {
                FieldRef::Username => "Types the username",
                FieldRef::Password => "Types the password",
                FieldRef::Totp => "Types a one-time code",
                FieldRef::Custom(_) => "Types this custom field",
            },
        ),
    }
}

/// What a [`Send`] is called **in a log line**, which is not what it is called
/// on screen.
///
/// **It never spells a custom field's name**, and that is the whole reason it
/// exists. [`send_label`] returns `FieldRef::label`, which for
/// `FieldRef::Custom` is the user's own field name -- right on a button they
/// are looking at, wrong in a diagnostic written to a log file on disk and
/// read later by whoever is debugging. The crate's one existing precedent for
/// spelling the name (`crate::injector::sequence::Refusal::Unresolved`, "a
/// field called PIN") is a message shown *to that user, about their own item,
/// at the moment they asked for it*; a warning about an event the picker is
/// discarding is not that, so it does not follow it.
pub fn describe_send(send: &Send) -> &'static str {
    match send {
        Send::All => "username, Tab and password",
        Send::Sequence => "the item's own sequence",
        Send::Field(FieldRef::Username) => "the username",
        Send::Field(FieldRef::Password) => "the password",
        Send::Field(FieldRef::Totp) => "a one-time code",
        Send::Field(FieldRef::Custom(_)) => "a custom field",
    }
}

/// The rows the second step offers, in the order they are shown.
///
/// Pure, so the ordering and the bound below are testable without a window.
///
/// **Bounded by construction, and that is load-bearing.** This card does not
/// scroll and cannot be resized, so an entry past the last slot is one the
/// user can neither see nor reach. [`Palette::fields`] is
/// `crate::key_sequence::field_palette`'s answer, which is unbounded -- an
/// item may carry any number of custom fields -- so a card sized for it is a
/// card that cannot be sized. `crate::app::fill_choices` met the same wall and
/// answered it the same way, with the same reason: the sequence builder
/// already covers custom fields, and an unbounded row count is a geometry
/// hazard for a fixed-size surface. This is that decision again rather than a
/// second one.
pub fn palette_rows(palette: &Palette) -> Vec<Send> {
    // The item's own sequence and nothing else, for `fill_choices`' reason:
    // the user wrote it precisely because the generic rows were not what that
    // app wanted, so offering them back is offering the thing they rejected.
    if palette.has_sequence {
        return vec![Send::Sequence];
    }
    let has = |field: FieldRef| palette.fields.contains(&field);
    let mut out = Vec::new();
    if has(FieldRef::Username) && has(FieldRef::Password) {
        // First, because it is what the overwhelming majority of screens want;
        // the single-field rows below exist for the ones that do not.
        out.push(Send::All);
    }
    for field in [FieldRef::Username, FieldRef::Password, FieldRef::Totp] {
        if has(field.clone()) {
            out.push(Send::Field(field));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The empty card -- design 3a, on this window rather than on a second one.
// ---------------------------------------------------------------------------

/// One of the two things the card offers when **nothing** in the vault looks
/// like this app.
///
/// This is the most common state this card is opened in: most windows a user
/// presses `CTRL+ALT+B` in front of have no saved login at all. It used to be
/// a separate egui window of its own -- design 3a, in `overlay_ui` -- which
/// cost ~102 MB for a card that says one sentence and offers two buttons.
/// It is a *mode of this window* rather than a second window for the
/// reason the two steps above it are: a surface that leads to typing a
/// password must not be drawn by two renderers, because then it has two
/// palettes, two layouts and two chances to be got wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmptyAction {
    /// Create a login for this app. Leads to design 3c, the save-a-login
    /// form -- which is where 3a's own *New login* led.
    NewLogin,
    /// Open the vault window with this app's name in its search box. Answers
    /// [`Event::Overflow`], and so [`Outcome::SearchVault`], because that is
    /// already exactly what it means.
    SearchVault,
}

/// The rows the empty card offers, in the order it draws them.
///
/// **Rows rather than footer buttons**, and both of them: the footer has one
/// free slot beside *Cancel*, and 3a offered two actions. Putting them in the
/// list is also what lets them carry the one-line explanation each -- the same
/// name-over-description shape every other row on this card has -- instead of
/// being two bare words.
///
/// *New login* first, because it is the answer to the question the card just
/// asked ("there is nothing saved for this"); *Search vault* second, for the
/// user whose login is saved under a name this app's window does not say.
///
/// Bounded by construction, exactly as [`palette_rows`] is: two is two,
/// forever, and the card has room for [`ROW_CAP`]. That is not a formality --
/// this card does not scroll, so a third row added here without a taller card
/// would be one the user could neither see nor reach.
pub fn empty_rows() -> Vec<EmptyAction> {
    vec![EmptyAction::NewLogin, EmptyAction::SearchVault]
}

/// What the empty card's *New login* row says, and **the only card that may
/// NOT carry it**.
///
/// `crate::locked_card` does not carry it, deliberately: design 3c ends in
/// `VaultCache::create_item`, which is a write through `bw serve` against an
/// unlocked vault, so a *New login* button on the locked card would be an offer
/// the process cannot honour -- the same class of defect as the locked card's
/// own correction (a card claiming something about a vault it cannot read).
/// `locked_card::the_card_offers_only_the_unlock_it_can_honour` reads that
/// card's painted runs rather than trusting the argument.
///
/// A constant for the reason [`crate::locked_card::LOCKED_LABEL`] is one: it is
/// the string a test finds in the painted output rather than one it re-spells.
/// **It lived in `overlay_ui` until that module was deleted**; it is here now
/// because this is the card that offers it.
pub const NEW_LOGIN_LABEL: &str = "New login";

/// What the empty card's other row says, and **the only card that may NOT carry
/// it**.
///
/// `crate::locked_card` does not carry it either, for a plainer reason than the
/// one [`NEW_LOGIN_LABEL`] gives: while the vault is locked there is nothing to
/// search, and a vault window opened with a query in its box would show an
/// empty list that means "locked" and reads as "nothing found".
pub const SEARCH_VAULT_LABEL: &str = "Search vault";

/// What each empty-card row is called, and what it says it will do.
///
/// **The names are [`NEW_LOGIN_LABEL`] and [`SEARCH_VAULT_LABEL`] themselves**,
/// not copies of their words. Those two constants exist so that "3a offers
/// exactly these two" is one string a test can find rather than one it
/// re-spells. Re-typing them here would be the second spelling they were
/// written to prevent.
pub fn empty_label(action: EmptyAction) -> (&'static str, &'static str) {
    match action {
        EmptyAction::NewLogin => (NEW_LOGIN_LABEL, "Save a login for this app"),
        EmptyAction::SearchVault => (SEARCH_VAULT_LABEL, "Look for it under another name"),
    }
}

/// The empty card's heading pair: **the app's name, and why there is nothing
/// for it.**
///
/// Both lines are 3a's own, kept word for word rather than rewritten, and for
/// the reason 3a gave for the second one: matching is by process name and
/// window title, so an app whose window says something unexpected can be
/// unmatched while its login *is* saved. That fact is what makes *Search
/// vault* a sensible thing to offer rather than a shrug.
///
/// `app_name` is `crate::app::window_label`'s answer -- an executable name or
/// a window title, either of which the user (or the app they ran) chooses. It
/// is drawn into a fixed-width single-line run that clips, so no length of it
/// can change the card's geometry; see [`layout`], whose height is a constant.
pub fn empty_text(app_name: &str) -> (String, &'static str) {
    (
        format!("No saved login for {app_name}"),
        "Deskwarden matches windows by process name and title.",
    )
}

/// **Puts the card on screen and answers what the user did with it.**
///
/// The production [`REAL`] calls, [`run_with`]'s decision, and nothing else.
/// `run_with`'s `palette` argument is a bare `fn` pointer -- so it cannot
/// close over anything -- and what it reads is the [`Offer`] slice this
/// function parks for it, which is why the offers go in and come back out
/// around the call rather than being a parameter of the seam.
///
/// **An empty `offers` slice is a card, not a no-op.** It puts the empty mode
/// on screen -- design 3a's content, naming `app_name` -- and answers
/// [`Outcome::NewLogin`] or [`Outcome::Cancelled`], or, if the user takes its
/// *Search vault* row, whatever the search mode leads to.
///
/// `search` is the caller's [`Searcher`]: this module has no way to reach a
/// vault and deliberately keeps none.
pub fn ask(offers: &[Offer], app_name: &str, search: Searcher) -> Outcome {
    let candidates: Vec<Candidate> = offers.iter().map(|o| o.candidate.clone()).collect();
    if let Ok(mut slot) = OFFERS.lock() {
        *slot = offers.to_vec();
    }
    let outcome = run_with(&REAL, &candidates, app_name, palette_of, search);
    if let Ok(mut slot) = OFFERS.lock() {
        slot.clear();
    }
    outcome
}

/// The offers [`ask`] parked, read by [`palette_of`] and by the window.
static OFFERS: std::sync::Mutex<Vec<Offer>> = std::sync::Mutex::new(Vec::new());

/// [`run_with`]'s `palette` argument in production: the parked offer's own.
///
/// An empty palette for an id that is not there is not a silent nothing -- the
/// second step then shows no rows, which is visible on screen -- and the id
/// came out of the same slice one line earlier, so it cannot honestly happen.
fn palette_of(id: &str) -> Palette {
    OFFERS
        .lock()
        .ok()
        .and_then(|offers| offers.iter().find(|o| o.candidate.id == id).map(|o| o.palette.clone()))
        .unwrap_or(Palette { fields: Vec::new(), has_sequence: false })
}

/// The production [`PickerCalls`].
pub static REAL: PickerCalls = PickerCalls {
    open: win32::open,
    protect: win32::protect,
    next: win32::next,
    show_palette: win32::show_palette,
    show_search: win32::show_search,
    close: win32::close,
};

// ---------------------------------------------------------------------------
// Layout
//
// Logical pixels, at 100%, every one of them read off `theme` or off the
// surfaces this card sits beside. Numbers invented here would be a second
// layout that has to agree with a first, which is this codebase's standing
// defect shape -- `unlock_prompt::layout`'s header says the same, and this is
// that discipline rather than a copy of that window.
// ---------------------------------------------------------------------------

/// The card's width, and so the window's. Narrower than
/// `unlock_prompt::WIDTH`: that card holds a 470px form, this one holds a list
/// whose longest line is an item name.
pub const WIDTH: i32 = 380;

/// Content inset, and the top margin.
const MARGIN_X: i32 = 16;
const MARGIN_TOP: i32 = 16;

/// One row. Tall enough for a name over a username, and for a square icon
/// gutter beside them -- [`crate::win32_draw::draw_row`] takes the gutter to
/// be the row's own height, so this is also the icon column's width.
const ROW_H: i32 = 44;

/// **How many CANDIDATES the card has room for.**
///
/// The card does not scroll and cannot be resized, so this is not a viewport
/// onto a longer list -- it is the whole of what is reachable.
///
/// **This is the candidate cap, not the row cap.** The *Search the vault* row
/// -- the card's one route out of a wrong guess -- is drawn under every
/// populated card, and it is a row the card is [`LIST_ROWS`] tall *for*, not
/// one it takes from the candidates: spending a slot on it meant a list of
/// exactly `ROW_CAP` matches showed `ROW_CAP - 1` of them and reported a
/// truncation that had not happened.
pub const ROW_CAP: usize = 5;

/// **How many row slots the populated card lays out**: [`ROW_CAP`] candidates
/// plus the *Search the vault* row that always follows them.
///
/// Every rectangle, every control and every bound in this module is measured
/// against this rather than against [`ROW_CAP`], because this is the number of
/// rows that can be on screen at once. The card is one row taller than the
/// candidate cap and that is the whole of the difference.
pub const LIST_ROWS: usize = ROW_CAP + 1;

/// Button height. `theme::BUTTON_HEIGHT`.
const BUTTON_H: i32 = 32;

/// The search box's height. `theme::SEARCH_FIELD_HEIGHT` -- design 2b's search
/// box, which is deliberately shorter than a form field, and this is the same
/// control doing the same job on a smaller surface. Pinned against `theme` by
/// [`the_cards_dimensions_are_the_themes`], so a redesign there cannot leave
/// this card drawing a box of its own invented height.
const SEARCH_H: i32 = 34;

/// The gap above and below the search box, which is the gap the card already
/// has between its subtitle and its list.
const SEARCH_GAP: i32 = 10;

/// **How many search results the card has room for.**
///
/// The same [`ROW_CAP`] the candidate list uses, because it is the same list
/// area with the same number of controls behind it. The card is [`LIST_ROWS`]
/// slots tall, so the slot the *Search the vault* row held in the list is the
/// slot the overflow notice holds here -- see [`search_overflow_label`].
pub const SEARCH_CAP: usize = ROW_CAP;

/// The *New login* / *Edit binding* button's width: its label, and room for
/// the [`NEW_LOGIN_SHORTCUT`] chip beside it. See [`layout`].
const SECONDARY_W: i32 = 168;

/// The *Cancel* button's width: its label, and room for the [`ESC_SHORTCUT`]
/// chip beside it, the same relationship [`SECONDARY_W`] has to
/// [`NEW_LOGIN_SHORTCUT`]. Wider than the bare-label 84 px it used to be --
/// `ESC` is a shorter chip than `CTRL+ALT+N`, so it does not need as much of
/// an increase, but a chip drawn into a button never widened for it is a chip
/// drawn over its own label. At the bare-label 84 px there was 86 px of slack
/// between the footer pair and the card's left margin, so this 20 px went into
/// slack the card already had: with [`SECONDARY_W`] beside it the pair now
/// starts 66 px inside [`MARGIN_X`], and both buttons are still on screen --
/// see `nothing_the_card_lays_out_falls_off_the_bottom_of_it`, which pins that
/// against `MARGIN_X` rather than against the window's edge.
const CANCEL_W: i32 = 104;

/// One rectangle of the card, in logical pixels from the window's top left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Box2 {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Box2 {
    pub fn bottom(self) -> i32 {
        self.y + self.h
    }
    pub fn right(self) -> i32 {
        self.x + self.w
    }
}

/// Every rectangle the card paints, computed once.
///
/// Pure arithmetic with no Win32 in it, for `unlock_prompt::layout`'s reason:
/// a control whose bottom edge fell past the window's would simply be
/// invisible on a window that neither scrolls nor resizes, and that is a
/// property worth asserting without opening anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub window: Box2,
    /// The brand lockup's shield, and the wordmark beside it. **The card had
    /// no brand at all after the port**: the egui card it replaced carried
    /// `theme::card_header`'s shield and letterspaced DESKWARDEN, and a
    /// frameless always-on-top window that lists the accounts this user holds
    /// has to say whose window it is. The compact lockup, not the login
    /// window's -- see [`crate::win32_draw::draw_card_lockup`].
    pub mark: Box2,
    pub wordmark: Box2,
    pub title: Box2,
    pub subtitle: Box2,
    /// The search box, in search mode only. `None` in every other mode, rather
    /// than a zero-height rectangle: a `Box2` with `h: 0` is a control the card
    /// would still place, still hit-test and still paint a border around, and
    /// "there is no search box here" is exactly the thing that must not be
    /// expressible as a number.
    pub search: Option<Box2>,
    /// The whole list area. Individual rows are [`row_at`].
    pub list: Box2,
    /// The footer's left button: *New login* in the first step, *Edit binding*
    /// in the second.
    pub secondary: Box2,
    pub cancel: Box2,
    pub close_glyph: Box2,
}

/// The card's geometry, for a list `rows` rows tall.
///
/// **The two steps that share a live window are both laid out at
/// [`LIST_ROWS`], whichever of them is showing.** A window that shrank when the second step
/// had fewer rows than the first would move its own Cancel button out from
/// under the pointer at the moment the user is about to click it;
/// `unlock_prompt::layout` reserves its error row for exactly that reason, and
/// the same argument applies to a card whose two steps have different row
/// counts.
///
/// **That argument does not reach the empty card.** `MODE_EMPTY` is decided
/// once, in `open`, from an empty candidate slice, and never transitions to or
/// from anything -- so sizing it to [`empty_rows`]`.len()` moves no control
/// out from under a pointer that is already over it. Sizing it to `LIST_ROWS`
/// instead left a band of `theme::CARD` between its last offer and its Cancel
/// button, which reads as a list that lost its rows rather than as a card with
/// two.
pub fn layout(rows: usize) -> Layout {
    layout_for(rows, false)
}

/// [`layout`], told whether the card is in **search mode**.
///
/// The one difference is the search box between the subtitle and the list, and
/// the `SEARCH_H + SEARCH_GAP` it pushes everything below it down by. A second
/// layout function would be a second set of rectangles that has to agree with
/// the first; this is the same arithmetic with one box optionally in it, so
/// `nothing_the_card_lays_out_falls_off_the_bottom_of_it` can be run over both
/// shapes without either being a special case.
pub fn layout_for(rows: usize, search: bool) -> Layout {
    let content_w = WIDTH - 2 * MARGIN_X;

    let lockup = crate::win32_draw::card_lockup();
    let mark = Box2 { x: MARGIN_X, y: MARGIN_TOP, w: lockup.mark_w, h: lockup.mark_h };
    let wordmark =
        Box2 { x: mark.right() + lockup.gap, y: MARGIN_TOP, w: lockup.word_w, h: lockup.mark_h };

    let title =
        Box2 { x: MARGIN_X, y: mark.bottom() + lockup.gap_below, w: content_w, h: 21 };
    let subtitle = Box2 { x: MARGIN_X, y: title.bottom() + 1, w: content_w, h: 17 };
    let search = search.then(|| Box2 {
        x: MARGIN_X,
        y: subtitle.bottom() + SEARCH_GAP,
        w: content_w,
        h: SEARCH_H,
    });
    let list_y = match search {
        Some(box2) => box2.bottom() + SEARCH_GAP,
        None => subtitle.bottom() + SEARCH_GAP,
    };
    let list = Box2 { x: MARGIN_X, y: list_y, w: content_w, h: ROW_H * rows as i32 };

    // Right-aligned, Cancel outermost: the choice that does nothing sits where
    // the eye leaves the card.
    let cancel =
        Box2 { x: MARGIN_X + content_w - CANCEL_W, y: list.bottom() + 12, w: CANCEL_W, h: BUTTON_H };
    // Wider than Cancel because it carries its shortcut inside itself, the way
    // `theme::toolbar_button_with_shortcut` does: the label, then the chip,
    // in one pill that is clickable over the whole of it.
    let secondary =
        Box2 { x: cancel.x - 10 - SECONDARY_W, y: cancel.y, w: SECONDARY_W, h: BUTTON_H };

    let window = Box2 { x: 0, y: 0, w: WIDTH, h: cancel.bottom() + MARGIN_TOP };
    // The ✕ moves up onto the lockup's line, which is where every card header
    // in the design carries it -- and where it has to be now that the title is
    // no longer the top line.
    let close_glyph = Box2 { x: WIDTH - MARGIN_X - 20, y: MARGIN_TOP - 2, w: 20, h: 20 };

    Layout { window, mark, wordmark, title, subtitle, search, list, secondary, cancel, close_glyph }
}

/// One row of the **populated** card, in the order they are drawn.
///
/// The last row is always [`ListRow::SearchVault`]: the matcher that produced
/// the candidates is deliberately loose, so a card whose two offers are both
/// wrong is an ordinary state -- and dismissing the card was, for a while, the
/// only way out of it. See [`populated_rows`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListRow {
    /// The `usize`th candidate of the slice `open` was given.
    Candidate(usize),
    /// The route into the vault's own search. `truncated` says whether
    /// candidates were dropped to make room for it, which is the only thing
    /// that changes about the row.
    SearchVault { truncated: bool },
}

/// The populated card's rows, for a candidate list `candidates` long.
///
/// **The *Search the vault* row is not the overflow notice.** It is the card's
/// one route out of a wrong guess, so it is drawn whether or not anything
/// overflowed; when candidates did have to be dropped, the same row carries
/// that news in its second line rather than a second row doing it. It sits in
/// [`LIST_ROWS`]' own extra slot, so it costs no candidate its place. See
/// [`search_row_label`].
pub fn populated_rows(candidates: usize) -> Vec<ListRow> {
    let (shown, truncated) = crate::win32_draw::visible_rows(candidates, ROW_CAP);
    let mut rows: Vec<ListRow> = (0..shown).map(ListRow::Candidate).collect();
    rows.push(ListRow::SearchVault { truncated });
    rows
}

/// What the *Search the vault* row says, on its two lines.
///
/// The second line is where the truncation is told, because the row itself is
/// now always there. The non-truncated wording is the empty card's own line
/// for the same action -- see [`empty_label`] -- so one action does not have
/// two voices.
pub fn search_row_label(truncated: bool) -> (&'static str, &'static str) {
    (
        "Search the vault",
        if truncated {
            "More accounts match than fit on this card"
        } else {
            "Look for it under another name"
        },
    )
}

// ---------------------------------------------------------------------------
// Search mode -- the same card, a focused text box, and the same rows.
//
// Asked for by the app's owner: "search should open the same text box and
// focus on it where user can find something, not separate window, results
// shown in the same window". The row that used to answer `Outcome::SearchVault`
// -- and so open the ~100 MB egui vault window to search a vault the daemon
// already holds in memory -- now switches this card into the mode below.
// ---------------------------------------------------------------------------

/// One row of the card in **search mode**, in the order they are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchRow {
    /// The `usize`th of [`SearchResults::offers`].
    Result(usize),
    /// **The cap, said out loud.** Drawn only when the vault held more matches
    /// than the card has room for. `shown` is what is on screen above it,
    /// `total` is what matched.
    Overflow { shown: usize, total: usize },
    /// Nothing matched what was typed. A row rather than an empty list,
    /// because an empty list on a card that does not scroll is
    /// indistinguishable from a card that has stopped working.
    Nothing,
}

/// The rows search mode draws for `shown` results out of `total` matches.
///
/// **Bounded by construction at [`LIST_ROWS`]**, exactly as [`populated_rows`]
/// and [`palette_rows`] are, and for the same reason: this card neither scrolls
/// nor resizes, so a row past the last slot is one the user can neither see nor
/// reach. `shown` is [`SEARCH_CAP`] at most and the notice takes the one extra
/// slot the card is [`LIST_ROWS`] tall for.
pub fn search_rows(shown: usize, total: usize) -> Vec<SearchRow> {
    let shown = shown.min(SEARCH_CAP);
    if shown == 0 {
        return vec![SearchRow::Nothing];
    }
    let mut rows: Vec<SearchRow> = (0..shown).map(SearchRow::Result).collect();
    if total > shown {
        rows.push(SearchRow::Overflow { shown, total });
    }
    rows
}

/// What a non-result row of search mode says, on its two lines.
///
/// **The overflow row names the number it is hiding.** A cap that drops matches
/// without saying so is the defect this project keeps finding, and the *Search
/// the vault* row's own truncation line -- see [`search_row_label`] -- is the
/// precedent this follows rather than invents: the card says how many matched,
/// how many it can show, and what the user can do about it.
///
/// `SearchRow::Result` has no text of its own: it is an account, drawn from its
/// [`Candidate`] by the same row painter the candidate list uses.
pub fn search_row_text(row: SearchRow) -> (String, String) {
    match row {
        SearchRow::Result(_) => (String::new(), String::new()),
        SearchRow::Overflow { shown, total } => (
            format!("{total} accounts match"),
            format!("Showing the first {shown} -- keep typing to narrow it"),
        ),
        SearchRow::Nothing => (
            "No matches".to_string(),
            "Nothing in your vault is named like that".to_string(),
        ),
    }
}

/// Search mode's heading pair.
///
/// A function rather than literals at the paint site, for [`empty_text`]'s
/// reason: the words the card shows are then the words a test can read.
pub fn search_text() -> (&'static str, &'static str) {
    (
        "Search the vault",
        "Type to filter your accounts. Nothing is typed until you pick one.",
    )
}

// ---------------------------------------------------------------------------
// The keyboard shortcuts, and the words that make them findable.
//
// Asked for by the app's owner after using the card: "add shortcuts like
// Ctrl + Alt + 1 (2,3,4 for items in the list), New as well, Cancel (Esc)".
// Escape already cancelled -- `win32::next` handles it ahead of
// `IsDialogMessageW`, which only cancels for a real dialog box -- so nothing
// here re-implements it.
// ---------------------------------------------------------------------------

/// **The chord that runs the *New login* offer.**
///
/// `CTRL+ALT+N` rather than a bare `N`: every control on this card is a
/// `BUTTON`, and a bare letter is a mnemonic Windows may already be routing.
/// `CTRL+ALT` is also the modifier pair the card's own hotkey (`CTRL+ALT+B`)
/// is on, so the whole card answers to one chord family. It collides with
/// nothing this crate registers -- `crate::hotkey` registers `CTRL+ALT+B`
/// alone -- and it is read from this window's own pump while the card has
/// focus, so no other application sees it.
pub const NEW_LOGIN_SHORTCUT: &str = "CTRL+ALT+N";

/// **The chord that cancels the card.**
///
/// Just `ESC`, not `CTRL+ALT+ESC`: this one was never a chord, it is the key
/// Escape has always been -- `win32::next` answers `VK_ESCAPE` with
/// `Event::Cancel` ahead of `IsDialogMessageW`, and has done so since before
/// any of the other shortcuts existed. The chip only advertises it; it does
/// not change what fires.
pub const ESC_SHORTCUT: &str = "ESC";

/// The chord shown on -- and accepted by -- the `index`th **candidate** row.
///
/// `None` past the candidate cap, and that is the point: the row after the
/// candidates is *Search the vault*, which means something else entirely, and
/// a digit that landed on it would be a trap. Numbering is what the user sees:
/// `1` is the topmost row as drawn.
pub fn row_shortcut(index: usize) -> Option<String> {
    (index < ROW_CAP).then(|| format!("CTRL+ALT+{}", index + 1))
}

/// **Which candidate a digit chooses**, given how many candidate rows are
/// actually on screen.
///
/// The one place the rule lives, read by the window's key handling and by the
/// tests, so there is no second answer: a digit past the shown rows chooses
/// nothing at all -- it does not beep, close the card, or fall through to the
/// *Search the vault* row -- and no digit can ever reach that row, because the
/// count this is measured against is the candidates' and not the rows'.
pub fn candidate_for_digit(digit: u32, shown: usize) -> Option<usize> {
    if !(1..=9).contains(&digit) {
        return None;
    }
    let index = digit as usize - 1;
    (index < shown.min(ROW_CAP)).then_some(index)
}

/// The `index`th row's rectangle, in logical pixels, on a card laid out for
/// `rows` rows -- the same count [`layout`] was given, so the row and the list
/// it sits in can never be measured against two different cards.
pub fn row_at(rows: usize, index: usize) -> Box2 {
    row_at_for(rows, false, index)
}

/// [`row_at`], told whether the card is in search mode -- the same flag
/// [`layout_for`] takes, and read off the same `Layout`, so a row and the list
/// it sits in can never be measured against two different cards.
pub fn row_at_for(rows: usize, search: bool, index: usize) -> Box2 {
    let list = layout_for(rows, search).list;
    Box2 { x: list.x, y: list.y + ROW_H * index as i32, w: list.w, h: ROW_H }
}

// ---------------------------------------------------------------------------
// The Win32 half. No decisions live below this line.
// ---------------------------------------------------------------------------

/// Which step the card is showing.
///
/// A static because a window procedure is an `extern "system"` function with
/// nowhere to keep state -- the same reason `unlock_prompt`'s `PENDING` is one.
static MODE: AtomicIsize = AtomicIsize::new(MODE_LIST);
const MODE_LIST: isize = 0;
const MODE_PALETTE: isize = 1;
/// Nothing in the vault looks like this app: design 3a's content, on this
/// card. See [`empty_rows`].
const MODE_EMPTY: isize = 2;
/// **The card's search mode**: a focused text box over the same list, filtering
/// the whole vault through the [`Searcher`] seam. Entered from the *Search the
/// vault* row of either of the other two modes, and not left -- see
/// [`run_with`].
const MODE_SEARCH: isize = 3;

/// The app the card is about, as [`empty_text`] needs it.
///
/// A static for `MODE`'s reason -- a window procedure has nowhere to keep
/// state -- and set by `open` from the name that came in over
/// [`PickerCalls::open`], never read from anywhere else.
static APP_NAME: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// Set by `WM_DESTROY`, so a window that goes away underneath the pump is
/// reported as [`Event::Closed`] rather than pumped forever.
static GONE: AtomicBool = AtomicBool::new(false);

/// What the window procedure last recorded. **Taken** by `next`, never merely
/// read -- see that function for why an event that could be delivered twice
/// would turn `run_with`'s ignore-and-continue arms into a spin.
static PENDING: std::sync::Mutex<Option<Event>> = std::sync::Mutex::new(None);

/// The candidates the first step is showing, and whether candidates had to be
/// dropped to fit them.
///
/// The *Search the vault* row below them is drawn either way -- see
/// [`populated_rows`] -- so `TRUNCATED` decides only what its second line
/// says, not whether it exists.
static SHOWN: std::sync::Mutex<Vec<Candidate>> = std::sync::Mutex::new(Vec::new());
static TRUNCATED: AtomicBool = AtomicBool::new(false);

/// The second step's rows. Empty while the first step is showing.
static ENTRIES: std::sync::Mutex<Vec<Send>> = std::sync::Mutex::new(Vec::new());

/// **Search mode's rows, as display strings and nothing else.**
///
/// [`Candidate`]s -- id, name, username -- exactly like [`SHOWN`], and for the
/// same reason: the window has to paint them, and a window that could paint a
/// secret is a window that can leak one. The [`Palette`] each result carries
/// stays with [`run_with`], which is what needs it; the card never sees it.
static SEARCH_SHOWN: std::sync::Mutex<Vec<Candidate>> = std::sync::Mutex::new(Vec::new());

/// How many items matched the current query in the whole vault, before
/// [`SEARCH_CAP`]. Read only by the overflow row, which is the one place the
/// difference between this and `SEARCH_SHOWN.len()` is told.
static SEARCH_TOTAL: AtomicIsize = AtomicIsize::new(0);

/// The Win32 calls, and **nothing else**.
///
/// # Why every pixel here is painted by hand
///
/// `crate::unlock_prompt`'s `win32` module carries the whole argument and it
/// is not restated: a themed control renders in the shell's grey with the
/// shell's font, and the last raw-Win32 surface in this project was deleted
/// for looking foreign rather than for being broken. The rows and the footer
/// buttons here are real `BUTTON` windows -- which is what buys focus, the
/// space bar, and `IsDialogMessage` traversal -- with their painting taken
/// over completely and handed to [`crate::win32_draw`], the module both this
/// card and that prompt draw through so neither can drift from the palette.
///
/// # GDI only
///
/// Nothing here creates a Direct2D or Direct3D device. That is measured rather
/// than stylistic: the daemon/UI split put an egui window at ~102 MB and a D2D
/// device at 53.85 MB against the Win32 prompt's 1.79 MB, and a card that cost
/// either would have no reason to exist.
///
/// # GDI object hygiene
///
/// Every brush, pen, font, DC and DIB created below is restored and deleted
/// before its function returns. This is a daemon's repaint path -- one card per
/// hotkey press for as long as the machine is up -- and a leaked handle here
/// exhausts the table over a session rather than over a run.
///
/// Nothing in this module decides anything. See [`run_with`].
mod win32 {
    use super::{
        Box2, Candidate, EmptyAction, Event, Palette, PickerWindow, SearchResults, SearchRow,
        APP_NAME, ENTRIES, ESC_SHORTCUT, GONE, LIST_ROWS, MODE, MODE_EMPTY, MODE_LIST,
        MODE_PALETTE, MODE_SEARCH, NEW_LOGIN_SHORTCUT, PENDING, PICKER_PROMPT_TITLE, ROW_CAP,
        SEARCH_CAP, SEARCH_SHOWN, SEARCH_TOTAL, SHOWN, TRUNCATED,
    };
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicI32, AtomicIsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        AddFontMemResourceEx, AlphaBlend, BeginPaint, BitBlt, CreateCompatibleBitmap,
        CreateCompatibleDC, CreateDIBSection, CreateFontIndirectW, CreatePen, CreateSolidBrush,
        DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, GetDC, GetDeviceCaps,
        InvalidateRect, ReleaseDC, RoundRect, SelectObject, SetBkMode, SetTextColor, AC_SRC_ALPHA,
        AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, CLEARTYPE_QUALITY,
        DIB_RGB_COLORS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FW_BOLD, FW_NORMAL,
        HBITMAP, HBRUSH, HDC, HFONT, LOGFONTW, LOGPIXELSX, PAINTSTRUCT, PS_SOLID, SRCCOPY,
        SetBkColor, TRANSPARENT,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GetClientRect, GetDlgItem, GetWindowLongPtrW, GetWindowTextW, IsDialogMessageW,
        LoadCursorW, MoveWindow, PeekMessageW, RegisterClassW, SendMessageW, SetForegroundWindow,
        SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage,
        BN_CLICKED, BS_PUSHBUTTON, CS_HREDRAW, CS_VREDRAW, ES_AUTOHSCROLL, GWLP_WNDPROC, HMENU,
        HTCAPTION, IDC_ARROW, MSG, PM_REMOVE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SW_HIDE,
        SW_SHOW, WDA_EXCLUDEFROMCAPTURE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_COMMAND,
        WM_CTLCOLOREDIT, WM_DESTROY, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_NCHITTEST,
        WM_PAINT, WM_QUIT, WM_SETFONT, WNDCLASSW, WS_CHILD, WS_EX_TOPMOST, WS_POPUP, WS_TABSTOP,
        WS_VISIBLE,
    };

    /// `EN_CHANGE`, the `EDIT` control's "your text changed" notification.
    ///
    /// Named here rather than left as a bare hex literal at the call, exactly
    /// as `unlock_prompt` names `EM_SETSEL`: the `windows` crate does not
    /// project the `EDIT` control's notification codes under the features this
    /// crate enables, and enabling more of them re-pins `job_object.rs`'s
    /// whole-file hash of `Cargo.toml`.
    const EN_CHANGE: u32 = 0x0300;

    use crate::win32_draw::{
        draw_button_with_shortcut, draw_row, rgb, ButtonSkin, RowState,
    };

    /// Row `i` is control `ID_ROW + i`; the footer's two ids sit below them
    /// all, so a row id can never collide with a button id however many rows
    /// there are.
    const ID_ROW: usize = 200;
    const ID_SECONDARY: usize = 101;
    const ID_CANCEL: usize = 102;
    /// The search box. Below `ID_ROW` with the footer's two, so the `id <
    /// ID_ROW` test in [`clicked`] keeps meaning "not a row".
    const ID_SEARCH: usize = 103;

    const CLASS_NAME: PCWSTR = w!("DeskwardenAccountPicker");

    /// The window's DPI as a percentage of 96, sampled once per open.
    ///
    /// **The SYSTEM DPI, not the monitor's**, and a known limitation rather
    /// than an oversight -- `unlock_prompt`'s own `DPI_PERCENT` carries the
    /// whole argument: `GetDpiForWindow` lives behind a `windows` crate
    /// feature this crate does not enable, and enabling it re-pins
    /// `job_object.rs`'s whole-file hash of `Cargo.toml`.
    static DPI_PERCENT: AtomicI32 = AtomicI32::new(100);

    fn scale(v: i32) -> i32 {
        v * DPI_PERCENT.load(Ordering::SeqCst) / 100
    }

    /// The icon's drawn size inside the row's square gutter.
    const ICON_SIDE: i32 = 24;

    // ---- fonts -------------------------------------------------------------

    /// Registers the bundled Archivo cuts privately with GDI, once.
    ///
    /// `AddFontMemResourceEx` makes a face available to **this process only**
    /// -- nothing is installed and nothing touches the user's font list -- and
    /// the handles are deliberately never released, because freeing one while
    /// a window still has it selected is how a surface repaints in the
    /// fallback face.
    fn register_fonts() {
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| unsafe {
            for (_, _, _, bytes) in crate::theme::ARCHIVO_FACES {
                // A `Cell` rather than a `mut` local: GDI writes the count back
                // through a `*const u32`, so a plain immutable binding read
                // afterwards is a value the compiler may fold to its
                // initialiser.
                let installed = std::cell::Cell::new(0u32);
                let handle = AddFontMemResourceEx(
                    bytes.as_ptr() as *const c_void,
                    bytes.len() as u32,
                    None,
                    installed.as_ptr(),
                );
                if handle.0.is_null() || installed.get() == 0 {
                    // Cosmetic degradation, never a reason to refuse to offer
                    // the accounts. GDI falls back to the shell font.
                    log::warn!("could not register a bundled Archivo face with GDI");
                }
            }
        });
    }

    /// An `HFONT` for one of the app's faces at one logical size. The GDI
    /// family and weight come from `crate::theme::gdi_face_for`, which reads
    /// them out of the files' own `name` records rather than guessing.
    fn font(family: &str, px: i32) -> HFONT {
        let (face, weight) = crate::theme::gdi_face_for(family);
        unsafe {
            let mut lf = LOGFONTW {
                lfHeight: -scale(px),
                lfWeight: if weight >= 700 { FW_BOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
                // ClearType, explicitly: the default quality on a memory DC is
                // not it, and greyscale-antialiased Archivo beside the app's
                // ClearType egui text is exactly the "almost right" that reads
                // as a different program.
                lfQuality: CLEARTYPE_QUALITY,
                ..Default::default()
            };
            for (i, ch) in face.encode_utf16().take(31).enumerate() {
                lf.lfFaceName[i] = ch;
            }
            CreateFontIndirectW(&lf)
        }
    }

    /// The keyboard chips' face, asked of the OS by the name
    /// `crate::theme::GDI_MONO_FACE` gives it -- the same file
    /// `theme::system_monospace` hands egui, so a chip on this card and a chip
    /// in an egui window are the same typeface.
    fn mono(px: i32) -> HFONT {
        unsafe {
            let mut lf = LOGFONTW {
                lfHeight: -scale(px),
                lfWeight: FW_NORMAL.0 as i32,
                lfQuality: CLEARTYPE_QUALITY,
                ..Default::default()
            };
            for (i, ch) in crate::theme::GDI_MONO_FACE.encode_utf16().take(31).enumerate() {
                lf.lfFaceName[i] = ch;
            }
            CreateFontIndirectW(&lf)
        }
    }

    /// Every face the card paints with, created at open and destroyed at
    /// close. Kept together so `close` cannot leak one by forgetting it.
    struct Fonts {
        /// The lockup's wordmark: `theme::CARD_HEADER_WORD_PX` in the bold
        /// cut, which is what `theme::card_header` letterspaces "DESKWARDEN"
        /// in.
        brand: HFONT,
        title: HFONT,
        subtitle: HFONT,
        name: HFONT,
        username: HFONT,
        button: HFONT,
        /// The search box's face. The app's regular cut at the size a row's
        /// name is set in, so what the user types reads as the same kind of
        /// text as the rows it filters.
        field: HFONT,
        /// The keyboard hints' face: `theme::GDI_MONO_FACE` at
        /// `theme::CHIP_TEXT_PX`, which is what `theme::kbd_chip` renders in.
        hint: HFONT,
    }

    impl Fonts {
        fn build() -> Self {
            use crate::theme::{BOLD, REGULAR, SEMIBOLD};
            Fonts {
                brand: font(BOLD, crate::win32_draw::card_lockup().word_px),
                title: font(BOLD, 15),
                subtitle: font(REGULAR, 12),
                name: font(SEMIBOLD, 13),
                username: font(REGULAR, 11),
                button: font(SEMIBOLD, 12),
                field: font(REGULAR, 13),
                hint: mono(crate::theme::CHIP_TEXT_PX as i32),
            }
        }

        fn destroy(&self) {
            unsafe {
                for f in [
                    self.brand,
                    self.title,
                    self.subtitle,
                    self.name,
                    self.username,
                    self.button,
                    self.field,
                    self.hint,
                ] {
                    let _ = DeleteObject(f);
                }
            }
        }
    }

    static FONTS: Mutex<Option<Fonts>> = Mutex::new(None);
    // `Fonts` holds raw GDI handles, which are process-wide rather than
    // thread-owned. The card is modal on one thread, so nothing shares them;
    // the `Mutex` is only what lets them live in a `static` beside a window
    // procedure that has nowhere else to keep state.
    unsafe impl std::marker::Send for Fonts {}

    // ---- icons -------------------------------------------------------------

    /// One decoded favicon as a 32-bit premultiplied DIB, ready to blend.
    struct Icon {
        bitmap: HBITMAP,
        w: i32,
        h: i32,
    }

    static ICONS: Mutex<Vec<Option<Icon>>> = Mutex::new(Vec::new());
    // Same reason as `Fonts`: a GDI handle is process-wide, and this static is
    // only what lets one live beside a window procedure.
    unsafe impl std::marker::Send for Icon {}

    /// Turns one cached PNG into a DIB section this card can blend.
    ///
    /// **Decoded once, at open, and never in the paint path.** A repaint runs
    /// on every hover; a PNG decode there would put milliseconds between the
    /// pointer moving and the row lighting up, and a file read there would put
    /// the disk on it.
    ///
    /// **Premultiplied, because `AC_SRC_ALPHA` says so.** `AlphaBlend` with
    /// that flag reads the source as premultiplied; handing it straight RGBA
    /// draws a bright halo around every pixel with partial alpha, which is
    /// most of a favicon's edge.
    ///
    /// `None` at any step draws the row without an icon. An icon is decoration
    /// on a row that already says the account's name and its username, and no
    /// part of this card may block on one.
    fn make_icon(png: &[u8]) -> Option<Icon> {
        let (width, height, rgba) = crate::favicon::decode_rgba(png)?;
        if width == 0 || height == 0 || rgba.len() < width * height * 4 {
            return None;
        }
        unsafe {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    // Negative: a top-down DIB, so the rows arrive in the order
                    // `decode_rgba` hands them over and nothing has to be
                    // flipped.
                    biHeight: -(height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            if bits.is_null() {
                let _ = DeleteObject(bitmap);
                return None;
            }
            let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, width * height * 4);
            for i in 0..width * height {
                let r = rgba[i * 4] as u32;
                let g = rgba[i * 4 + 1] as u32;
                let b = rgba[i * 4 + 2] as u32;
                let a = rgba[i * 4 + 3] as u32;
                // BGRA, premultiplied.
                pixels[i * 4] = ((b * a) / 255) as u8;
                pixels[i * 4 + 1] = ((g * a) / 255) as u8;
                pixels[i * 4 + 2] = ((r * a) / 255) as u8;
                pixels[i * 4 + 3] = a as u8;
            }
            Some(Icon { bitmap, w: width as i32, h: height as i32 })
        }
    }

    /// Blends one icon into the row's gutter, centred and square.
    fn draw_icon(hdc: HDC, gutter: RECT, icon: &Icon) {
        unsafe {
            let side = scale(ICON_SIDE);
            let x = gutter.left + ((gutter.right - gutter.left) - side) / 2;
            let y = gutter.top + ((gutter.bottom - gutter.top) - side) / 2;
            let mem = CreateCompatibleDC(hdc);
            let old = SelectObject(mem, icon.bitmap);
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = AlphaBlend(hdc, x, y, side, side, mem, 0, 0, icon.w, icon.h, blend);
            SelectObject(mem, old);
            let _ = DeleteDC(mem);
        }
    }

    /// Frees every decoded icon. Called from `open` before it decodes a new
    /// set and from `close` on the way out, so a card's DIBs never outlive it.
    fn drop_icons() {
        if let Ok(mut icons) = ICONS.lock() {
            for icon in icons.drain(..).flatten() {
                unsafe {
                    let _ = DeleteObject(icon.bitmap);
                }
            }
        }
    }

    /// Which control the pointer is over, as a control id, or 0.
    static HOVERED: AtomicIsize = AtomicIsize::new(0);

    /// The subclassed controls' original procedure.
    ///
    /// **One slot for every control**, unlike `unlock_prompt`'s slot per
    /// button: every control here is the same `BUTTON` class registered by the
    /// same comctl32, so the procedure it replaces is the same pointer -- and
    /// a slot per control would be `ROW_CAP + 2` statics that must all hold
    /// one value.
    static ORIGINAL_PROC: AtomicIsize = AtomicIsize::new(0);

    // ---- the window --------------------------------------------------------

    pub(super) fn open(candidates: &[Candidate], app_name: &str) -> Option<PickerWindow> {
        register_fonts();
        GONE.store(false, Ordering::SeqCst);
        HOVERED.store(0, Ordering::SeqCst);
        // **The mode is decided here and nowhere else.** An empty candidate
        // slice is design 3a's card -- see `super::empty_rows` -- and every
        // `MODE_EMPTY` branch below reads this one decision rather than
        // re-testing `candidates.is_empty()` against a list it no longer has.
        MODE.store(
            if candidates.is_empty() { MODE_EMPTY } else { MODE_LIST },
            Ordering::SeqCst,
        );
        if let Ok(mut slot) = APP_NAME.lock() {
            *slot = app_name.to_string();
        }
        if let Ok(mut slot) = PENDING.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = ENTRIES.lock() {
            slot.clear();
        }
        if let Ok(mut slot) = SEARCH_SHOWN.lock() {
            slot.clear();
        }
        SEARCH_TOTAL.store(0, Ordering::SeqCst);

        // **The cap, and the slot the *Search the vault* row always holds.**
        // See `win32_draw::visible_rows` and `super::populated_rows`: the last
        // slot is that row whatever the list looks like, because a card whose
        // few guesses are all wrong needs a way out, and this window cannot
        // scroll to one. When candidates were dropped to fit, the same row
        // says so -- a card that hid candidates silently is the defect this
        // project keeps finding.
        let (shown, truncated) = crate::win32_draw::visible_rows(candidates.len(), ROW_CAP);
        TRUNCATED.store(truncated, Ordering::SeqCst);
        if let Ok(mut slot) = SHOWN.lock() {
            *slot = candidates[..shown].to_vec();
        }

        unsafe {
            DPI_PERCENT.store(
                {
                    let dc = GetDC(None);
                    let dpi = GetDeviceCaps(dc, LOGPIXELSX);
                    ReleaseDC(None, dc);
                    if dpi > 0 {
                        dpi * 100 / 96
                    } else {
                        100
                    }
                },
                Ordering::SeqCst,
            );
        }

        // The icons, decoded here and only blended in the paint path. Never
        // read from disk and never fetched from the network at any point on
        // this path -- see `make_icon`.
        drop_icons();
        if let (Ok(offers), Ok(mut icons)) = (super::OFFERS.lock(), ICONS.lock()) {
            for candidate in &candidates[..shown] {
                let png = offers
                    .iter()
                    .find(|o| o.candidate.id == candidate.id)
                    .and_then(|o| o.icon.as_deref());
                icons.push(png.and_then(make_icon));
            }
        }

        register_class();
        // **Destroy the previous set before overwriting it.** `Fonts` has no
        // `Drop` -- it holds raw `HFONT`s -- so assigning over a `Some` would
        // leak five fonts per `open` that ran without a matching `close`.
        {
            let mut slot = match FONTS.lock() {
                Ok(slot) => slot,
                Err(_) => {
                    drop_icons();
                    return None;
                }
            };
            if let Some(previous) = slot.take() {
                previous.destroy();
            }
            *slot = Some(Fonts::build());
        }

        // **Sized from the one mode decision above**, never from a second
        // test of the candidate slice -- see `laid_out_rows`.
        let l = super::layout(laid_out_rows());
        let (w, h) = (scale(l.window.w), scale(l.window.h));
        // Centred on the primary work area rather than on the foreground
        // window, for `unlock_prompt::centred`'s reason: a card that jumped
        // around the desktop depending on which app happened to be in front is
        // one the user has to hunt for.
        //
        // **The retired egui no-match card anchored itself to the foreground
        // window** -- `overlay_position(hwnd, NO_MATCH_ROWS)` -- and that
        // anchoring is deliberately dropped here, now that the empty card is
        // this window's most common mode rather than a surface of its own. One
        // card in one place beats a card that lands wherever the app that
        // raised it happens to be.
        let (x, y) = centred(w, h);

        let window = unsafe {
            CreateWindowExW(
                // Topmost, because it is a question asked over whatever the
                // user was doing. It takes focus deliberately: the rows are
                // answered with Tab and Enter as well as with the pointer.
                WS_EX_TOPMOST,
                CLASS_NAME,
                &HSTRING::from(PICKER_PROMPT_TITLE),
                // Frameless. A `WS_CAPTION` frame is the loudest "system
                // dialog" signal there is, and this app's own windows are
                // frameless with drawn chrome.
                WS_POPUP | WS_VISIBLE,
                x,
                y,
                w,
                h,
                None,
                None,
                None,
                None,
            )
        }
        .ok()?;

        round_corners(window);

        // **Below this line the card is on screen.** `WS_VISIBLE` is in the
        // style, so a bare `?` here would return `None`, make `run_with`
        // answer `Unavailable`, and leave a frameless topmost card with no
        // controls and no way for the user to dismiss it -- `close` is only
        // reached with a `PickerWindow` in hand. Every failure path from here
        // on goes through `abandon`, which takes the window down and frees the
        // fonts and the DIBs before answering `None`.
        fn abandon(window: HWND) -> Option<PickerWindow> {
            unsafe {
                let _ = DestroyWindow(window);
            }
            if let Ok(mut slot) = FONTS.lock() {
                if let Some(fonts) = slot.take() {
                    fonts.destroy();
                }
            }
            drop_icons();
            // `close`'s reason, on the path that never reaches `close`: not a
            // secret, but it is the name of an app this user was in front of,
            // and `run_with` answers `Unavailable` and returns without ever
            // taking the card down -- so without this the name would sit in
            // the static until the next `open`.
            if let Ok(mut slot) = APP_NAME.lock() {
                slot.clear();
            }
            None
        }

        // The handles are copied out and the guard dropped at the end of this
        // statement: `abandon` locks `FONTS` itself, so holding the guard
        // across the `child` calls below would deadlock the failure path.
        let Some((name_font, button_font, field_font)) = FONTS
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|f| (f.name, f.button, f.field)))
        else {
            return abandon(window);
        };

        // Every slot gets a control whether or not this list fills it: the
        // second step reuses the same controls for its own rows, and creating
        // them lazily would mean creating a window from inside a repaint.
        //
        // **Created hidden, and `apply_mode` decides which come back.** The
        // empty card is laid out for two rows but still makes six, so slots
        // 2..6 are placed below its own footer and past the bottom of a 213 px
        // window. Created `WS_VISIBLE` they were four blank buttons painted
        // over the footer for the one frame between here and the
        // `ShowWindow(SW_HIDE)` in `apply_mode` -- which runs after this loop,
        // on a parent that is already up. `apply_mode` shows exactly the rows
        // `visible_row_count` claims, in every mode, so nothing that should be
        // on screen is left hidden by this.
        for index in 0..LIST_ROWS {
            let Some(control) = child(
                window,
                w!("BUTTON"),
                WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
                super::row_at(laid_out_rows(), index),
                ID_ROW + index,
                name_font,
            ) else {
                return abandon(window);
            };
            subclass(control);
        }
        let Some(secondary) = child(
            window,
            w!("BUTTON"),
            WS_VISIBLE.0 | WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            l.secondary,
            ID_SECONDARY,
            button_font,
        ) else {
            return abandon(window);
        };
        let Some(cancel) = child(
            window,
            w!("BUTTON"),
            WS_VISIBLE.0 | WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            l.cancel,
            ID_CANCEL,
            button_font,
        ) else {
            return abandon(window);
        };
        subclass(secondary);
        subclass(cancel);

        // **The search box, created here and shown only in `MODE_SEARCH`.**
        //
        // Created with the rest rather than when the mode is entered, for the
        // reason the unused rows are: creating a window from inside the
        // handling of a click on a window is a thing to avoid, and `apply_mode`
        // already owns which controls are on screen.
        //
        // **Not subclassed.** Every other control here is a `BUTTON` whose
        // painting this module takes over; an `EDIT` is a control the user
        // types into, and comctl32's own procedure is what draws the caret, the
        // selection and the horizontal scroll. What makes it look like this
        // app's is the box the parent paints around it (see `paint`) and
        // `WM_CTLCOLOREDIT`, which hands it the card's white and the app's ink
        // -- exactly what `unlock_prompt` does with its password field.
        //
        // Inset inside the painted box by the same 10px `unlock_prompt` uses,
        // and vertically centred in it, so the text sits off the border rather
        // than against it.
        let search_box = super::layout_for(LIST_ROWS, true)
            .search
            .expect("`layout_for(.., true)` is the search shape and always has the box");
        if child(
            window,
            w!("EDIT"),
            WS_TABSTOP.0 | ES_AUTOHSCROLL as u32,
            Box2 {
                x: search_box.x + 10,
                y: search_box.y + (search_box.h - 20) / 2,
                w: search_box.w - 20,
                h: 20,
            },
            ID_SEARCH,
            field_font,
        )
        .is_none()
        {
            return abandon(window);
        }

        apply_mode(window);

        unsafe {
            let _ = ShowWindow(window, SW_SHOW);
            // Allowed to refuse, and handled rather than asserted -- the
            // property `foreground` records. A refusal leaves a topmost card
            // on screen that the user clicks once to focus.
            let _ = SetForegroundWindow(window);
        }

        Some(PickerWindow(handle_of(window)))
    }

    /// **The protection, on the top-level window.**
    ///
    /// Applied to the card itself and never to a child: Windows refuses
    /// `SetWindowDisplayAffinity` on a child control with `E_INVALIDARG`, and
    /// the top-level flag covers every child it owns. What it protects is not
    /// a password -- there is none on this surface -- but *which accounts this
    /// user holds for the app they are in front of*, which is exactly the
    /// thing a screen recorder should not be handed.
    pub(super) fn protect(window: PickerWindow) -> bool {
        unsafe { SetWindowDisplayAffinity(hwnd(window.0), WDA_EXCLUDEFROMCAPTURE).is_ok() }
    }

    /// Pumps until the user does something.
    ///
    /// **This blocks.** It does not return until the window procedure has
    /// recorded an event or the window has gone away, and the event it hands
    /// back is *taken* out of `PENDING` rather than read from it -- so no
    /// event can be delivered twice. That is what [`super::run_with`]'s
    /// ignore-and-continue arms rest on: an implementation that returned the
    /// same ignorable event over and over would turn each of those `continue`s
    /// into a spin that filled the log.
    ///
    /// **`IsDialogMessageW` is what makes Tab, Shift+Tab, Space and Enter work
    /// at all.** A bare `TranslateMessage`/`DispatchMessage` pump around
    /// controls that are not in a dialog gives none of them. Escape is handled
    /// before it, because `IsDialogMessage` only cancels for a real dialog box.
    pub(super) fn next(window: PickerWindow) -> Event {
        use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
        use windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN;

        let top = hwnd(window.0);
        loop {
            if GONE.load(Ordering::SeqCst) {
                return Event::Closed;
            }
            if let Some(event) = take_pending() {
                return event;
            }

            let mut msg = MSG::default();
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        GONE.store(true, Ordering::SeqCst);
                        return Event::Closed;
                    }
                    if msg.message == WM_KEYDOWN && msg.wParam.0 as u16 == VK_ESCAPE.0 {
                        return Event::Cancel;
                    }
                    // **Only the CTRL+ALT chords, and only while both are
                    // down.** A blanket `WM_KEYDOWN` grab here would swallow
                    // Tab and Enter before `IsDialogMessageW` ever saw them,
                    // and that traversal is the whole of the card's keyboard
                    // behaviour.
                    if msg.message == WM_KEYDOWN && chord_held() && chord(msg.wParam.0 as u16) {
                        if let Some(event) = take_pending() {
                            return event;
                        }
                        // Ours, and it chose nothing -- a digit past the rows
                        // on screen. Swallowed rather than dispatched, so it
                        // cannot reach a control as a keystroke.
                        continue;
                    }
                    if !IsDialogMessageW(top, &msg).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    if GONE.load(Ordering::SeqCst) {
                        return Event::Closed;
                    }
                    if let Some(event) = take_pending() {
                        return event;
                    }
                }
            }
            // Idle. Nothing on this card animates, so this is a plain wait for
            // the next message rather than a frame tick.
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    }

    /// Whether CTRL and ALT are both down right now.
    ///
    /// Read at the moment the key arrives rather than tracked across messages:
    /// a modifier released while this window was not focused would otherwise
    /// leave a flag set that nothing ever clears.
    fn chord_held() -> bool {
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU};
        unsafe { (GetKeyState(VK_CONTROL.0 as i32) < 0) && (GetKeyState(VK_MENU.0 as i32) < 0) }
    }

    /// **What a CTRL+ALT chord does**, answering whether it was one of ours.
    ///
    /// Every arm goes through [`clicked`] -- the same function `WM_COMMAND`
    /// calls when the row or the button is clicked -- so a shortcut and a
    /// click are one path and not two, and `run_with` cannot tell them apart.
    ///
    /// A digit past the rows on screen is *ours and does nothing*: it answers
    /// `true` so it is swallowed rather than typed into a control, and posts
    /// no event, so the card neither beeps nor closes.
    fn chord(vk: u16) -> bool {
        // The virtual-key codes for the number row and for the letters are
        // their ASCII values, which is what makes these comparisons honest.
        const DIGIT_1: u16 = b'1' as u16;
        const DIGIT_9: u16 = b'9' as u16;
        const LETTER_N: u16 = b'N' as u16;
        if (DIGIT_1..=DIGIT_9).contains(&vk) {
            let digit = (vk - DIGIT_1 + 1) as u32;
            // **The first step and search mode number their rows; the other
            // two do not.** `MODE_PALETTE`'s rows are fields of one account
            // and `MODE_EMPTY`'s are two offers, and neither was asked for.
            //
            // **A digit is numbered in search mode too, even though a text box
            // has the keyboard there**, and that is a decision rather than an
            // oversight. `CTRL+ALT+3` is a chord, not the character `3`: this
            // arm is reached only with both modifiers down, and it is reached
            // for every digit whether or not it chooses anything -- so the
            // digit is swallowed here in every mode already and can never
            // reach the box as a keystroke. Given that, making it choose the
            // third result is strictly better than making it do nothing, and
            // it is the same chord family on the same rows the user is looking
            // at. A plain `3` still types a `3`.
            //
            // The count is the ACCOUNT rows', never the drawn rows', in both
            // modes: no digit can land on the *Search the vault* row or on
            // search mode's overflow notice.
            let mode = MODE.load(Ordering::SeqCst);
            if mode == MODE_LIST || mode == MODE_SEARCH {
                let shown = if mode == MODE_SEARCH {
                    SEARCH_SHOWN.lock().map(|s| s.len()).unwrap_or(0)
                } else {
                    SHOWN.lock().map(|s| s.len()).unwrap_or(0)
                };
                if let Some(index) = super::candidate_for_digit(digit, shown) {
                    clicked(ID_ROW + index);
                }
            }
            return true;
        }
        if vk == LETTER_N {
            if let Some(id) = new_login_control() {
                clicked(id);
            }
            return true;
        }
        false
    }

    /// Which control *New login* is on in the step that is showing, or `None`
    /// where the card does not offer it.
    ///
    /// The empty card's *New login* is a row rather than the footer button --
    /// `apply_mode` hides that button there -- and the second step's footer
    /// button is *Edit binding*, which is a different offer: a chord that
    /// silently edited a binding because the user expected a new login would
    /// be worse than one that did nothing at all.
    fn new_login_control() -> Option<usize> {
        match MODE.load(Ordering::SeqCst) {
            // Search mode keeps the footer button, and it is still *New login*:
            // "none of these" is exactly what a user who has searched their
            // whole vault and found nothing means.
            MODE_LIST | MODE_SEARCH => Some(ID_SECONDARY),
            MODE_EMPTY => super::empty_rows()
                .iter()
                .position(|action| *action == EmptyAction::NewLogin)
                .map(|index| ID_ROW + index),
            _ => None,
        }
    }

    fn take_pending() -> Option<Event> {
        PENDING.lock().ok().and_then(|mut slot| slot.take())
    }

    fn set_pending(event: Event) {
        if let Ok(mut slot) = PENDING.lock() {
            *slot = Some(event);
        }
    }

    /// **The longest filter the card will read out of its search box.**
    ///
    /// Not a limit on what can be typed -- the control takes whatever it is
    /// given -- but a bound on the buffer this reads it into. A paste of a
    /// megabyte is a filter that matches nothing, and reading all of it per
    /// keystroke to prove that would be the slowest possible way to say so.
    /// 256 characters is far longer than any vault item's name.
    const QUERY_CAP: usize = 256;

    /// What is in the search box right now.
    ///
    /// `GetWindowTextW` and not `WM_GETTEXT` through `SendMessageW`, because
    /// this is an ordinary control with ordinary contents: `unlock_prompt`
    /// reaches for the message form there so it can wipe the buffer it copied
    /// into, and that is a password. This is a filter over item names.
    fn search_query(window: HWND) -> String {
        unsafe {
            let Ok(control) = GetDlgItem(window, ID_SEARCH as i32) else {
                return String::new();
            };
            let mut buffer = [0u16; QUERY_CAP];
            let len = GetWindowTextW(control, &mut buffer);
            if len <= 0 {
                return String::new();
            }
            String::from_utf16_lossy(&buffer[..len as usize])
        }
    }

    /// The card's white as a brush, for `WM_CTLCOLOREDIT`.
    ///
    /// A `OnceLock` and never deleted, exactly as `unlock_prompt::card_brush`
    /// is: the value returned from `WM_CTLCOLOREDIT` is a handle the system
    /// keeps using after the handler returns, so a brush created per message
    /// and deleted would be a use-after-free -- and one created per message and
    /// not deleted would leak one GDI object per repaint of a control the user
    /// is typing into.
    fn card_brush() -> HBRUSH {
        static BRUSH: OnceLock<isize> = OnceLock::new();
        HBRUSH(*BRUSH.get_or_init(|| unsafe { CreateSolidBrush(rgb(crate::theme::CARD)).0 as isize })
            as *mut c_void)
    }

    /// Swaps the card to its second step: the chosen account's fields.
    pub(super) fn show_palette(window: PickerWindow, palette: &Palette) {
        if let Ok(mut slot) = ENTRIES.lock() {
            *slot = super::palette_rows(palette);
        }
        MODE.store(MODE_PALETTE, Ordering::SeqCst);
        let top = hwnd(window.0);
        apply_mode(top);
    }

    /// **Swaps the card into search mode, and refreshes its rows.**
    ///
    /// One entry point for both, as [`super::PickerCalls::show_search`] says:
    /// the window does the same thing either way, and two would be two chances
    /// for the mode and the rows behind it to disagree.
    ///
    /// **Only the display strings are kept.** `results.offers` carries a
    /// `Palette` per row; that stays with `run_with`, which is what needs it,
    /// and what is parked here is what the paint path has to have -- names and
    /// usernames. No icon either: `make_icon` decodes at open, and a keystroke
    /// is not a place to read the on-disk favicon cache.
    pub(super) fn show_search(window: PickerWindow, results: &SearchResults) {
        if let Ok(mut slot) = SEARCH_SHOWN.lock() {
            *slot = results
                .offers
                .iter()
                .take(SEARCH_CAP)
                .map(|offer| offer.candidate.clone())
                .collect();
        }
        SEARCH_TOTAL.store(results.total as isize, Ordering::SeqCst);
        let entering = MODE.swap(MODE_SEARCH, Ordering::SeqCst) != MODE_SEARCH;
        let top = hwnd(window.0);
        apply_mode(top);
        if entering {
            // **Focused, so the user types immediately.** This is the whole of
            // what was asked for -- "the same text box and focus on it" -- and
            // it happens only on the way in: refocusing on every keystroke
            // would put the caret back at the start of what was just typed.
            unsafe {
                if let Ok(control) = GetDlgItem(top, ID_SEARCH as i32) {
                    let _ = SetFocus(control);
                }
            }
        }
    }

    /// Whether the card is showing its search mode.
    fn in_search() -> bool {
        MODE.load(Ordering::SeqCst) == MODE_SEARCH
    }

    /// **This card's rectangles, for whichever mode is showing.**
    ///
    /// The one place the two arguments `super::layout_for` takes are decided,
    /// so a control placed against one shape and hit-tested against another is
    /// not expressible: every call below reads this.
    fn card() -> super::Layout {
        super::layout_for(laid_out_rows(), in_search())
    }

    /// Shows exactly the controls this step has rows for, hides the rest,
    /// places them where this mode's layout puts them, and puts the keyboard on
    /// the first of them.
    ///
    /// **Hiding is what stops an empty slot being a clickable nothing.** A
    /// `BUTTON` left visible with no label is still a tab stop and still posts
    /// `BN_CLICKED`.
    ///
    /// **And placing is what lets a mode change the card's shape.** Search mode
    /// puts a box between the subtitle and the list, so every row and both
    /// footer buttons move down and the window itself grows. `unlock_prompt`'s
    /// argument against a window that resizes between steps -- it moves its own
    /// Cancel button out from under a pointer that is about to click it -- is
    /// answered rather than ignored: every transition into or out of this mode
    /// is made by clicking a *row*, never the footer, so the pointer is never
    /// over the button that moves.
    fn apply_mode(window: HWND) {
        let count = visible_row_count();
        let l = card();
        let rows = laid_out_rows();
        let search = in_search();
        unsafe {
            // The window first, so a control moved into the new area is not
            // placed outside the client rect for a frame.
            let _ = SetWindowPos(
                window,
                None,
                0,
                0,
                scale(l.window.w),
                scale(l.window.h),
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            place(window, ID_SEARCH, {
                let box2 = l.search.unwrap_or(l.subtitle);
                Box2 {
                    x: box2.x + 10,
                    y: box2.y + (box2.h - 20) / 2,
                    w: box2.w - 20,
                    h: 20,
                }
            });
            if let Ok(control) = GetDlgItem(window, ID_SEARCH as i32) {
                let _ = ShowWindow(control, if search { SW_SHOW } else { SW_HIDE });
            }
            place(window, ID_SECONDARY, l.secondary);
            place(window, ID_CANCEL, l.cancel);
            for index in 0..LIST_ROWS {
                place(window, ID_ROW + index, super::row_at_for(rows, search, index));
                if let Ok(control) = GetDlgItem(window, (ID_ROW + index) as i32) {
                    let _ = ShowWindow(control, if index < count { SW_SHOW } else { SW_HIDE });
                }
            }
            // **The empty card's footer is Cancel alone.** Its two offers are
            // rows, so a *New login* button beside them would be the same
            // choice twice; and hiding it is what stops it -- an empty
            // `BUTTON` left visible is still a tab stop and still posts
            // `BN_CLICKED`, which is the same reason the unused rows above
            // are hidden rather than merely unlabelled.
            if let Ok(control) = GetDlgItem(window, ID_SECONDARY as i32) {
                let _ = ShowWindow(
                    control,
                    if MODE.load(Ordering::SeqCst) == MODE_EMPTY { SW_HIDE } else { SW_SHOW },
                );
            }
            // **Only when there is a row to focus.** A palette that came back
            // empty -- `palette_of` missing its item -- hides every row, and
            // focusing a hidden control puts the keyboard nowhere the user can
            // see it.
            //
            // **And never in search mode.** The keyboard belongs in the search
            // box there: `apply_mode` runs on every keystroke, so moving focus
            // to the first row here would take it off the box the user is
            // typing into after the first character.
            if count > 0 && !search {
                if let Ok(control) = GetDlgItem(window, ID_ROW as i32) {
                    let _ = SetFocus(control);
                }
            }
        }
        repaint(window);
    }

    /// Moves one control to a **logical** rectangle, scaling it as
    /// `child` did when it created it.
    fn place(window: HWND, id: usize, at: Box2) {
        unsafe {
            if let Ok(control) = GetDlgItem(window, id as i32) {
                let _ =
                    MoveWindow(control, scale(at.x), scale(at.y), scale(at.w), scale(at.h), true);
            }
        }
    }

    /// How many rows this step's card is **laid out** for -- which is not
    /// [`visible_row_count`].
    ///
    /// `MODE_LIST` and `MODE_PALETTE` are two steps of one live window, so
    /// both are laid out at `LIST_ROWS` whatever they are showing: see
    /// [`super::layout`] for why a window that resized between them would move
    /// its own Cancel button. `MODE_EMPTY` never transitions -- `open` decides
    /// it once and nothing sets it -- so it is sized to its own two offers.
    fn laid_out_rows() -> usize {
        if MODE.load(Ordering::SeqCst) == MODE_EMPTY {
            super::empty_rows().len().min(LIST_ROWS)
        } else {
            // Search mode included, and deliberately at the full height: its
            // row count changes with every keystroke, and a card that grew and
            // shrank as the user typed would move its own footer under their
            // hand between one character and the next.
            LIST_ROWS
        }
    }

    /// How many row controls this step is using.
    fn visible_row_count() -> usize {
        let mode = MODE.load(Ordering::SeqCst);
        if mode == MODE_SEARCH {
            let shown = SEARCH_SHOWN.lock().map(|s| s.len()).unwrap_or(0);
            let total = SEARCH_TOTAL.load(Ordering::SeqCst).max(0) as usize;
            super::search_rows(shown, total).len().min(LIST_ROWS)
        } else if mode == MODE_EMPTY {
            super::empty_rows().len().min(LIST_ROWS)
        } else if mode == MODE_PALETTE {
            ENTRIES.lock().map(|e| e.len()).unwrap_or(0).min(LIST_ROWS)
        } else {
            // The candidates, plus the *Search the vault* row that always
            // follows them. `visible_rows` capped the candidates at `ROW_CAP`
            // and the card lays out `LIST_ROWS = ROW_CAP + 1` slots, so that
            // row is one the card is tall for rather than one it takes from
            // the candidates.
            let rows = SHOWN.lock().map(|s| s.len()).unwrap_or(0);
            (rows + 1).min(LIST_ROWS)
        }
    }

    pub(super) fn close(window: PickerWindow) {
        unsafe {
            let _ = DestroyWindow(hwnd(window.0));
        }
        if let Ok(mut slot) = FONTS.lock() {
            if let Some(fonts) = slot.take() {
                fonts.destroy();
            }
        }
        drop_icons();
        if let Ok(mut slot) = SHOWN.lock() {
            slot.clear();
        }
        if let Ok(mut slot) = ENTRIES.lock() {
            slot.clear();
        }
        // Not secrets either, but they are item names out of this user's own
        // vault, and nothing needs them once the card is down.
        if let Ok(mut slot) = SEARCH_SHOWN.lock() {
            slot.clear();
        }
        SEARCH_TOTAL.store(0, Ordering::SeqCst);
        if let Ok(mut slot) = PENDING.lock() {
            *slot = None;
        }
        // Not a secret, but it is the name of an app this user was in front
        // of, and nothing needs it once the card is down.
        if let Ok(mut slot) = APP_NAME.lock() {
            slot.clear();
        }
    }

    // ---- plumbing ----------------------------------------------------------

    fn handle_of(h: HWND) -> isize {
        h.0 as isize
    }

    fn hwnd(h: isize) -> HWND {
        HWND(h as *mut c_void)
    }

    fn repaint(window: HWND) {
        unsafe {
            let _ = InvalidateRect(window, None, false);
        }
    }

    fn centred(w: i32, h: i32) -> (i32, i32) {
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{
                SystemParametersInfoW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
            };
            let mut area = RECT::default();
            let ok = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut area as *mut _ as *mut c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
            if ok.is_err() || area.right <= area.left {
                return (200, 200);
            }
            (
                area.left + (area.right - area.left - w) / 2,
                // Slightly above centre, where every OS credential prompt puts
                // itself: a card the eye has to find sits better a little high.
                area.top + (area.bottom - area.top - h) * 2 / 5,
            )
        }
    }

    fn register_class() {
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| unsafe {
            let class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                lpszClassName: CLASS_NAME,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                // No background brush: `WM_ERASEBKGND` is answered and the
                // whole client area is painted from one back buffer, which is
                // what keeps the card from flashing system grey on a repaint.
                hbrBackground: HBRUSH::default(),
                ..Default::default()
            };
            RegisterClassW(&class);
        });
    }

    /// One child control. It is created with **no text**: every label on this
    /// card is painted by `paint_control` from the app's own palette and type,
    /// so a control's own caption would only ever be a second, stale copy.
    fn child(
        parent: HWND,
        class: PCWSTR,
        style: u32,
        at: Box2,
        id: usize,
        font: HFONT,
    ) -> Option<HWND> {
        let h = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!(""),
                // `WS_VISIBLE` is the CALLER's, not this helper's: a control
                // created visible at a rectangle its mode does not use is on
                // screen for the frame between `CreateWindowExW` and the
                // `ShowWindow(SW_HIDE)` that `apply_mode` gets to afterwards.
                WINDOW_STYLE(WS_CHILD.0 | style),
                scale(at.x),
                scale(at.y),
                scale(at.w),
                scale(at.h),
                parent,
                HMENU(id as *mut c_void),
                None,
                None,
            )
        }
        .ok()?;
        unsafe {
            SendMessageW(h, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
        }
        Some(h)
    }

    fn round_corners(window: HWND) {
        // The same `DWMWCP_ROUND` the login window's frameless chrome asks
        // for, so every surface in this app has the same silhouette.
        unsafe {
            use windows::Win32::Graphics::Dwm::{
                DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
            };
            let preference = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                window,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &preference as *const _ as *const c_void,
                std::mem::size_of_val(&preference) as u32,
            );
        }
    }

    /// Takes over a control's painting without losing the focus and keyboard
    /// behaviour that makes `IsDialogMessage` work.
    fn subclass(control: HWND) {
        unsafe {
            let previous =
                SetWindowLongPtrW(control, GWLP_WNDPROC, control_proc as *const () as isize);
            if previous != 0 {
                ORIGINAL_PROC.store(previous, Ordering::SeqCst);
            }
        }
    }

    // ---- the window procedures ---------------------------------------------

    unsafe extern "system" fn wnd_proc(
        window: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint(window);
                LRESULT(0)
            }
            // The search box sits inside a box the parent painted, so its own
            // background has to be the card's white rather than the system's --
            // `unlock_prompt`'s password field is themed the same way, by the
            // same three calls.
            WM_CTLCOLOREDIT => {
                let hdc = HDC(wparam.0 as *mut c_void);
                SetBkColor(hdc, rgb(crate::theme::CARD));
                SetTextColor(hdc, rgb(crate::theme::INK));
                LRESULT(card_brush().0 as isize)
            }
            // Frameless windows are dragged by their background.
            WM_NCHITTEST => {
                let hit = DefWindowProcW(window, msg, wparam, lparam);
                if hit.0 == 1 {
                    LRESULT(HTCAPTION as isize)
                } else {
                    hit
                }
            }
            WM_LBUTTONDOWN => {
                if in_close_glyph(lparam) {
                    set_pending(Event::Cancel);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                // A pointer that left a control without entering another one is
                // seen here rather than by the control it left.
                if HOVERED.swap(0, Ordering::SeqCst) != 0 {
                    repaint(window);
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = (wparam.0 & 0xffff) as i32;
                let notification = ((wparam.0 >> 16) & 0xffff) as u32;
                // **`EN_CHANGE` before `BN_CLICKED`, because they collide.**
                // Both are notification codes in the same 16 bits and
                // `BN_CLICKED` is 0; the control id is what tells them apart,
                // so the search box's notifications are taken by id first and
                // never fall through to `clicked`.
                if id == ID_SEARCH as i32 {
                    if notification == EN_CHANGE && MODE.load(Ordering::SeqCst) == MODE_SEARCH {
                        set_pending(Event::Typed(search_query(window)));
                    }
                    return LRESULT(0);
                }
                if notification == BN_CLICKED {
                    clicked(id as usize);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                // **NO `PostQuitMessage` HERE, EVER.** This window is opened
                // on the daemon thread, and that thread goes on to run egui
                // windows -- the save-a-login form after *New login*, the
                // preflight host after *Password* / *One-time code*.
                // `close()` calls `DestroyWindow`, which dispatches this
                // message synchronously on that thread, so a `PostQuitMessage`
                // here leaves the thread's quit flag set with nothing left to
                // drain it: `next()` has already returned and no pump of ours
                // runs again. The next `eframe::run_native` then takes that
                // stale `WM_QUIT` out of `GetMessageW`, leaves its loop before
                // it draws a frame, and returns its default answer -- the form
                // never appears, and the preflight reports "not confirmed" so
                // nothing is typed.
                //
                // Quitting is not this handler's job in the first place:
                // `GONE` on the line above is what `next()` reads to report
                // `Event::Closed`, and the `WM_QUIT` branch in `next()` stays
                // for a quit posted from outside.
                GONE.store(true, Ordering::SeqCst);
                LRESULT(0)
            }
            _ => DefWindowProcW(window, msg, wparam, lparam),
        }
    }

    /// **What a click on control `id` means**, which is the only place the two
    /// steps differ in behaviour.
    ///
    /// A row past the end of this step's list is ignored rather than answered:
    /// the control is hidden there, so it can be reached by neither pointer
    /// nor Tab, and inventing an answer for it would be inventing a choice the
    /// user did not make.
    fn clicked(id: usize) {
        let mode = MODE.load(Ordering::SeqCst);
        let palette = mode == MODE_PALETTE;
        let empty = mode == MODE_EMPTY;
        let search = mode == MODE_SEARCH;
        if id == ID_CANCEL {
            set_pending(Event::Cancel);
            return;
        }
        if id == ID_SECONDARY {
            // Hidden on the empty card -- `apply_mode` -- so this cannot be
            // reached there by pointer or by Tab. Answering nothing is what
            // makes that a fact rather than a layout accident.
            if !empty {
                set_pending(if palette { Event::EditSelected } else { Event::NewLogin });
            }
            return;
        }
        if id < ID_ROW {
            return;
        }
        let index = id - ID_ROW;
        if index >= visible_row_count() {
            return;
        }
        if empty {
            // The two offers answer the two events the populated card's own
            // *New login* button and overflow row answer, so `run_with` needs
            // no arm of its own for this mode.
            match super::empty_rows().get(index) {
                Some(EmptyAction::NewLogin) => set_pending(Event::NewLogin),
                Some(EmptyAction::SearchVault) => set_pending(Event::Search),
                None => {}
            }
            return;
        }
        if search {
            // Only the result rows answer. The overflow notice and the "no
            // matches" row are text, not offers -- clicking one and getting
            // some account's field palette would be the card inventing a
            // choice the user did not make.
            let shown = SEARCH_SHOWN.lock().map(|s| s.len()).unwrap_or(0);
            if index < shown {
                set_pending(Event::Chose(index));
            }
            return;
        }
        if palette {
            if let Some(send) = ENTRIES.lock().ok().and_then(|e| e.get(index).cloned()) {
                set_pending(Event::Sends(send));
            }
            return;
        }
        let shown = SHOWN.lock().map(|s| s.len()).unwrap_or(0);
        if index >= shown {
            // The *Search the vault* row, which is the last row of every
            // populated card -- see `super::populated_rows`. It no longer
            // leaves this card: `run_with` answers it by switching the same
            // window into search mode.
            set_pending(Event::Search);
        } else {
            set_pending(Event::Chose(index));
        }
    }

    /// The subclassed controls: everything except painting and hover is the
    /// original `BUTTON` procedure's, which is what keeps focus, the space bar
    /// and `IsDialogMessage`'s traversal working.
    unsafe extern "system" fn control_proc(
        control: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let id = GetWindowLongPtrW(control, windows::Win32::UI::WindowsAndMessaging::GWLP_ID);
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint_control(control, id as usize);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if HOVERED.swap(id, Ordering::SeqCst) != id {
                    repaint(control);
                }
                LRESULT(0)
            }
            _ => {
                let original = ORIGINAL_PROC.load(Ordering::SeqCst);
                if original == 0 {
                    DefWindowProcW(control, msg, wparam, lparam)
                } else {
                    CallWindowProcW(
                        Some(std::mem::transmute::<
                            isize,
                            unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
                        >(original)),
                        control,
                        msg,
                        wparam,
                        lparam,
                    )
                }
            }
        }
    }

    fn in_close_glyph(lparam: LPARAM) -> bool {
        let l = card();
        let x = (lparam.0 & 0xffff) as i16 as i32;
        let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
        x >= scale(l.close_glyph.x)
            && x < scale(l.close_glyph.right())
            && y >= scale(l.close_glyph.y)
            && y < scale(l.close_glyph.bottom())
    }

    // ---- painting ----------------------------------------------------------

    /// The card's own surface: the heading pair, the list's card and the close
    /// glyph. Every row and every button is a child control that paints itself.
    fn paint(window: HWND) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(window, &mut ps);
            let mut client = RECT::default();
            let _ = GetClientRect(window, &mut client);
            let (w, h) = (client.right, client.bottom);

            // Double-buffered, for `unlock_prompt::paint`'s reason: a surface
            // painted straight to the window flickers on every hover.
            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            let old = SelectObject(mem, bmp);

            let guard = FONTS.lock();
            let fonts = guard.as_ref().ok().and_then(|slot| slot.as_ref());

            fill(mem, client, crate::theme::WINDOW_BG);
            SetBkMode(mem, TRANSPARENT);

            let l = card();
            // The card the rows sit on, so a row's own white reads as part of
            // one surface rather than as five floating strips.
            rounded(mem, l.list, 8, crate::theme::CARD, Some((1, crate::theme::HAIRLINE)));

            // The search box's own box, and its focus halo. The `EDIT` is a
            // child sitting inside this, painted by comctl32 in the colours
            // `WM_CTLCOLOREDIT` hands it -- the same division of labour
            // `unlock_prompt` draws its password field with, and the same
            // 3px `theme::FOCUS_RING` flush against the border's outer edge.
            if let Some(field) = l.search {
                let focused = GetDlgItem(window, ID_SEARCH as i32)
                    .map(|control| GetFocus() == control)
                    .unwrap_or(false);
                if focused {
                    rounded(
                        mem,
                        Box2 {
                            x: field.x - 2,
                            y: field.y - 2,
                            w: field.w + 4,
                            h: field.h + 4,
                        },
                        9,
                        crate::theme::FOCUS_RING,
                        None,
                    );
                }
                rounded(
                    mem,
                    field,
                    8,
                    crate::theme::CARD,
                    Some((
                        1,
                        if focused {
                            crate::theme::BLUE
                        } else {
                            crate::theme::BORDER_STRONG
                        },
                    )),
                );
            }

            if let Some(fonts) = fonts {
                let (title_run, subtitle_run) = headings();
                paint_lockup(mem, &l, fonts.brand);
                text(mem, fonts.title, l.title, &title_run, crate::theme::INK);
                text(mem, fonts.subtitle, l.subtitle, subtitle_run, crate::theme::TEXT_FAINT);
            }

            paint_close_glyph(mem, l.close_glyph);

            drop(guard);
            let _ = BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            let _ = EndPaint(window, &ps);
        }
    }

    /// One child control: a row in either step, or one of the two footer
    /// buttons.
    fn paint_control(control: HWND, id: usize) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(control, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(control, &mut rc);

            let hovered = HOVERED.load(Ordering::SeqCst) == id as isize;
            let focused = GetFocus() == control;

            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, rc.right, rc.bottom);
            let old = SelectObject(mem, bmp);
            let whole = RECT { left: 0, top: 0, right: rc.right, bottom: rc.bottom };

            let guard = FONTS.lock();
            let fonts = guard.as_ref().ok().and_then(|slot| slot.as_ref());

            if id >= ID_ROW {
                // The row sits on the list's card, so anything `draw_row` does
                // not cover is that card's white. `draw_row` fills the whole
                // rect edge to edge, which is what keeps a hover from hugging
                // just the text.
                fill(mem, whole, crate::theme::CARD);
                if let Some(fonts) = fonts {
                    // **Focus is drawn as selection.** These rows are reached
                    // by Tab as readily as by the pointer, and a focused row
                    // with no mark on it is a keyboard user pressing Enter on
                    // a card that never said which account they were on.
                    paint_row(
                        mem,
                        whole,
                        id - ID_ROW,
                        RowState { selected: focused, hovered },
                        fonts,
                    );
                }
            } else {
                // The footer sits on the window's own background, not on the
                // card -- otherwise the button's rounded corners show system
                // grey through them.
                fill(mem, whole, crate::theme::WINDOW_BG);
                SetBkMode(mem, TRANSPARENT);
                let skin =
                    if hovered { ButtonSkin::secondary().hovered() } else { ButtonSkin::secondary() };
                if let Some(fonts) = fonts {
                    let label = footer_label(id);
                    // *Cancel* always shows `ESC` -- Escape cancels the card
                    // in every mode, unlike *New login*, which `new_login_control`
                    // hides under `MODE_PALETTE`.
                    let hint = if id == ID_CANCEL {
                        Some(ESC_SHORTCUT)
                    } else {
                        (id == ID_SECONDARY && MODE.load(Ordering::SeqCst) != MODE_PALETTE)
                            .then_some(NEW_LOGIN_SHORTCUT)
                    }
                    .map(|text| (text, fonts.hint));
                    let dpi = DPI_PERCENT.load(Ordering::SeqCst);
                    if focused {
                        // **The ring is given LOGICAL size, from `layout`.**
                        // `rounded` scales everything it is handed, and `rc`
                        // came back from `GetClientRect` in device pixels
                        // already: passing it drew the ring at 1.5x the
                        // control at 150%, running past the client area and
                        // being clipped -- losing exactly the rounded corners
                        // the ring exists to draw. `layout` knows the button's
                        // logical size independently, so nothing has to divide
                        // device pixels back down and round badly doing it.
                        let l = card();
                        let button = if id == ID_CANCEL { l.cancel } else { l.secondary };
                        rounded(
                            mem,
                            Box2 { x: 0, y: 0, w: button.w, h: button.h },
                            8,
                            crate::theme::FOCUS_RING,
                            None,
                        );
                        let inner = RECT {
                            left: whole.left + 2,
                            top: whole.top + 2,
                            right: whole.right - 2,
                            bottom: whole.bottom - 2,
                        };
                        draw_button_with_shortcut(
                            mem, inner, &label, fonts.button, skin, scale(7), hint, dpi,
                        );
                    } else {
                        draw_button_with_shortcut(
                            mem, whole, &label, fonts.button, skin, scale(7), hint, dpi,
                        );
                    }
                }
            }
            drop(guard);

            let _ = BitBlt(hdc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            let _ = EndPaint(control, &ps);
        }
    }

    /// The card's heading pair for whichever step is showing.
    ///
    /// The empty card's pair is [`super::empty_text`]'s rather than a literal
    /// here, so the words the card shows are the ones a test can read.
    fn headings() -> (String, &'static str) {
        match MODE.load(Ordering::SeqCst) {
            MODE_PALETTE => (
                "What should I type?".to_string(),
                "Pick a field. Nothing is typed until you do.",
            ),
            MODE_EMPTY => {
                let app = APP_NAME.lock().map(|n| n.clone()).unwrap_or_default();
                super::empty_text(&app)
            }
            MODE_SEARCH => {
                let (title, subtitle) = super::search_text();
                (title.to_string(), subtitle)
            }
            _ => (
                "Fill from vault".to_string(),
                "These accounts look like they belong to this app.",
            ),
        }
    }

    fn footer_label(id: usize) -> String {
        if id == ID_CANCEL {
            return "Cancel".to_string();
        }
        if MODE.load(Ordering::SeqCst) == MODE_PALETTE {
            "Edit binding".to_string()
        } else {
            "New login".to_string()
        }
    }

    /// One row, in whichever step is showing.
    ///
    /// Both steps go through [`crate::win32_draw::draw_row`] rather than one
    /// of them growing its own painter: the edge-to-edge highlight is the
    /// property that function exists to hold, and a second row painter is a
    /// second place for it to be got wrong.
    fn paint_row(hdc: HDC, rect: RECT, index: usize, state: RowState, fonts: &Fonts) {
        let dpi = DPI_PERCENT.load(Ordering::SeqCst);
        if MODE.load(Ordering::SeqCst) == MODE_SEARCH {
            let shown = SEARCH_SHOWN.lock().map(|s| s.clone()).unwrap_or_default();
            let total = SEARCH_TOTAL.load(Ordering::SeqCst).max(0) as usize;
            let Some(row) = super::search_rows(shown.len(), total).get(index).copied() else {
                return;
            };
            match row {
                SearchRow::Result(at) => {
                    let Some(candidate) = shown.get(at) else { return };
                    // **The same chip on the same rows.** A result is chosen by
                    // the same chord a candidate is -- see `chord` -- so it
                    // says so, from the same function.
                    let shortcut = super::row_shortcut(at);
                    let hint = shortcut.as_deref().map(|text| (text, fonts.hint));
                    draw_row(hdc, rect, candidate, state, fonts.name, fonts.username, hint, dpi);
                }
                // **No chip on either.** Neither is an account, and a number on
                // one would be a trap -- the same rule the *Search the vault*
                // row is drawn under.
                SearchRow::Overflow { .. } | SearchRow::Nothing => {
                    let (name, says) = super::search_row_text(row);
                    let row =
                        Candidate { id: String::new(), name, username: says };
                    draw_row(hdc, rect, &row, state, fonts.name, fonts.username, None, dpi);
                }
            }
            return;
        }
        if MODE.load(Ordering::SeqCst) == MODE_EMPTY {
            let Some(action) = super::empty_rows().get(index).copied() else {
                return;
            };
            let (name, says) = super::empty_label(action);
            let row =
                Candidate { id: String::new(), name: name.to_string(), username: says.to_string() };
            // The empty card's *New login* answers `NEW_LOGIN_SHORTCUT` too --
            // see `new_login_control` -- so it says so.
            let hint =
                (action == EmptyAction::NewLogin).then_some((NEW_LOGIN_SHORTCUT, fonts.hint));
            draw_row(hdc, rect, &row, state, fonts.name, fonts.username, hint, dpi);
            return;
        }
        if MODE.load(Ordering::SeqCst) == MODE_PALETTE {
            let Some(send) = ENTRIES.lock().ok().and_then(|e| e.get(index).cloned()) else {
                return;
            };
            let (name, says) = super::send_label(&send);
            let row = Candidate { id: String::new(), name, username: says.to_string() };
            // No hint: the second step's rows are not numbered -- see `chord`.
            draw_row(hdc, rect, &row, state, fonts.name, fonts.username, None, dpi);
            return;
        }

        let shown = SHOWN.lock().map(|s| s.clone()).unwrap_or_default();
        if let Some(candidate) = shown.get(index) {
            // **The shortcut is drawn on the row it runs.** A shortcut nobody
            // can see is a shortcut nobody uses, and this chip is the only
            // place the card says the digits exist.
            let shortcut = super::row_shortcut(index);
            let hint = shortcut.as_deref().map(|text| (text, fonts.hint));
            draw_row(hdc, rect, candidate, state, fonts.name, fonts.username, hint, dpi);
            // The gutter `draw_row` deliberately leaves blank.
            if let Ok(icons) = ICONS.lock() {
                if let Some(Some(icon)) = icons.get(index) {
                    let gutter = RECT {
                        left: rect.left,
                        top: rect.top,
                        right: rect.left + (rect.bottom - rect.top),
                        bottom: rect.bottom,
                    };
                    draw_icon(hdc, gutter, icon);
                }
            }
            return;
        }
        // The *Search the vault* row, drawn under every populated card's
        // candidates. Its second line is the only place a truncated list is
        // told it was truncated -- see `super::search_row_label`.
        let (name, says) = super::search_row_label(TRUNCATED.load(Ordering::SeqCst));
        let row = Candidate {
            id: String::new(),
            name: name.to_string(),
            username: says.to_string(),
        };
        // **No chip, because no digit reaches this row.** A number on it would
        // be a trap: it is the one row that is not an account.
        draw_row(hdc, rect, &row, state, fonts.name, fonts.username, None, dpi);
    }

    /// The brand lockup, through [`crate::win32_draw::draw_card_lockup`] --
    /// the crate's one mark painter, which `unlock_prompt` also draws through.
    /// What is this card's own is only the logical-to-device conversion, which
    /// no other card's `Box2` type can share.
    fn paint_lockup(hdc: HDC, l: &super::Layout, font: HFONT) {
        let dev = |b: Box2| RECT {
            left: scale(b.x),
            top: scale(b.y),
            right: scale(b.right()),
            bottom: scale(b.bottom()),
        };
        let tracking = scale(crate::win32_draw::card_lockup().tracking);
        crate::win32_draw::draw_card_lockup(hdc, dev(l.mark), dev(l.wordmark), font, tracking);
    }

    /// The header's close glyph, drawn as two strokes because no bundled face
    /// has it at this weight.
    fn paint_close_glyph(hdc: HDC, at: Box2) {
        unsafe {
            use windows::Win32::Graphics::Gdi::{LineTo, MoveToEx};
            let pen = CreatePen(PS_SOLID, scale(1).max(1), rgb(crate::theme::TEXT_FAINT));
            let old = SelectObject(hdc, pen);
            let (x, y, w, h) = (scale(at.x), scale(at.y), scale(at.w), scale(at.h));
            let pad = w / 3;
            let _ = MoveToEx(hdc, x + pad, y + pad, None);
            let _ = LineTo(hdc, x + w - pad, y + h - pad);
            let _ = MoveToEx(hdc, x + w - pad, y + pad, None);
            let _ = LineTo(hdc, x + pad, y + h - pad);
            SelectObject(hdc, old);
            let _ = DeleteObject(pen);
        }
    }

    fn fill(hdc: HDC, rc: RECT, colour: eframe::egui::Color32) {
        unsafe {
            let brush = CreateSolidBrush(rgb(colour));
            FillRect(hdc, &rc, brush);
            let _ = DeleteObject(brush);
        }
    }

    /// A rounded rectangle in logical coordinates, optionally stroked.
    fn rounded(
        hdc: HDC,
        at: Box2,
        radius: i32,
        fill_colour: eframe::egui::Color32,
        border: Option<(i32, eframe::egui::Color32)>,
    ) {
        unsafe {
            let brush = CreateSolidBrush(rgb(fill_colour));
            let (width, colour) = border.unwrap_or((1, fill_colour));
            let pen = CreatePen(PS_SOLID, scale(width).max(1), rgb(colour));
            let old_brush = SelectObject(hdc, brush);
            let old_pen = SelectObject(hdc, pen);
            let r = scale(radius) * 2;
            let _ =
                RoundRect(hdc, scale(at.x), scale(at.y), scale(at.right()), scale(at.bottom()), r, r);
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(brush);
            let _ = DeleteObject(pen);
        }
    }

    /// One run of text, left-aligned and vertically centred in `at`.
    fn text(hdc: HDC, font: HFONT, at: Box2, run: &str, colour: eframe::egui::Color32) {
        unsafe {
            let old = SelectObject(hdc, font);
            SetTextColor(hdc, rgb(colour));
            let mut rc = RECT {
                left: scale(at.x),
                top: scale(at.y),
                right: scale(at.right()),
                bottom: scale(at.bottom()),
            };
            let mut chars: Vec<u16> = run.encode_utf16().collect();
            // `DT_NOPREFIX`: these are the app's own words, and an `&` in one
            // of them is an ampersand rather than a mnemonic.
            DrawTextW(hdc, &mut chars, &mut rc, DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
            SelectObject(hdc, old);
        }
    }
}

/// **The card's own content decisions**, which are the ones that can be made
/// without a window: which rows the second step offers, what each is called,
/// and what a log line is allowed to say about one.
#[cfg(test)]
mod card_tests {
    use super::*;

    fn palette(fields: Vec<FieldRef>, has_sequence: bool) -> Palette {
        Palette { fields, has_sequence }
    }

    #[test]
    fn an_item_with_a_stored_sequence_offers_that_and_nothing_else() {
        let rows = palette_rows(&palette(vec![FieldRef::Username, FieldRef::Password], true));
        assert_eq!(
            rows,
            vec![Send::Sequence],
            "the user wrote the sequence precisely because the generic rows were not what that \
             app wanted, so offering them back is offering the thing they rejected -- the same \
             decision `app::fill_choices` makes"
        );
    }

    #[test]
    fn both_credentials_put_the_pair_first_and_then_each_alone() {
        assert_eq!(
            palette_rows(&palette(vec![FieldRef::Username, FieldRef::Password], false)),
            vec![
                Send::All,
                Send::Field(FieldRef::Username),
                Send::Field(FieldRef::Password),
            ]
        );
    }

    #[test]
    fn one_credential_alone_is_not_offered_as_a_pair() {
        assert_eq!(
            palette_rows(&palette(vec![FieldRef::Password], false)),
            vec![Send::Field(FieldRef::Password)],
            "`Send::All` types a Tab between two values; offered for an item that has only one, \
             it would type the password into whatever field followed the empty username"
        );
    }

    /// **Two wrong guesses must still have a way out.**
    ///
    /// Reported from use: "Fill from vault -- no search -- it shows two
    /// options, both are miss, what do I do?". The matcher is loose on
    /// purpose, so a short list of wrong guesses is an ordinary state, and the
    /// card cannot be the one surface with no route into the vault's search.
    #[test]
    fn a_short_list_of_wrong_guesses_still_offers_the_search() {
        let rows = populated_rows(2);
        assert_eq!(
            rows,
            vec![
                ListRow::Candidate(0),
                ListRow::Candidate(1),
                ListRow::SearchVault { truncated: false },
            ],
            "a two-candidate card offered no *Search the vault* row, so a user whose two offers              are both wrong can only dismiss the card"
        );
        // And the row tells the truth about why it is there: nothing was cut.
        assert_eq!(
            search_row_label(false).1,
            empty_label(EmptyAction::SearchVault).1,
            "the same action says two different things on the two cards"
        );
    }

    /// **The truncation news survives the row becoming unconditional.**
    ///
    /// A cap that hides candidates without saying so is the defect this rule
    /// exists to prevent. The row now does both jobs -- route and notice --
    /// because the card has room for one row, not two.
    #[test]
    fn an_overflowing_list_still_says_it_was_cut_and_still_reaches_search() {
        let rows = populated_rows(9);
        assert_eq!(rows.len(), LIST_ROWS, "the card has room for exactly {LIST_ROWS} rows");
        assert_eq!(
            rows.last(),
            Some(&ListRow::SearchVault { truncated: true }),
            "the truncated card lost its route into the vault"
        );
        assert_eq!(
            rows.iter().filter(|r| matches!(r, ListRow::Candidate(_))).count(),
            ROW_CAP,
            "the search row has a slot of its own -- the card is {LIST_ROWS} rows tall for it -- \
             so a truncated list still shows the full {ROW_CAP} candidates"
        );
        assert_eq!(
            search_row_label(true).1,
            "More accounts match than fit on this card",
            "the row is the only place the truncation is told now"
        );
    }

    /// **A list of exactly the cap is shown whole, and is not a truncation.**
    ///
    /// The regression this pin exists for: making the *Search the vault* row
    /// permanent was right, but it took one of `ROW_CAP`'s slots, so a user
    /// with exactly five matches saw four of them and was told the card had
    /// cut the list. Nothing had been cut. The row is additional to the
    /// candidates now -- the card is [`LIST_ROWS`] rows tall -- so the cap
    /// means what it says.
    #[test]
    fn exactly_the_cap_shows_every_candidate_and_reports_no_truncation() {
        let rows = populated_rows(ROW_CAP);
        assert_eq!(
            rows.iter().filter(|r| matches!(r, ListRow::Candidate(_))).count(),
            ROW_CAP,
            "{ROW_CAP} candidates fit a card whose cap is {ROW_CAP}, and one of them was dropped"
        );
        assert_eq!(
            rows.last(),
            Some(&ListRow::SearchVault { truncated: false }),
            "nothing was cut, so the card must not say it was -- and it must still offer the \
             route out of a wrong guess"
        );
        assert_eq!(rows.len(), LIST_ROWS);

        // And the boundary above it still is one: the cap is a real cap.
        let over = populated_rows(ROW_CAP + 1);
        assert_eq!(
            over.last(),
            Some(&ListRow::SearchVault { truncated: true }),
            "one more candidate than fits is a truncation, and a card that hid it silently is \
             the defect this whole rule exists to prevent"
        );
        assert_eq!(over.len(), LIST_ROWS, "and the card does not grow to swallow it");
    }

    /// Every row the populated card plans is one the window has a control for
    /// and the layout has a rectangle for.
    #[test]
    fn the_populated_cards_rows_all_fit_the_card() {
        for candidates in 0..12 {
            let rows = populated_rows(candidates);
            assert!(
                rows.len() <= LIST_ROWS,
                "{candidates} candidates planned {} rows onto a card with room for {LIST_ROWS}",
                rows.len()
            );
            let last = row_at(LIST_ROWS, rows.len() - 1);
            assert!(last.bottom() <= layout(LIST_ROWS).list.bottom());
        }
    }

    /// **The bound that makes a card with no scrolling honest.**
    ///
    /// `field_palette` is unbounded -- an item may carry any number of custom
    /// fields -- and this card cannot grow or scroll, so a row past
    /// [`ROW_CAP`] is one the user can neither see nor reach. The rows are
    /// therefore built from the three fields that are bounded, plus at most a
    /// pair row, which is four; the custom fields are covered by the sequence
    /// builder, exactly as `app::fill_choices` records.
    #[test]
    fn a_wall_of_custom_fields_cannot_push_a_row_off_the_card() {
        let customs: Vec<FieldRef> = (0..40)
            .map(|i| FieldRef::Custom(format!("field {i}")))
            .chain([FieldRef::Username, FieldRef::Password, FieldRef::Totp])
            .collect();
        let rows = palette_rows(&palette(customs, false));
        assert!(
            rows.len() <= LIST_ROWS,
            "the second step offered {} rows onto a card with room for {LIST_ROWS}, and this \
             card does not scroll -- the rest would simply be unreachable",
            rows.len()
        );
        let last = row_at(LIST_ROWS, rows.len() - 1);
        assert!(
            last.bottom() <= layout(LIST_ROWS).list.bottom(),
            "the second step's last row is outside the list area it shares with the first"
        );
        assert_eq!(
            rows,
            vec![
                Send::All,
                Send::Field(FieldRef::Username),
                Send::Field(FieldRef::Password),
                Send::Field(FieldRef::Totp),
            ]
        );
    }

    /// The label on screen defers to `FieldRef::label`, so a field renamed
    /// there cannot end up named two different things in two parts of this UI.
    #[test]
    fn a_field_is_called_on_screen_what_the_rest_of_the_app_calls_it() {
        let (label, _) = send_label(&Send::Field(FieldRef::Totp));
        assert_eq!(label, FieldRef::Totp.label());
        let (custom, _) = send_label(&Send::Field(FieldRef::Custom("Recovery PIN".to_string())));
        assert_eq!(custom, "Recovery PIN", "on the button the user is looking at, the name is right");
    }

    /// **And the log line is not the button.**
    ///
    /// The same value described for a diagnostic never spells the custom
    /// field's name. See [`describe_send`] for why the crate's one precedent
    /// for spelling it -- a refusal shown to that user about their own item --
    /// does not extend to a line written into a file on disk.
    #[test]
    fn a_log_line_never_spells_a_custom_fields_name() {
        assert_eq!(
            describe_send(&Send::Field(FieldRef::Custom("Recovery PIN".to_string()))),
            "a custom field"
        );
        // Controls: the built-in fields ARE named, so the line above is a
        // deliberate omission rather than a function that says nothing useful.
        assert_eq!(describe_send(&Send::Field(FieldRef::Password)), "the password");
        assert_eq!(describe_send(&Send::All), "username, Tab and password");
    }

    /// **Every control the card lays out is inside the window it lays out.**
    ///
    /// The card neither scrolls nor resizes, so a control whose bottom edge
    /// fell past the window's would simply be invisible -- and the last row is
    /// the one that would go first.
    #[test]
    fn nothing_the_card_lays_out_falls_off_the_bottom_of_it() {
        let l = layout(LIST_ROWS);

        // **The brand lockup**, which the port had dropped entirely and which
        // this card now carries again. Pinned to the new truth rather than
        // loosened: the card grew by the lockup's height plus its gap, and the
        // window's own height assertions below are what hold that honest.
        let lockup = crate::win32_draw::card_lockup();
        assert_eq!(
            (l.mark.x, l.mark.y),
            (MARGIN_X, MARGIN_TOP),
            "the lockup does not start at the card's own top-left inset"
        );
        assert_eq!(l.mark.h, lockup.mark_h);
        assert_eq!(
            l.mark.w,
            crate::win32_draw::mark_width(l.mark.h),
            "the mark's box is not the design artboard's ratio, so the shield would be              letterboxed inside it and drift away from the word beside it"
        );
        assert!(l.mark.right() < l.wordmark.x, "the wordmark is drawn over the shield");
        assert_eq!(l.wordmark.h, l.mark.h, "the lockup's two halves are different heights");
        assert!(
            l.wordmark.right() <= l.close_glyph.x,
            "the wordmark runs under the ✕"
        );
        assert!(
            l.wordmark.bottom() <= l.title.y,
            "the card's title runs into the brand lockup above it"
        );
        assert!(
            l.close_glyph.right() <= l.window.right() - MARGIN_X,
            "the close glyph has crossed the card's right margin"
        );

        assert!(l.subtitle.bottom() <= l.list.y);
        assert!(l.list.bottom() <= l.cancel.y);
        // **Against the MARGIN, not against the window's edge.** The card's
        // rule is `MARGIN_X` either side and `MARGIN_TOP` under the footer; a
        // pin that only forbade a control leaving the window is 16 px slacker
        // than the layout it guards, and would have watched `CANCEL_W` grow
        // from 84 to 104 without a word. What is asserted is the rule.
        assert!(
            l.cancel.bottom() + MARGIN_TOP <= l.window.bottom(),
            "the footer has eaten the card's bottom margin"
        );
        assert!(l.secondary.right() < l.cancel.x, "the two footer buttons overlap");
        assert!(
            l.secondary.x >= MARGIN_X,
            "the footer pair has outgrown the card's left margin: it starts at {} px, inside \
             MARGIN_X of {MARGIN_X}",
            l.secondary.x
        );
        let last = row_at(LIST_ROWS, LIST_ROWS - 1);
        assert!(
            last.bottom() <= l.list.bottom(),
            "the last row is outside the list area, and this card cannot scroll to it"
        );
        assert!(
            l.close_glyph.right() <= l.window.right() - MARGIN_X,
            "the close glyph has crossed the card's right margin"
        );
        // The bottom row of a full card is the *Search the vault* row, and it
        // is the one that goes first if `LIST_ROWS` and `ROW_CAP` ever drift
        // apart again.
        assert_eq!(
            populated_rows(ROW_CAP).len(),
            LIST_ROWS,
            "a full card plans {} rows onto {LIST_ROWS} laid-out slots",
            populated_rows(ROW_CAP).len()
        );
    }

    /// **The empty card is exactly as tall as the offers it has.**
    ///
    /// Not the same claim as
    /// [`nothing_the_card_lays_out_falls_off_the_bottom_of_it`] and
    /// [`a_wall_of_custom_fields_cannot_push_a_row_off_the_card`], which bound
    /// the last row from above: two rows trivially fit a card with room for
    /// five, and a test that asserted only that let a card sized for five rows
    /// it does not have ship with 132 px of bare `theme::CARD` under its last
    /// offer. What is asserted here is the other direction -- that the card
    /// asks the OS for the height its own content needs, and that no dead band
    /// is left between the last offer and the Cancel button.
    ///
    /// `MODE_EMPTY` is allowed to do this and the other two steps are not:
    /// see [`layout`]. It is decided once in `open` and never transitions, so
    /// there is no live window whose Cancel button could move.
    #[test]
    fn the_empty_card_is_no_taller_than_the_offers_it_has() {
        let rows = empty_rows();
        assert_eq!(
            rows,
            vec![EmptyAction::NewLogin, EmptyAction::SearchVault],
            "3a offered exactly these two, in this order, and the card that replaced it offers \
             the same two under the same two names"
        );
        assert!(
            rows.len() <= LIST_ROWS,
            "the empty card offered {} rows onto a card with room for {LIST_ROWS}, and this card \
             does not scroll -- the rest would simply be unreachable",
            rows.len()
        );

        let l = layout(rows.len());
        let last = row_at(rows.len(), rows.len() - 1);
        assert!(last.bottom() <= l.list.bottom());
        assert_eq!(
            last.bottom(),
            l.list.bottom(),
            "the empty card's list is drawn taller than its own offers: {} px of bare \
             `theme::CARD` sits under the last one. The card cannot scroll and has no rows to \
             fill that band, so it reads as a list that lost its contents",
            l.list.bottom() - last.bottom()
        );

        // The window follows the list, so the dead band is not merely pushed
        // out of the card and into the gap above Cancel.
        let full = layout(LIST_ROWS);
        let needed = full.window.h - ROW_H * (LIST_ROWS - rows.len()) as i32;
        assert_eq!(
            l.window.h,
            needed,
            "the empty card asks the OS for a {} px window when its own {} offers need {needed} \
             px. A card sized for rows it does not have is {} px of bare `theme::CARD` between \
             *Search vault* and *Cancel* -- and the fixed height buys nothing here, because \
             `MODE_EMPTY` never transitions and so has no Cancel button that could move out \
             from under the pointer",
            l.window.h,
            rows.len(),
            l.window.h - needed
        );
        assert_eq!(
            l.cancel.y - l.list.bottom(),
            full.cancel.y - full.list.bottom(),
            "the empty card's footer does not sit the same distance below its list as the \
             populated card's does"
        );
    }

    /// **Search mode's cap is a cap, and it says so.**
    ///
    /// The card neither scrolls nor resizes, so a result past the last slot is
    /// one the user can neither see nor reach -- and a cap that hides matches
    /// without saying so is the defect this project keeps finding. The overflow
    /// row is where it is said, and it names the number.
    #[test]
    fn a_capped_search_says_how_many_it_is_not_showing() {
        let rows = search_rows(SEARCH_CAP, 42);
        assert_eq!(rows.len(), LIST_ROWS, "the card has room for exactly {LIST_ROWS} rows");
        assert_eq!(
            rows.last(),
            Some(&SearchRow::Overflow { shown: SEARCH_CAP, total: 42 }),
            "the capped search lost the row that says it was capped"
        );
        let (name, says) = search_row_text(SearchRow::Overflow { shown: SEARCH_CAP, total: 42 });
        assert!(
            name.contains("42"),
            "the overflow row does not name how many matched, so the user cannot tell a cap \
             from an answer: {name:?}"
        );
        assert!(
            says.contains(&SEARCH_CAP.to_string()),
            "the overflow row does not say how many of them are on screen: {says:?}"
        );

        // And the boundary below it is one: exactly the cap is the whole
        // answer, and must not claim otherwise.
        let exact = search_rows(SEARCH_CAP, SEARCH_CAP);
        assert_eq!(
            exact.len(),
            SEARCH_CAP,
            "a search that matched exactly the cap reported a truncation that did not happen -- \
             the same off-by-one the *Search the vault* row was fixed for"
        );
        assert!(exact.iter().all(|row| matches!(row, SearchRow::Result(_))));
    }

    /// **Nothing matching is a row, not an empty card.**
    ///
    /// This card does not scroll and has no other content in search mode, so a
    /// list drawn with no rows in it is indistinguishable from a card that has
    /// stopped answering.
    #[test]
    fn a_search_that_matches_nothing_says_so() {
        assert_eq!(search_rows(0, 0), vec![SearchRow::Nothing]);
        let (name, says) = search_row_text(SearchRow::Nothing);
        assert!(!name.is_empty() && !says.is_empty(), "the empty-result row says nothing at all");
    }

    /// Every row search mode plans is one the window has a control for and the
    /// layout has a rectangle for, at every result count.
    #[test]
    fn the_search_cards_rows_all_fit_the_card() {
        for shown in 0..=SEARCH_CAP {
            for total in [shown, shown + 1, shown + 500] {
                let rows = search_rows(shown, total);
                assert!(
                    rows.len() <= LIST_ROWS,
                    "{shown} of {total} results planned {} rows onto a card with room for \
                     {LIST_ROWS}",
                    rows.len()
                );
                let last = row_at_for(LIST_ROWS, true, rows.len() - 1);
                assert!(last.bottom() <= layout_for(LIST_ROWS, true).list.bottom());
            }
        }
    }

    /// **The search card's own geometry**, held to the same rule as the list's.
    ///
    /// The mode adds a control between the subtitle and the list, and this card
    /// neither scrolls nor resizes to reach anything that fell off the bottom
    /// because of it.
    #[test]
    fn nothing_the_search_card_lays_out_falls_off_the_bottom_of_it() {
        let l = layout_for(LIST_ROWS, true);
        let field = l.search.expect("the search shape has a search box");
        assert!(l.subtitle.bottom() <= field.y, "the search box overlaps the subtitle");
        assert!(field.bottom() <= l.list.y, "the search box overlaps the list");
        assert!(field.x >= MARGIN_X && field.right() <= l.window.right() - MARGIN_X);
        assert!(l.list.bottom() <= l.cancel.y);
        assert!(
            l.cancel.bottom() + MARGIN_TOP <= l.window.bottom(),
            "the footer has eaten the search card's bottom margin"
        );
        assert!(l.secondary.right() < l.cancel.x, "the two footer buttons overlap");
        let last = row_at_for(LIST_ROWS, true, LIST_ROWS - 1);
        assert!(
            last.bottom() <= l.list.bottom(),
            "the search card's last row is outside its list area, and this card cannot scroll"
        );

        // The mode costs exactly the box and its two gaps, and nothing else
        // moved: a card that gained height for some other reason is a card
        // whose layout has drifted from the list's.
        let list = layout(LIST_ROWS);
        assert_eq!(
            l.window.h - list.window.h,
            SEARCH_H + SEARCH_GAP,
            "search mode changed the card's height by something other than its own box"
        );
        assert_eq!(l.title, list.title);
        assert_eq!(l.subtitle, list.subtitle);
        assert_eq!(
            list.search, None,
            "the list mode is laying out a search box it does not show, which is a control the \
             user could tab into and a border the card would paint"
        );
    }

    /// The card's dimensions are `theme`'s, and the search box is no exception.
    #[test]
    fn the_search_boxs_height_is_the_themes() {
        assert_eq!(
            SEARCH_H as f32,
            crate::theme::SEARCH_FIELD_HEIGHT,
            "the card's search box has grown a height of its own, so a redesign of design 2b's \
             search box would leave this one behind"
        );
        assert_eq!(BUTTON_H as f32, crate::theme::BUTTON_HEIGHT);
    }

    /// Search mode says what it is and what to do.
    #[test]
    fn the_search_card_says_what_it_is() {
        let (title, subtitle) = search_text();
        assert!(!title.is_empty() && !subtitle.is_empty());
        assert!(
            subtitle.to_lowercase().contains("type"),
            "the search card does not tell the user to type, which is the one thing its focused \
             box is for: {subtitle:?}"
        );
    }

    /// The card says which app it has nothing for. A card that said only "no
    /// saved login" would be a card the user cannot tell apart from a card
    /// about some other window.
    #[test]
    fn the_empty_card_names_the_app_it_has_nothing_for() {
        let (title, subtitle) = empty_text("Ledgerline.exe");
        assert!(
            title.contains("Ledgerline.exe"),
            "the empty card's heading does not name the app it is about: {title:?}"
        );
        assert!(
            subtitle.contains("process name and title"),
            "3a's second line is the fact that makes *Search vault* worth offering rather than a \
             shrug, and it is gone: {subtitle:?}"
        );
    }

    /// Every offer says what it does, and neither is a bare word.
    #[test]
    fn each_empty_offer_says_what_it_will_do() {
        for action in empty_rows() {
            let (name, says) = empty_label(action);
            assert!(!name.is_empty() && !says.is_empty(), "{action:?} has an empty label");
        }
        assert_eq!(empty_label(EmptyAction::NewLogin).0, "New login");
        assert_eq!(empty_label(EmptyAction::SearchVault).0, "Search vault");
    }

    /// **This window may not post a thread quit.**
    ///
    /// A source pin, because no test can open the real window: the defect is
    /// a `WM_QUIT` left sitting in the daemon thread's queue *after* `next()`
    /// has already returned, and nothing this crate can drive in a test
    /// observes that queue.
    ///
    /// The shape is this crate's established one -- read the file, cut at the
    /// first column-0 `#[cfg(test)]`, scan the production half -- the same cut
    /// `job_object.rs`'s pins make, with a control assertion so a scan that
    /// read nothing cannot pass.
    #[test]
    fn the_picker_window_never_posts_a_thread_quit() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let raw =
            std::fs::read_to_string(src.join("picker_prompt.rs")).unwrap().replace("\r\n", "\n");
        let production = raw.split(concat!("\n#[cfg(", "test)]\n")).next().unwrap();
        // **Comments stripped, and the rule reads CODE.** The `WM_DESTROY` arm
        // carries a comment naming the very call this forbids -- that comment
        // is the reason the call is not there, and a scan that could not tell
        // the two apart would forbid explaining itself.
        let code: String = production
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");

        // CONTROL, so a pin that scanned nothing cannot pass: the cut must
        // have thrown something away, and the half it kept must be the half
        // that carries the window procedure this rule is about.
        assert!(
            production.len() < raw.len(),
            "control: the `#[cfg(test)]` cut marker was not found in picker_prompt.rs, so this \
             scan is reading the test module as production and the rule below is meaningless"
        );
        assert!(
            code.contains("WM_DESTROY =>"),
            "control: the production cut of picker_prompt.rs does not contain the window \
             procedure's WM_DESTROY arm, so the cut is in the wrong place and this pin is \
             scanning the wrong text"
        );
        assert!(
            code.contains("GONE.store(true, Ordering::SeqCst);"),
            "control: the comment stripper has eaten code -- the WM_DESTROY arm's one              surviving statement is not in the text this rule scans"
        );

        assert!(
            !code.contains(concat!("PostQuit", "Message")),
            "picker_prompt.rs's production half posts a thread quit. This window is opened on \
             the daemon thread, and that thread goes on to run egui windows: the design-3c \
             save-a-login form after *New login*, and the preflight host after *Password* / \
             *One-time code*. `close()` calls `DestroyWindow`, which dispatches WM_DESTROY \
             synchronously on that thread, and nothing drains the queue afterwards -- `next()` \
             has already returned. The next `eframe::run_native` takes the stale WM_QUIT out of \
             `GetMessageW`, leaves its loop before it draws, and returns its DEFAULT answer: \
             the save form never appears, and the preflight reports \"not confirmed\" so the \
             password the user picked is silently never typed. `GONE` is what `next()` reads; \
             quitting the thread is not this window's job."
        );
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::key_sequence::{FieldRef, Token};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A [`Searcher`] for the tests that never enter search mode.
    ///
    /// It **panics** rather than answering nothing: a test that expected to
    /// stay on the candidate list and quietly entered search mode instead
    /// would otherwise pass on an empty result set, which is exactly the
    /// silence these tests exist to catch.
    fn no_search(_: &str, _: usize) -> SearchResults {
        panic!("this card was not supposed to reach the vault search")
    }

    fn one(name: &str) -> Vec<Candidate> {
        vec![Candidate {
            id: "id-1".to_string(),
            name: name.to_string(),
            username: "me@example.com".to_string(),
        }]
    }

    #[test]
    fn all_types_username_tab_password_and_never_presses_enter() {
        let tokens = tokens_for(&Send::All, None);
        let tab = crate::key_sequence::key_named("TAB").expect("TAB is a known key");
        assert_eq!(
            tokens,
            vec![
                Token::Field(FieldRef::Username),
                Token::Key(tab),
                Token::Field(FieldRef::Password),
            ],
            "a trailing Enter submits, and if the target's field order differs from this \
             assumption it submits the wrong content -- typing without submitting fails \
             visibly, submitting fails invisibly"
        );
    }

    #[test]
    fn one_field_is_one_token_and_nothing_else() {
        assert_eq!(
            tokens_for(&Send::Field(FieldRef::Totp), None),
            vec![Token::Field(FieldRef::Totp)]
        );
    }

    #[test]
    fn the_sequence_choice_runs_the_items_own_sequence() {
        let tokens = tokens_for(&Send::Sequence, Some("{USERNAME}{TAB}{PASSWORD}{ENTER}"));
        assert_eq!(
            tokens,
            crate::key_sequence::parse("{USERNAME}{TAB}{PASSWORD}{ENTER}"),
            "the configured sequence goes through the existing parser, not a second \
             interpretation of the same string"
        );
    }

    #[test]
    fn choosing_a_row_then_a_field_answers_that_item_and_that_field() {
        let calls = PickerCalls {
            open: |_, _| Some(PickerWindow(1)),
            protect: |_| true,
            next: |_| {
                use std::sync::atomic::{AtomicUsize, Ordering};
                static STEP: AtomicUsize = AtomicUsize::new(0);
                match STEP.fetch_add(1, Ordering::SeqCst) {
                    0 => Event::Chose(0),
                    _ => Event::Sends(Send::Field(FieldRef::Password)),
                }
            },
            show_palette: |_, _| {},
            show_search: |_, _| {},
            close: |_| {},
        };
        let outcome = run_with(
            &calls,
            &one("Slack"),
            "Slack.exe",
            |_| Palette { fields: vec![FieldRef::Password], has_sequence: false },
            no_search,
        );
        assert_eq!(
            outcome,
            Outcome::Fill { id: "id-1".to_string(), send: Send::Field(FieldRef::Password) }
        );
    }

    /// **The empty card is a card, and its two offers answer.**
    ///
    /// This is the state design 3a's egui window existed for, and it is by
    /// far the most common one this hotkey lands in -- most windows have no
    /// saved login at all. Nothing about it may be a silent nothing: the card
    /// goes up (`open` is called), it is protected, and each of its two rows
    /// produces the `Outcome` that maps onto the `NoMatchFollowUp` 3a
    /// produced.
    #[test]
    fn an_empty_card_still_opens_and_still_answers() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static OPENED_EMPTY: AtomicUsize = AtomicUsize::new(usize::MAX);
        fn calls(next: fn(PickerWindow) -> Event) -> PickerCalls {
            PickerCalls {
                open: |candidates, app_name| {
                    OPENED_EMPTY.store(candidates.len(), Ordering::SeqCst);
                    assert_eq!(
                        app_name, "Ledgerline.exe",
                        "the card is told which app it is about"
                    );
                    Some(PickerWindow(1))
                },
                protect: |_| true,
                next,
                show_palette: |_, _| {},
                show_search: |_, _| {},
                close: |_| {},
            }
        }
        fn empty(_: &str) -> Palette {
            Palette { fields: vec![], has_sequence: false }
        }

        assert_eq!(
            run_with(&calls(|_| Event::NewLogin), &[], "Ledgerline.exe", empty, no_search),
            Outcome::NewLogin,
            "*New login* on the empty card is 3a's own button, and it must still lead somewhere"
        );
        assert_eq!(
            OPENED_EMPTY.load(Ordering::SeqCst),
            0,
            "an empty offer list must still put a card on screen -- there is no other surface \
             for it now that the egui no-match window is gone"
        );
        assert_eq!(
            run_with(&calls(|_| Event::Cancel), &[], "Ledgerline.exe", empty, no_search),
            Outcome::Cancelled,
            "and dismissing it is still dismissing it"
        );

        // **And *Search vault* stays on this card.** It used to answer
        // `Outcome::SearchVault`, which `main` spent the ~100 MB egui vault
        // window on. It now reaches the vault through the searcher seam and
        // shows the answer here: the card is never closed for it, so the only
        // way this loop ends is the Cancel that follows.
        static SEARCHED: AtomicUsize = AtomicUsize::new(0);
        static SEARCH_STEP: AtomicUsize = AtomicUsize::new(0);
        SEARCHED.store(0, Ordering::SeqCst);
        SEARCH_STEP.store(0, Ordering::SeqCst);
        let outcome = run_with(
            &calls(|_| match SEARCH_STEP.fetch_add(1, Ordering::SeqCst) {
                0 => Event::Search,
                _ => Event::Cancel,
            }),
            &[],
            "Ledgerline.exe",
            empty,
            |query, cap| {
                SEARCHED.fetch_add(1, Ordering::SeqCst);
                assert_eq!(query, "", "search mode opens on the unfiltered vault");
                assert_eq!(cap, SEARCH_CAP);
                SearchResults::default()
            },
        );
        assert_eq!(outcome, Outcome::Cancelled);
        assert_eq!(
            SEARCHED.load(Ordering::SeqCst),
            1,
            "*Search vault* did not reach the vault at all. It is the empty card's only route \
             to a login saved under another name, and the whole point of answering it here is \
             that it must not open the ~100 MB vault window to do it"
        );
    }

    /// A row choice on a card with no candidates cannot invent an account.
    ///
    /// `Event::Chose` is not something the empty card's window procedure
    /// posts -- its rows post `NewLogin` and `Overflow` -- but `run_with` is
    /// the layer that must not trust that, because the alternative to
    /// ignoring it is filling from an item the user never picked.
    #[test]
    fn a_row_choice_on_an_empty_card_fills_nothing() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static STEP: AtomicUsize = AtomicUsize::new(0);
        let calls = PickerCalls {
            open: |_, _| Some(PickerWindow(1)),
            protect: |_| true,
            next: |_| match STEP.fetch_add(1, Ordering::SeqCst) {
                0 => Event::Chose(0),
                _ => Event::Cancel,
            },
            show_palette: |_, _| panic!("there is no candidate to show a palette for"),
            show_search: |_, _| {},
            close: |_| {},
        };
        assert_eq!(
            run_with(
                &calls,
                &[],
                "Ledgerline.exe",
                |_| Palette { fields: vec![], has_sequence: false },
                no_search,
            ),
            Outcome::Cancelled
        );
    }


    // -----------------------------------------------------------------------
    // Search mode: the same card, the same rows, the same dispatch.
    // -----------------------------------------------------------------------

    fn found(id: &str, name: &str) -> Offer {
        Offer {
            candidate: Candidate {
                id: id.to_string(),
                name: name.to_string(),
                username: "ada@example.com".to_string(),
            },
            palette: Palette { fields: vec![FieldRef::Password], has_sequence: false },
            icon: None,
        }
    }

    /// **The whole point of the feature, end to end.**
    ///
    /// Asking to search does not close the card and does not answer an
    /// outcome; it filters, the rows come back, and picking one goes through
    /// the *same* `show_palette` step the candidate list leads to and produces
    /// the same `Outcome::Fill`. One dispatch path, which is why the mode lives
    /// on this card at all.
    #[test]
    fn a_searched_result_fills_through_the_same_step_a_candidate_does() {
        static STEP: AtomicUsize = AtomicUsize::new(0);
        static CLOSED: AtomicUsize = AtomicUsize::new(0);
        static QUERIES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        static PALETTES: AtomicUsize = AtomicUsize::new(0);
        STEP.store(0, Ordering::SeqCst);
        CLOSED.store(0, Ordering::SeqCst);
        PALETTES.store(0, Ordering::SeqCst);
        QUERIES.lock().unwrap().clear();

        let calls = PickerCalls {
            open: |_, _| Some(PickerWindow(1)),
            protect: |_| true,
            next: |_| match STEP.fetch_add(1, Ordering::SeqCst) {
                // The *Search the vault* row.
                0 => Event::Search,
                // One keystroke.
                1 => Event::Typed("north".to_string()),
                // The second result row.
                2 => Event::Chose(1),
                _ => Event::Sends(Send::Field(FieldRef::Password)),
            },
            show_palette: |_, palette| {
                PALETTES.fetch_add(1, Ordering::SeqCst);
                assert_eq!(
                    palette.fields,
                    vec![FieldRef::Password],
                    "the palette shown is not the one the search result carried"
                );
            },
            show_search: |_, _| {},
            close: |_| {
                CLOSED.fetch_add(1, Ordering::SeqCst);
            },
        };
        let outcome = run_with(
            &calls,
            // The candidate list is deliberately NOT what is searched: a
            // result index that fell through to this slice would fill from
            // whatever happened to sit at it.
            &one("Slack"),
            "Slack.exe",
            |_| panic!("a search result carries its own palette; the candidate lookup is wrong"),
            |query, cap| {
                QUERIES.lock().unwrap().push(query.to_string());
                assert_eq!(cap, SEARCH_CAP);
                SearchResults {
                    offers: vec![
                        found("id-north-1", "Northwind VPN"),
                        found("id-north-2", "Northwind Payroll"),
                    ],
                    total: 2,
                }
            },
        );
        assert_eq!(
            outcome,
            Outcome::Fill {
                id: "id-north-2".to_string(),
                send: Send::Field(FieldRef::Password)
            },
            "the second search result did not fill from the account it named. A row index means \
             one thing in the candidate list and another in the results, and this is the \
             mistake that types one account's password into another's login form"
        );
        assert_eq!(
            *QUERIES.lock().unwrap(),
            vec!["".to_string(), "north".to_string()],
            "search mode must open on the unfiltered vault and refilter on the keystroke"
        );
        assert_eq!(PALETTES.load(Ordering::SeqCst), 1, "the *what should I type?* step was skipped");
        assert_eq!(CLOSED.load(Ordering::SeqCst), 1, "close runs once, on the way out");
    }

    /// **Asking to search closes nothing and answers no outcome.**
    ///
    /// The regression this guards is the one being removed: `Event::Search`
    /// used to answer `Outcome::SearchVault`, which `main` spent the ~100 MB
    /// egui vault window on -- to search a vault the daemon already holds in
    /// memory, from a card that costs ~2 MB.
    #[test]
    fn asking_to_search_does_not_take_the_card_down() {
        static STEP: AtomicUsize = AtomicUsize::new(0);
        static CLOSED_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
        static SHOWN_ROWS: AtomicUsize = AtomicUsize::new(usize::MAX);
        STEP.store(0, Ordering::SeqCst);
        CLOSED_AT.store(usize::MAX, Ordering::SeqCst);
        SHOWN_ROWS.store(usize::MAX, Ordering::SeqCst);

        let calls = PickerCalls {
            open: |_, _| Some(PickerWindow(1)),
            protect: |_| true,
            next: |_| match STEP.fetch_add(1, Ordering::SeqCst) {
                0 => Event::Search,
                _ => Event::Cancel,
            },
            show_palette: |_, _| panic!("nothing was chosen"),
            show_search: |_, results| {
                SHOWN_ROWS.store(results.offers.len(), Ordering::SeqCst);
                assert!(
                    CLOSED_AT.load(Ordering::SeqCst) == usize::MAX,
                    "the card was closed before it was asked to show search results"
                );
            },
            close: |_| {
                CLOSED_AT.store(STEP.load(Ordering::SeqCst), Ordering::SeqCst);
            },
        };
        let outcome = run_with(&calls, &three(), "Slack.exe", no_search_palette, |_, _| {
            SearchResults { offers: vec![found("id-1", "Slack")], total: 1 }
        });
        assert_eq!(
            outcome,
            Outcome::Cancelled,
            "the only way out of search mode is the user leaving the card"
        );
        assert_eq!(
            SHOWN_ROWS.load(Ordering::SeqCst),
            1,
            "the card was never handed the rows it asked for, so search mode showed nothing"
        );
    }

    /// A result index the search did not produce fills nothing.
    ///
    /// The same rule `a_row_choice_on_an_empty_card_fills_nothing` holds for
    /// the candidate list, and it matters more here: the results change on
    /// every keystroke, so a stale click is a real race rather than a
    /// hypothetical one.
    #[test]
    fn a_row_past_the_search_results_fills_nothing() {
        static STEP: AtomicUsize = AtomicUsize::new(0);
        STEP.store(0, Ordering::SeqCst);
        let calls = PickerCalls {
            open: |_, _| Some(PickerWindow(1)),
            protect: |_| true,
            next: |_| match STEP.fetch_add(1, Ordering::SeqCst) {
                0 => Event::Search,
                1 => Event::Chose(4),
                _ => Event::Cancel,
            },
            show_palette: |_, _| panic!("there is no fifth result to show a palette for"),
            show_search: |_, _| {},
            close: |_| {},
        };
        assert_eq!(
            run_with(&calls, &three(), "Slack.exe", no_search_palette, |_, _| SearchResults {
                offers: vec![found("id-1", "Slack")],
                total: 1,
            }),
            Outcome::Cancelled,
            "a row index past the results fell through to the candidate slice, which would fill \
             from an account the user never picked"
        );
    }

    /// The palette lookup for tests that must never reach it: in search mode
    /// the result carries its own, so a call here is a bug.
    fn no_search_palette(_: &str) -> Palette {
        panic!("a search result carries its own palette")
    }

    // -----------------------------------------------------------------------
    // The keyboard shortcuts.
    //
    // Driven through the `PickerCalls` seam, so no window is opened: `press`
    // is what the card's own `win32::chord` does with a digit -- it asks
    // `candidate_for_digit`, the one function that decides -- and the fake
    // `next` hands the resulting event to `run_with` exactly as the window
    // procedure would. What is asserted is therefore the whole path from a
    // keystroke to an `Outcome`, minus the pump.
    // -----------------------------------------------------------------------

    fn three() -> Vec<Candidate> {
        ["Slack", "Ledgerline", "Northwind VPN"]
            .iter()
            .enumerate()
            .map(|(i, name)| Candidate {
                id: format!("id-{}", i + 1),
                name: name.to_string(),
                username: format!("user{i}@example.com"),
            })
            .collect()
    }

    /// The event a digit produces on a card showing `shown` candidate rows,
    /// or `None` where the card does nothing at all.
    fn press(digit: u32, shown: usize) -> Option<Event> {
        candidate_for_digit(digit, shown).map(Event::Chose)
    }

    /// Digit *n* fills from the *n*th row **as drawn**, and 1 is the top one.
    #[test]
    fn a_digit_chooses_the_row_it_is_drawn_on() {
        for (digit, expected) in [(1u32, "id-1"), (2, "id-2"), (3, "id-3")] {
            static PRESSED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            static STEP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            PRESSED.store(digit, Ordering::SeqCst);
            STEP.store(0, Ordering::SeqCst);
            let calls = PickerCalls {
                open: |_, _| Some(PickerWindow(1)),
                protect: |_| true,
                next: |_| match STEP.fetch_add(1, Ordering::SeqCst) {
                    0 => press(PRESSED.load(Ordering::SeqCst), 3)
                        .expect("a digit within the list chooses a row"),
                    _ => Event::Sends(Send::Field(FieldRef::Password)),
                },
                show_palette: |_, _| {},
                show_search: |_, _| {},
                close: |_| {},
            };
            let outcome = run_with(
                &calls,
                &three(),
                "Slack.exe",
                |_| Palette { fields: vec![FieldRef::Password], has_sequence: false },
                no_search,
            );
            assert_eq!(
                outcome,
                Outcome::Fill {
                    id: expected.to_string(),
                    send: Send::Field(FieldRef::Password)
                },
                "CTRL+ALT+{digit} filled from the wrong account -- the numbering the user sees is \
                 the rows as drawn, and an off-by-one here types one account's password into \
                 another's login form"
            );
        }
    }

    /// **A digit past the rows on screen does nothing.** Not a beep, not a
    /// dismissal, and above all not a fill: the card is showing three
    /// accounts, so there is no fourth for `CTRL+ALT+4` to mean.
    #[test]
    fn a_digit_past_the_shown_rows_chooses_nothing() {
        for digit in 4..=9 {
            assert_eq!(
                press(digit, 3),
                None,
                "CTRL+ALT+{digit} answered something on a card showing three accounts"
            );
        }
        // And the card stays up: `run_with` is never handed an event at all,
        // so the next thing it sees is whatever the user does next.
        static STEP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        STEP.store(0, Ordering::SeqCst);
        let calls = PickerCalls {
            open: |_, _| Some(PickerWindow(1)),
            protect: |_| true,
            next: |_| {
                assert!(
                    press(7, 3).is_none(),
                    "the pump would have posted an event for a digit with no row"
                );
                assert_eq!(STEP.fetch_add(1, Ordering::SeqCst), 0, "the card was pumped twice");
                Event::Cancel
            },
            show_palette: |_, _| panic!("nothing was chosen, so nothing may be offered"),
            show_search: |_, _| {},
            close: |_| {},
        };
        assert_eq!(
            run_with(
                &calls,
                &three(),
                "Slack.exe",
                |_| Palette { fields: vec![], has_sequence: false },
                no_search,
            ),
            Outcome::Cancelled
        );
    }

    /// **No digit reaches the *Search the vault* row.**
    ///
    /// It is the one row on the card that is not an account, and a number on
    /// it would be a trap: the user counting rows down the card would press
    /// the digit under their eye and get the vault window instead of a fill.
    /// The count digits are measured against is the CANDIDATES', so the row
    /// below them is unreachable by construction, at every list length.
    #[test]
    fn no_digit_can_land_on_the_search_row() {
        for candidates in 0..=ROW_CAP + 3 {
            let (shown, _) = crate::win32_draw::visible_rows(candidates, ROW_CAP);
            let rows = populated_rows(candidates);
            let search = rows
                .iter()
                .position(|row| matches!(row, ListRow::SearchVault { .. }))
                .expect("every populated card has the row");
            for digit in 1..=9u32 {
                let chosen = candidate_for_digit(digit, shown);
                assert_ne!(
                    chosen,
                    Some(search),
                    "with {candidates} candidates, CTRL+ALT+{digit} lands on the *Search the \
                     vault* row"
                );
                if let Some(index) = chosen {
                    assert!(
                        matches!(rows.get(index), Some(ListRow::Candidate(_))),
                        "CTRL+ALT+{digit} chose row {index}, which is not a candidate row"
                    );
                }
            }
        }
    }

    /// The digits are on screen, and they say what they do.
    ///
    /// A shortcut nobody can see is a shortcut nobody uses. Every candidate
    /// row the card can draw carries its own chip, they are all distinct, and
    /// the row past the candidates carries none -- which is the drawn half of
    /// [`no_digit_can_land_on_the_search_row`].
    #[test]
    fn every_numbered_row_says_which_chord_runs_it() {
        let hints: Vec<String> = (0..ROW_CAP).map(|i| row_shortcut(i).expect("numbered")).collect();
        assert_eq!(hints[0], "CTRL+ALT+1", "the topmost row as drawn is 1");
        assert_eq!(hints.last().map(String::as_str), Some("CTRL+ALT+5"));
        let unique: std::collections::BTreeSet<&String> = hints.iter().collect();
        assert_eq!(unique.len(), hints.len(), "two rows offer the same chord");
        assert_eq!(
            row_shortcut(ROW_CAP),
            None,
            "the row after the candidates is *Search the vault*, and a chip on it would promise \
             a chord that must never fire there"
        );
        assert_eq!(NEW_LOGIN_SHORTCUT, "CTRL+ALT+N");
        assert!(
            !hints.contains(&NEW_LOGIN_SHORTCUT.to_string()),
            "*New login*'s chord is also a row's"
        );
    }

    /// **Escape was already handled, and still is.**
    ///
    /// A source pin, because the key arrives in the card's own pump and no
    /// test can open that window. What is decidable is that `next` still
    /// answers `VK_ESCAPE` with `Event::Cancel` *before* `IsDialogMessageW`,
    /// which only cancels for a real dialog box -- and that the chord handling
    /// added beside it did not become a blanket `WM_KEYDOWN` grab, which would
    /// swallow the Tab and Enter traversal that same call buys.
    #[test]
    fn escape_still_cancels_and_tab_still_traverses() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let raw =
            std::fs::read_to_string(src.join("picker_prompt.rs")).unwrap().replace("\r\n", "\n");
        let production = raw.split(concat!("\n#[cfg(", "test)]\n")).next().unwrap();
        let code: String = production
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            production.len() < raw.len(),
            "control: the `#[cfg(test)]` cut marker was not found, so this scan is reading the \
             test module as production"
        );
        assert!(
            code.contains("IsDialogMessageW(top, &msg)"),
            "control: the production cut does not contain the pump this rule is about"
        );

        let escape = code
            .find("VK_ESCAPE.0 {")
            .expect("`next` no longer answers Escape at all -- Cancel by keyboard is gone");
        let traversal = code.find("IsDialogMessageW(top, &msg)").expect("checked above");
        assert!(
            escape < traversal,
            "Escape is now handled after `IsDialogMessageW`, which only cancels for a real \
             dialog box -- so this frameless card would swallow it and never close"
        );
        assert!(
            code.contains("chord_held() && chord("),
            "the CTRL+ALT chords are no longer gated on both modifiers being down. An ungated \
             `WM_KEYDOWN` arm ahead of `IsDialogMessageW` eats Tab, Shift+Tab, Space and Enter, \
             which is the whole of this card's focus traversal"
        );
    }

    /// **The *Cancel* button advertises the key that has always cancelled
    /// it.**
    ///
    /// A source pin, not a paint assertion -- nothing here can open the
    /// window `paint_control` draws into. What is decidable is that the
    /// production code hands `ESC_SHORTCUT` to `draw_button_with_shortcut`
    /// for `ID_CANCEL`, the same call `ID_SECONDARY` uses for
    /// `NEW_LOGIN_SHORTCUT`, so the two footer buttons get their chips from
    /// one function rather than a second chip style growing beside it.
    #[test]
    fn the_cancel_button_shows_its_escape_chip() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let raw =
            std::fs::read_to_string(src.join("picker_prompt.rs")).unwrap().replace("\r\n", "\n");
        let production = raw.split(concat!("\n#[cfg(", "test)]\n")).next().unwrap();
        // Comments stripped, like the pins beside this one: the prose above
        // the branch names both `ID_CANCEL` and `ESC_SHORTCUT`, and a scan
        // that could not tell code from the comment explaining it would pass
        // on the explanation alone.
        let code: String = production
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        // **Whitespace-normalised, so this pin is about the code and not
        // about its indentation.** Matching twenty-four exact spaces made a
        // `rustfmt` reflow of `paint_control` -- a wrapped `if`, a renamed
        // binding one line up -- fail as though the chip had been deleted. A
        // false alarm on a pin is how pins get deleted. The claim is the
        // same: `ID_CANCEL`'s branch is the one that yields `ESC_SHORTCUT`.
        let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            production.len() < raw.len(),
            "control: the `#[cfg(test)]` cut marker was not found, so this scan is reading the \
             test module as production"
        );
        assert!(
            flat.contains("if id == ID_CANCEL { Some(ESC_SHORTCUT)"),
            "the production code no longer hands `ESC_SHORTCUT` to `ID_CANCEL`'s branch -- the \
             Cancel button's chip is gone"
        );
        assert!(
            flat.contains("draw_button_with_shortcut"),
            "control: this scan is not reading the function that paints the footer buttons"
        );
        assert!(
            flat.contains("ID_SECONDARY && MODE.load"),
            "control: the whitespace-normalised scan is not reading `paint_control`'s hint \
             branch at all, so the rule above could be matching some other text"
        );
        assert_eq!(ESC_SHORTCUT, "ESC");
    }

    #[test]
    fn the_window_is_protected_before_it_is_ever_pumped() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static ORDER: AtomicUsize = AtomicUsize::new(0);
        static PROTECTED_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
        static PUMPED_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
        let calls = PickerCalls {
            open: |_, _| Some(PickerWindow(1)),
            protect: |_| {
                PROTECTED_AT.store(ORDER.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
                true
            },
            next: |_| {
                // Record only the FIRST pump. If every pump overwrote this,
                // the last write would win and the assertion below would
                // only mean "protect happened before the final pump" -- which
                // passes even if an earlier pump ran before protect.
                let _ = PUMPED_AT.compare_exchange(
                    usize::MAX,
                    ORDER.fetch_add(1, Ordering::SeqCst),
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                Event::Cancel
            },
            show_palette: |_, _| {},
            show_search: |_, _| {},
            close: |_| {},
        };
        let _ = run_with(
            &calls,
            &one("Slack"),
            "Slack.exe",
            |_| Palette { fields: vec![], has_sequence: false },
            no_search,
        );
        assert!(
            PROTECTED_AT.load(Ordering::SeqCst) < PUMPED_AT.load(Ordering::SeqCst),
            "a window that can be typed into before it is excluded from capture is a window a \
             recorder can catch a keystroke in"
        );
    }

    #[test]
    fn closing_the_window_closes_it_and_fills_nothing() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static CLOSED: AtomicBool = AtomicBool::new(false);
        let calls = PickerCalls {
            open: |_, _| Some(PickerWindow(1)),
            protect: |_| true,
            next: |_| Event::Closed,
            show_palette: |_, _| {},
            show_search: |_, _| {},
            close: |_| CLOSED.store(true, Ordering::SeqCst),
        };
        assert_eq!(
            run_with(
                &calls,
                &one("Slack"),
                "Slack.exe",
                |_| Palette { fields: vec![], has_sequence: false },
                no_search,
            ),
            Outcome::Cancelled
        );
        assert!(CLOSED.load(Ordering::SeqCst), "close runs on every exit path");
    }

    #[test]
    fn a_window_that_cannot_be_opened_is_unavailable_and_not_a_silent_nothing() {
        let calls = PickerCalls {
            open: |_, _| None,
            protect: |_| true,
            next: |_| Event::Cancel,
            show_palette: |_, _| {},
            show_search: |_, _| {},
            close: |_| {},
        };
        assert_eq!(
            run_with(
                &calls,
                &one("Slack"),
                "Slack.exe",
                |_| Palette { fields: vec![], has_sequence: false },
                no_search,
            ),
            Outcome::Unavailable
        );
    }

    #[test]
    fn the_fill_path_also_closes_the_window() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        static CLOSED: AtomicBool = AtomicBool::new(false);
        static STEP: AtomicUsize = AtomicUsize::new(0);
        let calls = PickerCalls {
            open: |_, _| Some(PickerWindow(1)),
            protect: |_| true,
            next: |_| match STEP.fetch_add(1, Ordering::SeqCst) {
                0 => Event::Chose(0),
                _ => Event::Sends(Send::Field(FieldRef::Password)),
            },
            show_palette: |_, _| {},
            show_search: |_, _| {},
            close: |_| CLOSED.store(true, Ordering::SeqCst),
        };
        let outcome = run_with(
            &calls,
            &one("Slack"),
            "Slack.exe",
            |_| Palette { fields: vec![FieldRef::Password], has_sequence: false },
            no_search,
        );
        assert_eq!(
            outcome,
            Outcome::Fill { id: "id-1".to_string(), send: Send::Field(FieldRef::Password) }
        );
        assert!(
            CLOSED.load(Ordering::SeqCst),
            "the Fill path is the one that most needs close -- the window's lifetime bounds an \
             un-wipeable copy of typed text"
        );
    }

    #[test]
    fn choosing_a_row_shows_the_palette_it_was_given() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static STEP: AtomicUsize = AtomicUsize::new(0);
        static SHOWN_FIELDS: std::sync::Mutex<Vec<FieldRef>> = std::sync::Mutex::new(Vec::new());
        static SHOWN_HAS_SEQUENCE: AtomicUsize = AtomicUsize::new(usize::MAX);
        let calls = PickerCalls {
            open: |_, _| Some(PickerWindow(1)),
            protect: |_| true,
            next: |_| match STEP.fetch_add(1, Ordering::SeqCst) {
                0 => Event::Chose(0),
                _ => Event::Cancel,
            },
            show_palette: |_, palette| {
                *SHOWN_FIELDS.lock().unwrap() = palette.fields.clone();
                SHOWN_HAS_SEQUENCE.store(palette.has_sequence as usize, Ordering::SeqCst);
            },
            show_search: |_, _| {},
            close: |_| {},
        };
        let outcome = run_with(
            &calls,
            &one("Slack"),
            "Slack.exe",
            |id| {
                assert_eq!(id, "id-1");
                Palette { fields: vec![FieldRef::Totp], has_sequence: true }
            },
            no_search,
        );
        assert_eq!(outcome, Outcome::Cancelled);
        assert_eq!(*SHOWN_FIELDS.lock().unwrap(), vec![FieldRef::Totp]);
        assert_eq!(SHOWN_HAS_SEQUENCE.load(Ordering::SeqCst), 1);
    }
}

/// **No `GetClientRect`-derived value reaches a scaling helper.**
///
/// The `win32` submodule's convention is that a [`Box2`] is LOGICAL -- `rounded`
/// and `text` scale every coordinate they are handed -- while a `RECT` in that
/// module is device pixels, because that is what `GetClientRect` returns and
/// what `fill`, `draw_row` and `draw_button_with_shortcut` want. The focus ring
/// on the footer buttons was built as a `Box2` out of a `GetClientRect` `RECT`,
/// so it was scaled twice: at 150% it was drawn half again the size of the
/// control, clipped by the client area, and lost the rounded corners it exists
/// to draw.
///
/// No test can open the real window and this crate does not fake `scale()`, so
/// what is pinned is what is decidable: this file's own source, in the crate's
/// established shape ([`crate::unlock_prompt`]'s `no_thread_quit_pin`,
/// [`crate::job_object`]'s scanners). Every `Box2` literal in the code that
/// SHIPS is checked to mention none of the device-pixel names that the paint
/// functions bind `GetClientRect` output to.
///
/// **Normalised first.** This is a CRLF checkout with no `.gitattributes`;
/// slicing lines without trimming the carriage return makes the cut a no-op and
/// the whole pin vacuous. The control tests below are what prove it did not
/// silently scan nothing, or the wrong half.
#[cfg(test)]
mod no_device_pixels_in_a_logical_box_pin {
    /// The names the paint functions bind device-pixel rectangles to: `rc` and
    /// `client` straight out of `GetClientRect`, and `whole`, which is built
    /// from `rc`. Split across two literals apiece, on one line, in this
    /// crate's idiom: `include_str!` pulls this module in too.
    const DEVICE: [&str; 3] = [concat!("r", "c."), concat!("who", "le."), concat!("clie", "nt.")];
    const LOGICAL_LITERAL: &str = concat!("Box", "2 {");

    /// `source` with CRLF normalised, every top-level `#[cfg(test)]` module
    /// removed, and every `//` comment stripped.
    ///
    /// The module cut is line-based and anchored at column zero: a
    /// `#[cfg(test)]` on its own unindented line, up to and including the next
    /// unindented `}`. Every gated module in this file has that shape, and
    /// `the_cut_really_discards_something` checks that rather than assuming it.
    fn production_only(source: &str) -> String {
        let mut out = String::new();
        let mut skipping = false;
        for line in source.lines() {
            let flat = line.trim_end();
            if !skipping && flat == "#[cfg(test)]" {
                skipping = true;
                continue;
            }
            if skipping {
                if flat == "}" {
                    skipping = false;
                }
                continue;
            }
            // Comments only. A `//` inside a string literal would be cut too,
            // but cutting too much can only make the scan MISS a literal, never
            // invent one -- and the comment above the ring names `rc` on
            // purpose, to explain what it must not be given.
            let code = match flat.find("//") {
                Some(at) => &flat[..at],
                None => flat,
            };
            out.push_str(code);
            out.push('\n');
        }
        assert!(!skipping, "a gated module never closed at column zero; the cut is unreliable");
        out
    }

    fn source() -> String {
        production_only(include_str!("picker_prompt.rs"))
    }

    /// Every `Box2 { .. }` literal in `code`, as the text between the brace
    /// pair. These literals are flat -- no nested braces anywhere in this file
    /// -- so the first `}` closes the one that was opened.
    fn logical_literals(code: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut rest = code;
        while let Some(at) = rest.find(LOGICAL_LITERAL) {
            let body = &rest[at + LOGICAL_LITERAL.len()..];
            let end = body.find('}').expect("a `Box2 {` literal never closed");
            found.push(body[..end].to_string());
            rest = &body[end..];
        }
        found
    }

    /// Control: the cut discarded something, and this module with it.
    #[test]
    fn the_cut_really_discards_something() {
        let whole_file = include_str!("picker_prompt.rs");
        let kept = source();
        assert!(!kept.is_empty(), "the cut kept nothing at all; the pin would be vacuous");
        assert!(
            kept.len() < whole_file.len(),
            "the cut discarded nothing, so the gated modules -- including this one, which \
             names the device-pixel bindings -- are still being scanned"
        );
        assert!(
            !kept.contains("mod no_device_pixels_in_a_logical_box_pin"),
            "this pin's own module survived the cut, so it would scan itself"
        );
    }

    /// Control: the half that was KEPT is the painting half.
    #[test]
    fn the_kept_half_still_contains_the_paint_functions() {
        let kept = source();
        assert!(
            kept.contains("fn paint_control"),
            "the kept half no longer contains `paint_control`, so the pin is not scanning \
             the code it exists to guard"
        );
        assert!(
            kept.contains("fn rounded"),
            "the kept half no longer contains `rounded`, the helper that does the scaling"
        );
    }

    /// Control: the scan finds the literals that are really there, including
    /// the ring's own.
    #[test]
    fn the_scan_finds_the_literals_it_is_meant_to_read() {
        let found = logical_literals(&source());
        assert!(
            found.len() >= 8,
            "only {} logical literals found in the shipping code; `layout` alone writes more \
             than that, so the scanner is not reading this file",
            found.len()
        );
        assert!(
            found.iter().any(|body| body.contains("button.w")),
            "the focus ring's own literal was not among those scanned"
        );
    }

    /// Control: the scan would notice a device value if one were there.
    #[test]
    fn the_scan_would_notice_a_device_value() {
        let planted = production_only(&format!(
            "    {LOGICAL_LITERAL} x: 0, y: 0, w: rc.right, h: rc.bottom }}\n"
        ));
        let bodies = logical_literals(&planted);
        assert_eq!(bodies.len(), 1, "the scanner did not read the planted literal");
        assert!(
            DEVICE.iter().any(|name| bodies[0].contains(name)),
            "the scanner cannot see a device-pixel value that is present"
        );
    }

    #[test]
    fn no_logical_box_is_built_out_of_device_pixels() {
        for body in logical_literals(&source()) {
            for name in DEVICE {
                assert!(
                    !body.contains(name),
                    "a `Box2` -- which every helper in the `win32` submodule scales itself -- \
                     is built out of `{name}`, which is already device pixels from \
                     `GetClientRect`. It will be scaled a second time: at 150% it is drawn \
                     half again the size of the control, clipped by the client area, and the \
                     rounded corners it exists to draw are the first thing lost. The logical \
                     size is known from `layout()` without dividing device pixels back down. \
                     The offending literal was: `{body}`"
                );
            }
        }
    }
}

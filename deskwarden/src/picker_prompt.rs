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
    /// Too many candidates to list; the user asked to search the vault
    /// instead of picking from the truncated card.
    SearchVault,
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
    /// Picked the candidate at this index into the slice `run_with` was
    /// given.
    Chose(usize),
    /// Asked to search the vault instead -- offered only when the card is
    /// truncated. See `crate::win32_draw::visible_rows`.
    Overflow,
    /// Asked to create a new login for this app.
    NewLogin,
    /// Asked to edit the previously chosen candidate's binding.
    EditSelected,
    /// Picked which field of the previously chosen candidate to type.
    Sends(Send),
}

/// The Win32 half, as `fn` pointers so [`run_with`] can be driven without a
/// desktop. Nothing here decides anything; every decision lives in
/// [`run_with`].
pub struct PickerCalls {
    /// Lays out and shows the card of candidates. `None` if it could not be
    /// put on screen.
    pub open: fn(&[Candidate]) -> Option<PickerWindow>,
    /// `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on the top-level
    /// window, called before the first `next` -- see the module doc.
    pub protect: fn(PickerWindow) -> bool,
    /// Pumps until the user does something.
    pub next: fn(PickerWindow) -> Event,
    /// Shows the field palette for the chosen candidate: the fields offered,
    /// and whether the item has a stored sequence worth offering as
    /// [`Send::Sequence`].
    pub show_palette: fn(PickerWindow, &[FieldRef], bool),
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
pub fn run_with(
    calls: &PickerCalls,
    candidates: &[Candidate],
    palette: fn(&str) -> (Vec<FieldRef>, bool),
) -> Outcome {
    let Some(window) = (calls.open)(candidates) else {
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

    let mut chosen: Option<usize> = None;

    loop {
        match (calls.next)(window) {
            Event::Cancel | Event::Closed => {
                (calls.close)(window);
                return Outcome::Cancelled;
            }
            Event::Overflow => {
                (calls.close)(window);
                return Outcome::SearchVault;
            }
            Event::NewLogin => {
                (calls.close)(window);
                return Outcome::NewLogin;
            }
            Event::Chose(index) => {
                let Some(candidate) = candidates.get(index) else {
                    continue;
                };
                chosen = Some(index);
                let (fields, has_sequence) = palette(&candidate.id);
                (calls.show_palette)(window, &fields, has_sequence);
            }
            Event::EditSelected => {
                if let Some(candidate) = chosen.and_then(|index| candidates.get(index)) {
                    let id = candidate.id.clone();
                    (calls.close)(window);
                    return Outcome::Edit(id);
                }
            }
            Event::Sends(send) => {
                if let Some(candidate) = chosen.and_then(|index| candidates.get(index)) {
                    let id = candidate.id.clone();
                    (calls.close)(window);
                    return Outcome::Fill { id, send };
                }
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
            let mut tokens = vec![Token::Field(FieldRef::Username)];
            if let Some(tab) = crate::key_sequence::key_named("TAB") {
                tokens.push(Token::Key(tab));
            }
            tokens.push(Token::Field(FieldRef::Password));
            tokens
        }
        Send::Sequence => sequence.map(crate::key_sequence::parse).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key_sequence::{FieldRef, Token};

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
            open: |_| Some(PickerWindow(1)),
            protect: |_| true,
            next: |_| {
                use std::sync::atomic::{AtomicUsize, Ordering};
                static STEP: AtomicUsize = AtomicUsize::new(0);
                match STEP.fetch_add(1, Ordering::SeqCst) {
                    0 => Event::Chose(0),
                    _ => Event::Sends(Send::Field(FieldRef::Password)),
                }
            },
            show_palette: |_, _, _| {},
            close: |_| {},
        };
        let outcome = run_with(&calls, &one("Slack"), |_| (vec![FieldRef::Password], false));
        assert_eq!(
            outcome,
            Outcome::Fill { id: "id-1".to_string(), send: Send::Field(FieldRef::Password) }
        );
    }

    #[test]
    fn the_window_is_protected_before_it_is_ever_pumped() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static ORDER: AtomicUsize = AtomicUsize::new(0);
        static PROTECTED_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
        static PUMPED_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
        let calls = PickerCalls {
            open: |_| Some(PickerWindow(1)),
            protect: |_| {
                PROTECTED_AT.store(ORDER.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
                true
            },
            next: |_| {
                PUMPED_AT.store(ORDER.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
                Event::Cancel
            },
            show_palette: |_, _, _| {},
            close: |_| {},
        };
        let _ = run_with(&calls, &one("Slack"), |_| (vec![], false));
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
            open: |_| Some(PickerWindow(1)),
            protect: |_| true,
            next: |_| Event::Closed,
            show_palette: |_, _, _| {},
            close: |_| CLOSED.store(true, Ordering::SeqCst),
        };
        assert_eq!(run_with(&calls, &one("Slack"), |_| (vec![], false)), Outcome::Cancelled);
        assert!(CLOSED.load(Ordering::SeqCst), "close runs on every exit path");
    }

    #[test]
    fn a_window_that_cannot_be_opened_is_unavailable_and_not_a_silent_nothing() {
        let calls = PickerCalls {
            open: |_| None,
            protect: |_| true,
            next: |_| Event::Cancel,
            show_palette: |_, _, _| {},
            close: |_| {},
        };
        assert_eq!(run_with(&calls, &one("Slack"), |_| (vec![], false)), Outcome::Unavailable);
    }
}

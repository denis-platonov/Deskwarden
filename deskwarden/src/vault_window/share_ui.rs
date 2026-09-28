//! **Personal sharing, as the vault window draws it and asks for it**
//! (design 6, `docs/design/6-sharing-mock.html`).
//!
//! The window holds none of sharing's state of record. It holds the daemon's
//! answer to `GET /sharing` ([`SharingPane::view`]), asks again when the
//! daemon's change feed says `sharing`, and sends every request through
//! [`crate::daemon_link::SharingClient`] on a worker thread
//! ([`spawn_sharing_call`], the one thread this file starts). A window with no
//! line to the daemon has no view, and its Share control offers a Send.
//!
//! What is drawn here -- person lines, the Share dialog, the people block of
//! the SHARING card, share rows in Sent, received rows in Shared with me --
//! is called from `detail`, `send_ui` and `mod`, which own their panes.
//!
//! # The engine's sentences are shown, never logged
//!
//! A refusal the daemon answers with is the sharing engine's own sentence,
//! and some of them name a DECRYPTED record ("Couldn't stop sharing Acme VPN
//! ..." from a forget that did not finish). So every `Err` string a
//! [`SharingReport`] carries goes to the pane -- a notice, a dialog's error --
//! and nowhere else: no `log::` line here takes one, and the report's `Debug`
//! is written by hand to leave them out, so a `{:?}` added later cannot leak
//! one either (`a_reports_debug_never_carries_the_engines_sentence`).

use std::sync::mpsc;

use eframe::egui;
use zeroize::Zeroizing;

use crate::daemon_link::SharingClient;
use crate::local_time::LocalOffset;
use crate::rest::organizations::normalise_email;
use crate::rest::sharing::{Fingerprint, PersonState, ShareResult, SharingView};
use crate::rest::sharing_api::InviteCode;
use crate::sent_history::{SentHistory, SentKind};
use crate::theme;
use crate::vault_bridge::VaultItem;

// ---- the words (mock 6e's STATUS LINES) -----------------------------------------

/// The header's Share tooltip (mock 6b's hover).
pub(crate) const SHARE_TOOLTIP: &str = "Share with a person \u{b7} view-only";
pub(crate) const SHARE_UNAVAILABLE_TOOLTIP: &str =
    "Sharing with a person needs a NodeWarden server. This sends a link instead.";
/// A "Share anyway" with no fingerprint behind it -- see
/// [`SharingPane::share_anyway`]. The card never builds one (it offers the
/// button only beside a fingerprint it fetched), so this is the sentence a
/// wiring mistake would show, rather than a confirm the user never checked.
pub(crate) const CHECK_FINGERPRINT_FIRST: &str = "Show their fingerprint and compare it before sharing anyway.";
/// A worker that panicked: the call's own kind of failure, in words.
pub(crate) const PANICKED: &str = "Sharing stopped unexpectedly. Try again.";
/// A write asked for while another is still out -- see [`request`], which
/// says this rather than dropping the second one without a word.
pub(crate) const BUSY: &str = "Another sharing change is still running. Try again when it finishes.";

pub(crate) fn shared_toast(email: &str) -> String {
    format!("Shared with {email}. It is in their vault now.")
}

pub(crate) fn waiting_toast(email: &str, record: &str) -> String {
    format!("{email} was added. {record} appears in their vault once they accept.")
}

/// A share that left an invitation for an address with no account, said as a
/// toast when no dialog is open to say it (mock 6e's dialog line, without
/// the expiry the report does not carry).
pub(crate) fn waiting_account_toast(email: &str) -> String {
    format!("Waiting for {email} to create an account.")
}

const MILLIS_PER_DAY: i64 = 86_400_000;

// ---- one person on one record ---------------------------------------------------------

/// A person as the SHARING card lists them. Built for every record the read
/// pane draws (`detail::PeopleSharing::people`) and drawn by Task 17's card;
/// until then only this file's tests read one.
///
/// `pub` and not `pub(crate)` because `detail::PeopleSharing`, a field of
/// `detail::draw_detail_read`'s public signature, carries a slice of them.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonLine {
    pub(crate) email: String,
    pub(crate) state: PersonState,
    /// The status line under the address (mock 6b).
    pub(crate) line: String,
}

// Read by Task 17's SHARING card.
#[allow(dead_code)]
impl PersonLine {
    pub(crate) fn pill(&self) -> theme::SharePillKind {
        match self.state {
            PersonState::Shared => theme::SharePillKind::Shared,
            PersonState::WaitingAccept | PersonState::WaitingSignIn => theme::SharePillKind::Invited,
            PersonState::WaitingAccount => theme::SharePillKind::NoAccount,
            PersonState::Expired => theme::SharePillKind::Expired,
            PersonState::KeyChanged => theme::SharePillKind::KeyChanged,
            PersonState::Gone => theme::SharePillKind::Gone,
        }
    }

    /// Re-invite is offered where a fresh invitation helps (spec 6.1).
    pub(crate) fn reinvite(&self) -> bool {
        matches!(self.state, PersonState::WaitingAccount | PersonState::Expired | PersonState::Gone)
    }

    /// Show fingerprint only where a key is held (mock 6b: "only for someone
    /// whose key we hold").
    pub(crate) fn fingerprint(&self) -> bool {
        matches!(self.state, PersonState::Shared | PersonState::KeyChanged)
    }

    /// Where the card may put "Share anyway" -- beside a changed key. The
    /// button itself only ever sends [`SharingPane::share_anyway`]'s call,
    /// which does not exist until Show fingerprint has answered.
    pub(crate) fn share_anyway(&self) -> bool {
        self.state == PersonState::KeyChanged
    }
}

/// `14 Sep` in the user's zone.
fn day(millis: i64, zone: &dyn LocalOffset) -> String {
    let parts = crate::local_time::local_parts(millis, zone);
    format!("{} {}", parts.day, crate::local_time::month_name(parts.month))
}

/// Whole days until an ISO instant, rounded up; `None` without one.
fn days_left(expires_at: Option<&str>, now_ms: i64) -> Option<i64> {
    let at = crate::send::parse_iso_utc_millis(expires_at?)?;
    Some(((at - now_ms).max(0) + MILLIS_PER_DAY - 1) / MILLIS_PER_DAY)
}

fn days_words(days: i64) -> String {
    if days == 1 { "1 day left".to_string() } else { format!("{days} days left") }
}

/// The last time this PC shared `item_id` with `email` -- "since" on the card.
/// A share has no server timestamp; this is the local record of it (plan
/// decision 12).
fn since(history: &SentHistory, item_id: &str, email: &str) -> Option<i64> {
    history
        .entries
        .iter()
        .filter(|e| e.kind == SentKind::Share && e.item_id == item_id && e.recipient.trim().eq_ignore_ascii_case(email.trim()))
        .map(|e| e.sent_at_unix_millis)
        .max()
}

/// **The people one record is shared with**, each with its status line --
/// what the window hands the read pane in `detail::PeopleSharing::people`.
pub(crate) fn person_lines(
    view: Option<&SharingView>,
    item_id: &str,
    history: &SentHistory,
    now_ms: i64,
    zone: &dyn LocalOffset,
) -> Vec<PersonLine> {
    let Some(view) = view else {
        return Vec::new();
    };
    view.people_of(item_id)
        .into_iter()
        .map(|person| {
            let line = match person.state {
                PersonState::Shared => match since(history, item_id, &person.email) {
                    Some(at) => format!("Shared \u{b7} since {}", day(at, zone)),
                    None => "Shared".to_string(),
                },
                PersonState::WaitingAccept => match since(history, item_id, &person.email) {
                    Some(at) => format!("Waiting to accept \u{b7} invited {}", day(at, zone)),
                    None => "Waiting to accept".to_string(),
                },
                PersonState::WaitingAccount => match days_left(person.expires_at.as_deref(), now_ms) {
                    Some(days) => format!("Waiting to create an account \u{b7} {}", days_words(days)),
                    None => "Waiting to create an account".to_string(),
                },
                PersonState::Expired => match person.expires_at.as_deref().and_then(crate::send::parse_iso_utc_millis) {
                    Some(at) => format!("The invite ran out on {} \u{b7} they never created an account", day(at, zone)),
                    None => "The invite ran out \u{b7} they never created an account".to_string(),
                },
                PersonState::KeyChanged => "Their key changed. Check the fingerprint before sharing.".to_string(),
                PersonState::WaitingSignIn => "Waiting for them to sign in once to a Bitwarden app".to_string(),
                PersonState::Gone => "No longer receiving this".to_string(),
            };
            PersonLine { email: person.email.clone(), state: person.state, line }
        })
        .collect()
}

/// The Sent row's short state (mock 6c: "to anna@example.com · Shared").
/// Called by Task 18's Sent rows.
#[allow(dead_code)]
pub(crate) fn short_state(state: PersonState, expires_at: Option<&str>, now_ms: i64) -> String {
    match state {
        PersonState::Shared => "Shared".to_string(),
        PersonState::WaitingAccept => "Waiting to accept".to_string(),
        PersonState::WaitingAccount => days_left(expires_at, now_ms).map_or_else(|| "No account yet".to_string(), days_words),
        PersonState::Expired => "Invite expired".to_string(),
        PersonState::KeyChanged => "Key changed".to_string(),
        PersonState::WaitingSignIn => "Waiting for a first sign-in".to_string(),
        PersonState::Gone => "Not receiving".to_string(),
    }
}

/// **The header's Share control** for one record: `Some(true)` shares,
/// `Some(false)` offers a Send instead (sharing is NodeWarden-only; owner's
/// decision), `None` draws nothing -- a received record (view-only), or a
/// window that has not heard from the daemon yet. Read by the header through
/// [`people_sharing`].
pub(crate) fn share_control(view: Option<&SharingView>, linked: bool, item: &VaultItem) -> Option<bool> {
    if !linked {
        return Some(false);
    }
    let view = view?;
    if view.received_from(&item.id).is_some() {
        return None;
    }
    Some(view.available)
}

/// The names of the records shared with `email`, for the forget question
/// (spec 11.7: records and Sends in one list) -- `vault_window::mod`'s
/// `forget_request_for`.
pub(crate) fn records_shared_with(view: Option<&SharingView>, email: &str, items: &[VaultItem]) -> Vec<String> {
    let Some(view) = view else {
        return Vec::new();
    };
    let email = normalise_email(email);
    view.records
        .iter()
        .filter(|r| r.people.iter().any(|p| *p == email))
        .filter_map(|r| items.iter().find(|i| i.id == r.item_id).map(|i| i.name.clone()))
        .collect()
}

// ---- the window's sharing state ----------------------------------------------------

/// **Everything the window keeps about sharing between frames.** The view is
/// the daemon's; the rest is what the user is in the middle of.
#[derive(Default)]
pub(crate) struct SharingPane {
    /// The daemon's last answer to `GET /sharing`.
    pub(crate) view: Option<SharingView>,
    stale: bool,
    fetching: bool,
    /// The last view read failed: when, in egui seconds, to try again. See
    /// [`Self::wants_view`].
    retry_at: Option<f64>,
    /// A write is in flight; one at a time. A fingerprint is a read and
    /// does not take the slot -- see [`request`].
    busy: bool,
    /// The Share dialog, while it is open.
    pub(crate) dialog: Option<ShareDialog>,
    /// Stop sharing's step two is up for `(item id, email)`.
    pub(crate) stop_confirming: Option<(String, String)>,
    /// A stop is running for `(item id, email)`.
    pub(crate) stopping: Option<(String, String)>,
    /// **The fingerprint shown**: `(normalised email, the daemon's answer)`.
    ///
    /// The whole [`Fingerprint`] and not just its phrase, because the hash in
    /// it is what "Share anyway" sends back: the engine confirms only the key
    /// the user compared (Task 12), and this is where the window keeps which
    /// key that was. Spent by the Confirm it binds -- see [`request`].
    pub(crate) fingerprint: Option<(String, Fingerprint)>,
    /// A toast and when it goes, in egui seconds.
    pub(crate) toast: Option<(String, f64)>,
    /// A failed request's sentence, shown on the card until the next write.
    /// Shown, never logged: see the module doc.
    pub(crate) notice: Option<String>,
    /// **The daemon's forget for the person the forget question is about**:
    /// their normalised email, and `None` until the answer comes, then
    /// whether it stopped every share. See [`Self::daemon_forget`].
    daemon_forget: Option<(String, Option<bool>)>,
}

/// Where the daemon's half of forgetting one person stands -- what the
/// window's forget question waits on before it forgets the person locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DaemonForget {
    /// No forget was sent for them: nothing was shared, so there is nothing
    /// on the daemon's side to wait for.
    NotAsked,
    /// Sent; no answer yet.
    Waiting,
    /// Every share with them is stopped.
    Stopped,
    /// It did not finish. The person is kept, and the sentence is shown.
    Failed,
}

/// How long a toast stays.
const TOAST_SECONDS: f64 = 5.0;

/// How long after a failed view read the window asks again, unasked. The
/// vault window's own frame schedule already wakes it at least every half
/// second, so the check needs no timer of its own.
pub(crate) const VIEW_RETRY_SECONDS: f64 = 30.0;

impl SharingPane {
    /// The daemon said the view changed, or this window changed it.
    pub(crate) fn invalidate(&mut self) {
        self.stale = true;
    }

    /// **Whether the frame should ask for the view now**: once at the start,
    /// and again whenever it went stale -- never while a read is out.
    ///
    /// **A read that FAILED is asked again [`VIEW_RETRY_SECONDS`] later**, or
    /// sooner when something invalidates the view (the feed, or a write's
    /// report). Not on the next frame: "no view yet" alone would re-ask at
    /// once, and the worker's own repaint makes that frame come at once -- a
    /// daemon answering 401 would be asked hundreds of times a second. And not
    /// never: with push off, or no sharing change for hours, a window whose
    /// first read failed would sit with no view, a rail that counts nothing
    /// shared and no Share control. `now_seconds` is egui's input time.
    pub(crate) fn wants_view(&self, now_seconds: f64) -> bool {
        let due = match self.retry_at {
            Some(at) => now_seconds >= at,
            None => self.view.is_none(),
        };
        !self.fetching && (self.stale || due)
    }

    /// A view request was started.
    pub(crate) fn begin_view(&mut self) {
        self.fetching = true;
        self.stale = false;
    }

    /// Sent's count of person shares: one per (record, person), as the list
    /// draws them (mock 6c).
    pub(crate) fn sent_pairs(&self) -> usize {
        self.view.as_ref().map_or(0, |v| v.records.iter().map(|r| r.people.len()).sum())
    }

    pub(crate) fn received_count(&self) -> usize {
        self.view.as_ref().map_or(0, |v| v.received.len())
    }

    pub(crate) fn has_incoming(&self) -> bool {
        self.view.as_ref().is_some_and(|v| !v.incoming.is_empty())
    }

    /// Handed to the read pane by [`people_sharing`], for Task 17's card.
    pub(crate) fn confirming_for(&self, item_id: &str) -> Option<&str> {
        self.stop_confirming.as_ref().filter(|(id, _)| id == item_id).map(|(_, e)| e.as_str())
    }

    /// Handed to the read pane by [`people_sharing`], for Task 17's card.
    pub(crate) fn stopping_for(&self, item_id: &str) -> Option<&str> {
        self.stopping.as_ref().filter(|(id, _)| id == item_id).map(|(_, e)| e.as_str())
    }

    /// **The only way to a "Share anyway"**: a Confirm for `email`, carrying
    /// the key hash of the fingerprint this window was shown for them -- or
    /// `None` while no fingerprint for that person is held, or they are not
    /// a changed key in the view. The card's button sends this and nothing
    /// else (Task 17), so a Confirm always follows a Show fingerprint.
    ///
    /// The view's state is a display fact: its people come from collection
    /// names, and a name is not proof of who holds the key. The binding that
    /// matters is the hash, which the engine checks against the key it would
    /// confirm and refuses on a mismatch -- this is the window's half, the
    /// engine's is the one that cannot be bypassed.
    #[allow(dead_code)]
    pub(crate) fn share_anyway(&self, email: &str) -> Option<SharingCall> {
        let email = normalise_email(email);
        let (shown_for, fingerprint) = self.fingerprint.as_ref()?;
        if *shown_for != email {
            return None;
        }
        if self.view.as_ref()?.person(&email)?.state != PersonState::KeyChanged {
            return None;
        }
        Some(SharingCall::Confirm { email, key_sha256: fingerprint.key_sha256.clone() })
    }

    /// Whether a Confirm is for the person, and the key, last shown.
    fn was_shown(&self, email: &str, key_sha256: &str) -> bool {
        self.fingerprint
            .as_ref()
            .is_some_and(|(shown_for, f)| *shown_for == normalise_email(email) && f.key_sha256 == key_sha256)
    }

    /// **The daemon's forget for `email` has started** -- call it only when
    /// [`request`] answered `true`. From here until the answer,
    /// [`Self::daemon_forget`] says [`DaemonForget::Waiting`], and the window
    /// does not forget the person locally.
    ///
    /// `from_dialog`: the forget question came from the Share dialog's
    /// suggestions, so the answer is that dialog's to show (the flag goes on
    /// the dialog itself, so a dialog opened later does not inherit it).
    pub(crate) fn begin_forget(&mut self, email: &str, from_dialog: bool) {
        self.daemon_forget = Some((normalise_email(email), None));
        if from_dialog {
            if let Some(dialog) = self.dialog.as_mut() {
                dialog.forget_asked = true;
            }
        }
    }

    /// Where the daemon's half of forgetting `email` stands.
    pub(crate) fn daemon_forget(&self, email: &str) -> DaemonForget {
        match &self.daemon_forget {
            Some((who, answer)) if *who == normalise_email(email) => match answer {
                None => DaemonForget::Waiting,
                Some(true) => DaemonForget::Stopped,
                Some(false) => DaemonForget::Failed,
            },
            _ => DaemonForget::NotAsked,
        }
    }

    /// The forget question is settled, either way: nothing is waited on.
    pub(crate) fn end_forget(&mut self) {
        self.daemon_forget = None;
    }
}

/// **The Share dialog's state** (mock 6a), drawn by [`draw_share_modal`] and
/// answered by [`answer_dialog`]. It lives in [`SharingPane::dialog`], so a
/// report can reach it whether or not the frame is drawing it.
///
/// No `Debug`: it holds the typed master password (the crate's rule for a
/// body that mentions `Zeroizing`), and an [`InviteCode`], which is a
/// credential for registering.
pub(crate) struct ShareDialog {
    pub(crate) item_id: String,
    pub(crate) item_name: String,
    pub(crate) email: String,
    /// A share is out and its answer has not come: the dialog's spinner.
    /// Set only by [`answer_dialog`] when `request` started the call, and
    /// taken down by the answer or, on a refusal, at once.
    pub(crate) in_flight: bool,
    /// The last attempt's sentence.
    pub(crate) error: Option<String>,
    /// A share that left an invitation for an address with no account.
    pub(crate) waiting_account: Option<String>,
    /// The master password, typed to mint an invite code. Wiped on drop, and
    /// taken out -- leaving an empty one -- the moment Create is pressed,
    /// whatever the mint answers. Never logged, never in a `Debug`.
    pub(crate) password: Zeroizing<String>,
    /// A mint is out: [`Self::in_flight`]'s twin for the invite code.
    pub(crate) minting: bool,
    pub(crate) invite_code: Option<InviteCode>,
    // A suggestion's ✕ is not state: the dialog REPORTS it, as
    // [`ShareDialogAction::Forget`], in the frame it was pressed, and the
    // window asks the forget question. The `forget` field that stood here for
    // it had no reader.
    /// The address box has been given the keyboard once. 6a draws the box
    /// focused on open -- one field, so the dialog is for typing into it --
    /// and asking once rather than every frame is what lets a click on a
    /// suggestion or a button take the focus away.
    pub(crate) focused: bool,
    /// The daemon's forget running now was asked from THIS dialog's
    /// suggestions -- so its answer lands here, and only here
    /// ([`SharingPane::begin_forget`]).
    pub(crate) forget_asked: bool,
}

impl ShareDialog {
    /// A fresh dialog for one record. Called by `vault_window::mod`'s
    /// `permit_share` alone, behind the re-prompt
    /// (`exactly_one_line_opens_the_share_dialog`).
    pub(crate) fn opening(item_id: &str, item_name: &str) -> Self {
        Self {
            item_id: item_id.to_string(),
            item_name: item_name.to_string(),
            email: String::new(),
            in_flight: false,
            error: None,
            waiting_account: None,
            password: Zeroizing::new(String::new()),
            minting: false,
            invite_code: None,
            focused: false,
            forget_asked: false,
        }
    }
}

// ---- the calls, on one worker thread each --------------------------------------------

/// **One request to the daemon's sharing routes.**
///
/// No `Debug`: `InviteCode` carries the master password. It stays
/// `Zeroizing` from the dialog's field to [`SharingClient::invite_code`]'s
/// borrowed `&str`, and is wiped when the call is dropped at the end of
/// [`run`] -- never copied into a plain `String` on the way.
///
/// `View`, and the Share dialog's three (Share, Forget, InviteCode), have
/// callers; the rest are sent by the cards Tasks 17-20 draw (Stop, Reinvite,
/// Fingerprint, Confirm: 17; Accept, Decline, Leave: 20).
#[allow(dead_code)]
pub(crate) enum SharingCall {
    View,
    Share { item_id: String, item_name: String, email: String },
    Stop { item_id: String, email: String },
    Reinvite(String),
    Forget(String),
    /// "Share anyway", to the key whose fingerprint was compared. Built by
    /// [`SharingPane::share_anyway`], and refused by [`request`] unless it
    /// matches the fingerprint the pane holds.
    Confirm { email: String, key_sha256: String },
    Fingerprint(String),
    /// A registration code for `email`'s pending invitation (admin only).
    InviteCode { email: String, password: Zeroizing<String> },
    Accept(String),
    Decline(String),
    Leave(String),
}

impl SharingCall {
    /// **This call's failure, as its own kind of report** -- what a worker
    /// that panicked sends, so [`apply`] clears exactly the state the call
    /// set: a `View` that panicked clears `fetching` (a generic failure would
    /// not, and the window would never ask for the view again), a mint stops
    /// the dialog's spinner, a stop clears `stopping`. Built before the call
    /// runs, since running it moves it; it copies no password.
    pub(crate) fn failed(&self, sentence: &str) -> SharingReport {
        let err = sentence.to_string();
        match self {
            Self::View => SharingReport::View(Err(err)),
            Self::Share { email, item_name, .. } => {
                SharingReport::Shared { email: email.clone(), item_name: item_name.clone(), result: Err(err) }
            }
            Self::Stop { email, .. } => SharingReport::Stopped { email: email.clone(), result: Err(err) },
            Self::Forget(_) => SharingReport::Forgot(Err(err)),
            Self::Fingerprint(email) => SharingReport::Fingerprint { email: email.clone(), result: Err(err) },
            Self::InviteCode { .. } => SharingReport::InviteCode(Err(err)),
            Self::Reinvite(_) | Self::Confirm { .. } | Self::Accept(_) | Self::Decline(_) | Self::Leave(_) => {
                SharingReport::Done(Err(err))
            }
        }
    }
}

/// **What came back.** Every `Err` is the engine's or the daemon's sentence,
/// for the pane only -- see the module doc, and this type's `Debug` below.
pub(crate) enum SharingReport {
    View(Result<SharingView, String>),
    /// `item_name` is the call's own, so a toast can name the record
    /// whether or not the dialog is still open when the answer comes.
    Shared { email: String, item_name: String, result: Result<ShareResult, String> },
    Stopped { email: String, result: Result<Option<String>, String> },
    Done(Result<(), String>),
    /// The names of the records a forget stopped -- decrypted names.
    Forgot(Result<Vec<String>, String>),
    Fingerprint { email: String, result: Result<Fingerprint, String> },
    InviteCode(Result<InviteCode, String>),
}

/// **Hand-written, and says only which report and whether it succeeded.**
/// A derived `Debug` would print the engine's sentences -- which can name a
/// decrypted record -- the record names a forget stopped, and the addresses
/// of the people involved, into whatever `{:?}` it reached. What a debugger
/// needs is the shape; the words are the pane's.
impl std::fmt::Debug for SharingReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn outcome<T>(result: &Result<T, String>) -> &'static str {
            if result.is_ok() { "Ok(..)" } else { "Err(..)" }
        }
        let (kind, result) = match self {
            Self::View(r) => ("View", outcome(r)),
            Self::Shared { result, .. } => ("Shared", outcome(result)),
            Self::Stopped { result, .. } => ("Stopped", outcome(result)),
            Self::Done(r) => ("Done", outcome(r)),
            Self::Forgot(r) => ("Forgot", outcome(r)),
            Self::Fingerprint { result, .. } => ("Fingerprint", outcome(result)),
            Self::InviteCode(r) => ("InviteCode", outcome(r)),
        };
        write!(f, "SharingReport::{kind}({result})")
    }
}

/// **One call, synchronously** -- the worker thread's whole body, and
/// callable from a test without a thread.
pub(crate) fn run(client: &SharingClient, call: SharingCall) -> SharingReport {
    match call {
        SharingCall::View => SharingReport::View(client.view()),
        SharingCall::Share { item_id, item_name, email } => {
            let result = client.share(&item_id, &email);
            SharingReport::Shared { email, item_name, result }
        }
        SharingCall::Stop { item_id, email } => {
            let result = client.stop(&item_id, &email);
            SharingReport::Stopped { email, result }
        }
        SharingCall::Reinvite(email) => SharingReport::Done(client.reinvite(&email).map(|_| ())),
        SharingCall::Forget(email) => SharingReport::Forgot(client.forget(&email)),
        SharingCall::Confirm { email, key_sha256 } => SharingReport::Done(client.confirm(&email, &key_sha256)),
        SharingCall::Fingerprint(email) => {
            let result = client.fingerprint(&email);
            SharingReport::Fingerprint { email, result }
        }
        // `password` is dropped -- and so wiped -- when this arm ends.
        SharingCall::InviteCode { email, password } => SharingReport::InviteCode(client.invite_code(&email, &password)),
        SharingCall::Accept(id) => SharingReport::Done(client.accept(&id)),
        SharingCall::Decline(id) => SharingReport::Done(client.decline(&id)),
        SharingCall::Leave(org) => SharingReport::Done(client.leave(&org)),
    }
}

/// **The one thread this file starts**: a call, its report on `tx`, a
/// repaint. A panic becomes the call's own failed report rather than a silent
/// nothing -- `spawn_reporting`'s rule in `vault_window::mod`, with
/// [`SharingCall::failed`] as its `on_panic`. Registered in `job_object`'s
/// `THREAD_SPAWN_SITES`.
pub(crate) fn spawn_sharing_call(
    ctx: egui::Context,
    tx: mpsc::Sender<SharingReport>,
    client: SharingClient,
    call: SharingCall,
) {
    let on_panic = call.failed(PANICKED);
    std::thread::spawn(move || {
        let report = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&client, call))).unwrap_or(on_panic);
        let _ = tx.send(report);
        ctx.request_repaint();
    });
}

/// **Every sharing request the window makes goes through here**: one write
/// at a time, a Confirm only for the key that was shown, and a window with no
/// line to the daemon is told why.
///
/// **`true` when the call was started**, and `false` -- with a sentence in
/// [`SharingPane::notice`] -- when it was refused. A caller that raised a
/// spinner before asking takes it down on `false`: a refused call has no
/// report coming, so nothing else ever would. A refusal is never silent.
///
/// The Confirm check comes first, before the line is looked at: a Confirm
/// that does not match the held fingerprint is a mistake in the caller
/// whatever the line, and is refused without spending the fingerprint.
///
/// **Show fingerprint does not take the write slot.** It is a read -- the
/// daemon hashes a key it holds -- so it may run beside a write, and its
/// report leaves `busy` and the view alone (see [`apply`]).
#[must_use]
pub(crate) fn request(
    pane: &mut SharingPane,
    client: Option<&SharingClient>,
    ctx: &egui::Context,
    tx: &mpsc::Sender<SharingReport>,
    call: SharingCall,
) -> bool {
    if let SharingCall::Confirm { email, key_sha256 } = &call {
        if !pane.was_shown(email, key_sha256) {
            pane.notice = Some(CHECK_FINGERPRINT_FIRST.to_string());
            return false;
        }
    }
    let Some(client) = client else {
        pane.notice = Some(SHARE_UNAVAILABLE_TOOLTIP.to_string());
        return false;
    };
    match &call {
        SharingCall::View => pane.begin_view(),
        SharingCall::Fingerprint(_) => {}
        _ if pane.busy => {
            pane.notice = Some(BUSY.to_string());
            return false;
        }
        SharingCall::Stop { item_id, email } => {
            pane.busy = true;
            pane.stopping = Some((item_id.clone(), email.clone()));
        }
        // **Spent once sent**: one comparison, one confirm. A confirm that
        // fails needs the fingerprint shown again -- the key may have moved
        // on in between, and the next confirm should be to what is shown then.
        SharingCall::Confirm { .. } => {
            pane.busy = true;
            pane.fingerprint = None;
        }
        _ => pane.busy = true,
    }
    // A write clears the last write's sentence; the view read does NOT. A
    // failed write marks the view stale, and the very next frame asks for it
    // -- clearing the notice there would take the sentence down one frame
    // after it went up (`a_failed_writes_sentence_outlives_the_view_read_it_causes`).
    if !matches!(call, SharingCall::View) {
        pane.notice = None;
    }
    spawn_sharing_call(ctx.clone(), tx.clone(), client.clone(), call);
    true
}

/// Takes every report that has arrived. Called once per frame.
pub(crate) fn drain(rx: &mpsc::Receiver<SharingReport>, pane: &mut SharingPane, now_seconds: f64) {
    while let Ok(report) = rx.try_recv() {
        apply(pane, report, now_seconds);
    }
    if pane.toast.as_ref().is_some_and(|(_, until)| *until <= now_seconds) {
        pane.toast = None;
    }
}

/// One report into the pane.
pub(crate) fn apply(pane: &mut SharingPane, report: SharingReport, now_seconds: f64) {
    let toast = |pane: &mut SharingPane, text: String| pane.toast = Some((text, now_seconds + TOAST_SECONDS));
    match report {
        SharingReport::View(result) => {
            pane.fetching = false;
            match result {
                Ok(view) => {
                    pane.view = Some(view);
                    pane.retry_at = None;
                }
                // The reason is NOT in the line: it is the daemon's sentence
                // (module doc). The view on screen stays as it was, and the
                // next change on the feed -- or the retry -- asks again.
                Err(_) => {
                    pane.retry_at = Some(now_seconds + VIEW_RETRY_SECONDS);
                    log::info!("the sharing view could not be read from the daemon");
                }
            }
            return;
        }
        // **A fingerprint is a read**: it took no write slot (see
        // [`request`]), so its answer must not free one a write still holds,
        // and it changed nothing the view shows.
        SharingReport::Fingerprint { email, result } => {
            match result {
                Ok(fingerprint) => pane.fingerprint = Some((normalise_email(&email), fingerprint)),
                Err(sentence) => pane.notice = Some(sentence),
            }
            return;
        }
        // **An answer lands only in the dialog that asked** -- the one with a
        // share out (`in_flight`). A dialog closed before its answer came and
        // a fresh one opened since, on this record or another, did not ask:
        // painting the old share's "no account yet" or failure into it would
        // put one record's outcome on another's dialog. Those answers are
        // said the way a closed dialog's are, as a toast or a notice
        // (`an_answer_does_not_land_in_a_dialog_that_did_not_ask`).
        SharingReport::Shared { email, item_name, result } => {
            let asked = pane.dialog.as_ref().is_some_and(|d| d.in_flight);
            match result {
                Ok(ShareResult::Shared) => {
                    if asked {
                        pane.dialog = None;
                    }
                    toast(pane, shared_toast(&email));
                }
                Ok(ShareResult::WaitingAccept) => {
                    if asked {
                        pane.dialog = None;
                    }
                    toast(pane, waiting_toast(&email, &item_name));
                }
                // The open dialog says it in its own words (mock 6a); with
                // the dialog closed before the answer came, a toast does.
                Ok(ShareResult::WaitingAccount) => match pane.dialog.as_mut().filter(|_| asked) {
                    Some(dialog) => {
                        dialog.in_flight = false;
                        dialog.waiting_account = Some(email);
                    }
                    None => toast(pane, waiting_account_toast(&email)),
                },
                // The engine's refusals -- a record with attachments, in the
                // trash, in another organisation, waiting to come home -- are
                // its own sentences, and the dialog is where they are read.
                Err(sentence) => match pane.dialog.as_mut().filter(|_| asked) {
                    Some(dialog) => {
                        dialog.in_flight = false;
                        dialog.error = Some(sentence);
                    }
                    None => pane.notice = Some(sentence),
                },
            }
        }
        SharingReport::Stopped { email, result } => {
            pane.stopping = None;
            pane.stop_confirming = None;
            match result {
                Ok(_) => toast(pane, format!("Stopped sharing with {email}.")),
                Err(sentence) => pane.notice = Some(sentence),
            }
        }
        SharingReport::Done(result) => {
            if let Err(sentence) = result {
                pane.notice = Some(sentence);
            }
        }
        // **The daemon's forget came back.** Its outcome is recorded for the
        // forget question the window is carrying out (`daemon_forget`), which
        // forgets the person locally only on `Ok` -- the Sends' rule, "only
        // once every deletion came back".
        //
        // A failure can name a decrypted record ("Couldn't stop sharing
        // Wi-Fi ..."), so it is SHOWN -- in the Share dialog that asked, the
        // share answer's rule above, and on the card otherwise -- and never
        // logged (module doc).
        SharingReport::Forgot(result) => {
            if let Some((_, answer)) = pane.daemon_forget.as_mut() {
                answer.get_or_insert(result.is_ok());
            }
            let asked = pane.dialog.as_mut().filter(|d| d.forget_asked);
            match (asked, result) {
                (Some(dialog), result) => {
                    dialog.forget_asked = false;
                    if let Err(sentence) = result {
                        dialog.error = Some(sentence);
                    }
                }
                (None, Err(sentence)) => pane.notice = Some(sentence),
                (None, Ok(_)) => {}
            }
        }
        // Only the dialog that is minting takes a code -- the share answer's
        // rule above, for the same reason.
        SharingReport::InviteCode(result) => match pane.dialog.as_mut().filter(|d| d.minting) {
            Some(dialog) => {
                dialog.minting = false;
                dialog.password = Zeroizing::new(String::new());
                match result {
                    Ok(code) => dialog.invite_code = Some(code),
                    Err(sentence) => dialog.error = Some(sentence),
                }
            }
            None => {
                if let Err(sentence) = result {
                    pane.notice = Some(sentence);
                }
            }
        },
    }
    pane.busy = false;
    pane.stale = true;
}

// ---- the address book, re-read when sharing moves ------------------------------------

/// **A change this window made to the sent history and could not save.**
/// Held so a re-read of the file -- which the window does when the daemon
/// says sharing moved, because the daemon re-keys the file when a record
/// comes home -- does not quietly undo it: the file never heard of it.
pub(crate) enum UnsavedSent {
    Record(crate::sent_history::SentRecord),
    Forget(String),
}

impl UnsavedSent {
    fn apply_to(&self, history: &mut SentHistory) {
        match self {
            Self::Record(entry) => history.record(entry.clone()),
            Self::Forget(email) => {
                history.forget(email);
            }
        }
    }
}

/// **The sent history as the file has it now, with this window's unsaved
/// changes kept.**
///
/// The unsaved changes are saved first -- one more attempt, through
/// `sent_history::update`'s read-modify-write, so they land on top of the
/// daemon's re-keyed file rather than over it -- and cleared when that
/// works. Then the file is read. Whatever still could not be saved is
/// applied to what was read, in memory, and kept for the next attempt.
///
/// The alternatives were worse both ways: re-reading alone drops a Send the
/// user just made from the Recipient suggestions (and brings back a person
/// they just forgot), and saving the WHOLE in-memory history would write the
/// pre-re-key item ids back over the daemon's.
pub(crate) fn reload_sent_history(path: &std::path::Path, unsaved: &mut Vec<UnsavedSent>) -> SentHistory {
    if !unsaved.is_empty() {
        match crate::sent_history::update(path, |history| unsaved.iter().for_each(|change| change.apply_to(history))) {
            Ok(_) => unsaved.clear(),
            Err(e) => log::warn!("the sent history still could not be saved: {e:?}"),
        }
    }
    let mut history = SentHistory::load(path);
    unsaved.iter().for_each(|change| change.apply_to(&mut history));
    history
}

// ---- the read pane's half (design 6a, 6b, 6d) -------------------------------------

/// **Everything the read pane is told about sharing, for one record**, in
/// one place: the Share control's answer, the people and their lines, the
/// stop and fingerprint states, and who shares a received record. Both of
/// the window's read-pane callers build it here, so the pane behind the
/// sequence builder and the live one cannot resolve the header two ways.
pub(crate) fn people_sharing<'a>(
    pane: &'a SharingPane,
    linked: bool,
    item: &VaultItem,
    people: &'a [PersonLine],
) -> super::detail::PeopleSharing<'a> {
    super::detail::PeopleSharing {
        share_control: share_control(pane.view.as_ref(), linked, item),
        people,
        confirming: pane.confirming_for(&item.id),
        stopping: pane.stopping_for(&item.id),
        fingerprint: pane.fingerprint.as_ref().map(|(email, f)| (email.as_str(), f.phrase.as_str())),
        view_only_from: pane.view.as_ref().and_then(|v| v.received_from(&item.id)),
    }
}

// ---- the Share dialog (mock 6a) ------------------------------------------------------
//
// **Built from the Send composer's parts, on purpose.** 6a's own note is that
// the dialog is "styled like Send a record, narrower because it has one
// field", so it is `theme::form_card` and its three bands, `record_ui`'s
// RECORD chip, `send_ui`'s suggestion list and the composer's footer --
// called, not copied -- and only what 6a draws that 5a does not (the share
// mark on the title, the access sentence, the no-account band) is new here.

pub(crate) const SHARE_BUTTON: &str = "Share";
pub(crate) const CANCEL_BUTTON: &str = "Cancel";
pub(crate) const DONE_BUTTON: &str = "Done";
/// The administrator's button on the no-account band. An invite code is
/// NodeWarden's way in for a new account (plan decision 4: no email is sent),
/// minted for the one pending invitation the share just left.
pub(crate) const MAKE_CODE_BUTTON: &str = "Create an invite code";
pub(crate) const COPY_CODE_BUTTON: &str = "Copy";
/// What the copy confirmation calls the code: "Invite code copied".
pub(crate) const INVITE_CODE_LABEL: &str = "Invite code";
const RECORD_EYEBROW: &str = "RECORD";
const WITH_EYEBROW: &str = "WITH";
const WITH_HINT: &str = "Their email address";
/// 6a's eyebrow over the suggestions: the Send composer's `SENT TO BEFORE`,
/// renamed because this list is fed by shares as well as Sends.
const SUGGESTIONS_EYEBROW: &str = "SHARED WITH BEFORE";
/// 6a's footer note. `Sent` is the rail's own word for the list a share is
/// filed in (`sidebar::SENDS_ROW_LABEL`).
const APPEARS_IN_SENT: &str = "Appears in Sent";
/// The footer note while a share is out -- mock 6e's `Stopping…` idiom.
const SHARING_NOW: &str = "Sharing\u{2026}";
/// The footer note while a code is being made.
const MAKING_CODE: &str = "Making a code\u{2026}";
const PASSWORD_HINT: &str = "Your master password";
/// 6a's `.modal { width: 560px }`: 5a's card, narrower by the Access block it
/// does not have. Wide enough that `theme::form_card_pad_x` gives every band
/// the design's 18 (its threshold is 470).
const MODAL_WIDTH: f32 = 560.0;
/// 6a's `ic('eye', 16, ...)` before the access sentence, its `margin-top:
/// 1px`, and the `gap: 10px` after it.
const ACCESS_EYE: f32 = 16.0;
const ACCESS_EYE_GAP: f32 = 10.0;
/// 6a's `.mband`: `padding: 12px 18px`, a 15-point info mark with `gap: 9px`.
const BAND_PAD_Y: i8 = 12;
const BAND_MARK: f32 = 15.0;
const BAND_MARK_GAP: f32 = 9.0;
/// The password box beside Create an invite code.
const PASSWORD_BOX_WIDTH: f32 = 220.0;
/// The scrim of the forget question, which can stand over this dialog (a
/// suggestion's ✕ asks it). While it is up, Escape is that question's.
const FORGET_SCRIM: &str = "record-forget-scrim";

/// 6a's access sentence, with `view-only` in the ink and weight the mock's
/// `<b>` gives it, wrapped at `width`. 12px at a 1.5 line height, the
/// mock's `.access .t`.
fn access_job(record: &str, width: f32) -> egui::text::LayoutJob {
    let plain = egui::TextFormat {
        font_id: egui::FontId::new(12.0, egui::FontFamily::Proportional),
        color: theme::TEXT_SECONDARY,
        line_height: Some(18.0),
        ..Default::default()
    };
    let strong = egui::TextFormat {
        font_id: egui::FontId::new(12.0, egui::FontFamily::Name(theme::SEMIBOLD.into())),
        color: theme::INK,
        ..plain.clone()
    };
    let mut job = egui::text::LayoutJob::default();
    job.append("They get ", 0.0, plain.clone());
    job.append("view-only", 0.0, strong);
    job.append(
        &format!(
            " access: they can see, copy and autofill it, but not edit, delete or share it on. \
             When you edit {record}, their copy changes too. You can stop sharing at any time \
             from the record."
        ),
        0.0,
        plain,
    );
    job.wrap.max_width = width;
    job
}

/// **The no-account band's two sentences** -- the bold line and the one
/// under it.
///
/// **NodeWarden sends no email** (plan decision 4), so this says what really
/// happens next: the person needs an invite code to sign up, and the band
/// never says an invitation was emailed, which is mock 6a's wording for the
/// official server's flow. An administrator is told they can make that code
/// here; anybody else, who makes it.
fn no_account_band(email: &str, record: &str, admin: bool) -> (String, String) {
    let head = format!("Waiting for {email} to create an account \u{b7} the invitation lasts 7 days.");
    let who = if admin {
        "They need an invite code to sign up. You are this server's administrator, so you can \
         create one below and give it to them."
    } else {
        "They need an invite code from your server's administrator to sign up."
    };
    let body = format!(
        "{who} {record} reaches their vault after they sign up and accept, and you can re-invite \
         them from the record."
    );
    (head, body)
}

/// What an invite code is handed on as: the link where the server gave one,
/// the bare code otherwise.
pub(crate) fn invite_text(code: &InviteCode) -> &str {
    code.link.as_deref().unwrap_or(&code.code)
}

/// A suggestion's count, by kind (mock 6a: "2 shared", "3 sent").
fn person_count_words(person: &crate::sent_history::Person) -> String {
    match (person.sent, person.shared) {
        (0, shared) => format!("{shared} shared"),
        (sent, 0) => super::send_ui::sent_count_words(sent),
        (sent, shared) => format!("{shared} shared, {sent} sent"),
    }
}

/// What the dialog asked for this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShareDialogAction {
    None,
    Submit,
    Cancel,
    MintCode,
    /// A suggestion's ✕: the window asks the forget question.
    Forget(String),
    Done,
    /// The made invite code's Copy: the window copies it the way it copies
    /// every credential (`clipboard::copy_secret`), which this file does not
    /// reach.
    CopyCode,
}

/// **Draws the Share dialog** over the window: scrim, card, the record,
/// the WITH field with suggestions from the one address book (Sends and
/// shares), what view-only means, and Share/Cancel -- or, after a share to an
/// address with no account, the band that says so and Done.
///
/// **It decides nothing it draws.** What a press does is [`answer_dialog`]'s;
/// this only reports which press it was. Share, Cancel, Done, the corner ✕
/// and Escape are all refused while a share or a mint is out -- the Send
/// composer's rule, and the reason is the same: a dialog closed under its own
/// spinner has an answer coming that nothing is waiting for.
pub(crate) fn draw_share_modal(
    ctx: &egui::Context,
    dialog: &mut ShareDialog,
    item: Option<&VaultItem>,
    folder: Option<&str>,
    history: &SentHistory,
    view: Option<&SharingView>,
) -> ShareDialogAction {
    let mut action = ShareDialogAction::None;
    let busy = dialog.in_flight || dialog.minting;
    let admin = view.is_some_and(|v| v.can_mint_invites);
    theme::modal_scrim(ctx, egui::Area::new(egui::Id::new("record-share-scrim")));
    theme::movable_modal(ctx, egui::Area::new(egui::Id::new("record-share-modal"))).show(ctx, |ui| {
        // Before the card, not after: a drag strip laid over the card's
        // controls would swallow their clicks (`theme::modal_drag_handle`).
        theme::modal_drag_handle(ui, theme::FORM_CARD_HEADER_HEIGHT);
        ui.set_max_width(MODAL_WIDTH);
        let (card, title) = theme::form_card(ui, |ui| {
            let title = theme::form_card_header_share(ui, &format!("Share {}", dialog.item_name));
            theme::form_card_body(ui, |ui| {
                ui.set_width(ui.available_width());
                theme::eyebrow(ui, RECORD_EYEBROW);
                ui.add_space(theme::EYEBROW_GAP);
                super::record_ui::record_chip(ui, &dialog.item_name, &super::record_ui::chip_subtitle(item, folder));
                ui.add_space(theme::BLOCK_GAP);

                // **The WITH box** -- read-only once the share has left an
                // invitation (6a's fourth state draws the address greyed):
                // the answer on the band is about that address, and editing
                // it under the band would make the two disagree.
                theme::eyebrow(ui, WITH_EYEBROW);
                ui.add_space(theme::EYEBROW_GAP);
                let editable = !busy && dialog.waiting_account.is_none();
                let width = ui.available_width();
                let field = ui
                    .add_enabled_ui(editable, |ui| theme::inline_field(ui, &mut dialog.email, WITH_HINT, width, false))
                    .inner;
                if editable && !dialog.focused {
                    field.request_focus();
                    dialog.focused = true;
                }
                if let Some(email) = super::send_ui::draw_people_suggestions(
                    ui,
                    egui::Id::new("record-share-suggestions"),
                    &field,
                    &mut dialog.email,
                    history,
                    editable,
                    SUGGESTIONS_EYEBROW,
                    &person_count_words,
                ) {
                    action = ShareDialogAction::Forget(email);
                }
                // The ↵ on the Share button is a promise: Enter in the box
                // shares. A single-line box gives up the keyboard on Enter,
                // which is how the press is seen here.
                if editable
                    && field.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                    && !dialog.email.trim().is_empty()
                {
                    action = ShareDialogAction::Submit;
                }
                ui.add_space(theme::BLOCK_GAP);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let (mark, _) = ui.allocate_exact_size(egui::Vec2::splat(ACCESS_EYE), egui::Sense::hover());
                    theme::paint_view_eye(ui.painter(), mark.translate(egui::vec2(0.0, 1.0)), theme::TEXT_MUTED);
                    ui.add_space(ACCESS_EYE_GAP);
                    let width = ui.available_width();
                    ui.add(egui::Label::new(access_job(&dialog.item_name, width)).wrap());
                });
                // The last attempt's sentence -- the engine's refusal (a
                // record with attachments, in the trash, in another
                // organisation, waiting to come home), a refused request, a
                // forget that did not finish. Shown here and never logged.
                if let Some(error) = &dialog.error {
                    ui.add_space(10.0);
                    ui.add(egui::Label::new(egui::RichText::new(error).size(12.0).color(theme::ERROR)).wrap());
                }
            });
            if let Some(email) = dialog.waiting_account.clone() {
                if let Some(asked) = no_account_block(ui, dialog, &email, admin) {
                    action = asked;
                }
            }
            // **5a's footer, answers left and a standing note right** --
            // and the note says what is running while something is.
            theme::form_card_footer(ui, |ui| {
                let note = if dialog.in_flight {
                    SHARING_NOW
                } else if dialog.minting {
                    MAKING_CODE
                } else {
                    APPEARS_IN_SENT
                };
                ui.horizontal(|ui| {
                    if dialog.waiting_account.is_some() {
                        if theme::primary_button_enabled(ui, DONE_BUTTON, None, !busy).clicked() {
                            action = ShareDialogAction::Done;
                        }
                    } else {
                        let ready = !busy && !dialog.email.trim().is_empty();
                        if theme::primary_button_enabled(ui, SHARE_BUTTON, Some("\u{21b5}"), ready).clicked() {
                            action = ShareDialogAction::Submit;
                        }
                        ui.add_space(theme::FORM_FOOTER_GAP);
                        if ui.add_enabled_ui(!busy, |ui| theme::secondary_button(ui, CANCEL_BUTTON)).inner.clicked() {
                            action = ShareDialogAction::Cancel;
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        theme::form_footer_note(ui, note);
                    });
                });
            });
            title
        });
        // 6a's `.mhead .x`: the ✕ answers what Cancel answers, gated by
        // what gates it -- `record_ui::draw_export_form`'s rule.
        if theme::modal_corner_mark_gated(ui, card, title, !busy) {
            action = ShareDialogAction::Cancel;
        }
        // **The app's own copy confirmation, over this card** -- the invite
        // code's Copy says "Invite code copied" the way every copy in this
        // window does. Drawn inside the modal's area, on its layer, for
        // `totp_add`'s reason: the read pane's toast would be painted under
        // this card (and stands down while a modal is up).
        super::detail::draw_copy_toast(ui, card);
    });
    // Escape is Cancel -- unless the forget question is standing over this
    // dialog, where Escape is that question's "No" and must not also close
    // the dialog underneath it.
    let forget_up = ctx.memory(|m| {
        m.areas().is_visible(&egui::LayerId::new(theme::SCRIM_ORDER, egui::Id::new(FORGET_SCRIM)))
    });
    if action == ShareDialogAction::None && !busy && !forget_up && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = ShareDialogAction::Cancel;
    }
    action
}

/// **6a's no-account band**, between the body and the footer: the amber of
/// `totp_add`'s 6c band rather than `theme::form_card_caution`'s red, and an
/// info mark rather than a warning, because nothing went wrong -- the share
/// went through and is waiting for somebody to sign up.
///
/// An administrator also gets the invite code here: a master-password box and
/// Create an invite code, then the code with a Copy. Only an administrator
/// (`SharingView::can_mint_invites`), and only here -- a code is minted for
/// the one pending invitation this share left, so there is nothing to mint
/// for before it exists.
fn no_account_block(
    ui: &mut egui::Ui,
    dialog: &mut ShareDialog,
    email: &str,
    admin: bool,
) -> Option<ShareDialogAction> {
    let mut asked = None;
    let (head, body) = no_account_band(email, &dialog.item_name, admin);
    // 6a's `border-top: 1px solid var(--caution-edge)`.
    let (rule, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rule, egui::CornerRadius::ZERO, theme::CAUTION_EDGE);
    egui::Frame::new()
        .fill(theme::CAUTION_WASH)
        .inner_margin(egui::Margin::symmetric(theme::form_card_pad_x(ui.available_width()), BAND_PAD_Y))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let (mark, _) = ui.allocate_exact_size(egui::Vec2::splat(BAND_MARK), egui::Sense::hover());
                theme::paint_info_mark(ui.painter(), mark.translate(egui::vec2(0.0, 1.0)), theme::CAUTION_MARK);
                ui.add_space(BAND_MARK_GAP);
                ui.vertical(|ui| {
                    let width = ui.available_width();
                    ui.add(egui::Label::new(band_job(&head, &body, width)).wrap());
                    if !admin {
                        return;
                    }
                    ui.add_space(10.0);
                    match &dialog.invite_code {
                        Some(code) => {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = theme::FORM_FOOTER_GAP;
                                ui.label(
                                    egui::RichText::new(invite_text(code))
                                        .size(13.0)
                                        .monospace()
                                        .color(theme::INK),
                                );
                                if theme::row_button(ui, COPY_CODE_BUTTON).clicked() {
                                    asked = Some(ShareDialogAction::CopyCode);
                                }
                            });
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(format!("Give it to {email}. They enter it when they sign up."))
                                    .size(12.0)
                                    .color(theme::CAUTION_INK),
                            );
                        }
                        None => {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = theme::FORM_FOOTER_GAP;
                                let field = ui
                                    .add_enabled_ui(!dialog.minting, |ui| {
                                        theme::inline_field(ui, &mut dialog.password, PASSWORD_HINT, PASSWORD_BOX_WIDTH, true)
                                    })
                                    .inner;
                                let can = !dialog.minting && !dialog.password.is_empty();
                                let pressed = ui
                                    .add_enabled_ui(can, |ui| theme::secondary_button(ui, MAKE_CODE_BUTTON))
                                    .inner
                                    .clicked();
                                let entered = can && field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                if pressed || entered {
                                    asked = Some(ShareDialogAction::MintCode);
                                }
                            });
                        }
                    }
                });
            });
        });
    asked
}

/// The band's paragraph: the bold line, a break, the plain one -- 6a's
/// `<b>…</b><br>…` at 12px and a 1.5 line height, in the caution ink.
fn band_job(head: &str, body: &str, width: f32) -> egui::text::LayoutJob {
    let plain = egui::TextFormat {
        font_id: egui::FontId::new(12.0, egui::FontFamily::Proportional),
        color: theme::CAUTION_INK,
        line_height: Some(18.0),
        ..Default::default()
    };
    let strong = egui::TextFormat {
        font_id: egui::FontId::new(12.0, egui::FontFamily::Name(theme::BOLD.into())),
        ..plain.clone()
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(head, 0.0, strong);
    job.append("\n", 0.0, plain.clone());
    job.append(body, 0.0, plain);
    job.wrap.max_width = width;
    job
}

/// **What a press in the dialog does**, and the one place it is decided:
/// the dialog goes back into the pane or is dropped, and a share or a mint
/// is asked for through [`request`].
///
/// **A refused request never leaves a spinner up.** `request` answers
/// `false` with its sentence in [`SharingPane::notice`] and no report coming
/// -- no line to the daemon, or a write already out -- so the spinner this
/// raised is taken down here, at once, and the sentence moves into the
/// dialog, which is what the user is looking at
/// (`a_refused_request_never_leaves_the_dialogs_spinner_up`).
///
/// **The master password leaves the dialog on Create**, taken rather than
/// copied: the call carries the one copy and wipes it when it is dropped,
/// whether it ran or was refused. Cancel and Done drop the dialog, which
/// wipes whatever was typed.
///
/// Answers what the window must do itself: [`ShareDialogAction::Forget`]
/// (the forget question is the window's) and
/// [`ShareDialogAction::CopyCode`] (so is the clipboard); `None` otherwise.
pub(crate) fn answer_dialog(
    pane: &mut SharingPane,
    mut dialog: ShareDialog,
    action: ShareDialogAction,
    client: Option<&SharingClient>,
    ctx: &egui::Context,
    tx: &mpsc::Sender<SharingReport>,
) -> ShareDialogAction {
    let busy = dialog.in_flight || dialog.minting;
    match action {
        ShareDialogAction::Cancel | ShareDialogAction::Done => ShareDialogAction::None,
        ShareDialogAction::Submit if !busy && !dialog.email.trim().is_empty() => {
            dialog.in_flight = true;
            dialog.error = None;
            let call = SharingCall::Share {
                item_id: dialog.item_id.clone(),
                item_name: dialog.item_name.clone(),
                email: dialog.email.trim().to_string(),
            };
            pane.dialog = Some(dialog);
            if !request(pane, client, ctx, tx, call) {
                say_refusal_in_dialog(pane);
            }
            ShareDialogAction::None
        }
        ShareDialogAction::MintCode if !busy => {
            let password = std::mem::replace(&mut dialog.password, Zeroizing::new(String::new()));
            match dialog.waiting_account.clone().filter(|_| !password.is_empty()) {
                Some(email) => {
                    dialog.minting = true;
                    dialog.error = None;
                    pane.dialog = Some(dialog);
                    if !request(pane, client, ctx, tx, SharingCall::InviteCode { email, password }) {
                        say_refusal_in_dialog(pane);
                    }
                }
                None => pane.dialog = Some(dialog),
            }
            ShareDialogAction::None
        }
        ShareDialogAction::Forget(email) => {
            pane.dialog = Some(dialog);
            ShareDialogAction::Forget(email)
        }
        ShareDialogAction::CopyCode => {
            pane.dialog = Some(dialog);
            ShareDialogAction::CopyCode
        }
        ShareDialogAction::None | ShareDialogAction::Submit | ShareDialogAction::MintCode => {
            pane.dialog = Some(dialog);
            ShareDialogAction::None
        }
    }
}

/// **A request the pane refused, said in the open dialog**: its spinners
/// down and [`request`]'s sentence moved out of the notice into the dialog's
/// error, so it is said once and where the user is looking. For the window's
/// own requests made from the dialog (a forget from a suggestion's ✕) as
/// well as [`answer_dialog`]'s.
pub(crate) fn say_refusal_in_dialog(pane: &mut SharingPane) {
    if let Some(dialog) = pane.dialog.as_mut() {
        dialog.in_flight = false;
        dialog.minting = false;
        dialog.error = pane.notice.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest::sharing::{Fingerprint, PersonEntry, RecordEntry};

    const NOW: i64 = 1_790_000_000_000; // 2026-09-21
    const DAY: i64 = 86_400_000;

    fn view(people: Vec<(&str, PersonState, Option<&str>)>, records: Vec<(&str, Vec<&str>)>) -> SharingView {
        SharingView {
            available: true,
            people: people.into_iter().map(|(e, s, x)| PersonEntry { email: e.into(), state: s, expires_at: x.map(str::to_string) }).collect(),
            records: records.into_iter().map(|(id, p)| RecordEntry { item_id: id.into(), people: p.into_iter().map(str::to_string).collect() }).collect(),
            ..SharingView::default()
        }
    }

    fn fingerprint(hash: &str) -> Fingerprint {
        Fingerprint { phrase: "cobalt-hinge-lantern-mossy-quill".into(), key_sha256: hash.into() }
    }

    /// **Every status line of mock 6e**, in its words.
    #[test]
    fn each_persons_line_says_where_they_stand_in_the_mocks_words() {
        let expires = crate::local_time::FixedOffset(0);
        let in_six = {
            let parts = crate::local_time::civil_parts(NOW + 6 * DAY - 1);
            format!("{:04}-{:02}-{:02}T{:02}:{:02}:00Z", parts.year, parts.month, parts.day, parts.hour, parts.minute)
        };
        let v = view(
            vec![
                ("anna@x.io", PersonState::Shared, None),
                ("bob@x.io", PersonState::WaitingAccept, None),
                ("carol@x.io", PersonState::WaitingAccount, Some(in_six.as_str())),
                ("dan@x.io", PersonState::Expired, Some("2026-09-01T00:00:00Z")),
                ("eve@x.io", PersonState::KeyChanged, None),
                ("fay@x.io", PersonState::WaitingSignIn, None),
                ("gus@x.io", PersonState::Gone, None),
            ],
            vec![("r1", vec!["anna@x.io", "bob@x.io", "carol@x.io", "dan@x.io", "eve@x.io", "fay@x.io", "gus@x.io"])],
        );
        let mut history = crate::sent_history::SentHistory::default();
        history.record(crate::sent_history::SentRecord {
            kind: crate::sent_history::SentKind::Share,
            item_id: "r1".into(),
            recipient: "anna@x.io".into(),
            sent_at_unix_millis: NOW - 7 * DAY,
            ..Default::default()
        });
        let lines = person_lines(Some(&v), "r1", &history, NOW, &expires);
        let line = |email: &str| lines.iter().find(|l| l.email == email).map(|l| l.line.clone()).expect("a line");
        assert_eq!(line("anna@x.io"), "Shared \u{b7} since 14 Sep");
        assert_eq!(line("bob@x.io"), "Waiting to accept");
        assert_eq!(line("carol@x.io"), "Waiting to create an account \u{b7} 6 days left");
        assert_eq!(line("dan@x.io"), "The invite ran out on 1 Sep \u{b7} they never created an account");
        assert_eq!(line("eve@x.io"), "Their key changed. Check the fingerprint before sharing.");
        assert_eq!(line("fay@x.io"), "Waiting for them to sign in once to a Bitwarden app");
        assert_eq!(line("gus@x.io"), "No longer receiving this");
        let pill = |email: &str| lines.iter().find(|l| l.email == email).map(PersonLine::pill).expect("a pill");
        assert_eq!(pill("anna@x.io"), theme::SharePillKind::Shared);
        assert_eq!(pill("carol@x.io"), theme::SharePillKind::NoAccount);
        assert!(lines.iter().find(|l| l.email == "anna@x.io").is_some_and(PersonLine::fingerprint));
        assert!(lines.iter().find(|l| l.email == "bob@x.io").is_some_and(|l| !l.fingerprint()), "no key held yet (mock 6b)");
        assert!(lines.iter().find(|l| l.email == "dan@x.io").is_some_and(PersonLine::reinvite));
        assert!(lines.iter().find(|l| l.email == "eve@x.io").is_some_and(PersonLine::share_anyway));
        assert_eq!(short_state(PersonState::Shared, None, NOW), "Shared");
        assert_eq!(short_state(PersonState::WaitingAccount, Some(&in_six), NOW), "6 days left");
    }

    /// **Where the Share control goes** (owner decision: NodeWarden only,
    /// a Send elsewhere): shares when available, offers a Send when not or
    /// when the window has no line to the daemon, and is absent on a
    /// received record and before the view is known.
    #[test]
    fn the_share_control_shares_on_nodewarden_offers_a_send_elsewhere_and_is_absent_on_a_received_record() {
        let item = |id: &str| -> VaultItem { serde_json::from_value(serde_json::json!({ "id": id, "name": id })).expect("item") };
        let on = SharingView { available: true, ..SharingView::default() };
        let off = SharingView::unavailable(crate::rest::sharing::REASON_NOT_NODEWARDEN);
        let mut received = on.clone();
        received.received.push(crate::rest::sharing::ReceivedEntry { item_id: "theirs".into(), from: "d@x.io".into() });
        assert_eq!(share_control(Some(&on), true, &item("mine")), Some(true));
        assert_eq!(share_control(Some(&off), true, &item("mine")), Some(false));
        assert_eq!(share_control(None, false, &item("mine")), Some(false), "no line to the daemon: a Send");
        assert_eq!(share_control(None, true, &item("mine")), None, "not known yet");
        assert_eq!(share_control(Some(&received), true, &item("theirs")), None);
    }

    /// **A report updates the pane**: a view arrives, a share that landed
    /// closes the dialog with a toast, a share that failed keeps it open with
    /// the sentence.
    #[test]
    fn reports_update_the_pane_the_way_the_mock_says() {
        let mut pane = SharingPane::default();
        assert!(pane.wants_view(0.0), "a window starts by asking");
        pane.begin_view();
        assert!(!pane.wants_view(0.0), "one question at a time");
        apply(&mut pane, SharingReport::View(Ok(view(vec![("anna@x.io", PersonState::Shared, None)], vec![("r1", vec!["anna@x.io"])]))), 0.0);
        assert_eq!(pane.sent_pairs(), 1);
        // A dialog whose share is out -- the only kind an answer lands in
        // (`an_answer_does_not_land_in_a_dialog_that_did_not_ask`).
        let asked = || ShareDialog { in_flight: true, ..ShareDialog::opening("r1", "Acme VPN") };
        pane.dialog = Some(asked());
        apply(&mut pane, SharingReport::Shared { email: "anna@x.io".into(), item_name: "Acme VPN".into(), result: Ok(ShareResult::Shared) }, 1.0);
        assert!(pane.dialog.is_none());
        assert_eq!(pane.toast.as_ref().map(|(t, _)| t.as_str()), Some("Shared with anna@x.io. It is in their vault now."));
        assert!(pane.wants_view(0.0), "a write makes the view stale");
        pane.dialog = Some(asked());
        apply(&mut pane, SharingReport::Shared { email: "b@x.io".into(), item_name: "Acme VPN".into(), result: Err("Couldn't share the record. It's still in your vault.".into()) }, 2.0);
        assert_eq!(pane.dialog.as_ref().and_then(|d| d.error.as_deref()), Some("Couldn't share the record. It's still in your vault."));
    }

    /// **"Share anyway" is bound to the key the user looked at** (Task 12's
    /// hash-bound confirm): there is no Confirm to send until Show
    /// fingerprint has answered for that very person, the Confirm carries the
    /// hash that answer came with, and it is spent once sent. A Confirm built
    /// by hand for a hash the window never showed is refused before any
    /// thread starts -- with or without a line to the daemon.
    #[test]
    fn share_anyway_needs_the_fingerprint_that_was_shown_and_carries_its_hash() {
        let mut pane = SharingPane::default();
        pane.view = Some(view(vec![("eve@x.io", PersonState::KeyChanged, None)], vec![("r1", vec!["eve@x.io"])]));
        assert!(pane.share_anyway("eve@x.io").is_none(), "no fingerprint fetched yet");
        apply(&mut pane, SharingReport::Fingerprint { email: "eve@x.io".into(), result: Ok(fingerprint("ab12")) }, 0.0);
        assert_eq!(pane.fingerprint.as_ref().map(|(e, f)| (e.as_str(), f.phrase.as_str())), Some(("eve@x.io", "cobalt-hinge-lantern-mossy-quill")));
        assert!(pane.share_anyway("someone@x.io").is_none(), "the fingerprint shown is someone else's");
        match pane.share_anyway("EVE@x.io") {
            Some(SharingCall::Confirm { email, key_sha256 }) => {
                assert_eq!(email, "eve@x.io");
                assert_eq!(key_sha256, "ab12");
            }
            _ => panic!("expected a Confirm for eve"),
        }

        let (tx, rx) = mpsc::channel();
        let ctx = egui::Context::default();
        let forged = SharingCall::Confirm { email: "eve@x.io".into(), key_sha256: "ffff".into() };
        assert!(!request(&mut pane, None, &ctx, &tx, forged));
        assert_eq!(pane.notice.as_deref(), Some(CHECK_FINGERPRINT_FIRST));
        assert!(pane.fingerprint.is_some(), "a refused forgery spends nothing");
        let unseen = SharingCall::Confirm { email: "bob@x.io".into(), key_sha256: "ab12".into() };
        assert!(!request(&mut pane, None, &ctx, &tx, unseen));
        assert_eq!(pane.notice.as_deref(), Some(CHECK_FINGERPRINT_FIRST));
        assert!(rx.try_recv().is_err(), "nothing was started");

        let call = pane.share_anyway("eve@x.io").expect("a Confirm");
        assert!(!request(&mut pane, None, &ctx, &tx, call));
        assert_eq!(pane.notice.as_deref(), Some(SHARE_UNAVAILABLE_TOOLTIP), "the matching Confirm reaches the line check");
        assert!(pane.fingerprint.is_some(), "nothing was sent, so nothing was spent");

        let call = pane.share_anyway("eve@x.io").expect("a Confirm");
        assert!(request(&mut pane, Some(&dead_line()), &ctx, &tx, call));
        assert!(pane.notice.is_none(), "a write clears the last sentence");
        assert!(pane.fingerprint.is_none(), "a Confirm spends the fingerprint it was built from");
        assert!(pane.share_anyway("eve@x.io").is_none());
        match rx.recv_timeout(std::time::Duration::from_secs(30)).expect("a report") {
            SharingReport::Done(Err(_)) => {}
            other => panic!("expected the confirm's failure, got {other:?}"),
        }
    }

    /// A client whose daemon is gone: a port that was bound and let go, so a
    /// call on it is refused at once, and nothing else can be listening.
    fn dead_line() -> SharingClient {
        let port = std::net::TcpListener::bind("127.0.0.1:0").expect("a port").local_addr().expect("its address").port();
        SharingClient::new(&format!("http://127.0.0.1:{port}"), &Zeroizing::new("grant".to_string()))
    }

    /// **A failed write's sentence outlives the view read it causes.** Every
    /// report marks the view stale, so the frame after a failure asks for the
    /// view -- and if asking cleared the notice, the sentence would be on
    /// screen for one frame. Only a new write clears it.
    #[test]
    fn a_failed_writes_sentence_outlives_the_view_read_it_causes() {
        let mut pane = SharingPane::default();
        pane.view = Some(SharingView { available: true, ..SharingView::default() });
        apply(&mut pane, SharingReport::Done(Err("Couldn't stop sharing.".into())), 0.0);
        assert!(pane.wants_view(0.0));
        let (tx, rx) = mpsc::channel();
        let ctx = egui::Context::default();
        let line = dead_line();
        assert!(request(&mut pane, Some(&line), &ctx, &tx, SharingCall::View));
        assert_eq!(pane.notice.as_deref(), Some("Couldn't stop sharing."));
        assert!(!pane.wants_view(0.0), "one view read at a time");
        let report = rx.recv_timeout(std::time::Duration::from_secs(30)).expect("a report");
        assert!(matches!(report, SharingReport::View(Err(_))), "{report:?}");
        apply(&mut pane, report, 1.0);
        assert_eq!(pane.notice.as_deref(), Some("Couldn't stop sharing."), "a failed view read says nothing over it");
        assert!(pane.view.is_some(), "the view on screen stays");
        assert!(request(&mut pane, Some(&line), &ctx, &tx, SharingCall::Reinvite("anna@x.io".into())));
        assert!(pane.notice.is_none(), "the next write clears it");
        let _ = rx.recv_timeout(std::time::Duration::from_secs(30));
    }

    /// **No sentence of the engine's reaches a `Debug` string**: a refusal
    /// can name a decrypted record ("Couldn't stop sharing Acme VPN ..."),
    /// and a report printed with `{:?}` is one `log::` line away from a file.
    /// The variant and Ok/Err are enough to debug by.
    #[test]
    fn a_reports_debug_never_carries_the_engines_sentence() {
        let secret = "Couldn't stop sharing Acme VPN with anna@x.io";
        let reports = [
            SharingReport::View(Err(secret.into())),
            SharingReport::Shared { email: "anna@x.io".into(), item_name: "Acme VPN".into(), result: Err(secret.into()) },
            SharingReport::Stopped { email: "anna@x.io".into(), result: Err(secret.into()) },
            SharingReport::Done(Err(secret.into())),
            SharingReport::Forgot(Err(secret.into())),
            SharingReport::Forgot(Ok(vec!["Acme VPN".into()])),
            SharingReport::Fingerprint { email: "anna@x.io".into(), result: Err(secret.into()) },
            SharingReport::InviteCode(Err(secret.into())),
        ];
        for report in &reports {
            let text = format!("{report:?}");
            assert!(!text.contains("Acme VPN"), "{text}");
            assert!(!text.contains("anna@x.io"), "{text}");
        }
    }

    /// **A panicking call reports as its own kind**, so the state that call
    /// set is cleared: a view that panicked lets the next change ask again
    /// (a generic failure would leave `fetching` set, and the window would
    /// never read the view again), and a mint that panicked stops the
    /// dialog's spinner.
    #[test]
    fn a_call_that_fails_outright_reports_as_its_own_kind() {
        let mut pane = SharingPane::default();
        pane.begin_view();
        apply(&mut pane, SharingCall::View.failed(PANICKED), 0.0);
        assert!(!pane.wants_view(0.0), "a failed read is not retried every frame");
        pane.invalidate();
        assert!(pane.wants_view(0.0), "the next change asks again: the read is not stuck in flight");
        assert!(pane.notice.is_none(), "a view that could not be read says nothing on the card");

        pane.dialog = Some(ShareDialog::opening("r1", "Acme VPN"));
        if let Some(dialog) = pane.dialog.as_mut() {
            dialog.minting = true;
            dialog.password.push_str("hunter2");
        }
        let mint = SharingCall::InviteCode { email: "carol@x.io".into(), password: Zeroizing::new("hunter2".into()) };
        apply(&mut pane, mint.failed(PANICKED), 1.0);
        let dialog = pane.dialog.as_ref().expect("the dialog stays");
        assert!(!dialog.minting);
        assert!(dialog.password.is_empty(), "the typed password is wiped whatever the answer");
        assert_eq!(dialog.error.as_deref(), Some(PANICKED));

        let stop = SharingCall::Stop { item_id: "r1".into(), email: "anna@x.io".into() };
        pane.stopping = Some(("r1".into(), "anna@x.io".into()));
        apply(&mut pane, stop.failed(PANICKED), 2.0);
        assert!(pane.stopping_for("r1").is_none());
        assert_eq!(pane.notice.as_deref(), Some(PANICKED));
    }

    /// The rail's numbers: one per (record, person) in Sent, one per record
    /// in Shared with me, and the dot while an invitation waits.
    #[test]
    fn the_rail_counts_come_from_the_view() {
        let mut pane = SharingPane::default();
        assert_eq!((pane.sent_pairs(), pane.received_count(), pane.has_incoming()), (0, 0, false));
        let mut v = view(
            vec![("anna@x.io", PersonState::Shared, None), ("bob@x.io", PersonState::WaitingAccept, None)],
            vec![("r1", vec!["anna@x.io", "bob@x.io"]), ("r2", vec!["anna@x.io"])],
        );
        v.received.push(crate::rest::sharing::ReceivedEntry { item_id: "t1".into(), from: "d@x.io".into() });
        v.incoming.push(crate::rest::sharing::IncomingEntry { id: "i1".into(), from: "e@x.io".into(), expires_at: None });
        pane.view = Some(v);
        assert_eq!((pane.sent_pairs(), pane.received_count(), pane.has_incoming()), (3, 1, true));
        pane.stop_confirming = Some(("r1".into(), "bob@x.io".into()));
        assert_eq!(pane.confirming_for("r1"), Some("bob@x.io"));
        assert_eq!(pane.confirming_for("r2"), None);
    }

    /// The forget question names the records that would stop, by name.
    #[test]
    fn records_shared_with_a_person_are_named() {
        let items: Vec<VaultItem> = vec![
            serde_json::from_value(serde_json::json!({ "id": "r1", "name": "Acme VPN" })).expect("item"),
            serde_json::from_value(serde_json::json!({ "id": "r2", "name": "Wi-Fi" })).expect("item"),
        ];
        let v = view(vec![("anna@x.io", PersonState::Shared, None)], vec![("r1", vec!["anna@x.io"]), ("r2", vec![])]);
        assert_eq!(records_shared_with(Some(&v), "Anna@X.io", &items), vec!["Acme VPN".to_string()]);
        assert!(records_shared_with(None, "anna@x.io", &items).is_empty());
    }

    /// **A second write while one is running is refused out loud**: no
    /// thread, `false` back so the caller can take its spinner down, and the
    /// reason on the card. Show fingerprint is a read and runs beside the
    /// write -- and its answer neither frees the write's slot nor marks the
    /// view stale.
    #[test]
    fn a_second_write_while_one_is_running_is_refused_out_loud() {
        let mut pane = SharingPane::default();
        pane.view = Some(view(vec![], vec![]));
        let (tx, rx) = mpsc::channel();
        let ctx = egui::Context::default();
        let line = dead_line();
        assert!(request(&mut pane, Some(&line), &ctx, &tx, SharingCall::Reinvite("anna@x.io".into())));
        let stop = SharingCall::Stop { item_id: "r1".into(), email: "bob@x.io".into() };
        assert!(!request(&mut pane, Some(&line), &ctx, &tx, stop), "a second write is refused");
        assert_eq!(pane.notice.as_deref(), Some(BUSY));
        assert!(pane.stopping_for("r1").is_none(), "a refused stop raises no spinner");
        assert!(request(&mut pane, Some(&line), &ctx, &tx, SharingCall::Fingerprint("anna@x.io".into())), "a read runs beside the write");

        let wait = std::time::Duration::from_secs(30);
        let reports = [rx.recv_timeout(wait).expect("a report"), rx.recv_timeout(wait).expect("a report")];
        assert!(rx.recv_timeout(std::time::Duration::from_secs(3)).is_err(), "the refused stop started nothing");
        let [a, b] = reports;
        let (fingerprint, write) = if matches!(a, SharingReport::Fingerprint { .. }) { (a, b) } else { (b, a) };
        assert!(matches!(fingerprint, SharingReport::Fingerprint { .. }), "{fingerprint:?}");
        assert!(matches!(write, SharingReport::Done(_)), "{write:?}");

        apply(&mut pane, fingerprint, 0.0);
        assert!(!pane.wants_view(0.0), "a fingerprint changes nothing the view shows");
        assert!(!request(&mut pane, Some(&line), &ctx, &tx, SharingCall::Reinvite("bob@x.io".into())), "the write still holds the slot");
        apply(&mut pane, write, 1.0);
        assert!(pane.wants_view(1.0), "a write's answer does mark the view stale");
        assert!(request(&mut pane, Some(&line), &ctx, &tx, SharingCall::Reinvite("bob@x.io".into())), "and frees the slot");
        let _ = rx.recv_timeout(wait);
    }

    /// **A failed view read is asked again after [`VIEW_RETRY_SECONDS`]** --
    /// not on the next frame (that is a busy loop against a failing daemon)
    /// and not never (with push off, nothing else would ask). A read that
    /// works clears the retry; a refresh of a view already held that fails is
    /// retried the same way.
    #[test]
    fn a_failed_view_read_is_asked_again_after_the_retry_interval() {
        let mut pane = SharingPane::default();
        assert!(pane.wants_view(0.0));
        pane.begin_view();
        apply(&mut pane, SharingReport::View(Err("no".into())), 100.0);
        assert!(!pane.wants_view(100.0), "not on the next frame");
        assert!(!pane.wants_view(100.0 + VIEW_RETRY_SECONDS - 0.5), "not before the interval");
        assert!(pane.wants_view(100.0 + VIEW_RETRY_SECONDS), "once it has passed");
        pane.begin_view();
        assert!(!pane.wants_view(1_000.0), "one read at a time, however late");
        apply(&mut pane, SharingReport::View(Err("no".into())), 131.0);
        assert!(!pane.wants_view(131.0 + VIEW_RETRY_SECONDS - 0.5), "a second failure waits again");
        pane.begin_view();
        apply(&mut pane, SharingReport::View(Ok(view(vec![], vec![]))), 170.0);
        assert!(!pane.wants_view(1_000_000.0), "a view in hand is not re-read on a timer");

        pane.invalidate();
        pane.begin_view();
        apply(&mut pane, SharingReport::View(Err("no".into())), 200.0);
        assert!(pane.view.is_some(), "the view on screen stays");
        assert!(!pane.wants_view(200.0 + VIEW_RETRY_SECONDS - 0.5));
        assert!(pane.wants_view(200.0 + VIEW_RETRY_SECONDS), "a failed refresh is retried too");
    }

    /// **A share's answer says where it stands even with the dialog closed**:
    /// the record's name travels with the call, and "no account yet" -- which
    /// the open dialog says in its own words -- becomes a toast.
    #[test]
    fn a_share_answer_with_the_dialog_closed_still_says_where_it_stands() {
        let mut pane = SharingPane::default();
        let shared = |email: &str, result| SharingReport::Shared { email: email.into(), item_name: "Acme VPN".into(), result: Ok(result) };
        apply(&mut pane, shared("bob@x.io", ShareResult::WaitingAccept), 0.0);
        assert_eq!(pane.toast.as_ref().map(|(t, _)| t.as_str()), Some("bob@x.io was added. Acme VPN appears in their vault once they accept."));
        apply(&mut pane, shared("carol@x.io", ShareResult::WaitingAccount), 1.0);
        assert_eq!(pane.toast.as_ref().map(|(t, _)| t.as_str()), Some("Waiting for carol@x.io to create an account."));

        pane.toast = None;
        pane.dialog = Some(ShareDialog { in_flight: true, ..ShareDialog::opening("r1", "Acme VPN") });
        apply(&mut pane, shared("carol@x.io", ShareResult::WaitingAccount), 2.0);
        assert!(pane.toast.is_none(), "the open dialog says it instead");
        assert_eq!(pane.dialog.as_ref().and_then(|d| d.waiting_account.as_deref()), Some("carol@x.io"));
    }

    /// **Re-reading the address book keeps what this window could not
    /// save.** The daemon's re-keyed file is read, this window's unsaved
    /// Send is saved on top of it (not over it), and a forget that never
    /// reached the file is carried out there too. Where the file still
    /// cannot be written, the changes are applied in memory and kept for the
    /// next attempt.
    #[test]
    fn reloading_the_sent_history_keeps_what_this_window_could_not_save() {
        use crate::sent_history::SentRecord;
        let entry = |id: &str, to: &str| SentRecord { item_id: id.into(), recipient: to.into(), send_id: format!("s-{id}"), ..SentRecord::default() };
        let dir = crate::test_scratch::ScratchDir::new("share-ui-sent-reload");
        let path = dir.path().join("sent.json");
        crate::sent_history::update(&path, |h| {
            h.record(entry("rekeyed", "anna@x.io"));
            h.record(entry("old", "gone@x.io"));
        })
        .expect("the daemon's file");
        let mut unsaved = vec![UnsavedSent::Record(entry("mine", "bob@x.io")), UnsavedSent::Forget("gone@x.io".into())];
        let history = reload_sent_history(&path, &mut unsaved);
        let ids = |h: &SentHistory| {
            let mut ids: Vec<String> = h.entries.iter().map(|e| e.item_id.clone()).collect();
            ids.sort();
            ids
        };
        assert_eq!(ids(&history), vec!["mine".to_string(), "rekeyed".to_string()]);
        assert!(unsaved.is_empty(), "saved, so nothing is held");
        assert_eq!(ids(&SentHistory::load(&path)), ids(&history), "and the file has them");

        // A path under a FILE cannot be written: the change stays held, and
        // what the window shows still has it.
        let blocker = dir.path().join("not-a-dir");
        std::fs::write(&blocker, b"x").expect("a file");
        let stuck = blocker.join("sent.json");
        let mut unsaved = vec![UnsavedSent::Record(entry("mine", "bob@x.io"))];
        let history = reload_sent_history(&stuck, &mut unsaved);
        assert_eq!(ids(&history), vec!["mine".to_string()]);
        assert_eq!(unsaved.len(), 1, "kept for the next attempt");
    }

    /// A harness frame: the dialog drawn over a 1100x800 window, and every
    /// string it painted.
    fn modal_frame(dialog: &mut ShareDialog, history: &SentHistory, view: Option<&SharingView>, events: Vec<egui::Event>) -> (ShareDialogAction, Vec<(String, egui::Rect)>) {
        let ctx = egui::Context::default();
        let input = |events: Vec<egui::Event>| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 800.0))),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input(Vec::new()), |_ui| {});
        theme::apply(&ctx);
        // TWO frames of the modal before the measured one. The first is the
        // anchored area's sizing pass, which paints nothing and registers no
        // widget; egui hit-tests a press against the widgets of the frame
        // BEFORE it, so a click on the frame after the sizing pass lands on
        // nothing. The second is the one a click is tested against.
        for _ in 0..2 {
            let _ = ctx.run_ui(input(Vec::new()), |ui| {
                let _ = draw_share_modal(ui.ctx(), dialog, None, None, history, view);
            });
        }
        let mut action = ShareDialogAction::None;
        let output = ctx.run_ui(input(events), |ui| {
            action = draw_share_modal(ui.ctx(), dialog, None, None, history, view);
        });
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_texts(&clipped.shape, &mut texts);
        }
        (action, texts)
    }

    fn collect_texts(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => out.push((text.galley.text().to_owned(), text.visual_bounding_rect())),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect_texts(s, out)),
            _ => {}
        }
    }

    fn click(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
        ]
    }

    /// **Mock 6a**: the record, the WITH field, what view-only means, Share
    /// and Cancel -- and nothing about organisations anywhere.
    #[test]
    fn the_share_dialog_says_what_the_mock_says_and_never_names_an_organisation() {
        let mut dialog = ShareDialog::opening("r1", "Acme VPN");
        let (_, texts) = modal_frame(&mut dialog, &SentHistory::default(), None, Vec::new());
        let has = |needle: &str| texts.iter().any(|(t, _)| t.contains(needle));
        assert!(has("Share Acme VPN"), "{texts:?}");
        assert!(has("RECORD") && has("WITH"));
        assert!(has("They get view-only access"));
        assert!(has(SHARE_BUTTON) && has(CANCEL_BUTTON));
        assert!(has("Appears in Sent"), "6a's footer note: {texts:?}");
        assert!(!texts.iter().any(|(t, _)| t.to_lowercase().contains("organi")), "the dialog names an organisation: {texts:?}");
    }

    /// Pressing Share with an address reports `Submit`; with none, nothing.
    #[test]
    fn share_submits_only_with_an_address() {
        let mut dialog = ShareDialog::opening("r1", "Acme VPN");
        let (_, texts) = modal_frame(&mut dialog, &SentHistory::default(), None, Vec::new());
        let at = texts.iter().find(|(t, _)| t == SHARE_BUTTON).map(|(_, r)| r.center()).expect("the Share button");
        let (action, _) = modal_frame(&mut dialog, &SentHistory::default(), None, click(at));
        assert_eq!(action, ShareDialogAction::None, "an empty address does not submit");
        dialog.email = "anna@example.com".into();
        let (action, _) = modal_frame(&mut dialog, &SentHistory::default(), None, click(at));
        assert_eq!(action, ShareDialogAction::Submit);
    }

    /// **Enter in the address box shares**, as the ↵ on the button says.
    #[test]
    fn enter_in_the_address_box_shares() {
        let mut dialog = ShareDialog::opening("r1", "Acme VPN");
        dialog.email = "anna@example.com".into();
        let enter = vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }];
        let (action, _) = modal_frame(&mut dialog, &SentHistory::default(), None, enter);
        assert_eq!(action, ShareDialogAction::Submit, "the box opens focused, and Enter in it shares");
    }

    /// **No account** (mock 6a, fourth state): the dialog stays open to say
    /// so, and an administrator is offered an invite code.
    #[test]
    fn a_person_without_an_account_is_said_and_an_admin_can_make_an_invite_code() {
        let mut dialog = ShareDialog::opening("r1", "Acme VPN");
        dialog.waiting_account = Some("carol@example.com".into());
        let admin = SharingView { available: true, can_mint_invites: true, ..SharingView::default() };
        let (_, texts) = modal_frame(&mut dialog, &SentHistory::default(), Some(&admin), Vec::new());
        let has = |needle: &str| texts.iter().any(|(t, _)| t.contains(needle));
        assert!(has("Waiting for carol@example.com to create an account"), "{texts:?}");
        assert!(has(MAKE_CODE_BUTTON));
        assert!(has(DONE_BUTTON));
        assert!(!has(SHARE_BUTTON) || !texts.iter().any(|(t, _)| t == SHARE_BUTTON), "Done replaces Share and Cancel");
        let plain = SharingView { available: true, ..SharingView::default() };
        let (_, texts) = modal_frame(&mut dialog, &SentHistory::default(), Some(&plain), Vec::new());
        assert!(!texts.iter().any(|(t, _)| t == MAKE_CODE_BUTTON), "only an administrator can mint");
    }

    /// **An administrator is offered a code only for somebody with no
    /// account**: the invite code is per pending invitation, and before a
    /// share has left one there is no invitation to make it for.
    #[test]
    fn an_invite_code_is_offered_only_once_a_share_is_waiting_for_an_account() {
        let admin = SharingView { available: true, can_mint_invites: true, ..SharingView::default() };
        let mut dialog = ShareDialog::opening("r1", "Acme VPN");
        let (_, texts) = modal_frame(&mut dialog, &SentHistory::default(), Some(&admin), Vec::new());
        assert!(!texts.iter().any(|(t, _)| t == MAKE_CODE_BUTTON), "{texts:?}");
    }

    /// **NodeWarden sends no email** (plan decision 4), so the band never
    /// says one was sent -- to an administrator or to anyone else -- and says
    /// what does happen: an invite code.
    #[test]
    fn the_no_account_band_says_an_invite_code_and_never_an_email() {
        for admin in [false, true] {
            let (head, body) = no_account_band("carol@example.com", "Acme VPN", admin);
            let all = format!("{head} {body}").to_lowercase();
            assert!(!all.contains("email") && !all.contains("e-mail"), "{all}");
            assert!(all.contains("invite code"), "{all}");
            assert!(body.contains("Acme VPN") && head.contains("carol@example.com"));
        }
        assert!(no_account_band("c@x.io", "R", false).1.contains("administrator"), "a non-admin is told who makes codes");
    }

    /// **An invite code, once made, is on the band with a Copy** -- and the
    /// password box it was made with is gone.
    #[test]
    fn a_made_invite_code_is_shown_with_a_copy() {
        let admin = SharingView { available: true, can_mint_invites: true, ..SharingView::default() };
        let mut dialog = ShareDialog::opening("r1", "Acme VPN");
        dialog.waiting_account = Some("carol@example.com".into());
        dialog.invite_code = Some(InviteCode { code: "K7Q2-99XA".into(), link: None, expires_at: None });
        let (_, texts) = modal_frame(&mut dialog, &SentHistory::default(), Some(&admin), Vec::new());
        assert!(texts.iter().any(|(t, _)| t.contains("K7Q2-99XA")), "{texts:?}");
        assert!(texts.iter().any(|(t, _)| t == COPY_CODE_BUTTON));
        assert!(!texts.iter().any(|(t, _)| t == MAKE_CODE_BUTTON));
        let at = texts.iter().find(|(t, _)| t == COPY_CODE_BUTTON).map(|(_, r)| r.center()).expect("Copy");
        let (action, _) = modal_frame(&mut dialog, &SentHistory::default(), Some(&admin), click(at));
        assert_eq!(action, ShareDialogAction::CopyCode);
        assert_eq!(dialog.invite_code.as_ref().map(invite_text), Some("K7Q2-99XA"));
    }

    /// The suggestion's count says which kinds a person has had: shares,
    /// Sends, or both (mock 6a: "2 shared", "3 sent").
    #[test]
    fn a_suggestion_counts_shares_and_sends() {
        let person = |sent, shared| crate::sent_history::Person { email: "a@x.io".into(), last_sent_unix_millis: 0, send_ids: Vec::new(), sent, shared };
        assert_eq!(person_count_words(&person(0, 2)), "2 shared");
        assert_eq!(person_count_words(&person(3, 0)), "3 sent");
        assert_eq!(person_count_words(&person(3, 2)), "2 shared, 3 sent");
    }

    /// **A refused request takes the dialog's spinner down at once, and the
    /// dialog says why.** `request` answers `false` with its sentence in the
    /// pane's notice and no report coming -- so nothing else would ever end
    /// the spinner. Here: no line to the daemon, then a write already out.
    #[test]
    fn a_refused_request_never_leaves_the_dialogs_spinner_up() {
        let (tx, rx) = mpsc::channel();
        let ctx = egui::Context::default();

        let mut pane = SharingPane::default();
        let mut dialog = ShareDialog::opening("r1", "Acme VPN");
        dialog.email = " anna@x.io ".into();
        let left = answer_dialog(&mut pane, dialog, ShareDialogAction::Submit, None, &ctx, &tx);
        assert_eq!(left, ShareDialogAction::None);
        let dialog = pane.dialog.as_ref().expect("the dialog stays open");
        assert!(!dialog.in_flight, "no line to the daemon: the spinner is down");
        assert_eq!(dialog.error.as_deref(), Some(SHARE_UNAVAILABLE_TOOLTIP));
        assert!(pane.notice.is_none(), "said once, in the dialog");

        let line = dead_line();
        assert!(request(&mut pane, Some(&line), &ctx, &tx, SharingCall::Reinvite("bob@x.io".into())));
        let dialog = pane.dialog.take().expect("the dialog");
        let _ = answer_dialog(&mut pane, dialog, ShareDialogAction::Submit, Some(&line), &ctx, &tx);
        let dialog = pane.dialog.as_ref().expect("the dialog stays open");
        assert!(!dialog.in_flight, "a second write: the spinner is down");
        assert_eq!(dialog.error.as_deref(), Some(BUSY));

        let mut dialog = pane.dialog.take().expect("the dialog");
        dialog.waiting_account = Some("carol@x.io".into());
        dialog.password.push_str("hunter2");
        let _ = answer_dialog(&mut pane, dialog, ShareDialogAction::MintCode, Some(&line), &ctx, &tx);
        let dialog = pane.dialog.as_ref().expect("the dialog stays open");
        assert!(!dialog.minting, "a refused mint: the spinner is down");
        assert!(dialog.password.is_empty(), "the password is gone whatever the answer");
        assert_eq!(dialog.error.as_deref(), Some(BUSY));
        let _ = rx.recv_timeout(std::time::Duration::from_secs(30));
    }

    /// **A share that starts raises the spinner and sends the trimmed
    /// address**; its failure comes back into the dialog and takes it down.
    #[test]
    fn a_share_that_starts_raises_the_spinner_and_its_answer_takes_it_down() {
        let (tx, rx) = mpsc::channel();
        let ctx = egui::Context::default();
        let mut pane = SharingPane::default();
        let mut dialog = ShareDialog::opening("r1", "Acme VPN");
        dialog.email = " anna@x.io ".into();
        dialog.error = Some("an old sentence".into());
        let _ = answer_dialog(&mut pane, dialog, ShareDialogAction::Submit, Some(&dead_line()), &ctx, &tx);
        let dialog = pane.dialog.as_ref().expect("the dialog stays open while the share runs");
        assert!(dialog.in_flight);
        assert!(dialog.error.is_none(), "a new attempt clears the last one's sentence");
        let report = rx.recv_timeout(std::time::Duration::from_secs(30)).expect("a report");
        match &report {
            SharingReport::Shared { email, item_name, result: Err(_) } => {
                assert_eq!((email.as_str(), item_name.as_str()), ("anna@x.io", "Acme VPN"));
            }
            other => panic!("expected the share's failure, got {other:?}"),
        }
        apply(&mut pane, report, 1.0);
        let dialog = pane.dialog.as_ref().expect("a failed share keeps the dialog");
        assert!(!dialog.in_flight && dialog.error.is_some());
    }

    /// **Minting sends the pending invitation's address and the password,
    /// and the dialog's box is emptied as it goes** -- the one copy of the
    /// master password is in the call, and is wiped when the call is.
    #[test]
    fn minting_takes_the_password_out_of_the_dialog() {
        let (tx, rx) = mpsc::channel();
        let ctx = egui::Context::default();
        let mut pane = SharingPane::default();
        let mut dialog = ShareDialog::opening("r1", "Acme VPN");
        dialog.waiting_account = Some("carol@x.io".into());
        dialog.password.push_str("hunter2");
        let _ = answer_dialog(&mut pane, dialog, ShareDialogAction::MintCode, Some(&dead_line()), &ctx, &tx);
        let dialog = pane.dialog.as_ref().expect("the dialog");
        assert!(dialog.minting);
        assert!(dialog.password.is_empty());
        let report = rx.recv_timeout(std::time::Duration::from_secs(30)).expect("a report");
        assert!(matches!(report, SharingReport::InviteCode(Err(_))), "{report:?}");
        apply(&mut pane, report, 1.0);
        assert!(!pane.dialog.as_ref().expect("the dialog").minting);
    }

    /// Cancel and Done close the dialog -- dropping it, which wipes a
    /// half-typed password -- and a suggestion's ✕ is handed to the window,
    /// which owns the forget question, with the dialog left open under it.
    #[test]
    fn cancel_and_done_close_and_a_forget_is_the_windows() {
        let (tx, _rx) = mpsc::channel();
        let ctx = egui::Context::default();
        let mut pane = SharingPane::default();
        for close in [ShareDialogAction::Cancel, ShareDialogAction::Done] {
            let _ = answer_dialog(&mut pane, ShareDialog::opening("r1", "Acme VPN"), close, None, &ctx, &tx);
            assert!(pane.dialog.is_none());
        }
        let left = answer_dialog(&mut pane, ShareDialog::opening("r1", "Acme VPN"), ShareDialogAction::Forget("a@x.io".into()), None, &ctx, &tx);
        assert_eq!(left, ShareDialogAction::Forget("a@x.io".into()));
        assert!(pane.dialog.is_some());
        let open = pane.dialog.take().expect("open");
        let left = answer_dialog(&mut pane, open, ShareDialogAction::CopyCode, None, &ctx, &tx);
        assert_eq!(left, ShareDialogAction::CopyCode);
        assert!(pane.dialog.is_some());
    }

    /// **A forget that failed says so in the dialog that asked** -- its
    /// sentence can name a decrypted record, so it goes to the screen and
    /// nowhere else. A dialog that did not ask (the forget came from the Send
    /// composer, or this dialog was opened afterwards) is left alone and the
    /// card's notice says it: the share answer's rule.
    #[test]
    fn a_failed_forget_is_said_in_the_dialog_that_asked_and_nowhere_else() {
        let failed = || SharingReport::Forgot(Err("Couldn't stop sharing Wi-Fi.".into()));
        let mut pane = SharingPane::default();
        pane.dialog = Some(ShareDialog::opening("r1", "Acme VPN"));
        pane.begin_forget("anna@x.io", true);
        apply(&mut pane, failed(), 0.0);
        let dialog = pane.dialog.as_ref().expect("open");
        assert_eq!(dialog.error.as_deref(), Some("Couldn't stop sharing Wi-Fi."));
        assert!(!dialog.forget_asked, "answered: the flag is spent");
        assert!(pane.notice.is_none());

        let mut pane = SharingPane::default();
        pane.dialog = Some(ShareDialog::opening("r1", "Acme VPN"));
        pane.begin_forget("anna@x.io", false);
        apply(&mut pane, failed(), 0.0);
        assert!(pane.dialog.as_ref().is_some_and(|d| d.error.is_none()), "the composer's forget is not this dialog's");
        assert_eq!(pane.notice.as_deref(), Some("Couldn't stop sharing Wi-Fi."));

        let mut pane = SharingPane::default();
        apply(&mut pane, failed(), 0.0);
        assert_eq!(pane.notice.as_deref(), Some("Couldn't stop sharing Wi-Fi."));
    }

    /// **The daemon's forget is waited on, and its outcome is kept** for the
    /// window's forget question: `Waiting` until the answer, then `Stopped`
    /// or `Failed` -- for that person only -- until the question ends.
    #[test]
    fn the_daemons_forget_is_waited_on_and_its_outcome_kept() {
        let mut pane = SharingPane::default();
        assert_eq!(pane.daemon_forget("anna@x.io"), DaemonForget::NotAsked);
        pane.begin_forget("Anna@X.io", false);
        assert_eq!(pane.daemon_forget("anna@x.io"), DaemonForget::Waiting);
        assert_eq!(pane.daemon_forget("bob@x.io"), DaemonForget::NotAsked);
        apply(&mut pane, SharingReport::Forgot(Ok(vec!["Acme VPN".into()])), 0.0);
        assert_eq!(pane.daemon_forget("anna@x.io"), DaemonForget::Stopped);
        pane.end_forget();
        assert_eq!(pane.daemon_forget("anna@x.io"), DaemonForget::NotAsked);

        pane.begin_forget("anna@x.io", false);
        apply(&mut pane, SharingReport::Forgot(Err("no".into())), 1.0);
        assert_eq!(pane.daemon_forget("anna@x.io"), DaemonForget::Failed);
    }

    /// **An answer lands only in the dialog that asked.** A share closed with
    /// Cancel and then a fresh dialog opened -- on this record or another --
    /// must not have the old share's "no account yet" or failure painted into
    /// it; the answer is said as a toast or a notice instead.
    #[test]
    fn an_answer_does_not_land_in_a_dialog_that_did_not_ask() {
        let mut pane = SharingPane::default();
        pane.dialog = Some(ShareDialog::opening("r2", "Wi-Fi"));
        apply(&mut pane, SharingReport::Shared { email: "carol@x.io".into(), item_name: "Acme VPN".into(), result: Ok(ShareResult::WaitingAccount) }, 0.0);
        assert!(pane.dialog.as_ref().is_some_and(|d| d.waiting_account.is_none()));
        assert!(pane.toast.is_some());
        apply(&mut pane, SharingReport::Shared { email: "b@x.io".into(), item_name: "Acme VPN".into(), result: Err("no".into()) }, 1.0);
        assert!(pane.dialog.as_ref().is_some_and(|d| d.error.is_none()));
        assert_eq!(pane.notice.as_deref(), Some("no"));
        apply(&mut pane, SharingReport::InviteCode(Ok(InviteCode { code: "C".into(), link: None, expires_at: None })), 2.0);
        assert!(pane.dialog.as_ref().is_some_and(|d| d.invite_code.is_none()), "a code nobody in this dialog asked for");
    }

    /// **The worker reaches the daemon** through the real loopback listener:
    /// the one `std::thread::spawn` in this file carries a call and brings
    /// the report back on the channel.
    #[test]
    fn a_sharing_call_on_the_worker_thread_reaches_the_daemon_and_reports_back() {
        use crate::rest::backend::tests::{sharing_profile, signed_in_with_no_sync_route, sync_payload};
        let (mut server, backend) = signed_in_with_no_sync_route();
        server.mock("GET", "/api/accounts/revision-date").with_body("1700000000000").expect_at_least(0).create();
        server.mock("GET", "/api/sync?excludeDomains=true").with_body(sync_payload()).expect_at_least(0).create();
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).create();
        server.mock("GET", "/api/users/user-1/public-key")
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .create();
        backend.sharing_engine().set_up(crate::rest::sharing::SharingSetup::none());
        let cache = std::sync::Arc::new(crate::vault_cache::VaultCache::new(backend));
        assert_eq!(cache.populate().expect("loaded"), crate::vault_cache::PopulateOutcome::Populated);
        let api = std::sync::Arc::new(crate::vault_api::VaultApi::new(std::sync::Arc::clone(&cache), std::sync::Arc::new(crate::change_feed::ChangeFeed::with_instance(9))));
        api.sharing_pass(crate::rest::sharing::Trigger::WholeRead);
        let host = crate::vault_host::VaultHost::start(std::sync::Arc::clone(&api)).expect("a port");
        let link = crate::daemon_link::DaemonLink::from_line(&host.link_line_for("window")).expect("a line");
        let (tx, rx) = mpsc::channel();
        spawn_sharing_call(egui::Context::default(), tx, link.sharing(), SharingCall::View);
        match rx.recv_timeout(std::time::Duration::from_secs(20)).expect("a report") {
            SharingReport::View(Ok(view)) => assert!(view.available),
            other => panic!("expected the view, got {other:?}"),
        }
        drop(host);
    }
}

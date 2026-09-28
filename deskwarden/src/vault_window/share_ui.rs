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

// The header's Share tooltip: drawn by Task 16's Share control.
#[allow(dead_code)]
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

/// A person as the SHARING card lists them. Drawn by Task 17's card; until
/// then only this file's tests build one.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PersonLine {
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

/// **The people one record is shared with**, each with its status line.
/// Called by Task 17's SHARING card.
#[allow(dead_code)]
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
/// window that has not heard from the daemon yet. Called by Task 16's header.
#[allow(dead_code)]
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
/// (spec 11.7: records and Sends in one list). Called by Task 16's forget
/// question.
#[allow(dead_code)]
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

    /// Read by Task 17's card.
    #[allow(dead_code)]
    pub(crate) fn confirming_for(&self, item_id: &str) -> Option<&str> {
        self.stop_confirming.as_ref().filter(|(id, _)| id == item_id).map(|(_, e)| e.as_str())
    }

    /// Read by Task 17's card.
    #[allow(dead_code)]
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
}

/// **The Share dialog's state** (mock 6a). Drawn by Task 16's
/// `draw_share_modal`; [`apply`] and this file's tests are its only users
/// until then.
///
/// No `Debug`: it holds the typed master password (the crate's rule for a
/// body that mentions `Zeroizing`), and an [`InviteCode`], which is a
/// credential for registering.
#[allow(dead_code)]
pub(crate) struct ShareDialog {
    pub(crate) item_id: String,
    pub(crate) item_name: String,
    pub(crate) email: String,
    pub(crate) in_flight: bool,
    /// The last attempt's sentence.
    pub(crate) error: Option<String>,
    /// A share that left an invitation for an address with no account.
    pub(crate) waiting_account: Option<String>,
    /// The master password, typed to mint an invite code. Wiped on drop, and
    /// replaced by an empty one whatever the mint answers.
    pub(crate) password: Zeroizing<String>,
    pub(crate) minting: bool,
    pub(crate) invite_code: Option<InviteCode>,
    /// The ✕ on a suggestion: forget this person, asked first.
    pub(crate) forget: Option<String>,
}

impl ShareDialog {
    /// Opened by Task 16's Share control.
    #[allow(dead_code)]
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
            forget: None,
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
/// Only `View` has a caller in this task; the rest are sent by the cards
/// Tasks 16-20 draw (Share, Forget, InviteCode: 16; Stop, Reinvite,
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
        SharingReport::Shared { email, item_name, result } => match result {
            Ok(ShareResult::Shared) => {
                pane.dialog = None;
                toast(pane, shared_toast(&email));
            }
            Ok(ShareResult::WaitingAccept) => {
                pane.dialog = None;
                toast(pane, waiting_toast(&email, &item_name));
            }
            // The open dialog says it in its own words (mock 6a); with the
            // dialog closed before the answer came, a toast does.
            Ok(ShareResult::WaitingAccount) => match pane.dialog.as_mut() {
                Some(dialog) => {
                    dialog.in_flight = false;
                    dialog.waiting_account = Some(email);
                }
                None => toast(pane, waiting_account_toast(&email)),
            },
            Err(sentence) => {
                if let Some(dialog) = pane.dialog.as_mut() {
                    dialog.in_flight = false;
                    dialog.error = Some(sentence);
                } else {
                    pane.notice = Some(sentence);
                }
            }
        },
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
        SharingReport::Forgot(result) => {
            if let Err(sentence) = result {
                pane.notice = Some(sentence);
            }
        }
        SharingReport::InviteCode(result) => {
            if let Some(dialog) = pane.dialog.as_mut() {
                dialog.minting = false;
                dialog.password = Zeroizing::new(String::new());
                match result {
                    Ok(code) => dialog.invite_code = Some(code),
                    Err(sentence) => dialog.error = Some(sentence),
                }
            } else if let Err(sentence) = result {
                pane.notice = Some(sentence);
            }
        }
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
        pane.dialog = Some(ShareDialog::opening("r1", "Acme VPN"));
        apply(&mut pane, SharingReport::Shared { email: "anna@x.io".into(), item_name: "Acme VPN".into(), result: Ok(ShareResult::Shared) }, 1.0);
        assert!(pane.dialog.is_none());
        assert_eq!(pane.toast.as_ref().map(|(t, _)| t.as_str()), Some("Shared with anna@x.io. It is in their vault now."));
        assert!(pane.wants_view(0.0), "a write makes the view stale");
        pane.dialog = Some(ShareDialog::opening("r1", "Acme VPN"));
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
        pane.dialog = Some(ShareDialog::opening("r1", "Acme VPN"));
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

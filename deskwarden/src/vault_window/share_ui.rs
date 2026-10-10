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
//! the SHARING card, received rows in Shared with me -- is called from
//! `detail`, `send_ui` and `mod`, which own their panes. Sent's share rows and
//! a share's detail are the one exception: their data is here ([`ShareRow`],
//! [`PeopleRows`]) and `send_ui` draws them, in its own row and pane shapes.
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

use crate::daemon_link::{ShareAnswer, SharingClient};
use crate::local_time::LocalOffset;
use crate::rest::organizations::normalise_email;
use crate::rest::sharing::{Fingerprint, PersonState, ShareResult, SharingView};
use crate::rest::sharing_api::InviteCode;
use crate::sent_history::{SentHistory, SentKind};
use crate::theme;
use crate::vault_bridge::VaultItem;

use super::detail::ShareControl;

// ---- the words (mock 6e's STATUS LINES) -----------------------------------------

/// The header's Share tooltip (mock 6b's hover).
pub(crate) const SHARE_TOOLTIP: &str = "Share with a person \u{b7} view-only";
/// A sharing request from a window with no line to the daemon -- said on the
/// card by [`request`]. It no longer ends "This sends a link instead": no
/// Share-shaped control opens a Send any more (final review, I3), so the
/// sentence says only what is missing.
pub(crate) const SHARING_UNAVAILABLE: &str = "Sharing with a person needs direct sync with a self-hosted server.";
/// The greyed Share control's hover on a server too old to share
/// (`server-too-old`, official-path spec §2): a route sharing needs answered
/// as missing. Any server can be updated, so the sentence names none.
pub(crate) const SHARE_TOO_OLD_TOOLTIP: &str = "Sharing with a person needs a newer server.";
/// The Share dialog's warning where people a record is shared with can list
/// each other (official-path spec §3.4) --
/// [`SharingView::roster_visible_to_recipients`].
pub(crate) const ROSTER_VISIBLE_WARNING: &str = "On this server, people you share with can see each other's email addresses.";
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
/// A Show fingerprint while another is out -- [`request`]'s answer to it,
/// said rather than dropped. The card greys every Show while one is out, so
/// this is the sentence a wiring mistake would show.
pub(crate) const FINGERPRINT_BUSY: &str = "A fingerprint is still on its way. Try again when it arrives.";

pub(crate) fn shared_toast(email: &str) -> String {
    format!("Shared with {email}. It is in their vault now.")
}

pub(crate) fn waiting_toast(email: &str, record: &str) -> String {
    format!("{email} was added. {record} appears in their vault once they accept.")
}

/// Share anyway went through: the record is on its way to the key whose
/// fingerprint was compared.
pub(crate) fn confirmed_toast(email: &str) -> String {
    format!("Shared with {email} on their new key.")
}

/// Stop receiving went through: every record from them has left the vault.
pub(crate) fn stopped_receiving_toast(from: &str) -> String {
    format!("Stopped receiving from {from}.")
}

/// A Re-invite went through.
pub(crate) fn reinvited_toast(email: &str) -> String {
    format!("Invitation sent to {email}.")
}

/// A share the server left as a pending invitation, said as a toast when no
/// dialog is open to say it. It claims nothing about an account
/// (official-path spec §3.4).
pub(crate) fn invited_toast(email: &str) -> String {
    format!("Waiting for {email} to accept.")
}

const MILLIS_PER_DAY: i64 = 86_400_000;

// ---- one person on one record ---------------------------------------------------------

/// A person as the SHARING card lists them. Built for every record the read
/// pane draws (`detail::PeopleSharing::people`) and drawn by
/// [`draw_people_block`].
///
/// `pub` and not `pub(crate)` because `detail::PeopleSharing`, a field of
/// `detail::draw_detail_read`'s public signature, carries a slice of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonLine {
    pub(crate) email: String,
    pub(crate) state: PersonState,
    /// The status line under the address (mock 6b).
    pub(crate) line: String,
    /// The view's [`crate::rest::sharing::PersonEntry::authenticated`]: their
    /// collection's name was written under the org key. `false` is a person
    /// known only from a plaintext name the server could have written -- the
    /// card shows them, says so ([`UNVERIFIED_PREFIX`]), and offers Stop
    /// alone: Re-invite and Share anyway are grants (final review, I1).
    pub(crate) authenticated: bool,
}

/// What the status line of a person known only from a plaintext collection
/// name opens with. Shown rather than hidden: the record is filed where the
/// server says, so the row is who may see it, and Stop is the way to take it
/// back.
pub(crate) const UNVERIFIED_PREFIX: &str = "Not verified \u{b7} Deskwarden didn't name this person \u{b7} ";

impl PersonLine {
    pub(crate) fn pill(&self) -> theme::SharePillKind {
        match self.state {
            PersonState::Shared => theme::SharePillKind::Shared,
            PersonState::WaitingAccept | PersonState::WaitingSignIn | PersonState::Invited => theme::SharePillKind::Invited,
            PersonState::KeyChanged => theme::SharePillKind::KeyChanged,
            PersonState::Gone => theme::SharePillKind::Gone,
        }
    }

    /// Re-invite is offered where a fresh invitation helps -- a pending one,
    /// renewed with `POST …/users/{ouId}/reinvite` on every server, or a
    /// person who is gone (official-path spec §3.4) -- and never for an
    /// unverified person, whose Re-invite the engine refuses anyway
    /// (`Sharing::reinvite`): it can make them a recipient.
    pub(crate) fn reinvite(&self) -> bool {
        self.authenticated && matches!(self.state, PersonState::Invited | PersonState::Gone)
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
        self.authenticated && self.state == PersonState::KeyChanged
    }
}

/// `14 Sep` in the user's zone.
fn day(millis: i64, zone: &dyn LocalOffset) -> String {
    let parts = crate::local_time::local_parts(millis, zone);
    format!("{} {}", parts.day, crate::local_time::month_name(parts.month))
}

/// "invited 3 days ago" -- how long a person has been waiting to accept
/// (official-path spec §3.4). From this PC's own record of the share, as
/// "since" is ([`since`]): no server says when it invited anyone. No expiry
/// is claimed, because the server's setting is invisible to a client.
fn invited_ago(at: i64, now_ms: i64) -> String {
    match (now_ms - at).max(0) / MILLIS_PER_DAY {
        0 => "invited today".to_string(),
        1 => "invited 1 day ago".to_string(),
        days => format!("invited {days} days ago"),
    }
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
                // Invited and Accepted read alike (official-path spec §3.4):
                // the accept is the person's, and an Accepted one is
                // confirmed on the next pass.
                PersonState::WaitingAccept | PersonState::Invited => match since(history, item_id, &person.email) {
                    Some(at) => format!("Waiting to accept \u{b7} {}", invited_ago(at, now_ms)),
                    None => "Waiting to accept".to_string(),
                },
                PersonState::KeyChanged => "Their key changed. Check the fingerprint before sharing.".to_string(),
                PersonState::WaitingSignIn => "Waiting for them to sign in once to a Bitwarden app".to_string(),
                PersonState::Gone => "No longer receiving this".to_string(),
            };
            let line = if person.authenticated { line } else { format!("{UNVERIFIED_PREFIX}{line}") };
            PersonLine { email: person.email.clone(), state: person.state, line, authenticated: person.authenticated }
        })
        .collect()
}

/// The Sent row's short state (mock 6c: "to anna@example.com · Shared"),
/// for [`share_rows`].
pub(crate) fn short_state(state: PersonState) -> &'static str {
    match state {
        PersonState::Shared => "Shared",
        PersonState::WaitingAccept | PersonState::Invited => "Waiting to accept",
        PersonState::KeyChanged => "Key changed",
        PersonState::WaitingSignIn => "Waiting for a first sign-in",
        PersonState::Gone => "Not receiving",
    }
}

/// **The header's Share control** for one record, decided BY THE REASON the
/// view gives (final review, I3). Read by the header through
/// [`people_sharing`].
///
/// - Sharing available: [`ShareControl::Share`] -- unless the record cannot
///   be shared from here. One in another (real) organisation draws nothing:
///   it is that organisation's to share. One with attachments is greyed with
///   the engine's own sentence ([`crate::rest::sharing::HAS_ATTACHMENTS`]),
///   rather than refused after the user has typed an address.
/// - `server-too-old`: greyed, with [`SHARE_TOO_OLD_TOOLTIP`] -- sharing is
///   a thing this server will do once updated.
/// - `bw-serve`, `not-loaded`, any other reason, no view yet, no line to the
///   daemon, a received record: **nothing**. It used to offer a Send where
///   sharing was not available; the controller's call is
///   that the envelope beside it already is that door, so a Share-shaped
///   control that opens the Send composer is a duplicate under a wrong name.
///
/// Whatever this answers, `detail::header_controls_for` reserves room for
/// exactly what is painted.
pub(crate) fn share_control(view: Option<&SharingView>, linked: bool, item: &VaultItem) -> Option<ShareControl> {
    if !linked {
        return None;
    }
    let view = view?;
    if view.received_from(&item.id).is_some() {
        return None;
    }
    if !view.available {
        return match view.reason.as_deref() {
            Some(crate::rest::sharing::REASON_TOO_OLD) => Some(ShareControl::Disabled(SHARE_TOO_OLD_TOOLTIP)),
            _ => None,
        };
    }
    // In an organisation, and not in this account's sharing org (every
    // record there has a `records` entry, shared with anyone or not).
    let theirs = crate::rest::organizations::organization_of(item).is_some()
        && !view.records.iter().any(|r| r.item_id == item.id);
    if theirs {
        return None;
    }
    let attached = item.other.get("attachments").and_then(serde_json::Value::as_array).is_some_and(|a| !a.is_empty());
    if attached {
        return Some(ShareControl::Disabled(crate::rest::sharing::HAS_ATTACHMENTS));
    }
    Some(ShareControl::Share)
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
    /// When, in egui seconds, the last view read landed -- what
    /// [`Self::heartbeat`] ages the view by while no push can say it moved.
    viewed_at: Option<f64>,
    /// A write is in flight; one at a time. A fingerprint is a read and
    /// does not take the slot -- see [`request`].
    busy: bool,
    /// The Share dialog, while it is open.
    pub(crate) dialog: Option<ShareDialog>,
    /// **Paste an invite link's text**, between frames: lent to the list as
    /// [`PeopleRows::paste`] while it is drawn, and taken whole when Accept is
    /// pressed. Wiped on drop -- its token accepts the invitation.
    pub(crate) paste: Zeroizing<String>,
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
    /// **Where that person stood in the view when their fingerprint came
    /// back** -- `None` when the view did not list them. A view that answers
    /// with a DIFFERENT state for them drops the fingerprint ([`apply`]): a
    /// changed key's box must never show the words of the key before, and a
    /// state that moved is the one signal the window has that the key may
    /// have. (A key that changes twice with no state between is the engine's
    /// to catch: it confirms only the hash that was shown.)
    fingerprint_state: Option<PersonState>,
    /// **A fingerprint is being fetched** for this normalised email. The read
    /// has a slot of its own, one at a time: [`Self::fingerprint`] holds one
    /// answer, so two reads out at once would race for it, and the card
    /// greys every Show while this is set. Taken by [`request`], freed by the
    /// answer in [`apply`] -- a failure and a panicked worker included, since
    /// both come back as the call's own `Fingerprint` report.
    pub(crate) fingerprinting: Option<String>,
    /// A toast and when it goes, in egui seconds.
    pub(crate) toast: Option<(String, f64)>,
    /// A failed request's sentence, shown on the card until the next write.
    /// Shown, never logged: see the module doc. Set through [`Self::say`].
    pub(crate) notice: Option<String>,
    /// **The record [`Self::notice`] is about**, when it is a write's answer:
    /// the record on screen when that write was asked. `None` for a sentence
    /// about no record in particular -- a refusal, a busy slot, a fingerprint.
    /// The card shows a notice only on its own record, and one that arrives
    /// while another record is on screen is routed to a toast instead
    /// ([`route_stray_notice`]), so "Couldn't stop sharing" never stands on a
    /// record it is not about.
    notice_record: Option<String>,
    /// The record on screen when the write in flight was asked -- carried
    /// from [`request`] to its answer in [`apply`], which tags a failure with
    /// it. See [`request_on`].
    write_record: Option<String>,
    /// **The daemon's forget for the person the forget question is about**:
    /// their normalised email, and `None` until the answer comes, then
    /// whether it stopped every share. See [`Self::daemon_forget`].
    daemon_forget: Option<(String, Option<bool>)>,
    /// **A stop moved a record home under a new id**: `(old id, new id)`,
    /// from the stop's answer (`StopOutcome::moved_to`), for the window to
    /// re-point its selection -- the record the user was reading is still
    /// theirs, under another id (final review). Taken by
    /// [`Self::take_moved_home`].
    moved_home: Option<(String, String)>,
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

/// **How old the view may get while nothing can push a change**, in
/// seconds. With push off -- or the hub down -- the window hears of no
/// sharing change: an invitation accepted, a confirm the daemon's timer ran,
/// a record another PC shared. A view read is one loopback request answered
/// from the daemon's memory (spec 11.3), so asking once a minute costs
/// nothing worth saving (final review, I2). See [`SharingPane::heartbeat`].
pub(crate) const VIEW_HEARTBEAT_SECONDS: f64 = 60.0;

impl SharingPane {
    /// The last stop's move home, once: `(old id, new id)`. See
    /// `vault_window::mod`'s `follow_moved_home`.
    pub(crate) fn take_moved_home(&mut self) -> Option<(String, String)> {
        self.moved_home.take()
    }

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

    /// **The view ages while no push is live** (final review, I2): once it
    /// is [`VIEW_HEARTBEAT_SECONDS`] old it goes stale, and the frame's
    /// [`Self::wants_view`] asks again. With the hub live, a sharing change
    /// is pushed (`HubListener::sharing_changed`) and the view is left alone.
    ///
    /// No timer of its own and no repaint request: the vault window's frame
    /// schedule already wakes it every half second (`FRAME_INTERVAL`), so the
    /// check rides a frame that runs anyway, and the frame-schedule census is
    /// unchanged. A view never read, or one whose read failed, is
    /// [`Self::wants_view`]'s own business (`retry_at`): this stands down
    /// while a retry is pending, or a view that is old AND failing would be
    /// marked stale every frame -- and `stale` outranks `retry_at` there --
    /// turning a failing daemon into a request per frame.
    pub(crate) fn heartbeat(&mut self, now_seconds: f64, push_live: bool) {
        if !push_live && self.retry_at.is_none() && self.viewed_at.is_some_and(|at| now_seconds - at >= VIEW_HEARTBEAT_SECONDS) {
            self.stale = true;
        }
    }

    /// A view request was started.
    pub(crate) fn begin_view(&mut self) {
        self.fetching = true;
        self.stale = false;
    }

    // **`has_incoming` was here**, and the rail's dot read it. The dot reads
    // the frame's own waiting lines now -- what the list draws -- so the
    // badge = list rule holds for it as for the counts.

    /// A write holds the one slot ([`request`]): what greys Paste an invite
    /// link's Accept ([`PeopleRows::busy`]).
    pub(crate) fn is_busy(&self) -> bool {
        self.busy
    }

    /// Handed to the read pane by [`people_sharing`], for the SHARING card.
    pub(crate) fn confirming_for(&self, item_id: &str) -> Option<&str> {
        self.stop_confirming.as_ref().filter(|(id, _)| id == item_id).map(|(_, e)| e.as_str())
    }

    /// Handed to the read pane by [`people_sharing`], for the SHARING card.
    pub(crate) fn stopping_for(&self, item_id: &str) -> Option<&str> {
        self.stopping.as_ref().filter(|(id, _)| id == item_id).map(|(_, e)| e.as_str())
    }

    /// **The only way to a "Share anyway"**: a Confirm for `email`, carrying
    /// the key hash of the fingerprint this window was shown for them -- or
    /// `None` while no fingerprint for that person is held, or they are not
    /// a changed key in the view. The card's button sends this and nothing
    /// else (`vault_window::mod`'s `DetailAction::ShareAnyway` arm), so a
    /// Confirm always follows a Show fingerprint.
    ///
    /// The view's state is a display fact: its people come from collection
    /// names, and a name is not proof of who holds the key. The binding that
    /// matters is the hash, which the engine checks against the key it would
    /// confirm and refuses on a mismatch -- this is the window's half, the
    /// engine's is the one that cannot be bypassed.
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

    /// **The one way a sentence goes on the card**: the notice, and the
    /// record it is about -- `None` for one about no record in particular.
    /// Setting the two together is what keeps a tag from an earlier notice
    /// pinning a new one to the wrong record.
    pub(crate) fn say(&mut self, sentence: String, record: Option<String>) {
        self.notice = Some(sentence);
        self.notice_record = record;
    }

    /// The notice the card on `item_id` shows: one about that record, or
    /// about none.
    pub(crate) fn notice_for(&self, item_id: &str) -> Option<&str> {
        match &self.notice_record {
            Some(record) if record != item_id => None,
            _ => self.notice.as_deref(),
        }
    }

    /// Hide fingerprint: the words and the state they were fetched in go
    /// together, so Share anyway has nothing to confirm until Show runs again.
    pub(crate) fn hide_fingerprint(&mut self) {
        self.fingerprint = None;
        self.fingerprint_state = None;
    }

    /// A toast, for [`TOAST_SECONDS`] from `now_seconds` (egui's input time).
    fn toast(&mut self, text: String, now_seconds: f64) {
        self.toast = Some((text, now_seconds + TOAST_SECONDS));
    }
}

/// **A write's failure that belongs to another record goes to a toast.**
/// Called by the read pane's frame with the record on screen: a notice tagged
/// with a different record -- a stop asked on Acme VPN that failed after the
/// user moved on to Wi-Fi -- is said once in the corner, where it names
/// itself, rather than on a card it is not about or not at all.
pub(crate) fn route_stray_notice(pane: &mut SharingPane, item_id: &str, now_seconds: f64) {
    if pane.notice_record.as_deref().is_some_and(|record| record != item_id) {
        if let Some(sentence) = pane.notice.take() {
            pane.toast(sentence, now_seconds);
        }
        pane.notice_record = None;
    }
}

/// **Whether sending anything more to `email` should ask for the master
/// password first**: some record shared with them is re-prompt protected.
///
/// Share anyway and a Gone person's Re-invite do not send one record: they
/// grant the person's membership, and a membership reaches EVERY record
/// shared with them. So the question is asked of all of those records, not
/// of the one on screen -- a plain record's card must not be a door around
/// a protected one's prompt. No view, no records: nothing is sent anyway.
pub(crate) fn shares_a_protected_record(view: Option<&SharingView>, email: &str, items: &[VaultItem]) -> bool {
    let Some(view) = view else {
        return false;
    };
    let email = normalise_email(email);
    view.records
        .iter()
        .filter(|r| r.people.iter().any(|p| normalise_email(p) == email))
        .filter_map(|r| items.iter().find(|i| i.id == r.item_id))
        .any(crate::vault_bridge::reprompt_protected)
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
    /// A share that left the person invited, not yet accepted: on official
    /// Bitwarden and with invitation email on, they accept from the email's
    /// link; with no account yet, after they register (official-path spec
    /// §4.1).
    pub(crate) invited: Option<String>,
    /// **A share that stopped to ask** whether to use an organisation
    /// Deskwarden did not create (official-path spec §3.1, fix round 1):
    /// the dialog asks it with Yes and No until it is answered or closed.
    /// Closing answers nothing, and nothing is chosen for the user.
    pub(crate) adopt: Option<crate::rest::sharing::AdoptionQuestion>,
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
            invited: None,
            adopt: None,
            password: Zeroizing::new(String::new()),
            minting: false,
            invite_code: None,
            focused: false,
            forget_asked: false,
        }
    }
}

// ---- the calls, on one worker thread each --------------------------------------------

/// **How a share treats the organisation question** (official-path spec
/// §3.1, fix round 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OrgAnswer {
    /// A share as the user typed it: the daemon may stop and ask.
    Ask,
    /// Yes: the org is adopted, then the share runs again -- one worker,
    /// two requests, so nothing can come between them.
    Adopt(String),
    /// No: the share runs in a new org of its own.
    New,
}

/// **One request to the daemon's sharing routes.**
///
/// No `Debug`: `InviteCode` carries the master password. It stays
/// `Zeroizing` from the dialog's field to [`SharingClient::invite_code`]'s
/// borrowed `&str`, and is wiped when the call is dropped at the end of
/// [`run`] -- never copied into a plain `String` on the way.
///
/// `View`, and the Share dialog's three (Share, Forget, InviteCode), are the
/// window's and the dialog's; the rest are sent by the cards: Stop, Reinvite,
/// Fingerprint and Confirm by the SHARING card and Sent's share detail;
/// AcceptLink by Shared with me's Paste an invite link; Leave by a received
/// record's Stop receiving. AcceptLink's link, like InviteCode's password,
/// stays `Zeroizing` until the call is dropped at the end of [`run`].
pub(crate) enum SharingCall {
    View,
    /// `org` is the answer to the organisation question, when the dialog
    /// asked one ([`OrgAnswer`]).
    Share { item_id: String, item_name: String, email: String, org: OrgAnswer },
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
    /// Paste an invite link's Accept: the link as pasted, token and all.
    AcceptLink(Zeroizing<String>),
    /// Stop receiving: the sharer's org, and the sharer as the view names
    /// them -- for the toast that says it is done, which the daemon's plain
    /// "done" cannot name.
    Leave { organization_id: String, from: String },
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
            Self::Reinvite(email) => SharingReport::Reinvited { email: email.clone(), result: Err(err) },
            Self::Confirm { email, .. } => SharingReport::Confirmed { email: email.clone(), result: Err(err) },
            Self::AcceptLink(_) => SharingReport::LinkAccepted(Err(err)),
            Self::Leave { from, .. } => SharingReport::Left { from: from.clone(), result: Err(err) },
        }
    }

    /// The record a write carries in itself -- a stop's, a share's. The
    /// others are about a person, and [`request_on`] names the record they
    /// were asked on.
    fn record(&self) -> Option<&str> {
        match self {
            Self::Stop { item_id, .. } | Self::Share { item_id, .. } => Some(item_id),
            _ => None,
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
    /// The share stopped to ask about an organisation Deskwarden did not
    /// create; nothing was written.
    AdoptionAsked { email: String, item_name: String, question: crate::rest::sharing::AdoptionQuestion },
    Stopped { email: String, result: Result<Option<String>, String> },
    /// A Re-invite's answer, with the person it names in its toast.
    Reinvited { email: String, result: Result<(), String> },
    /// Share anyway's answer, likewise.
    Confirmed { email: String, result: Result<(), String> },
    /// Stop receiving's answer, with the sharer its toast names.
    Left { from: String, result: Result<(), String> },
    /// Paste an invite link's answer.
    LinkAccepted(Result<(), String>),
    /// The ids -- the OLD ids -- of the records a forget stopped
    /// (`ForgetOutcome::stopped`, carried by the daemon's `stopped`). Ids,
    /// not names: nothing in it was decrypted.
    Forgot(Result<Vec<String>, String>),
    Fingerprint { email: String, result: Result<Fingerprint, String> },
    InviteCode(Result<InviteCode, String>),
}

/// **Hand-written, and says only which report and whether it succeeded.**
/// A derived `Debug` would print the engine's sentences -- which can name a
/// decrypted record -- the record ids a forget stopped, and the addresses
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
            Self::AdoptionAsked { .. } => ("AdoptionAsked", "Ok(..)"),
            Self::Stopped { result, .. } => ("Stopped", outcome(result)),
            Self::Reinvited { result, .. } => ("Reinvited", outcome(result)),
            Self::Confirmed { result, .. } => ("Confirmed", outcome(result)),
            Self::Left { result, .. } => ("Left", outcome(result)),
            Self::LinkAccepted(r) => ("LinkAccepted", outcome(r)),
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
        SharingCall::Share { item_id, item_name, email, org } => {
            let answer = match &org {
                OrgAnswer::Ask => client.share(&item_id, &email, false),
                OrgAnswer::Adopt(organization) => client.adopt(organization).and_then(|()| client.share(&item_id, &email, false)),
                OrgAnswer::New => client.share(&item_id, &email, true),
            };
            match answer {
                Ok(ShareAnswer::Done(result)) => SharingReport::Shared { email, item_name, result: Ok(result) },
                Ok(ShareAnswer::Adopt(question)) => SharingReport::AdoptionAsked { email, item_name, question },
                Err(sentence) => SharingReport::Shared { email, item_name, result: Err(sentence) },
            }
        }
        SharingCall::Stop { item_id, email } => {
            let result = client.stop(&item_id, &email);
            SharingReport::Stopped { email, result }
        }
        SharingCall::Reinvite(email) => {
            let result = client.reinvite(&email).map(|_| ());
            SharingReport::Reinvited { email, result }
        }
        SharingCall::Forget(email) => SharingReport::Forgot(client.forget(&email)),
        SharingCall::Confirm { email, key_sha256 } => {
            let result = client.confirm(&email, &key_sha256);
            SharingReport::Confirmed { email, result }
        }
        SharingCall::Fingerprint(email) => {
            let result = client.fingerprint(&email);
            SharingReport::Fingerprint { email, result }
        }
        // `password` is dropped -- and so wiped -- when this arm ends.
        SharingCall::InviteCode { email, password } => SharingReport::InviteCode(client.invite_code(&email, &password)),
        // `link` is dropped -- and so wiped -- when this arm ends.
        SharingCall::AcceptLink(link) => SharingReport::LinkAccepted(client.accept_link(&link)),
        SharingCall::Leave { organization_id, from } => {
            let result = client.leave(&organization_id);
            SharingReport::Left { from, result }
        }
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
/// report leaves `busy` and the view alone (see [`apply`]). It has a slot of
/// its own instead, [`SharingPane::fingerprinting`]: a second while one is
/// out is refused with [`FINGERPRINT_BUSY`].
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
            pane.say(CHECK_FINGERPRINT_FIRST.to_string(), None);
            return false;
        }
    }
    let Some(client) = client else {
        pane.say(SHARING_UNAVAILABLE.to_string(), None);
        return false;
    };
    match &call {
        SharingCall::View => pane.begin_view(),
        SharingCall::Fingerprint(_) if pane.fingerprinting.is_some() => {
            pane.say(FINGERPRINT_BUSY.to_string(), None);
            return false;
        }
        SharingCall::Fingerprint(email) => pane.fingerprinting = Some(normalise_email(email)),
        _ if pane.busy => {
            pane.say(BUSY.to_string(), None);
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
            pane.hide_fingerprint();
        }
        _ => pane.busy = true,
    }
    // The record this write is about, for its answer to be said on
    // (`SharingPane::notice_record`): a stop's or a share's own, or none --
    // which [`request_on`] fills in for a write about a person.
    if pane.busy && !matches!(call, SharingCall::View | SharingCall::Fingerprint(_)) {
        pane.write_record = call.record().map(str::to_string);
    }
    // A write clears the last write's sentence; the two reads do NOT. A
    // failed write marks the view stale, and the very next frame asks for it
    // -- clearing the notice there would take the sentence down one frame
    // after it went up (`a_failed_writes_sentence_outlives_the_view_read_it_causes`).
    // A fingerprint is the user's own read, often of the person whose stop
    // just failed, and the sentence saying so stays beside the words it
    // fetched (`a_fingerprint_request_keeps_the_notice_and_holds_its_own_slot`).
    if !matches!(call, SharingCall::View | SharingCall::Fingerprint(_)) {
        pane.notice = None;
        pane.notice_record = None;
    }
    spawn_sharing_call(ctx.clone(), tx.clone(), client.clone(), call);
    true
}

/// [`request`], for a write about a PERSON asked from a record's card -- a
/// Re-invite, a Share anyway: the call itself names no record, so the one
/// on screen is recorded as the one its answer belongs to. A failure then
/// stands on that card, and is routed to a toast if the user has moved on
/// (`route_stray_notice`).
#[must_use]
pub(crate) fn request_on(
    pane: &mut SharingPane,
    record: &str,
    client: Option<&SharingClient>,
    ctx: &egui::Context,
    tx: &mpsc::Sender<SharingReport>,
    call: SharingCall,
) -> bool {
    let started = request(pane, client, ctx, tx, call);
    if started && pane.busy {
        pane.write_record = Some(record.to_string());
    }
    started
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
    let toast = |pane: &mut SharingPane, text: String| pane.toast(text, now_seconds);
    match report {
        SharingReport::View(result) => {
            pane.fetching = false;
            match result {
                Ok(view) => {
                    // **A fingerprint shown for a state that has moved is
                    // dropped**, so the changed-key box never shows the words
                    // of the key before (`SharingPane::fingerprint_state`).
                    let moved = pane
                        .fingerprint
                        .as_ref()
                        .is_some_and(|(who, _)| view.person(who).map(|p| p.state) != pane.fingerprint_state);
                    if moved {
                        pane.hide_fingerprint();
                    }
                    pane.view = Some(view);
                    pane.retry_at = None;
                    pane.viewed_at = Some(now_seconds);
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
            pane.fingerprinting = None;
            // The slot the refusal was about is free: its sentence is spent.
            if pane.notice.as_deref() == Some(FINGERPRINT_BUSY) {
                pane.notice = None;
            }
            let email = normalise_email(&email);
            match result {
                Ok(fingerprint) => {
                    pane.fingerprint_state = pane.view.as_ref().and_then(|v| v.person(&email)).map(|p| p.state);
                    pane.fingerprint = Some((email, fingerprint));
                }
                Err(sentence) => pane.say(sentence, None),
            }
            return;
        }
        _ => {}
    }
    // **Every report from here on answers the one write in flight**, so the
    // record it was asked on is taken here, and a failure is said on it.
    let asked_on = pane.write_record.take();
    let fail = |pane: &mut SharingPane, sentence: String| pane.say(sentence, asked_on.clone());
    match report {
        SharingReport::View(_) | SharingReport::Fingerprint { .. } => {}
        // **An answer lands only in the dialog that asked** -- the one with a
        // share out (`in_flight`). A dialog closed before its answer came and
        // a fresh one opened since, on this record or another, did not ask:
        // painting the old share's "waiting to accept" or failure into it would
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
                Ok(ShareResult::Invited) => match pane.dialog.as_mut().filter(|_| asked) {
                    Some(dialog) => {
                        dialog.in_flight = false;
                        dialog.invited = Some(email);
                    }
                    None => toast(pane, invited_toast(&email)),
                },
                // The engine's refusals -- a record with attachments, in the
                // trash, in another organisation, waiting to come home -- are
                // its own sentences, and the dialog is where they are read.
                Err(sentence) => match pane.dialog.as_mut().filter(|_| asked) {
                    Some(dialog) => {
                        dialog.in_flight = false;
                        dialog.error = Some(sentence);
                    }
                    None => fail(pane, sentence),
                },
            }
        }
        // **The organisation question, in the dialog that asked** (fix
        // round 1). Nothing was written, so a dialog closed before it came
        // gets a sentence on the record, never a guess: the next Share asks
        // again.
        SharingReport::AdoptionAsked { email, item_name, question } => {
            match pane.dialog.as_mut().filter(|d| d.in_flight) {
                Some(dialog) => {
                    dialog.in_flight = false;
                    dialog.adopt = Some(question);
                }
                None => fail(pane, format!("{item_name} wasn't shared with {email}: Deskwarden needs you to choose an organisation. Share it again to choose.")),
            }
        }
        SharingReport::Stopped { email, result } => {
            // Which record the stop was on: `stopping` names it, and there
            // is one write at a time (`request`), so it is this answer's.
            let stopped = pane.stopping.take();
            pane.stop_confirming = None;
            match result {
                Ok(moved_to) => {
                    // The last person went, and the record came home under a
                    // new id: kept for the window, which re-points the
                    // selection. Dropped here, the record open on screen
                    // vanished from under the user.
                    if let (Some((old, _)), Some(new)) = (stopped, moved_to) {
                        pane.moved_home = Some((old, new));
                    }
                    toast(pane, format!("Stopped sharing with {email}."));
                }
                Err(sentence) => fail(pane, sentence),
            }
        }
        SharingReport::Reinvited { email, result } => match result {
            Ok(()) => toast(pane, reinvited_toast(&email)),
            Err(sentence) => fail(pane, sentence),
        },
        SharingReport::Confirmed { email, result } => match result {
            Ok(()) => toast(pane, confirmed_toast(&email)),
            Err(sentence) => fail(pane, sentence),
        },
        // The records have gone from the vault (the feed says so); the
        // toast says why, since the record the user was reading went with them.
        SharingReport::Left { from, result } => match result {
            Ok(()) => toast(pane, stopped_receiving_toast(&from)),
            Err(sentence) => fail(pane, sentence),
        },
        // **A pasted link was accepted**: the records arrive once the sharer
        // finishes, and Shared with me's waiting line says so; the toast says
        // it now. A refusal is the engine's sentence -- not a link, someone
        // else's, the server's no -- said where the user is.
        SharingReport::LinkAccepted(result) => match result {
            Ok(()) => toast(pane, LINK_ACCEPTED.to_string()),
            Err(sentence) => fail(pane, sentence),
        },
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
                (None, Err(sentence)) => fail(pane, sentence),
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
                    fail(pane, sentence);
                }
            }
        },
    }
    pane.busy = false;
    pane.stale = true;
    // **The write slot is free, so a "still running" refusal is spent** --
    // on the card, and in an open dialog that was told it -- rather than
    // standing beside a slot that is no longer busy. A sentence this report
    // put up is a different one and stays.
    if pane.notice.as_deref() == Some(BUSY) {
        pane.notice = None;
        pane.notice_record = None;
    }
    if let Some(dialog) = pane.dialog.as_mut().filter(|d| d.error.as_deref() == Some(BUSY)) {
        dialog.error = None;
    }
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
        fingerprinting: pane.fingerprinting.as_deref(),
        notice: pane.notice_for(&item.id),
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
/// The organisation question's answers (official-path spec §3.1, fix round
/// 1): the owner's plain Yes and No, under a question that says what each
/// does.
pub(crate) const ADOPT_YES: &str = "Yes";
pub(crate) const ADOPT_NO: &str = "No";
/// The line under the question: what Deskwarden checked, what it cannot,
/// and what each answer does. Group access is the part a roster does not
/// show (`rest::sharing::roster_says_mine`), so the user's Yes is the check.
const ADOPT_BODY: &str = "Only you can see everything in it, but Deskwarden can't tell who made it, and access given \
     through a group doesn't show here. Say Yes only if you made it yourself, in the web vault or on another PC. \
     No shares from a new organisation instead.";
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

/// **The invited band's two sentences** -- the bold line and the one under
/// it -- for a share the server left as a pending invitation.
///
/// It claims nothing about an account: no server tells a client whether an
/// address has one (official-path spec §3.4), and no expiry is claimed
/// either. Nor does it say an invitation was emailed: whether one was is
/// the server's setting, which a client cannot see. Only NodeWarden's
/// administrator -- the one account [`SharingView::can_mint_invites`] is
/// ever true for -- is told about the invite code a newcomer there needs,
/// and that they can make it here.
fn invited_band(email: &str, record: &str, admin: bool) -> (String, String) {
    let head = format!("Waiting for {email} to accept.");
    let code = if admin {
        "If they have no account on this server yet, they need an invite code to sign up. You are \
         this server's administrator, so you can create one below and give it to them. "
    } else {
        ""
    };
    let body = format!("{code}{record} reaches their vault once they accept, and you can re-invite them from the record.");
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
    /// Yes to the organisation question.
    Adopt,
    /// No to it: share in a new organisation.
    NewOrganization,
}

/// **Draws the Share dialog** over the window: scrim, card, the record,
/// the WITH field with suggestions from the one address book (Sends and
/// shares), what view-only means, and Share/Cancel -- or, after a share the
/// server left as a pending invitation, the band that says so and Done.
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
                let editable = !busy && dialog.invited.is_none() && dialog.adopt.is_none();
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
                // **The roster-visibility warning** (official-path spec §3.4):
                // where members may read the roster, everyone a record is
                // shared with can list everyone else, and no request this app
                // makes can stop it. Decided by the server's own name
                // (`SharingView::roster_visible_to_recipients`).
                if view.is_some_and(|v| v.roster_visible_to_recipients) {
                    ui.add_space(10.0);
                    ui.add(egui::Label::new(egui::RichText::new(ROSTER_VISIBLE_WARNING).size(12.0).color(theme::TEXT_MUTED)).wrap());
                }
                // The last attempt's sentence -- the engine's refusal (a
                // record with attachments, in the trash, in another
                // organisation, waiting to come home), a refused request, a
                // forget that did not finish. Shown here and never logged.
                if let Some(error) = &dialog.error {
                    ui.add_space(10.0);
                    ui.add(egui::Label::new(egui::RichText::new(error).size(12.0).color(theme::ERROR)).wrap());
                }
            });
            if let Some(email) = dialog.invited.clone() {
                if let Some(asked) = invited_block(ui, dialog, &email, admin) {
                    action = asked;
                }
            }
            if let Some(question) = &dialog.adopt {
                question_band(ui, &question.sentence(), ADOPT_BODY);
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
                    if dialog.invited.is_some() {
                        if theme::primary_button_enabled(ui, DONE_BUTTON, None, !busy).clicked() {
                            action = ShareDialogAction::Done;
                        }
                    } else if dialog.adopt.is_some() {
                        // The question's answers stand where Share and Cancel
                        // stood; the ✕ and Escape still close the dialog,
                        // answering nothing.
                        if theme::primary_button_enabled(ui, ADOPT_YES, None, !busy).clicked() {
                            action = ShareDialogAction::Adopt;
                        }
                        ui.add_space(theme::FORM_FOOTER_GAP);
                        if ui.add_enabled_ui(!busy, |ui| theme::secondary_button(ui, ADOPT_NO)).inner.clicked() {
                            action = ShareDialogAction::NewOrganization;
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

/// **The invited band** (6a's fourth state), between the body and the footer:
/// the amber of `totp_add`'s 6c band rather than `theme::form_card_caution`'s
/// red, and an info mark rather than a warning, because nothing went wrong --
/// the share went through and is waiting for the person to accept.
///
/// An administrator also gets the invite code here: a master-password box and
/// Create an invite code, then the code with a Copy. Only an administrator
/// (`SharingView::can_mint_invites`), and only here -- a code is minted for
/// the one pending invitation this share left, so there is nothing to mint
/// for before it exists.
fn invited_block(
    ui: &mut egui::Ui,
    dialog: &mut ShareDialog,
    email: &str,
    admin: bool,
) -> Option<ShareDialogAction> {
    let mut asked = None;
    let (head, body) = invited_band(email, &dialog.item_name, admin);
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

/// **The organisation question's band** (fix round 1), between the body and
/// the footer: the no-account band's amber, rule and info mark, because it
/// too is a pause and not a failure -- nothing was written, and the share
/// goes on whichever way it is answered.
fn question_band(ui: &mut egui::Ui, head: &str, body: &str) {
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
                let width = ui.available_width();
                ui.add(egui::Label::new(band_job(head, body, width)).wrap());
            });
        });
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
                org: OrgAnswer::Ask,
            };
            pane.dialog = Some(dialog);
            if !request(pane, client, ctx, tx, call) {
                say_refusal_in_dialog(pane);
            }
            ShareDialogAction::None
        }
        // **The organisation question's answers** (fix round 1): the question
        // leaves the dialog as the call that answers it goes, so a second
        // press has nothing to answer. A refused request keeps the dialog
        // open with its sentence and the question gone: Share asks again.
        ShareDialogAction::Adopt | ShareDialogAction::NewOrganization if !busy && dialog.adopt.is_some() => {
            let question = dialog.adopt.take().expect("checked just above");
            let org = if action == ShareDialogAction::Adopt { OrgAnswer::Adopt(question.organization_id) } else { OrgAnswer::New };
            dialog.in_flight = true;
            dialog.error = None;
            let call = SharingCall::Share {
                item_id: dialog.item_id.clone(),
                item_name: dialog.item_name.clone(),
                email: dialog.email.trim().to_string(),
                org,
            };
            pane.dialog = Some(dialog);
            if !request(pane, client, ctx, tx, call) {
                say_refusal_in_dialog(pane);
            }
            ShareDialogAction::None
        }
        ShareDialogAction::MintCode if !busy => {
            let password = std::mem::replace(&mut dialog.password, Zeroizing::new(String::new()));
            match dialog.invited.clone().filter(|_| !password.is_empty()) {
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
        ShareDialogAction::None
        | ShareDialogAction::Submit
        | ShareDialogAction::MintCode
        | ShareDialogAction::Adopt
        | ShareDialogAction::NewOrganization => {
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

// ---- the SHARING card's blocks (mock 6b) ---------------------------------------------
//
// **One card, two blocks, one entry shape.** 6b lists People and then the
// Active Send, each under a stacked label with its mark, and every entry is
// the same thing: a 28-point tile, two lines of words, the controls at the
// right. A person's tile is round with their initial and a Send's is square
// with the paper plane (6e's TILES: "a share is a person", "a Send is a
// link"). The parts are here and `detail::sharing_card` calls them for the
// Send, so the two blocks of one card cannot come to disagree about where a
// control sits or how tall an entry is.

pub(crate) const STOP_LABEL: &str = "Stop sharing";
/// Stop sharing's hover, on the SHARING card and on a share's Sent detail.
pub(crate) const STOP_HOVER: &str = "Take this record out of their vault";
pub(crate) const REINVITE_LABEL: &str = "Re-invite";
pub(crate) const SHARE_ANYWAY_LABEL: &str = "Share anyway";
pub(crate) const SHOW_FINGERPRINT: &str = "Show fingerprint";
pub(crate) const HIDE_FINGERPRINT: &str = "Hide fingerprint";
/// The fingerprint box's own Hide, at the right of its eyebrow (mock 6b).
const HIDE_SHORT: &str = "Hide";
/// Where Show fingerprint stood, while its answer is on the way.
pub(crate) const FETCHING_FINGERPRINT: &str = "Fetching fingerprint\u{2026}";
pub(crate) const REMOVE_LABEL: &str = "Remove from their vault";
/// Mock 6e's "Stop sharing, running".
pub(crate) const STOPPING_LABEL: &str = "Stopping\u{2026}";
pub(crate) const STOP_HINT: &str =
    "Anything they already copied stays with them. If that matters, change the password afterwards.";
/// A greyed Share anyway's hover: what it is waiting for.
const SHARE_ANYWAY_HOVER: &str = "Show their fingerprint and compare it first";
const PEOPLE_BLOCK_LABEL: &str = "People";
const FINGERPRINT_EYEBROW: &str = "FINGERPRINT";
/// 6b's `.chead .side`, which the SHARING card wears once it lists people.
pub(crate) const SHARING_SIDE: &str = "people: view-only, updates when you edit";
/// 6b's `.cnote`, which speaks for both blocks -- and so replaces 5d's own
/// note once the card lists people.
pub(crate) const PEOPLE_NOTE: &str = "A person keeps a view-only copy in their own vault that follows your edits, \
    until you stop sharing. A Send is a link: anyone who has it can open it until it expires or you \
    revoke it.";

/// `.cblock`: `padding: 12px 16px 6px`, and a `border-bottom` in the row
/// rule's grey.
const BLOCK_PAD_X: i8 = 16;
const BLOCK_PAD_TOP: i8 = 12;
const BLOCK_PAD_BOTTOM: i8 = 6;
/// `.bl`: a 12-point mark, `gap: 8px`, 12px faint at the mock's 1.5 line
/// height, and `margin-bottom: 2px`.
const BLOCK_MARK: f32 = 12.0;
const BLOCK_MARK_GAP: f32 = 8.0;
const BLOCK_LABEL_SIZE: f32 = 12.0;
const BLOCK_LABEL_HEIGHT: f32 = 18.0;
const BLOCK_LABEL_GAP: f32 = 2.0;
/// `.pe`: `padding: 9px 0; gap: 11px`, round the 28-point `.pav` / `.stile`.
const ENTRY_PAD_Y: f32 = 9.0;
const ENTRY_TILE: f32 = 28.0;
const ENTRY_TILE_GAP: f32 = 11.0;
/// `.pav`'s 11px 600 initial, and `.stile`'s 14-point plane.
const AVATAR_INITIAL_SIZE: f32 = 11.0;
const TILE_PLANE: f32 = 14.0;
/// `.stile`'s `border-radius: 7px`.
const TILE_RADIUS: u8 = 7;
/// `.who`: two lines, `gap: 3px`. The first is as tall as the pill beside the
/// name; the second is the 11px status line.
const WHO_GAP: f32 = 3.0;
const WHO_LINE_2: f32 = 15.0;
const WHO_HEIGHT: f32 = theme::PILL_HEIGHT + WHO_GAP + WHO_LINE_2;
/// `.who .l1`: 14px, `gap: 8px` to the pill. `.l2`: 11px, `gap: 4px`.
const TITLE_SIZE: f32 = 14.0;
const TITLE_PILL_GAP: f32 = 8.0;
const LINE_SIZE: f32 = 11.0;
const LINE_GAP: f32 = 4.0;
/// `.cctl`: `gap: 8px`.
const CONTROL_GAP: f32 = 8.0;
/// **The narrowest the words may get beside the controls.** Under it, the
/// controls take a line of their own beneath the words -- the read pane's
/// narrowest width leaves an address a dozen points beside Re-invite and Stop
/// sharing, which is no address at all. The same room decides both states
/// of one entry (see [`sharing_entry`]), so a question going up never moves
/// the controls between the two shapes.
const WHO_MIN_WIDTH: f32 = 150.0;
const CONTROL_LINE_GAP: f32 = 8.0;
/// `.extra`: `padding: 0 0 12px 39px` -- under the words, clear of the tile.
const EXTRA_INDENT: i8 = 39;
const EXTRA_PAD_BOTTOM: i8 = 12;
/// `.confirm .h { margin-top: 4px }` and `.confirm .btn { margin-top: 10px }`.
const QUESTION_HINT_GAP: f32 = 4.0;
const QUESTION_BUTTON_GAP: f32 = 10.0;
/// `.fp`: `padding: 10px 12px; gap: 6px`, 8-point corners; the words at 13px
/// in the bold monospace, the eyebrow at 11px with `.08em` of tracking.
const FP_PAD_X: i8 = 12;
const FP_PAD_Y: i8 = 10;
const FP_GAP: f32 = 6.0;
const FP_RADIUS: u8 = 8;
const FP_WORDS_SIZE: f32 = 13.0;
const FP_SMALL_SIZE: f32 = 11.0;
const FP_EYEBROW_TRACKING: f32 = 0.88;
/// `.cnote`: `padding: 12px 16px 13px`, 11px ghost at a 1.5 line height.
const NOTE_PAD_TOP: i8 = 12;
const NOTE_PAD_BOTTOM: i8 = 13;
const NOTE_SIZE: f32 = 11.0;
const NOTE_LINE_HEIGHT: f32 = 16.5;
/// A notice: 12px in the error red, `6` above the note it sits over.
const NOTICE_SIZE: f32 = 12.0;
const NOTICE_GAP: f32 = 6.0;
/// Mock 6a's `.toast`: `right: 20px; bottom: 20px; padding: 10px 14px`, 13px
/// white on ink, 8-point corners -- the copy confirmation's own box.
const TOAST_INSET: f32 = 20.0;
const TOAST_PAD: egui::Vec2 = egui::vec2(14.0, 10.0);
const TOAST_TEXT_SIZE: f32 = 13.0;
const TOAST_RADIUS: u8 = 8;
/// The gap between the share toast and a copy confirmation under it.
const TOAST_STACK_GAP: f32 = 8.0;

/// Mock 6b's red question, with the record and the person in it.
pub(crate) fn stop_question(record: &str, email: &str) -> String {
    format!("Stop sharing {record} with {email}? It leaves their vault at once.")
}

/// What a shown fingerprint is to be compared with (mock 6b).
fn fingerprint_note(email: &str) -> String {
    format!(
        "This should match the phrase {email} sees under Account \u{203a} Fingerprint phrase. If it \
         does not, stop sharing: the record went to a key that is not theirs."
    )
}

/// **A changed key's fingerprint says what to do before Share anyway.** The
/// mock draws no changed key; the engine does (`PersonState::KeyChanged`),
/// and the one safe reading of it is that the words are compared out of band
/// -- the new key came over the same server the old one did, so a phrase
/// read off this screen and the other person's screen alike proves nothing
/// unless the two people say it to each other.
pub(crate) fn key_changed_note(email: &str) -> String {
    format!(
        "Their key changed. Before you share anyway, compare these words with {email} by phone or \
         in person: they see theirs under Account \u{203a} Fingerprint phrase. If the words do not \
         match, do not share."
    )
}

/// Two addresses name one person: the view's, the pane's and a report's are
/// each normalised on their own path, so the card asks the same way.
fn same_person(a: &str, b: &str) -> bool {
    normalise_email(a) == normalise_email(b)
}

/// **The width the card really shows** from this `Ui`'s left edge -- the
/// clip rect, `detail::app_card_content_width`'s rule, for its reason: the
/// read pane's body is a `ScrollArea`, which grows its content `Ui` to last
/// frame's widest child, and `available_width` reports that growth back.
fn content_width(ui: &egui::Ui) -> f32 {
    (ui.clip_rect().right() - ui.max_rect().left()).min(ui.available_width()).max(0.0)
}

/// Which mark a block's stacked label carries (6b's `.bl`).
#[derive(Clone, Copy)]
pub(crate) enum BlockMark {
    Person,
    Plane,
}

/// **One block of the SHARING card**: 6b's `.cblock` -- the stacked label
/// with its mark, then whatever `add` draws, then the rule under the block.
pub(crate) fn sharing_block(ui: &mut egui::Ui, mark: BlockMark, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    let width = (content_width(ui) - f32::from(BLOCK_PAD_X) * 2.0).max(0.0);
    egui::Frame::new()
        .inner_margin(egui::Margin { left: BLOCK_PAD_X, right: BLOCK_PAD_X, top: BLOCK_PAD_TOP, bottom: BLOCK_PAD_BOTTOM })
        .show(ui, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            let (band, _) = ui.allocate_exact_size(egui::vec2(width, BLOCK_LABEL_HEIGHT), egui::Sense::hover());
            let at = egui::Rect::from_min_size(
                egui::pos2(band.left(), band.center().y - BLOCK_MARK / 2.0),
                egui::Vec2::splat(BLOCK_MARK),
            );
            match mark {
                BlockMark::Person => theme::paint_person_mark(ui.painter(), at, theme::TEXT_FAINT),
                BlockMark::Plane => theme::paint_send_plane(ui.painter(), at, theme::TEXT_FAINT),
            }
            let galley = ui.painter().layout_no_wrap(
                label.to_string(),
                egui::FontId::proportional(BLOCK_LABEL_SIZE),
                theme::TEXT_FAINT,
            );
            let top = band.center().y - galley.size().y / 2.0;
            ui.painter().galley(egui::pos2(at.right() + BLOCK_MARK_GAP, top), galley, theme::TEXT_FAINT);
            ui.add_space(BLOCK_LABEL_GAP);
            add(ui);
        });
    theme::row_rule(ui);
}

/// The room a row of [`theme::row_button`]s takes, `.cctl`'s gap between
/// them -- measured off the galleys the buttons will lay, never estimated.
pub(crate) fn controls_room(ui: &egui::Ui, labels: &[&str]) -> f32 {
    let gaps = CONTROL_GAP * labels.len().saturating_sub(1) as f32;
    labels.iter().map(|label| theme::row_button_width(ui, label)).sum::<f32>() + gaps
}

/// An entry's leading mark.
#[derive(Clone, Copy)]
pub(crate) enum EntryTile<'a> {
    /// Round, with the first letter of this address.
    Person(&'a str),
    /// Square, with the paper plane.
    Send,
}

/// The pill after an entry's first line: a share pill for a person (5-point
/// corners, person mark), a Send's stadium state pill for a link.
#[derive(Clone, Copy)]
pub(crate) enum EntryPill<'a> {
    Share(theme::SharePillKind),
    Send(theme::PillTone, &'a str),
}

impl EntryPill<'_> {
    fn width(self, painter: &egui::Painter) -> f32 {
        match self {
            Self::Share(kind) => theme::share_pill_width(painter, kind),
            Self::Send(tone, label) => theme::state_pill_width(painter, tone, label),
        }
    }

    fn paint(self, painter: &egui::Painter, left_center: egui::Pos2) {
        match self {
            Self::Share(kind) => {
                theme::share_pill(painter, left_center, kind);
            }
            Self::Send(tone, label) => {
                theme::state_pill(painter, left_center, tone, label);
            }
        }
    }
}

fn paint_entry_tile(painter: &egui::Painter, rect: egui::Rect, tile: EntryTile<'_>) {
    match tile {
        EntryTile::Person(email) => {
            // Inside the 28 points, as the mock's `border: 1px` is.
            painter.circle(rect.center(), ENTRY_TILE / 2.0 - 0.5, theme::CARD, egui::Stroke::new(1.0, theme::HAIRLINE));
            let initial: String = email.trim().chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
            let font = egui::FontId::new(AVATAR_INITIAL_SIZE, egui::FontFamily::Name(theme::SEMIBOLD.into()));
            let galley = painter.layout_no_wrap(initial, font, theme::TEXT_MUTED);
            painter.galley(rect.center() - galley.size() / 2.0, galley, theme::TEXT_MUTED);
        }
        EntryTile::Send => {
            let corners = egui::CornerRadius::same(TILE_RADIUS);
            painter.rect_filled(rect, corners, theme::CARD);
            painter.rect_stroke(rect, corners, egui::Stroke::new(1.0, theme::HAIRLINE), egui::StrokeKind::Inside);
            let plane = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(TILE_PLANE));
            theme::paint_send_plane(painter, plane, theme::TEXT_MUTED);
        }
    }
}

/// **One entry of a block** (6b's `.pe`): the tile, the words `who` draws,
/// and the controls `controls` draws, laid RIGHT TO LEFT -- so the first
/// control drawn is the rightmost, and a two-step's Cancel drawn first takes
/// the very slot the control it replaces had, with no room held for the
/// others.
///
/// **Placed, not flowed.** The entry's rect is allocated whole and the three
/// parts are put into rects inside it, because a horizontal layout centres
/// each item against the row's height SO FAR, and a 28-point button after a
/// 38-point column of words would sit high.
///
/// `controls_room` is the room the controls have AT REST, and it is what
/// decides the words' width and whether the controls share their line --
/// never the controls of the moment: a question going up swaps Stop sharing
/// for a narrower Cancel, and an entry that re-measured would move its words,
/// or jump from two lines to one, under the pointer that pressed it.
pub(crate) fn sharing_entry(
    ui: &mut egui::Ui,
    tile: EntryTile<'_>,
    controls_room: f32,
    who: impl FnOnce(&mut egui::Ui),
    controls: impl FnOnce(&mut egui::Ui),
) {
    let width = ui.available_width();
    let text_left = ENTRY_TILE + ENTRY_TILE_GAP;
    let one_line = width - text_left - controls_room - CONTROL_GAP >= WHO_MIN_WIDTH;
    let controls_line = if one_line { 0.0 } else { CONTROL_LINE_GAP + theme::ROW_BUTTON_HEIGHT_2B };
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(width, ENTRY_PAD_Y * 2.0 + WHO_HEIGHT + controls_line),
        egui::Sense::hover(),
    );
    let body = egui::Rect::from_min_size(rect.min + egui::vec2(0.0, ENTRY_PAD_Y), egui::vec2(width, WHO_HEIGHT));
    let tile_rect = egui::Rect::from_min_size(
        egui::pos2(body.left(), body.center().y - ENTRY_TILE / 2.0),
        egui::Vec2::splat(ENTRY_TILE),
    );
    paint_entry_tile(ui.painter(), tile_rect, tile);
    let words_left = body.left() + text_left;
    let words_right = if one_line { body.right() - controls_room - CONTROL_GAP } else { body.right() };
    let words = egui::Rect::from_min_max(egui::pos2(words_left, body.top()), egui::pos2(words_right.max(words_left), body.bottom()));
    let mut words_ui = ui.new_child(egui::UiBuilder::new().max_rect(words).layout(egui::Layout::top_down(egui::Align::Min)));
    words_ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
    who(&mut words_ui);
    let slot = if one_line {
        egui::Rect::from_min_max(egui::pos2(words_right + CONTROL_GAP, body.top()), body.max)
    } else {
        egui::Rect::from_min_size(
            egui::pos2(words_left, body.bottom() + CONTROL_LINE_GAP),
            egui::vec2((body.right() - words_left).max(0.0), theme::ROW_BUTTON_HEIGHT_2B),
        )
    };
    let mut controls_ui = ui.new_child(egui::UiBuilder::new().max_rect(slot).layout(egui::Layout::right_to_left(egui::Align::Center)));
    controls_ui.spacing_mut().item_spacing = egui::vec2(CONTROL_GAP, 0.0);
    controls(&mut controls_ui);
}

/// An entry's first line: the name at 14px, cut short with an ellipsis
/// before it can push the pill after it off the entry, then the pill.
pub(crate) fn entry_title(ui: &mut egui::Ui, title: &str, pill: EntryPill<'_>) {
    let width = ui.available_width();
    let pill_width = pill.width(ui.painter());
    let row = egui::Layout::left_to_right(egui::Align::Center);
    ui.allocate_ui_with_layout(egui::vec2(width, theme::PILL_HEIGHT), row, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(TITLE_PILL_GAP, 0.0);
        let room = (width - pill_width - TITLE_PILL_GAP).max(0.0);
        ui.allocate_ui_with_layout(egui::vec2(room, theme::PILL_HEIGHT), row, |ui| {
            ui.add(egui::Label::new(egui::RichText::new(title).size(TITLE_SIZE).color(theme::INK)).truncate());
        });
        let (slot, _) = ui.allocate_exact_size(egui::vec2(pill_width, theme::PILL_HEIGHT), egui::Sense::hover());
        pill.paint(ui.painter(), egui::pos2(slot.left(), slot.center().y));
    });
}

/// An entry's second line: the status words at 11px, and after them -- past
/// a middle dot -- a link, `(label, pressable)`. Answers whether the link was
/// pressed. The words give way to the link, not the other way round.
pub(crate) fn entry_line(ui: &mut egui::Ui, line: &str, link: Option<(&str, bool)>) -> bool {
    let width = ui.available_width();
    let mut pressed = false;
    let row = egui::Layout::left_to_right(egui::Align::Center);
    ui.add_space(WHO_GAP);
    ui.allocate_ui_with_layout(egui::vec2(width, WHO_LINE_2), row, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(LINE_GAP, 0.0);
        let link_room = link.map_or(0.0, |(label, _)| {
            let measure = |text: &str, family: egui::FontFamily| {
                ui.painter().layout_no_wrap(text.to_string(), egui::FontId::new(LINE_SIZE, family), theme::TEXT_FAINT).size().x
            };
            measure("\u{b7}", egui::FontFamily::Proportional)
                + measure(label, egui::FontFamily::Name(theme::SEMIBOLD.into()))
                + LINE_GAP * 2.0
        });
        ui.allocate_ui_with_layout(egui::vec2((width - link_room).max(0.0), WHO_LINE_2), row, |ui| {
            ui.add(egui::Label::new(egui::RichText::new(line).size(LINE_SIZE).color(theme::TEXT_FAINT)).truncate());
        });
        if let Some((label, pressable)) = link {
            ui.label(egui::RichText::new("\u{b7}").size(LINE_SIZE).color(theme::TEXT_FAINT));
            // `.link`: 600 weight in the deep blue; greyed when it cannot be
            // pressed, and then it takes no click at all.
            let ink = if pressable { theme::BLUE_DEEP } else { theme::TEXT_GHOST };
            let sense = if pressable { egui::Sense::click() } else { egui::Sense::hover() };
            let response = ui.add(egui::Label::new(theme::semibold(label, LINE_SIZE).color(ink)).sense(sense));
            if pressable && response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            pressed = pressable && response.clicked();
        }
    });
    pressed
}

/// What goes under an entry (6b's `.extra`): the stop question, or the
/// fingerprint -- indented to the words, clear of the tile.
pub(crate) fn entry_extra(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let width = (ui.available_width() - f32::from(EXTRA_INDENT)).max(0.0);
    egui::Frame::new()
        .inner_margin(egui::Margin { left: EXTRA_INDENT, right: 0, top: 0, bottom: EXTRA_PAD_BOTTOM })
        .show(ui, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            add(ui);
        });
}

/// **The People block of the SHARING card** (mock 6b): one entry per person
/// -- a round avatar with their initial, the address and share pill, the
/// status line with its fingerprint link, and the controls. Stop sharing is
/// the app's two-step: Cancel in Stop's own slot, and the question and the
/// red button under the row, so nothing above moves under the pointer.
pub(crate) fn draw_people_block(
    ui: &mut egui::Ui,
    record: &str,
    people: &super::detail::PeopleSharing<'_>,
    action: &mut super::detail::DetailAction,
) {
    sharing_block(ui, BlockMark::Person, PEOPLE_BLOCK_LABEL, |ui| {
        for (index, person) in people.people.iter().enumerate() {
            // `.pe + .pe`: a rule between two people, none over the first.
            if index > 0 {
                theme::row_rule(ui);
            }
            person_entry(ui, record, person, people, action);
        }
    });
}

/// One person, with everything the pane is in the middle of for them.
///
/// **Share anyway is drawn, greyed, until their fingerprint is on screen.**
/// Drawn, so a changed key says where the way on is; greyed, because the
/// window can only send [`SharingPane::share_anyway`]'s call, which binds the
/// confirm to the key whose words were shown -- and a press that sent
/// nothing would be a button that lies.
fn person_entry(
    ui: &mut egui::Ui,
    record: &str,
    person: &PersonLine,
    people: &super::detail::PeopleSharing<'_>,
    action: &mut super::detail::DetailAction,
) {
    use super::detail::DetailAction;
    let email = person.email.as_str();
    let names_them = |who: Option<&str>| who.is_some_and(|who| same_person(who, email));
    let stopping = names_them(people.stopping);
    let confirming = names_them(people.confirming) && !stopping;
    let shown = people.fingerprint.filter(|(who, _)| same_person(who, email)).map(|(_, phrase)| phrase);
    // Rightmost first, the order the controls are laid in: 6b reads
    // `Re-invite, Stop sharing` left to right, and Share anyway -- a changed
    // key's, which the mock does not draw -- goes before them both.
    let mut resting = vec![STOP_LABEL];
    if person.reinvite() {
        resting.push(REINVITE_LABEL);
    }
    if person.share_anyway() {
        resting.push(SHARE_ANYWAY_LABEL);
    }
    let room = controls_room(ui, &resting);
    // One fetch at a time (`SharingPane::fingerprinting`): the person being
    // fetched says so where their link was, and every other Show greys.
    let link = person.fingerprint().then(|| match (shown, people.fingerprinting) {
        (Some(_), _) => (HIDE_FINGERPRINT, true),
        (None, Some(who)) if same_person(who, email) => (FETCHING_FINGERPRINT, false),
        (None, Some(_)) => (SHOW_FINGERPRINT, false),
        (None, None) => (SHOW_FINGERPRINT, true),
    });
    let mut link_pressed = false;
    sharing_entry(
        ui,
        EntryTile::Person(email),
        room,
        |ui| {
            entry_title(ui, email, EntryPill::Share(person.pill()));
            link_pressed = entry_line(ui, &person.line, link);
        },
        |ui| {
            if stopping {
                // No widget of any kind while the stop runs: the Send's
                // revoke rule, for the same reason.
                ui.label(egui::RichText::new(STOPPING_LABEL).size(12.0).color(theme::TEXT_MUTED));
                return;
            }
            if confirming {
                if theme::row_button(ui, CANCEL_BUTTON).clicked() {
                    *action = DetailAction::CancelStopSharing;
                }
                return;
            }
            if theme::row_button_danger(ui, STOP_LABEL)
                .on_hover_text(STOP_HOVER)
                .clicked()
            {
                *action = DetailAction::StopSharing(email.to_string());
            }
            if person.reinvite()
                && theme::row_button(ui, REINVITE_LABEL).on_hover_text("Send them a fresh invitation").clicked()
            {
                *action = DetailAction::Reinvite(email.to_string());
            }
            if person.share_anyway() {
                let ready = shown.is_some();
                let pressed = ui
                    .add_enabled_ui(ready, |ui| theme::row_button(ui, SHARE_ANYWAY_LABEL))
                    .inner
                    .on_disabled_hover_text(SHARE_ANYWAY_HOVER)
                    .clicked();
                if ready && pressed {
                    *action = DetailAction::ShareAnyway(email.to_string());
                }
            }
        },
    );
    if link_pressed {
        *action = if shown.is_some() { DetailAction::HideFingerprint } else { DetailAction::ShowFingerprint(email.to_string()) };
    }
    if confirming {
        entry_extra(ui, |ui| {
            ui.add(egui::Label::new(theme::semibold(stop_question(record, email), 14.0).color(theme::ERROR)).wrap());
            ui.add_space(QUESTION_HINT_GAP);
            ui.add(egui::Label::new(egui::RichText::new(STOP_HINT).size(11.0).color(theme::TEXT_FAINT)).wrap());
            ui.add_space(QUESTION_BUTTON_GAP);
            // Cancel wins an ambiguous frame, as on the Send's revoke: the
            // safe answer to one on a destructive control is not to destroy.
            if theme::destructive_button(ui, REMOVE_LABEL).clicked() && *action != DetailAction::CancelStopSharing {
                *action = DetailAction::ConfirmStopSharing(email.to_string());
            }
        });
    }
    if let Some(phrase) = shown {
        let note = if person.share_anyway() { key_changed_note(email) } else { fingerprint_note(email) };
        let mut hide = false;
        entry_extra(ui, |ui| hide = fingerprint_box(ui, phrase, &note));
        if hide {
            *action = DetailAction::HideFingerprint;
        }
    }
}

/// 6b's `.fp`: the eyebrow with its Hide, the five words, and what to compare
/// them with. Answers whether Hide was pressed.
fn fingerprint_box(ui: &mut egui::Ui, phrase: &str, note: &str) -> bool {
    let mut hide = false;
    egui::Frame::new()
        .fill(theme::BLUE_WASH)
        .stroke(egui::Stroke::new(1.0, theme::BLUE_EDGE))
        .corner_radius(egui::CornerRadius::same(FP_RADIUS))
        .inner_margin(egui::Margin::symmetric(FP_PAD_X, FP_PAD_Y))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = egui::vec2(0.0, FP_GAP);
            ui.horizontal(|ui| {
                ui.label(theme::letterspaced(FINGERPRINT_EYEBROW, FP_SMALL_SIZE, theme::BOLD, FP_EYEBROW_TRACKING, theme::BLUE_DEEP));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let link = egui::Label::new(theme::semibold(HIDE_SHORT, FP_SMALL_SIZE).color(theme::BLUE_DEEP)).sense(egui::Sense::click());
                    let response = ui.add(link);
                    if response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    hide = response.clicked();
                });
            });
            let words = egui::RichText::new(phrase)
                .size(FP_WORDS_SIZE)
                .family(egui::FontFamily::Name(theme::MONO_BOLD.into()))
                .color(theme::BLUE_DEEP);
            ui.add(egui::Label::new(words).wrap());
            ui.add(egui::Label::new(egui::RichText::new(note).size(FP_SMALL_SIZE).color(theme::BLUE_DEEP)).wrap());
        });
    hide
}

/// **The foot of the SHARING card**: a sharing request's sentence, if one is
/// standing, over the card's note (6b's `.cnote`, or 5d's with only a Send).
///
/// The notice is the engine's or the window's sentence -- a refusal, a stop
/// or a share that failed -- and it can name a decrypted record, so it is
/// drawn here and logged nowhere (module doc). It stands at the foot and not
/// the head for `detail::sharing_card`'s reason: a sentence arriving while
/// the pointer is on a row must not push that row away.
pub(crate) fn draw_card_note(ui: &mut egui::Ui, notice: Option<&str>, note: Option<&str>) {
    if notice.is_none() && note.is_none() {
        return;
    }
    let width = (content_width(ui) - f32::from(BLOCK_PAD_X) * 2.0).max(0.0);
    egui::Frame::new()
        .inner_margin(egui::Margin { left: BLOCK_PAD_X, right: BLOCK_PAD_X, top: NOTE_PAD_TOP, bottom: NOTE_PAD_BOTTOM })
        .show(ui, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            if let Some(notice) = notice {
                ui.add(egui::Label::new(egui::RichText::new(notice).size(NOTICE_SIZE).color(theme::ERROR)).wrap());
                if note.is_some() {
                    ui.add_space(NOTICE_GAP);
                }
            }
            if let Some(note) = note {
                let text = egui::RichText::new(note)
                    .size(NOTE_SIZE)
                    .line_height(Some(NOTE_LINE_HEIGHT))
                    .color(theme::TEXT_GHOST);
                ui.add(egui::Label::new(text).wrap());
            }
        });
}

/// **The share toast** (mock 6a): one dark box inside the pane's bottom
/// right, the copy confirmation's shape and inset, saying how a share or a
/// stop ended while no dialog is open to say it.
///
/// **A painter on a foreground layer, not an `Area`**, for
/// `detail::draw_copy_toast`'s reason: an `Area` spends its first frame
/// measuring itself and paints nothing, and a toast takes no input.
///
/// **No repaint is asked for.** The toast is taken down by [`drain`] once
/// its [`TOAST_SECONDS`] are up, and the vault window's own frame schedule
/// wakes it at least every half second -- a timer here would be a second
/// repaint loop for something the existing one already covers.
///
/// Wrapped to the pane: the engine's sentences are longer than a copy
/// confirmation's and a narrow pane is narrower than some of them.
///
/// **`clear_of` is a live copy confirmation's box**, if one is up: the two
/// share the corner, so this one stands [`TOAST_STACK_GAP`] above it rather
/// than on top of it (`detail::live_copy_toast_rect`).
pub(crate) fn draw_share_toast(ui: &egui::Ui, pane_rect: egui::Rect, clear_of: Option<egui::Rect>, pane: &SharingPane) {
    let Some((text, _)) = &pane.toast else {
        return;
    };
    let wrap = (pane_rect.width() - TOAST_INSET * 2.0 - TOAST_PAD.x * 2.0).max(1.0);
    let galley = ui.painter().layout(text.clone(), egui::FontId::proportional(TOAST_TEXT_SIZE), theme::CARD, wrap);
    let size = galley.size() + TOAST_PAD * 2.0;
    let bottom = clear_of.map_or(pane_rect.bottom() - TOAST_INSET, |copy| copy.top() - TOAST_STACK_GAP);
    let at = egui::pos2(
        (pane_rect.right() - TOAST_INSET - size.x).max(pane_rect.left() + TOAST_INSET),
        (bottom - size.y).max(pane_rect.top() + TOAST_INSET),
    );
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("share-toast")));
    let rect = egui::Rect::from_min_size(at, size);
    painter.rect_filled(rect, egui::CornerRadius::same(TOAST_RADIUS), theme::INK);
    painter.galley(rect.min + TOAST_PAD, galley, theme::CARD);
}

// ---- Sent: one row per record and person (mock 6c) -------------------------------------
//
// **The data is here; the drawing is `send_ui`'s.** A share row stands in the
// Sent list's column beside Send rows, and the owner's rule for that column
// is one row shape, read off `item_list`'s constants and drawn through the
// records list's own tile -- which `send_ui::draw_row` already does, with
// private helpers a second copy here would drift from. So `send_ui` draws the
// row and the detail (`draw_share_row`, `draw_share_detail`, both private to
// it), and this file says what a share IS: its key, its words, and what a
// press on it asks of the window.

/// A share row's selection key starts with this, so it can never be taken
/// for a Send's id -- `send_ui`'s detail tells the two apart by it.
pub(crate) const SHARE_KEY_PREFIX: &str = "share:";
/// 6c's header, beside the share pill.
pub(crate) const SHARED_WITH_A_PERSON: &str = "Shared with a person";
/// 6c's two cards.
pub(crate) const PERSON_CARD_TITLE: &str = "PERSON";
pub(crate) const WHAT_THEY_SEE_TITLE: &str = "WHAT THEY SEE";
/// 6c's PERSON rows. `Since` falls back to [`SINCE_UNKNOWN`] for a share this
/// PC holds no record of -- made on another PC, or before the sent history
/// kept shares. "On another PC" would be a guess; this is what is known.
pub(crate) const SHARED_WITH_ROW: &str = "Shared with";
pub(crate) const SINCE_ROW: &str = "Since";
pub(crate) const SINCE_UNKNOWN: &str = "Not recorded on this PC";
pub(crate) const ACCESS_ROW: &str = "Access";
pub(crate) const VIEW_ONLY: &str = "View-only";
pub(crate) const THEIR_COPY_ROW: &str = "Their copy";
pub(crate) const FOLLOWS_EDITS: &str = "Follows your edits";
/// A person who does not hold the record yet (or any more) has no copy to
/// follow anything, so their row says where they stand instead.
pub(crate) const STATUS_ROW: &str = "Status";
/// 6c's WHAT THEY SEE: the whole record, and 6c's Access hint split at its
/// own sentence boundary into what they can and cannot do -- one fact per
/// row, on the card's one label column, in the mock's words.
pub(crate) const WHOLE_RECORD: &str = "The whole record, now and after every edit.";
pub(crate) const CAN_ROW: &str = "They can";
pub(crate) const CAN_VALUE: &str = "See, copy and autofill it";
pub(crate) const CANNOT_ROW: &str = "They cannot";
pub(crate) const CANNOT_VALUE: &str = "Edit, delete or share it on";

/// **A share in the Sent list**: one record, one person (mock 6c).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShareRow {
    /// `share:{item id}:{email}` -- the Sent pane's selection key. See
    /// [`share_key`].
    pub(crate) key: String,
    pub(crate) item_id: String,
    /// The VAULT ITEM's name -- the decrypted record, whose cipher the vault
    /// authenticated -- and never anything the view carries: the view's
    /// people come from collection names, and a name is not proof.
    pub(crate) name: String,
    /// The person, normalised as the view holds them.
    pub(crate) email: String,
    pub(crate) state: PersonState,
    /// "to anna@example.com · Shared".
    pub(crate) subtitle: String,
    /// "14 Sep", from the local record of the share.
    pub(crate) since: Option<String>,
}

impl ShareRow {
    /// The pill the row and its detail wear -- the SHARING card's own
    /// ([`PersonLine::pill`]), so one person is one pill wherever they are.
    pub(crate) fn pill(&self) -> theme::SharePillKind {
        PersonLine { email: self.email.clone(), state: self.state, line: String::new(), authenticated: true }.pill()
    }

    /// The subtitle's state words, after its middle dot. An address has no
    /// spaces, so the first " · " is the one [`share_rows`] wrote. (Not
    /// named `status`: `job_object`'s scan reads a method call of that name
    /// as a `Command` starting a child.)
    pub(crate) fn state_words(&self) -> &str {
        self.subtitle.split_once(" \u{b7} ").map_or(self.subtitle.as_str(), |(_, state)| state)
    }
}

/// A share's selection key. An item id is a GUID and has no `:`, so
/// [`parse_share_key`] splits at the first one after the prefix and the
/// address -- which may not -- comes back whole.
pub(crate) fn share_key(item_id: &str, email: &str) -> String {
    format!("{SHARE_KEY_PREFIX}{item_id}:{email}")
}

/// `(item id, email)` back out of a [`share_key`]; `None` for a Send's id.
pub(crate) fn parse_share_key(key: &str) -> Option<(String, String)> {
    let rest = key.strip_prefix(SHARE_KEY_PREFIX)?;
    let (item_id, email) = rest.split_once(':')?;
    Some((item_id.to_string(), email.to_string()))
}

/// **Every share, one row per record and person** -- newest first by the
/// local record of it, since a share has no server timestamp (plan decision
/// 12). A share this PC has no record of sorts last, by record name.
///
/// **`sources` are the lists the record is looked for in, in order**: the
/// live vault, then the archive and the trash where the window has them. A
/// share of an ARCHIVED record is still live -- the person still has it --
/// so it must not drop out of Sent because the record left the live list.
///
/// A record none of them holds is left out: its name would be the only
/// thing to show, and the vault is where a name comes from. That covers a
/// view a frame older than a sync, and an archived or trashed record while
/// the window has not fetched that list yet (the archive and the trash are
/// fetched when first opened). The rail's Sent badge counts THIS list
/// (`vault_window::mod` hands it `share_rows.len()`), so the badge and the
/// column say one number whichever it is. A person the view lists on a
/// record twice is one row -- the key is the row's identity.
pub(crate) fn share_rows(
    view: Option<&SharingView>,
    sources: &[&[VaultItem]],
    history: &SentHistory,
    zone: &dyn LocalOffset,
) -> Vec<ShareRow> {
    let Some(view) = view else {
        return Vec::new();
    };
    let mut rows: Vec<(Option<i64>, ShareRow)> = Vec::new();
    for record in &view.records {
        let Some(item) = sources.iter().find_map(|list| list.iter().find(|i| i.id == record.item_id)) else {
            continue;
        };
        for email in &record.people {
            let Some(person) = view.person(email) else {
                continue;
            };
            let key = share_key(&item.id, &person.email);
            if rows.iter().any(|(_, row)| row.key == key) {
                continue;
            }
            let at = since(history, &item.id, &person.email);
            rows.push((
                at,
                ShareRow {
                    key,
                    item_id: item.id.clone(),
                    name: item.name.clone(),
                    email: person.email.clone(),
                    state: person.state,
                    subtitle: format!("to {} \u{b7} {}", person.email, short_state(person.state)),
                    since: at.map(|at| day(at, zone)),
                },
            ));
        }
    }
    rows.sort_by(|(a, ra), (b, rb)| b.cmp(a).then_with(|| ra.name.cmp(&rb.name)).then_with(|| ra.email.cmp(&rb.email)));
    rows.into_iter().map(|(_, row)| row).collect()
}

/// **What the Sent list is told about sharing, per frame, and what the user
/// asked of it** -- the window's half of the Stop sharing two-step on a
/// share's own detail, keyed by [`share_key`].
///
/// Built by the window from [`SharingPane`] before the Sends pane is drawn,
/// and read back after it (`asked`). It borrows only the frame's own rows, so
/// the pane is free to change while it is alive.
///
/// `pub` and not `pub(crate)`, for [`PersonLine`]'s reason: it is a parameter
/// of `send_ui::draw_send_pane`'s public signature, which the UI preview
/// example calls with [`PeopleRows::none`]. Its fields stay the crate's.
pub struct PeopleRows<'a> {
    pub(crate) shares: &'a [ShareRow],
    /// The share key whose stop is armed.
    pub(crate) confirming: Option<String>,
    /// The share key whose stop is running.
    pub(crate) stopping: Option<String>,
    /// **A sharing sentence for the share on screen** -- a stop that failed,
    /// a refusal -- which the detail shows at its foot. The engine's
    /// sentence, so shown and never logged (module doc). The window hands it
    /// over only for the record on screen ([`SharingPane::notice_for`]); one
    /// about another record went to a toast ([`route_stray_notice`]).
    pub(crate) notice: Option<String>,
    /// Out: what was pressed this frame.
    pub(crate) asked: Option<PeopleAsk>,
    /// **Shared with me's FROM PEOPLE** (mock 6d): the records other people
    /// share with this account, which [`draw_received_group`] lists above
    /// the imported Sends. Only rows: a received record has no two-step here
    /// -- a click opens it in the vault, and its pane is view-only there.
    pub(crate) received: &'a [ReceivedShareRow],
    /// **Paste an invite link's box** (official-path spec §4.1), lent by the
    /// window for the frame ([`SharingPane::paste`]) and handed back after
    /// it; `None` where sharing is not available, and the row is not drawn.
    /// [`draw_paste_and_waiting`] puts it first in Shared with me.
    pub(crate) paste: Option<Zeroizing<String>>,
    /// **The sharers this account accepted and who have not finished yet**
    /// -- the view's `waiting`, by the sharer's address. Each is one quiet
    /// [`waiting_line`] under the paste box: an accepted sharing org shows
    /// its member nothing until the sharer's daemon confirms them, and
    /// without the line an accept would look like it did nothing.
    pub(crate) waiting: &'a [String],
    /// **A sharing write is out** ([`SharingPane::is_busy`]): Paste's Accept
    /// is drawn greyed and senses nothing, rather than being pressed only for
    /// [`request`] to refuse it with [`BUSY`].
    pub(crate) busy: bool,
}

impl PeopleRows<'static> {
    /// Nothing about sharing: what every caller that is not about it passes.
    #[must_use]
    pub fn none() -> Self {
        Self {
            shares: &[],
            confirming: None,
            stopping: None,
            notice: None,
            asked: None,
            received: &[],
            paste: None,
            waiting: &[],
            busy: false,
        }
    }
}

/// A press in the Sends pane's sharing half. `Stop`'s keys are
/// [`share_key`]s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PeopleAsk {
    /// Stop sharing's step one: put the question up.
    Stop(String),
    /// "Remove from their vault": step two.
    ConfirmStop(String),
    CancelStop,
    /// Paste an invite link's Accept: the window sends the box's text.
    AcceptLink,
}

// ---- Shared with me: records from people (mock 6d) -----------------------------------
//
// **The data and the grouping are here; the row is `send_ui`'s**, for the Sent
// list's reason above: a received record stands in Shared with me's column
// beside the imported Sends, and that column is one row shape. So the row is
// `send_ui::draw_person_row` -- the band, the record's tile and the person
// mark the Sent list's share row wears -- with no pill: a received record's
// only state is "yours to read", which the subtitle says.

/// A received row's selection key starts with this, so it can never be taken
/// for an imported Send's key or a share's -- the Received detail finds no
/// row for it and shows its "nothing picked" pane, which is right: the row's
/// click has already opened the record in the vault.
pub(crate) const RECEIVED_KEY_PREFIX: &str = "recv:";
/// Mock 6d's two `.lgroup` eyebrows over Shared with me's list.
pub(crate) const FROM_PEOPLE_EYEBROW: &str = "FROM PEOPLE";
pub(crate) const FROM_SENDS_EYEBROW: &str = "IMPORTED FROM SENDS";
/// `.lgroup { font-size: 10px; letter-spacing: 0.1em; padding: 6px 4px 0 }`.
const GROUP_EYEBROW_PX: f32 = 10.0;
const GROUP_EYEBROW_TRACKING: f32 = 1.0;
const GROUP_EYEBROW_TOP: f32 = 6.0;
const GROUP_EYEBROW_INSET: f32 = 4.0;

/// A record someone shares with this account, in Shared with me.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReceivedShareRow {
    /// `recv:{item id}` -- see [`RECEIVED_KEY_PREFIX`].
    pub(crate) key: String,
    pub(crate) item_id: String,
    /// The VAULT ITEM's name, for [`ShareRow::name`]'s reason: the vault
    /// decrypted and authenticated the record, and the view only says which
    /// item it is.
    pub(crate) name: String,
    /// Who shares it, as the view names them. **For display only**: the
    /// view's sharer is read off the sharing org's name, which its owner
    /// wrote in plaintext and could have spelled as anybody. The view offers
    /// no authenticated alternative for a record received (the collection
    /// under it is the SHARER's, keyed by an org key only they vouch for), so
    /// this is shown as the claim it is and decides nothing.
    pub(crate) from: String,
}

/// **The received records the window holds, in the view's order.** An entry
/// whose item the vault does not hold -- a view a frame older than a sync --
/// is left out, for [`share_rows`]' reason: the vault is where a name comes
/// from, and a nameless row is not one to show.
pub(crate) fn received_share_rows(view: Option<&SharingView>, items: &[VaultItem]) -> Vec<ReceivedShareRow> {
    let Some(view) = view else {
        return Vec::new();
    };
    view.received
        .iter()
        .filter_map(|r| {
            let item = items.iter().find(|i| i.id == r.item_id)?;
            Some(ReceivedShareRow {
                key: format!("{RECEIVED_KEY_PREFIX}{}", item.id),
                item_id: item.id.clone(),
                name: item.name.clone(),
                from: r.from.clone(),
            })
        })
        .collect()
}

/// The height [`group_eyebrow`] takes, measured -- what `send_ui` adds to its
/// row count's height so its scroll bar decides on the list really drawn.
pub(crate) fn group_eyebrow_height(ui: &egui::Ui) -> f32 {
    let bold = egui::FontId::new(GROUP_EYEBROW_PX, egui::FontFamily::Name(theme::BOLD.into()));
    GROUP_EYEBROW_TOP + ui.ctx().fonts_mut(|f| f.row_height(&bold))
}

/// **One of mock 6d's list eyebrows** (`.lgroup`): 10pt bold, tracked 0.1em,
/// in the ghost ink, 6pt under whatever is above it and 4pt in from the row
/// edge. Not `theme::eyebrow`, which is the rail's 11pt header: the mock sets
/// these a size down, as headers INSIDE a list rather than over one.
pub(crate) fn group_eyebrow(ui: &mut egui::Ui, width: f32, text: &str) {
    let job = theme::letterspaced(text, GROUP_EYEBROW_PX, theme::BOLD, GROUP_EYEBROW_TRACKING, theme::TEXT_GHOST);
    let galley = ui.painter().layout_job(job);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, group_eyebrow_height(ui)), egui::Sense::hover());
    ui.painter().galley(egui::pos2(rect.left() + GROUP_EYEBROW_INSET, rect.top() + GROUP_EYEBROW_TOP), galley, theme::TEXT_GHOST);
}

/// **Shared with me's people half** (mock 6d): the FROM PEOPLE eyebrow and a
/// row per received record -- "from denis@example.com · view-only" under its
/// name. A row opens the record in the vault, where its pane is view-only.
/// Draws nothing, not even the eyebrow, when nobody shares anything, so the
/// screen of an account that only ever imported Sends is unchanged.
///
/// **IMPORTED FROM SENDS is not drawn here** although the mock puts it under
/// this group: this function cannot see whether there are imports, and an
/// eyebrow over no rows says a group exists that does not. `send_ui` draws it,
/// where both lists are known (`shared_with_me_puts_records_from_people_above
/// _the_imports`).
///
/// `records` are the vault's tiles, so a received record wears in this list
/// the favicon it wears in All items -- the Sent list's share row's rule.
pub(crate) fn draw_received_group(
    ui: &mut egui::Ui,
    people: &mut PeopleRows<'_>,
    width: f32,
    height: f32,
    selected: &mut Option<String>,
    records: crate::vault_window::send_ui::RecordTiles<'_>,
) -> Option<crate::vault_window::send_ui::SendUiAction> {
    if people.received.is_empty() {
        return None;
    }
    let mut action = None;
    group_eyebrow(ui, width, FROM_PEOPLE_EYEBROW);
    for row in people.received {
        let clicked = crate::vault_window::send_ui::draw_person_row(
            ui,
            crate::vault_window::send_ui::PersonRow {
                key: &row.key,
                item_id: &row.item_id,
                name: &row.name,
                subtitle: &format!("from {} \u{b7} view-only", row.from),
                pill: None,
            },
            width,
            height,
            selected,
            records,
        );
        if clicked {
            action = Some(crate::vault_window::send_ui::SendUiAction::OpenRecord(row.item_id.clone()));
        }
    }
    action
}

// ---- Shared with me: Paste an invite link, and the sharers being waited on ------------
//
// **There are no invitation rows any more** (official-path spec §4.1). No
// official server lets an invitee list their invitations, and NodeWarden's
// list is not read: an invitation arrives as a link -- in an email, or a
// message from the sharer -- and is accepted by pasting it here, or in the web
// vault, which accepts the same link. There is no Decline: Bitwarden has
// none, and an invitation nobody answers expires.

/// The paste box's hint, and its button.
pub(crate) const PASTE_HINT: &str = "Paste an invite link";
pub(crate) const PASTE_ACCEPT_LABEL: &str = "Accept";
/// A pasted link was accepted. The records arrive once the sharer's daemon
/// confirms; the waiting line under the box says so meanwhile.
pub(crate) const LINK_ACCEPTED: &str = "Invitation accepted. The records arrive once the person sharing them finishes.";
/// The paste row: one `theme::inline_field` high.
pub(crate) const PASTE_ROW_HEIGHT: f32 = theme::BUTTON_HEIGHT;
const PASTE_BUTTON_GAP: f32 = 8.0;
/// One [`waiting_line`]: 12pt words in a band as tall as a group eyebrow's
/// row pair, inset as the eyebrows are.
pub(crate) const WAITING_LINE_HEIGHT: f32 = 24.0;
const WAITING_LINE_SIZE: f32 = 12.0;

/// "Waiting for denis@… to finish sharing" (spec 6.9): accepted, not yet
/// confirmed. The sharer's daemon confirms on its next pass; until then the
/// org shows this account nothing, and this line is why.
pub(crate) fn waiting_line(from: &str) -> String {
    format!("Waiting for {from} to finish sharing")
}

/// One line of `text`, ended with an ellipsis where it would pass `width` --
/// an address too long for the row is cut, never wrapped under the buttons.
fn one_line(painter: &egui::Painter, text: &str, font: egui::FontId, color: egui::Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap.max_width = width.max(0.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('\u{2026}');
    painter.layout_job(job)
}

/// **Shared with me's top**: Paste an invite link, where the window offers
/// it ([`PeopleRows::paste`]) -- the box, and Accept beside it -- then a
/// quiet [`waiting_line`] for every sharer this account accepted and is
/// waiting on. Draws nothing when there is neither.
///
/// Accept is greyed, and deaf, while the box is empty or a sharing write is
/// out ([`PeopleRows::busy`]): [`request`] would only refuse a second write
/// with "still running". A press is reported as [`PeopleAsk::AcceptLink`];
/// the window takes the box's text and sends it.
pub(crate) fn draw_paste_and_waiting(ui: &mut egui::Ui, people: &mut PeopleRows<'_>, width: f32) {
    if let Some(buffer) = people.paste.as_mut() {
        let (row, _) = ui.allocate_exact_size(egui::vec2(width, PASTE_ROW_HEIGHT), egui::Sense::hover());
        let button_width = theme::small_button_width(ui.painter(), PASTE_ACCEPT_LABEL);
        let field = egui::Rect::from_min_size(row.min, egui::vec2((width - button_width - PASTE_BUTTON_GAP).max(0.0), PASTE_ROW_HEIGHT));
        let mut field_ui = ui.new_child(egui::UiBuilder::new().max_rect(field));
        let _ = theme::inline_field(&mut field_ui, buffer, PASTE_HINT, field.width(), false);
        let button = egui::Rect::from_min_size(
            egui::pos2(field.right() + PASTE_BUTTON_GAP, row.center().y - theme::SMALL_BUTTON_HEIGHT / 2.0),
            egui::vec2(button_width, theme::SMALL_BUTTON_HEIGHT),
        );
        let enabled = !people.busy && !buffer.trim().is_empty();
        if theme::small_action_button(ui, button, PASTE_ACCEPT_LABEL, theme::ActionTone::Primary, enabled).clicked() {
            people.asked = Some(PeopleAsk::AcceptLink);
        }
    }
    for from in people.waiting {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, WAITING_LINE_HEIGHT), egui::Sense::hover());
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let galley = one_line(
            &painter,
            &waiting_line(from),
            egui::FontId::new(WAITING_LINE_SIZE, egui::FontFamily::Proportional),
            theme::TEXT_FAINT,
            width - GROUP_EYEBROW_INSET * 2.0,
        );
        let y = rect.center().y - galley.size().y / 2.0;
        painter.galley(egui::pos2(rect.left() + GROUP_EYEBROW_INSET, y), galley, theme::TEXT_FAINT);
    }
}

/// The height [`draw_paste_and_waiting`] takes, with the list's `gap` BETWEEN
/// its rows -- `n` rows, `n - 1` gaps, and nothing for none. The gap joining
/// them to rows under them is the caller's: only it knows whether there are
/// any (`send_ui`'s `rows_area`, which counts gaps the same way).
pub(crate) fn paste_and_waiting_height(people: &PeopleRows<'_>, row_gap: f32) -> f32 {
    let paste = usize::from(people.paste.is_some());
    let rows = paste + people.waiting.len();
    paste as f32 * PASTE_ROW_HEIGHT + people.waiting.len() as f32 * WAITING_LINE_HEIGHT + rows.saturating_sub(1) as f32 * row_gap
}

/// The leave question's two answers.
pub(crate) const STOP_RECEIVING_LABEL: &str = "Stop receiving";
pub(crate) const KEEP_RECEIVING_LABEL: &str = "Keep";

pub(crate) fn stop_receiving_title(from: &str) -> String {
    format!("Stop receiving from {from}?")
}

///
/// **Says plainly that they can add you back** (final review): on NodeWarden
/// a manager adds an existing account directly -- no invitation to accept or
/// decline -- so "They can invite you again later" promised a question that
/// never comes.
pub(crate) fn stop_receiving_body(from: &str) -> String {
    format!(
        "Every record {from} shares with you leaves your vault now. On this server they can add you back \
         directly, without asking you first."
    )
}

/// **"Stop receiving from X?"** -- every record from them leaves this vault,
/// so it is asked first, over a scrim, as the forget question is
/// (`record_ui::draw_forget_confirm`, whose shape this is). `Some(true)` to
/// leave, `Some(false)` to keep -- Escape included -- and `None` while
/// undecided.
///
/// The scrim is its own and spelled in the declaration, so
/// `item_list::MODAL_SCRIM_AREAS` names it (its census reads the literal)
/// and the item list's arrow keys stand down behind the question.
pub(crate) fn draw_leave_confirm(ctx: &egui::Context, from: &str) -> Option<bool> {
    let mut answer = None;
    theme::modal_scrim(ctx, egui::Area::new(egui::Id::new("record-leave-scrim")));
    theme::movable_modal(ctx, egui::Area::new(egui::Id::new("record-leave-modal"))).show(ctx, |ui| {
        theme::modal_drag_handle(ui, theme::MODAL_PLAIN_HEADER_HEIGHT);
        egui::Frame::new()
            .fill(theme::CARD)
            .corner_radius(egui::CornerRadius::same(10))
            .stroke(egui::Stroke::new(1.0, theme::BORDER))
            .inner_margin(egui::Margin::same(20))
            .show(ui, |ui| {
                ui.set_width(380.0);
                ui.label(theme::bold(stop_receiving_title(from), 15.0).color(theme::INK));
                ui.add_space(10.0);
                ui.label(egui::RichText::new(stop_receiving_body(from)).size(12.0).color(theme::TEXT_SECONDARY));
                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    if theme::destructive_button(ui, STOP_RECEIVING_LABEL).clicked() {
                        answer = Some(true);
                    }
                    ui.add_space(theme::FORM_FOOTER_GAP);
                    if theme::secondary_button(ui, KEEP_RECEIVING_LABEL).clicked() {
                        answer = Some(false);
                    }
                });
            });
    });
    if answer.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        answer = Some(false);
    }
    answer
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
            people: people.into_iter().map(|(e, s, x)| PersonEntry { email: e.into(), state: s, expires_at: x.map(str::to_string), authenticated: true }).collect(),
            records: records.into_iter().map(|(id, p)| RecordEntry { item_id: id.into(), people: p.into_iter().map(str::to_string).collect() }).collect(),
            ..SharingView::default()
        }
    }

    fn fingerprint(hash: &str) -> Fingerprint {
        Fingerprint { phrase: "cobalt-hinge-lantern-mossy-quill".into(), key_sha256: hash.into() }
    }

    /// **Every status line**, in its words (mock 6e, and official-path spec
    /// §3.4 for the waits): Invited and Accepted both "Waiting to accept",
    /// with how long ago where this PC shared it; nothing about an account
    /// or an expiry.
    #[test]
    fn each_persons_line_says_where_they_stand_in_the_mocks_words() {
        let zone = crate::local_time::FixedOffset(0);
        let v = view(
            vec![
                ("anna@x.io", PersonState::Shared, None),
                ("bob@x.io", PersonState::WaitingAccept, None),
                ("carol@x.io", PersonState::Invited, Some("2999-01-01T00:00:00Z")),
                ("dan@x.io", PersonState::Invited, Some("2000-01-01T00:00:00Z")),
                ("eve@x.io", PersonState::KeyChanged, None),
                ("fay@x.io", PersonState::WaitingSignIn, None),
                ("gus@x.io", PersonState::Gone, None),
            ],
            vec![("r1", vec!["anna@x.io", "bob@x.io", "carol@x.io", "dan@x.io", "eve@x.io", "fay@x.io", "gus@x.io"])],
        );
        let mut history = crate::sent_history::SentHistory::default();
        for (who, ago) in [("anna@x.io", 7 * DAY), ("carol@x.io", 6 * DAY), ("bob@x.io", DAY)] {
            history.record(crate::sent_history::SentRecord {
                kind: crate::sent_history::SentKind::Share,
                item_id: "r1".into(),
                recipient: who.into(),
                sent_at_unix_millis: NOW - ago,
                ..Default::default()
            });
        }
        let lines = person_lines(Some(&v), "r1", &history, NOW, &zone);
        let line = |email: &str| lines.iter().find(|l| l.email == email).map(|l| l.line.clone()).expect("a line");
        assert_eq!(line("anna@x.io"), "Shared \u{b7} since 14 Sep");
        assert_eq!(line("bob@x.io"), "Waiting to accept \u{b7} invited 1 day ago");
        assert_eq!(line("carol@x.io"), "Waiting to accept \u{b7} invited 6 days ago");
        assert_eq!(line("dan@x.io"), "Waiting to accept", "no record of the share here, and no expiry claimed");
        assert_eq!(line("eve@x.io"), "Their key changed. Check the fingerprint before sharing.");
        assert_eq!(line("fay@x.io"), "Waiting for them to sign in once to a Bitwarden app");
        assert_eq!(line("gus@x.io"), "No longer receiving this");
        for said in lines.iter().map(|l| l.line.to_lowercase()) {
            assert!(!said.contains("account") && !said.contains("expire") && !said.contains("ran out"), "{said}");
        }
        let pill = |email: &str| lines.iter().find(|l| l.email == email).map(PersonLine::pill).expect("a pill");
        assert_eq!(pill("anna@x.io"), theme::SharePillKind::Shared);
        assert_eq!(pill("carol@x.io"), theme::SharePillKind::Invited);
        assert!(lines.iter().find(|l| l.email == "anna@x.io").is_some_and(PersonLine::fingerprint));
        assert!(lines.iter().find(|l| l.email == "bob@x.io").is_some_and(|l| !l.fingerprint()), "no key held yet (mock 6b)");
        assert!(lines.iter().find(|l| l.email == "dan@x.io").is_some_and(PersonLine::reinvite));
        assert!(lines.iter().find(|l| l.email == "bob@x.io").is_some_and(|l| !l.reinvite()), "an Accepted person has nothing to re-accept");
        assert!(lines.iter().find(|l| l.email == "eve@x.io").is_some_and(PersonLine::share_anyway));
        assert_eq!(short_state(PersonState::Shared), "Shared");
        assert_eq!(short_state(PersonState::Invited), "Waiting to accept");
    }

    /// **The Share control is decided by the view's reason** (final review,
    /// I3): shares on NodeWarden, greyed with its own reason on a NodeWarden
    /// too old to share, and ABSENT everywhere sharing does not exist --
    /// `bw-serve`, `not-nodewarden`, `not-loaded`, no line, no view yet --
    /// because the envelope beside it already offers the Send. Absent on a
    /// received record too.
    #[test]
    fn the_share_control_is_decided_by_the_views_reason() {
        use crate::rest::sharing::{REASON_BW_SERVE, REASON_NOT_LOADED, REASON_TOO_OLD};
        let item = |id: &str| -> VaultItem { serde_json::from_value(serde_json::json!({ "id": id, "name": id })).expect("item") };
        let on = SharingView { available: true, ..SharingView::default() };
        assert_eq!(share_control(Some(&on), true, &item("mine")), Some(ShareControl::Share));
        assert_eq!(
            share_control(Some(&SharingView::unavailable(REASON_TOO_OLD)), true, &item("mine")),
            Some(ShareControl::Disabled(SHARE_TOO_OLD_TOOLTIP)),
            "server-too-old: greyed, saying why"
        );
        // "not-nodewarden" is a reason an older daemon could still answer
        // with: like any reason this window does not name, it draws nothing.
        for reason in [REASON_NOT_LOADED, REASON_BW_SERVE, "not-nodewarden", "some-later-reason"] {
            assert_eq!(share_control(Some(&SharingView::unavailable(reason)), true, &item("mine")), None, "{reason}: no control");
        }
        assert_eq!(share_control(None, false, &item("mine")), None, "no line to the daemon: the envelope is the Send");
        assert_eq!(share_control(None, true, &item("mine")), None, "not known yet");
        let mut received = on.clone();
        received.received.push(crate::rest::sharing::ReceivedEntry { item_id: "theirs".into(), from: "d@x.io".into() });
        assert_eq!(share_control(Some(&received), true, &item("theirs")), None);
    }

    /// **A record that cannot be shared says so before anything is typed**:
    /// one in another organisation draws no Share (it is that organisation's
    /// to share), one with attachments is greyed with the engine's own
    /// sentence -- while a record in this account's own sharing org, which
    /// the view lists, is shared as usual.
    #[test]
    fn a_record_that_cannot_be_shared_is_not_offered_share() {
        let on = SharingView {
            available: true,
            records: vec![RecordEntry { item_id: "in-mine".into(), people: vec![] }],
            ..SharingView::default()
        };
        let item = |value: serde_json::Value| -> VaultItem { serde_json::from_value(value).expect("item") };
        let other_org = item(serde_json::json!({ "id": "work", "name": "work", "organizationId": "acme" }));
        assert_eq!(share_control(Some(&on), true, &other_org), None, "another organisation's record");
        let mine = item(serde_json::json!({ "id": "in-mine", "name": "x", "organizationId": "my-sharing-org" }));
        assert_eq!(share_control(Some(&on), true, &mine), Some(ShareControl::Share), "already in the sharing org");
        let attached = item(serde_json::json!({ "id": "p1", "name": "x", "attachments": [{ "id": "a1" }] }));
        assert_eq!(share_control(Some(&on), true, &attached), Some(ShareControl::Disabled(crate::rest::sharing::HAS_ATTACHMENTS)));
        let empty = item(serde_json::json!({ "id": "p2", "name": "x", "attachments": [] }));
        assert_eq!(share_control(Some(&on), true, &empty), Some(ShareControl::Share), "an empty list is no attachment");
    }

    /// **A stop that moved the record home says where it went** (final
    /// review): the old id and the new, once, for the window's selection. A
    /// stop that left others on it, or failed, says nothing.
    #[test]
    fn a_stop_that_moved_the_record_home_hands_over_its_new_id() {
        let mut pane = SharingPane::default();
        pane.stopping = Some(("s1".into(), "anna@x.io".into()));
        apply(&mut pane, SharingReport::Stopped { email: "anna@x.io".into(), result: Ok(Some("p-new".into())) }, 0.0);
        assert_eq!(pane.take_moved_home(), Some(("s1".to_string(), "p-new".to_string())));
        assert_eq!(pane.take_moved_home(), None, "handed over once");
        assert!(pane.stopping.is_none());

        pane.stopping = Some(("s2".into(), "bob@x.io".into()));
        apply(&mut pane, SharingReport::Stopped { email: "bob@x.io".into(), result: Ok(None) }, 0.0);
        assert_eq!(pane.take_moved_home(), None, "still shared with someone: the id did not change");
        pane.stopping = Some(("s3".into(), "bob@x.io".into()));
        apply(&mut pane, SharingReport::Stopped { email: "bob@x.io".into(), result: Err("no".into()) }, 0.0);
        assert_eq!(pane.take_moved_home(), None);
        assert!(stop_receiving_body("denis@x.io").contains("add you back directly"), "{}", stop_receiving_body("denis@x.io"));
        assert!(!stop_receiving_body("denis@x.io").contains("invite you again"));
    }

    /// **With push off, the view is asked again once it is a minute old**
    /// (final review, I2) -- and not before, not while the hub is live (a
    /// push says when it moved), and not while a failed read waits for its
    /// retry, which would otherwise make a failing daemon a request a frame.
    #[test]
    fn with_no_push_the_view_is_read_again_once_a_minute() {
        let mut pane = SharingPane::default();
        pane.begin_view();
        apply(&mut pane, SharingReport::View(Ok(view(vec![], vec![]))), 100.0);
        assert!(!pane.wants_view(100.0), "a fresh view is not asked again");

        pane.heartbeat(100.0 + VIEW_HEARTBEAT_SECONDS - 1.0, false);
        assert!(!pane.wants_view(100.0 + VIEW_HEARTBEAT_SECONDS - 1.0), "not a minute old yet");
        pane.heartbeat(100.0 + VIEW_HEARTBEAT_SECONDS, true);
        assert!(!pane.wants_view(100.0 + VIEW_HEARTBEAT_SECONDS), "a live hub pushes changes; no polling");
        pane.heartbeat(100.0 + VIEW_HEARTBEAT_SECONDS, false);
        assert!(pane.wants_view(100.0 + VIEW_HEARTBEAT_SECONDS), "push off: a minute-old view is asked again");

        // That read fails: the heartbeat stands down until the retry is due.
        pane.begin_view();
        let failed_at = 100.0 + VIEW_HEARTBEAT_SECONDS;
        apply(&mut pane, SharingReport::View(Err("unreachable".into())), failed_at);
        for frame in 1..10 {
            let now = failed_at + f64::from(frame) * 0.5;
            pane.heartbeat(now, false);
            assert!(!pane.wants_view(now), "a failing daemon is not asked every frame ({now})");
        }
        assert!(pane.wants_view(failed_at + VIEW_RETRY_SECONDS), "the retry still comes");
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
        assert!(pane.view.as_ref().is_some_and(|v| v.records.len() == 1));
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
        assert_eq!(pane.notice.as_deref(), Some(SHARING_UNAVAILABLE), "the matching Confirm reaches the line check");
        assert!(pane.fingerprint.is_some(), "nothing was sent, so nothing was spent");

        let call = pane.share_anyway("eve@x.io").expect("a Confirm");
        assert!(request(&mut pane, Some(&dead_line()), &ctx, &tx, call));
        assert!(pane.notice.is_none(), "a write clears the last sentence");
        assert!(pane.fingerprint.is_none(), "a Confirm spends the fingerprint it was built from");
        assert!(pane.share_anyway("eve@x.io").is_none());
        match rx.recv_timeout(std::time::Duration::from_secs(30)).expect("a report") {
            SharingReport::Confirmed { email, result: Err(_) } => assert_eq!(email, "eve@x.io"),
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
        apply(&mut pane, SharingReport::LinkAccepted(Err("Couldn't stop sharing.".into())), 0.0);
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
            SharingReport::LinkAccepted(Err(secret.into())),
            SharingReport::Reinvited { email: "anna@x.io".into(), result: Err(secret.into()) },
            SharingReport::Confirmed { email: "anna@x.io".into(), result: Err(secret.into()) },
            SharingReport::Left { from: "anna@x.io".into(), result: Err(secret.into()) },
            SharingReport::Left { from: "anna@x.io".into(), result: Ok(()) },
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

    /// The rail's numbers: one per record in Shared with me. (The dot is
    /// read off the window's waiting lines, in `vault_window::mod`. Sent's
    /// share count is `share_rows`'s length -- the list's own rows -- and is
    /// held by `sent_lists_one_row_for_each_record_and_person`; Shared with me's is
    /// `received_share_rows`' length on the same terms, held by
    /// `received_rows_are_named_by_the_vault_and_need_their_record`.)
    #[test]
    fn the_rail_counts_come_from_the_view() {
        let mut pane = SharingPane::default();
        let items: Vec<VaultItem> = vec![serde_json::from_value(serde_json::json!({ "id": "t1", "name": "Theirs" })).expect("item")];
        let received = |pane: &SharingPane| received_share_rows(pane.view.as_ref(), &items).len();
        assert_eq!(received(&pane), 0);
        let mut v = view(
            vec![("anna@x.io", PersonState::Shared, None), ("bob@x.io", PersonState::WaitingAccept, None)],
            vec![("r1", vec!["anna@x.io", "bob@x.io"]), ("r2", vec!["anna@x.io"])],
        );
        v.received.push(crate::rest::sharing::ReceivedEntry { item_id: "t1".into(), from: "d@x.io".into() });
        pane.view = Some(v);
        assert_eq!(received(&pane), 1);
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
        assert!(matches!(write, SharingReport::Reinvited { .. }), "{write:?}");

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
    /// the record's name travels with the call, and "waiting to accept" -- which
    /// the open dialog says in its own words -- becomes a toast.
    #[test]
    fn a_share_answer_with_the_dialog_closed_still_says_where_it_stands() {
        let mut pane = SharingPane::default();
        let shared = |email: &str, result| SharingReport::Shared { email: email.into(), item_name: "Acme VPN".into(), result: Ok(result) };
        apply(&mut pane, shared("bob@x.io", ShareResult::WaitingAccept), 0.0);
        assert_eq!(pane.toast.as_ref().map(|(t, _)| t.as_str()), Some("bob@x.io was added. Acme VPN appears in their vault once they accept."));
        apply(&mut pane, shared("carol@x.io", ShareResult::Invited), 1.0);
        assert_eq!(pane.toast.as_ref().map(|(t, _)| t.as_str()), Some("Waiting for carol@x.io to accept."));

        pane.toast = None;
        pane.dialog = Some(ShareDialog { in_flight: true, ..ShareDialog::opening("r1", "Acme VPN") });
        apply(&mut pane, shared("carol@x.io", ShareResult::Invited), 2.0);
        assert!(pane.toast.is_none(), "the open dialog says it instead");
        assert_eq!(pane.dialog.as_ref().and_then(|d| d.invited.as_deref()), Some("carol@x.io"));
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

    /// **Invited** (mock 6a, fourth state): the dialog stays open to say the
    /// share is waiting for the person to accept, and an administrator is
    /// offered an invite code.
    #[test]
    fn an_invited_person_is_said_and_an_admin_can_make_an_invite_code() {
        let mut dialog = ShareDialog::opening("r1", "Acme VPN");
        dialog.invited = Some("carol@example.com".into());
        let admin = SharingView { available: true, can_mint_invites: true, ..SharingView::default() };
        let (_, texts) = modal_frame(&mut dialog, &SentHistory::default(), Some(&admin), Vec::new());
        let has = |needle: &str| texts.iter().any(|(t, _)| t.contains(needle));
        assert!(has("Waiting for carol@example.com to accept."), "{texts:?}");
        assert!(has(MAKE_CODE_BUTTON));
        assert!(has(DONE_BUTTON));
        assert!(!has(SHARE_BUTTON) || !texts.iter().any(|(t, _)| t == SHARE_BUTTON), "Done replaces Share and Cancel");
        let plain = SharingView { available: true, ..SharingView::default() };
        let (_, texts) = modal_frame(&mut dialog, &SentHistory::default(), Some(&plain), Vec::new());
        assert!(!texts.iter().any(|(t, _)| t == MAKE_CODE_BUTTON), "only an administrator can mint");
    }

    /// **The Share dialog warns where recipients can see each other**
    /// (official-path spec §3.4), and only there.
    #[test]
    fn the_share_dialog_warns_where_recipients_can_see_each_other() {
        let mut dialog = ShareDialog::opening("r1", "Acme VPN");
        let official = SharingView { available: true, roster_visible_to_recipients: true, ..SharingView::default() };
        let (_, texts) = modal_frame(&mut dialog, &SentHistory::default(), Some(&official), Vec::new());
        assert!(texts.iter().any(|(t, _)| t == ROSTER_VISIBLE_WARNING), "{texts:?}");
        let hidden = SharingView { available: true, roster_visible_to_recipients: false, ..SharingView::default() };
        let (_, texts) = modal_frame(&mut dialog, &SentHistory::default(), Some(&hidden), Vec::new());
        assert!(!texts.iter().any(|(t, _)| t == ROSTER_VISIBLE_WARNING), "{texts:?}");
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

    /// **The invited band claims no account either way** (official-path spec
    /// §3.4) and never says an email went out: an administrator is told
    /// about the invite code a newcomer to this server needs; anyone else is
    /// told only that the person has to accept.
    #[test]
    fn the_invited_band_claims_no_account_and_no_email() {
        for admin in [false, true] {
            let (head, body) = invited_band("carol@example.com", "Acme VPN", admin);
            let all = format!("{head} {body}").to_lowercase();
            assert!(!all.contains("email") && !all.contains("e-mail"), "{all}");
            assert!(!all.contains("create an account") && !all.contains("7 days"), "{all}");
            assert_eq!(head, "Waiting for carol@example.com to accept.");
            assert!(body.contains("Acme VPN"), "{body}");
            assert_eq!(all.contains("invite code"), admin, "{all}");
        }
    }

    /// **An invite code, once made, is on the band with a Copy** -- and the
    /// password box it was made with is gone.
    #[test]
    fn a_made_invite_code_is_shown_with_a_copy() {
        let admin = SharingView { available: true, can_mint_invites: true, ..SharingView::default() };
        let mut dialog = ShareDialog::opening("r1", "Acme VPN");
        dialog.invited = Some("carol@example.com".into());
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

    fn a_question() -> crate::rest::sharing::AdoptionQuestion {
        crate::rest::sharing::AdoptionQuestion { organization_id: "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0".into(), name: "Shared by me@x.io".into() }
    }

    /// **The organisation question is asked in the Share dialog** (fix
    /// round 1): the owner's sentence naming the org and its short id, Yes
    /// and No in place of Share and Cancel, and the address box no longer
    /// editable -- the answer is about the share that asked.
    #[test]
    fn the_organisation_question_is_asked_in_the_dialog_with_yes_and_no() {
        let mut dialog = ShareDialog::opening("r1", "Acme VPN");
        dialog.email = "anna@x.io".into();
        dialog.adopt = Some(a_question());
        let (_, texts) = modal_frame(&mut dialog, &SentHistory::default(), None, Vec::new());
        let has = |needle: &str| texts.iter().any(|(t, _)| t.contains(needle));
        assert!(has("Use the organisation \u{201c}Shared by me@x.io\u{201d} (0f1e2d3c) for sharing? Deskwarden did not create it."), "{texts:?}");
        assert!(texts.iter().any(|(t, _)| t == ADOPT_YES) && texts.iter().any(|(t, _)| t == ADOPT_NO), "{texts:?}");
        assert!(!texts.iter().any(|(t, _)| t == SHARE_BUTTON), "Yes and No replace Share: {texts:?}");
        let at = |label: &str| texts.iter().find(|(t, _)| t == label).map(|(_, r)| r.center()).expect("the button");
        let (yes, no) = (at(ADOPT_YES), at(ADOPT_NO));
        let (action, _) = modal_frame(&mut dialog, &SentHistory::default(), None, click(yes));
        assert_eq!(action, ShareDialogAction::Adopt);
        let (action, _) = modal_frame(&mut dialog, &SentHistory::default(), None, click(no));
        assert_eq!(action, ShareDialogAction::NewOrganization);
    }

    /// **The question lands in the dialog that asked**, its spinner down and
    /// the write slot free; a dialog closed meanwhile gets a sentence on the
    /// record instead, and nothing is chosen for the user.
    #[test]
    fn the_organisation_question_lands_in_the_dialog_that_asked() {
        let asked = || SharingReport::AdoptionAsked { email: "anna@x.io".into(), item_name: "Acme VPN".into(), question: a_question() };
        let mut pane = SharingPane::default();
        let mut dialog = ShareDialog::opening("r1", "Acme VPN");
        dialog.in_flight = true;
        pane.dialog = Some(dialog);
        pane.busy = true;
        apply(&mut pane, asked(), 1.0);
        let dialog = pane.dialog.as_ref().expect("the dialog stays open to ask");
        assert!(!dialog.in_flight);
        assert_eq!(dialog.adopt, Some(a_question()));
        assert!(!pane.busy, "the write slot is free while the user answers");

        let mut pane = SharingPane::default();
        pane.busy = true;
        apply(&mut pane, asked(), 1.0);
        assert!(pane.notice.as_deref().is_some_and(|n| n.contains("Acme VPN")), "{:?}", pane.notice);
    }

    /// **Yes adopts and then shares; No shares in a new organisation**, each
    /// one call from the window's worker -- and pressing either sends the
    /// question away and raises the spinner.
    #[test]
    fn yes_adopts_then_shares_and_no_shares_in_a_new_organisation() {
        let mut server = crate::test_http::server();
        let line = SharingClient::new(&server.url(), &Zeroizing::new("grant".to_string()));
        let adopted = server
            .mock("POST", "/sharing/adopt")
            .match_body(crate::test_http::Matcher::Json(serde_json::json!({ "organizationId": a_question().organization_id })))
            .with_body(r#"{"success":true}"#)
            .expect(1)
            .create();
        let shared = server
            .mock("POST", "/sharing/item/r1")
            .match_body(crate::test_http::Matcher::PartialJson(serde_json::json!({ "newOrganization": false })))
            .with_body(r#"{"success":true,"data":{"result":"shared"}}"#)
            .expect(1)
            .create();
        let call = |org| SharingCall::Share { item_id: "r1".into(), item_name: "Acme VPN".into(), email: "anna@x.io".into(), org };
        let report = run(&line, call(OrgAnswer::Adopt(a_question().organization_id)));
        assert!(matches!(report, SharingReport::Shared { result: Ok(ShareResult::Shared), .. }), "{report:?}");
        adopted.assert();
        shared.assert();
        let fresh = server
            .mock("POST", "/sharing/item/r1")
            .match_body(crate::test_http::Matcher::PartialJson(serde_json::json!({ "newOrganization": true })))
            .with_body(r#"{"success":true,"data":{"result":"waitingAccept"}}"#)
            .expect(1)
            .create();
        let report = run(&line, call(OrgAnswer::New));
        assert!(matches!(report, SharingReport::Shared { result: Ok(ShareResult::WaitingAccept), .. }), "{report:?}");
        fresh.assert();

        let (tx, _rx) = mpsc::channel();
        let ctx = egui::Context::default();
        for action in [ShareDialogAction::Adopt, ShareDialogAction::NewOrganization] {
            let mut pane = SharingPane::default();
            let mut dialog = ShareDialog::opening("r1", "Acme VPN");
            dialog.email = "anna@x.io".into();
            dialog.adopt = Some(a_question());
            let _ = answer_dialog(&mut pane, dialog, action.clone(), Some(&dead_line()), &ctx, &tx);
            let dialog = pane.dialog.as_ref().expect("the dialog stays open while the share runs");
            assert!(dialog.in_flight && dialog.adopt.is_none(), "{action:?}");
        }
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
        assert_eq!(dialog.error.as_deref(), Some(SHARING_UNAVAILABLE));
        assert!(pane.notice.is_none(), "said once, in the dialog");

        let line = dead_line();
        assert!(request(&mut pane, Some(&line), &ctx, &tx, SharingCall::Reinvite("bob@x.io".into())));
        let dialog = pane.dialog.take().expect("the dialog");
        let _ = answer_dialog(&mut pane, dialog, ShareDialogAction::Submit, Some(&line), &ctx, &tx);
        let dialog = pane.dialog.as_ref().expect("the dialog stays open");
        assert!(!dialog.in_flight, "a second write: the spinner is down");
        assert_eq!(dialog.error.as_deref(), Some(BUSY));

        let mut dialog = pane.dialog.take().expect("the dialog");
        dialog.invited = Some("carol@x.io".into());
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
        dialog.invited = Some("carol@x.io".into());
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
    /// must not have the old share's "waiting to accept" or failure painted into
    /// it; the answer is said as a toast or a notice instead.
    #[test]
    fn an_answer_does_not_land_in_a_dialog_that_did_not_ask() {
        let mut pane = SharingPane::default();
        pane.dialog = Some(ShareDialog::opening("r2", "Wi-Fi"));
        apply(&mut pane, SharingReport::Shared { email: "carol@x.io".into(), item_name: "Acme VPN".into(), result: Ok(ShareResult::Invited) }, 0.0);
        assert!(pane.dialog.as_ref().is_some_and(|d| d.invited.is_none()));
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

    // ---- the SHARING card's people (mock 6b) ----------------------------------------

    /// A harness frame: the People block alone, drawn into a 700x800 window
    /// after one warm frame (a press is hit-tested against the widgets of the
    /// frame before it), and every string it painted.
    fn block_frame(people: super::super::detail::PeopleSharing<'_>, events: Vec<egui::Event>) -> (super::super::detail::DetailAction, Vec<(String, egui::Rect)>) {
        let ctx = egui::Context::default();
        let input = |events: Vec<egui::Event>| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(700.0, 800.0))),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input(Vec::new()), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(Vec::new()), |ui| {
            let mut ignored = super::super::detail::DetailAction::None;
            draw_people_block(ui, "Acme VPN", &people, &mut ignored);
        });
        let mut action = super::super::detail::DetailAction::None;
        let output = ctx.run_ui(input(events), |ui| draw_people_block(ui, "Acme VPN", &people, &mut action));
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_texts(&clipped.shape, &mut texts);
        }
        (action, texts)
    }

    fn lines() -> Vec<PersonLine> {
        vec![
            PersonLine { email: "anna@x.io".into(), state: PersonState::Shared, line: "Shared \u{b7} since 14 Sep".into(), authenticated: true },
            PersonLine { email: "carol@x.io".into(), state: PersonState::Invited, line: "Waiting to accept \u{b7} invited 6 days ago".into(), authenticated: true },
        ]
    }

    fn centre_of(texts: &[(String, egui::Rect)], needle: &str) -> egui::Pos2 {
        texts.iter().find(|(t, _)| t == needle).map(|(_, r)| r.center()).unwrap_or_else(|| panic!("no {needle:?} in {texts:?}"))
    }

    /// **Mock 6b at rest**: each person, their pill, their line; Stop
    /// sharing on every row; Re-invite where it helps; the fingerprint link
    /// only where a key is held.
    #[test]
    fn the_people_block_draws_each_person_with_their_controls() {
        let people_lines = lines();
        let people = super::super::detail::PeopleSharing { people: &people_lines, ..Default::default() };
        let (_, texts) = block_frame(people, Vec::new());
        let count = |needle: &str| texts.iter().filter(|(t, _)| t == needle).count();
        assert_eq!(count("anna@x.io"), 1);
        assert_eq!(count(STOP_LABEL), 2);
        assert_eq!(count(REINVITE_LABEL), 1, "only carol's invite can be renewed");
        assert_eq!(count(SHOW_FINGERPRINT), 1, "only anna's key is held");
        assert_eq!(count("Shared"), 1);
        assert_eq!(count("Invited"), 1, "carol's pill");
        assert_eq!(count(PEOPLE_BLOCK_LABEL), 1, "the stacked label over the entries");
        assert_eq!(count("A"), 1, "anna's avatar carries her initial: {texts:?}");
        assert_eq!(count("C"), 1, "carol's avatar carries hers");
    }

    /// **Stop sharing is two steps** (mock 6b): the first reports
    /// `StopSharing`; with the question up, Cancel sits in its slot and the red
    /// button under the row reports `ConfirmStopSharing`.
    #[test]
    fn stopping_is_asked_before_it_happens() {
        use super::super::detail::DetailAction;
        let people_lines = lines();
        let at_rest = super::super::detail::PeopleSharing { people: &people_lines, ..Default::default() };
        let (_, texts) = block_frame(at_rest, Vec::new());
        let stop = texts.iter().find(|(t, _)| t == STOP_LABEL).map(|(_, r)| r.center()).expect("Stop sharing");
        assert_eq!(block_frame(at_rest, click(stop)).0, DetailAction::StopSharing("anna@x.io".into()));

        let asking = super::super::detail::PeopleSharing { people: &people_lines, confirming: Some("anna@x.io"), ..Default::default() };
        let (_, texts) = block_frame(asking, Vec::new());
        assert!(texts.iter().any(|(t, _)| t == &stop_question("Acme VPN", "anna@x.io")), "{texts:?}");
        assert!(texts.iter().any(|(t, _)| t == STOP_HINT), "the note that a copy stays with them");
        let remove = texts.iter().find(|(t, _)| t == REMOVE_LABEL).map(|(_, r)| r.center()).expect("the red button");
        assert_eq!(block_frame(asking, click(remove)).0, DetailAction::ConfirmStopSharing("anna@x.io".into()));
        let cancel = texts.iter().find(|(t, _)| t == CANCEL_BUTTON).map(|(_, r)| r.center()).expect("Cancel");
        assert_eq!(block_frame(asking, click(cancel)).0, DetailAction::CancelStopSharing);
        // **Cancel is where Stop sharing was**, so a second rapid click on
        // Stop lands on the way out and never on the red button below.
        assert_eq!(block_frame(asking, click(stop)).0, DetailAction::CancelStopSharing, "a second click where Stop was");
        assert!(remove.y > stop.y, "the question went above the row, which moves it");
        assert_eq!(texts.iter().filter(|(t, _)| t == STOP_LABEL).count(), 1, "only carol's Stop is offered while anna's is asked");
    }

    /// While a stop runs, the row says so and offers nothing -- mock 6e's
    /// `Stopping…` -- and a click where Stop was reports nothing.
    #[test]
    fn a_running_stop_says_so_and_offers_nothing() {
        use super::super::detail::DetailAction;
        let people_lines = lines();
        let at_rest = super::super::detail::PeopleSharing { people: &people_lines, ..Default::default() };
        let (_, texts) = block_frame(at_rest, Vec::new());
        let stop = centre_of(&texts, STOP_LABEL);
        let running = super::super::detail::PeopleSharing {
            people: &people_lines,
            confirming: Some("anna@x.io"),
            stopping: Some("anna@x.io"),
            ..Default::default()
        };
        let (action, texts) = block_frame(running, click(stop));
        assert_eq!(action, DetailAction::None);
        assert!(texts.iter().any(|(t, _)| t == STOPPING_LABEL), "{texts:?}");
        assert!(!texts.iter().any(|(t, _)| t == REMOVE_LABEL || t == CANCEL_BUTTON), "{texts:?}");
    }

    /// The fingerprint, once fetched, is shown under the person with the
    /// sentence that says what to compare it with (mock 6b, fingerprint).
    #[test]
    fn a_shown_fingerprint_says_what_to_compare_it_with() {
        use super::super::detail::DetailAction;
        let people_lines = lines();
        let people = super::super::detail::PeopleSharing {
            people: &people_lines,
            fingerprint: Some(("anna@x.io", "turban-deftly-anime-chatroom-unselfish")),
            ..Default::default()
        };
        let (_, texts) = block_frame(people, Vec::new());
        assert!(texts.iter().any(|(t, _)| t == "turban-deftly-anime-chatroom-unselfish"));
        assert!(texts.iter().any(|(t, _)| t == HIDE_FINGERPRINT));
        assert!(texts.iter().any(|(t, _)| t.contains("Account \u{203a} Fingerprint phrase")));
        let hide = centre_of(&texts, HIDE_FINGERPRINT);
        assert_eq!(block_frame(people, click(hide)).0, DetailAction::HideFingerprint);
        let at_rest = super::super::detail::PeopleSharing { people: &people_lines, ..Default::default() };
        let (_, texts) = block_frame(at_rest, Vec::new());
        let show = centre_of(&texts, SHOW_FINGERPRINT);
        assert_eq!(block_frame(at_rest, click(show)).0, DetailAction::ShowFingerprint("anna@x.io".into()));
    }

    /// **Show fingerprint is not offered twice at once**: while one is
    /// being fetched the person's link says so, and every other Show is
    /// greyed and reports nothing.
    #[test]
    fn show_fingerprint_waits_while_one_is_being_fetched() {
        use super::super::detail::DetailAction;
        let people_lines = vec![
            PersonLine { email: "anna@x.io".into(), state: PersonState::Shared, line: "Shared".into(), authenticated: true },
            PersonLine { email: "eve@x.io".into(), state: PersonState::Shared, line: "Shared".into(), authenticated: true },
        ];
        let at_rest = super::super::detail::PeopleSharing { people: &people_lines, ..Default::default() };
        let (_, texts) = block_frame(at_rest, Vec::new());
        let eves = texts.iter().filter(|(t, _)| t == SHOW_FINGERPRINT).map(|(_, r)| r.center()).last().expect("eve's Show");
        let fetching = super::super::detail::PeopleSharing { people: &people_lines, fingerprinting: Some("anna@x.io"), ..Default::default() };
        let (action, texts) = block_frame(fetching, click(eves));
        assert_eq!(action, DetailAction::None, "a second Show while one is out");
        assert_eq!(texts.iter().filter(|(t, _)| t == FETCHING_FINGERPRINT).count(), 1, "{texts:?}");
        assert_eq!(texts.iter().filter(|(t, _)| t == SHOW_FINGERPRINT).count(), 1, "eve's, greyed");
    }

    /// **A changed key is shared anyway only after its fingerprint is on
    /// screen**: the button is there but does nothing until Show fingerprint
    /// has answered for that person, and then the five words come with the
    /// warning that says to compare them by phone or in person.
    #[test]
    fn a_changed_key_is_shared_anyway_only_after_its_fingerprint_is_shown() {
        use super::super::detail::DetailAction;
        let people_lines = vec![PersonLine {
            email: "eve@x.io".into(),
            state: PersonState::KeyChanged,
            line: "Their key changed. Check the fingerprint before sharing.".into(),
            authenticated: true,
        }];
        let unchecked = super::super::detail::PeopleSharing { people: &people_lines, ..Default::default() };
        let (_, texts) = block_frame(unchecked, Vec::new());
        assert!(texts.iter().any(|(t, _)| t == "Key changed"), "the pill: {texts:?}");
        let anyway = centre_of(&texts, SHARE_ANYWAY_LABEL);
        assert_eq!(block_frame(unchecked, click(anyway)).0, DetailAction::None, "no fingerprint seen yet");
        assert!(texts.iter().any(|(t, _)| t == SHOW_FINGERPRINT));

        let someone_elses = super::super::detail::PeopleSharing {
            people: &people_lines,
            fingerprint: Some(("anna@x.io", "cobalt-hinge-lantern-mossy-quill")),
            ..Default::default()
        };
        assert_eq!(block_frame(someone_elses, click(anyway)).0, DetailAction::None, "another person's fingerprint");

        let checked = super::super::detail::PeopleSharing {
            people: &people_lines,
            fingerprint: Some(("eve@x.io", "cobalt-hinge-lantern-mossy-quill")),
            ..Default::default()
        };
        let (_, texts) = block_frame(checked, Vec::new());
        assert!(texts.iter().any(|(t, _)| t == "cobalt-hinge-lantern-mossy-quill"));
        assert!(texts.iter().any(|(t, _)| t == &key_changed_note("eve@x.io")), "{texts:?}");
        assert!(key_changed_note("eve@x.io").contains("by phone or in person"));
        let anyway = centre_of(&texts, SHARE_ANYWAY_LABEL);
        assert_eq!(block_frame(checked, click(anyway)).0, DetailAction::ShareAnyway("eve@x.io".into()));
    }

    /// **An invited person can be re-invited**: Re-invite and Stop sharing
    /// stay, no fingerprint is offered, and Re-invite reports the person.
    #[test]
    fn an_invited_person_can_be_reinvited() {
        use super::super::detail::DetailAction;
        let people_lines = vec![PersonLine {
            email: "carol@x.io".into(),
            state: PersonState::Invited,
            line: "Waiting to accept \u{b7} invited 6 days ago".into(),
            authenticated: true,
        }];
        let people = super::super::detail::PeopleSharing { people: &people_lines, ..Default::default() };
        let (_, texts) = block_frame(people, Vec::new());
        assert!(texts.iter().any(|(t, _)| t == "Invited"));
        assert!(!texts.iter().any(|(t, _)| t == SHOW_FINGERPRINT), "no key is held for an invitation");
        let reinvite = centre_of(&texts, REINVITE_LABEL);
        assert_eq!(block_frame(people, click(reinvite)).0, DetailAction::Reinvite("carol@x.io".into()));
    }

    /// **A person known only from a plaintext name is shown, marked, and
    /// offered Stop alone** (final review, I1): no Re-invite and no Share
    /// anyway, whatever their state -- both are grants the engine refuses
    /// for them -- and the status line says why.
    #[test]
    fn an_unverified_person_is_marked_and_offered_no_reinvite() {
        let unverified = SharingView {
            available: true,
            people: vec![
                PersonEntry { email: "mal@x.io".into(), state: PersonState::Invited, expires_at: None, authenticated: false },
                PersonEntry { email: "eve@x.io".into(), state: PersonState::KeyChanged, expires_at: None, authenticated: false },
            ],
            records: vec![crate::rest::sharing::RecordEntry { item_id: "r1".into(), people: vec!["mal@x.io".into(), "eve@x.io".into()] }],
            ..SharingView::default()
        };
        let lines = person_lines(Some(&unverified), "r1", &SentHistory::default(), NOW, &crate::local_time::FixedOffset(0));
        assert_eq!(lines.len(), 2, "both are shown");
        for line in &lines {
            assert!(!line.authenticated && !line.reinvite() && !line.share_anyway(), "{line:?}");
            assert!(line.line.starts_with(UNVERIFIED_PREFIX), "{line:?}");
        }
        let people = super::super::detail::PeopleSharing { people: &lines, ..Default::default() };
        let (_, texts) = block_frame(people, Vec::new());
        assert!(!texts.iter().any(|(t, _)| t == REINVITE_LABEL || t == SHARE_ANYWAY_LABEL), "{texts:?}");
        assert!(texts.iter().any(|(t, _)| t == STOP_LABEL), "Stop is still offered: {texts:?}");
    }

    /// **A fingerprint is a read with its own one-at-a-time slot**: asking
    /// for one leaves the last write's sentence on the card (it is not a
    /// write), a second while it is out is refused out loud, and its answer
    /// -- either way -- frees the slot.
    #[test]
    fn a_fingerprint_request_keeps_the_notice_and_holds_its_own_slot() {
        let mut pane = SharingPane::default();
        pane.notice = Some("Couldn't stop sharing.".into());
        let (tx, rx) = mpsc::channel();
        let ctx = egui::Context::default();
        let line = dead_line();
        assert!(request(&mut pane, Some(&line), &ctx, &tx, SharingCall::Fingerprint("Anna@X.io".into())));
        assert_eq!(pane.notice.as_deref(), Some("Couldn't stop sharing."), "a read clears no sentence");
        assert_eq!(pane.fingerprinting.as_deref(), Some("anna@x.io"));
        assert!(!request(&mut pane, Some(&line), &ctx, &tx, SharingCall::Fingerprint("eve@x.io".into())), "one at a time");
        assert_eq!(pane.notice.as_deref(), Some(FINGERPRINT_BUSY));
        assert_eq!(pane.fingerprinting.as_deref(), Some("anna@x.io"), "the refused one took nothing over");
        let report = rx.recv_timeout(std::time::Duration::from_secs(30)).expect("a report");
        match &report {
            SharingReport::Fingerprint { email, .. } => assert_eq!(email, "Anna@X.io", "the answer is the first read's"),
            other => panic!("expected the fingerprint's answer, got {other:?}"),
        }
        apply(&mut pane, report, 0.0);
        assert!(pane.fingerprinting.is_none(), "answered, even with a failure: the slot is free");
        assert_ne!(pane.notice.as_deref(), Some(FINGERPRINT_BUSY), "the busy sentence went with the slot");
        // The refused read started no thread: had it, the slot would have
        // named eve, and its report could be waiting already. No wait here --
        // the slot is the evidence.
        assert!(rx.try_recv().is_err(), "the refused one started nothing");
    }

    /// **A busy sentence goes when the slot it was about frees** -- on the
    /// card and in an open dialog that was told it -- and a sentence the
    /// answer itself brings stays.
    #[test]
    fn a_busy_sentence_goes_when_the_slot_it_was_about_frees() {
        let mut pane = SharingPane::default();
        pane.say(BUSY.to_string(), None);
        pane.dialog = Some(ShareDialog { error: Some(BUSY.to_string()), ..ShareDialog::opening("r1", "Acme VPN") });
        apply(&mut pane, SharingReport::Reinvited { email: "bob@x.io".into(), result: Ok(()) }, 0.0);
        assert!(pane.notice.is_none(), "the write finished; 'still running' is no longer true");
        assert!(pane.dialog.as_ref().is_some_and(|d| d.error.is_none()));

        pane.say(BUSY.to_string(), None);
        apply(&mut pane, SharingReport::Reinvited { email: "bob@x.io".into(), result: Err("no".into()) }, 1.0);
        assert_eq!(pane.notice.as_deref(), Some("no"), "the answer's own sentence stays");

        pane.say(FINGERPRINT_BUSY.to_string(), None);
        apply(&mut pane, SharingReport::Fingerprint { email: "anna@x.io".into(), result: Ok(fingerprint("ab12")) }, 2.0);
        assert!(pane.notice.is_none());
        pane.say(BUSY.to_string(), None);
        apply(&mut pane, SharingReport::Fingerprint { email: "anna@x.io".into(), result: Ok(fingerprint("ab12")) }, 3.0);
        assert_eq!(pane.notice.as_deref(), Some(BUSY), "a read frees no write slot, so the write's busy stands");
    }

    /// **Share anyway and Re-invite say how they went**: a toast naming the
    /// person on success, the engine's sentence on failure.
    #[test]
    fn share_anyway_and_reinvite_say_how_they_went() {
        let mut pane = SharingPane::default();
        apply(&mut pane, SharingReport::Confirmed { email: "eve@x.io".into(), result: Ok(()) }, 0.0);
        assert_eq!(pane.toast.as_ref().map(|(t, _)| t.as_str()), Some("Shared with eve@x.io on their new key."));
        apply(&mut pane, SharingReport::Reinvited { email: "carol@x.io".into(), result: Ok(()) }, 1.0);
        assert_eq!(pane.toast.as_ref().map(|(t, _)| t.as_str()), Some("Invitation sent to carol@x.io."));
        pane.toast = None;
        apply(&mut pane, SharingReport::Reinvited { email: "carol@x.io".into(), result: Err("Couldn't re-invite.".into()) }, 2.0);
        assert!(pane.toast.is_none());
        assert_eq!(pane.notice.as_deref(), Some("Couldn't re-invite."));
        // A panicked worker reports as the call's own kind, so the toast's
        // person is still known and the slot is still freed.
        let panicked = SharingCall::Reinvite("carol@x.io".into()).failed(PANICKED);
        assert!(matches!(&panicked, SharingReport::Reinvited { email, .. } if email == "carol@x.io"));
        let panicked = SharingCall::Confirm { email: "eve@x.io".into(), key_sha256: "ab".into() }.failed(PANICKED);
        assert!(matches!(&panicked, SharingReport::Confirmed { email, .. } if email == "eve@x.io"));
    }

    /// **A fingerprint shown for a state that has moved is dropped** when
    /// the view says so -- the changed-key box must never show the words of
    /// the key before -- and one whose person still stands where they stood
    /// is kept.
    #[test]
    fn a_moved_state_drops_the_fingerprint_shown_for_the_state_before() {
        let mut pane = SharingPane::default();
        pane.view = Some(view(vec![("eve@x.io", PersonState::KeyChanged, None)], vec![("r1", vec!["eve@x.io"])]));
        apply(&mut pane, SharingReport::Fingerprint { email: "Eve@X.io".into(), result: Ok(fingerprint("ab12")) }, 0.0);
        apply(&mut pane, SharingReport::View(Ok(view(vec![("eve@x.io", PersonState::KeyChanged, None)], vec![("r1", vec!["eve@x.io"])]))), 1.0);
        assert!(pane.fingerprint.is_some(), "nothing moved: the words stay");
        assert!(pane.share_anyway("eve@x.io").is_some());
        apply(&mut pane, SharingReport::View(Ok(view(vec![("eve@x.io", PersonState::Shared, None)], vec![("r1", vec!["eve@x.io"])]))), 2.0);
        assert!(pane.fingerprint.is_none(), "the state moved: the words were for the state before");

        // Shown for a person the view did not list yet: the view that first
        // lists them has moved them too.
        let mut pane = SharingPane::default();
        pane.view = Some(view(vec![], vec![]));
        apply(&mut pane, SharingReport::Fingerprint { email: "eve@x.io".into(), result: Ok(fingerprint("ab12")) }, 0.0);
        apply(&mut pane, SharingReport::View(Ok(view(vec![("eve@x.io", PersonState::KeyChanged, None)], vec![]))), 1.0);
        assert!(pane.fingerprint.is_none());
    }

    /// **A write's failure is said on the record it was asked on.** The
    /// card on that record shows it and another record's card does not; if
    /// the user has moved on, the read pane routes it to a toast. A sentence
    /// about no record -- a refusal -- shows wherever the user is.
    #[test]
    fn a_writes_failure_is_said_on_the_record_it_was_asked_on() {
        let (tx, rx) = mpsc::channel();
        let ctx = egui::Context::default();
        let line = dead_line();
        let mut pane = SharingPane::default();
        assert!(request_on(&mut pane, "r1", Some(&line), &ctx, &tx, SharingCall::Reinvite("carol@x.io".into())));
        let report = rx.recv_timeout(std::time::Duration::from_secs(30)).expect("a report");
        apply(&mut pane, report, 0.0);
        let said = pane.notice.clone().expect("the failure is said");
        assert_eq!(pane.notice_for("r1"), Some(said.as_str()));
        assert_eq!(pane.notice_for("r2"), None, "another record's card");
        route_stray_notice(&mut pane, "r1", 1.0);
        assert!(pane.notice.is_some() && pane.toast.is_none(), "on its own record it stays on the card");
        route_stray_notice(&mut pane, "r2", 1.0);
        assert!(pane.notice.is_none());
        assert_eq!(pane.toast.as_ref().map(|(t, _)| t.as_str()), Some(said.as_str()), "moved on: said as a toast");

        // A stop carries its record in itself.
        pane.toast = None;
        let stop = SharingCall::Stop { item_id: "r7".into(), email: "anna@x.io".into() };
        assert!(request(&mut pane, Some(&line), &ctx, &tx, stop));
        let report = rx.recv_timeout(std::time::Duration::from_secs(30)).expect("a report");
        apply(&mut pane, report, 2.0);
        assert!(pane.notice_for("r7").is_some() && pane.notice_for("r1").is_none());

        // A refusal is about no record: it shows on any card, and is never
        // routed away -- a later refusal does not inherit the stop's record.
        assert!(!request(&mut pane, None, &ctx, &tx, SharingCall::Reinvite("x@x.io".into())));
        assert_eq!(pane.notice_for("r1"), Some(SHARING_UNAVAILABLE));
        route_stray_notice(&mut pane, "r1", 3.0);
        assert_eq!(pane.notice.as_deref(), Some(SHARING_UNAVAILABLE));
    }

    /// **The re-prompt question is asked of every record a membership
    /// reaches**: one protected record shared with the person is enough,
    /// whatever record is on screen; a protected record NOT shared with them
    /// is not their question.
    #[test]
    fn a_protected_record_shared_with_a_person_is_found() {
        let item = |id: &str, reprompt: u8| -> VaultItem {
            serde_json::from_value(serde_json::json!({ "id": id, "name": id, "reprompt": reprompt })).expect("item")
        };
        let items = vec![item("plain", 0), item("locked", 1), item("elsewhere", 1)];
        let v = view(
            vec![("eve@x.io", PersonState::KeyChanged, None), ("bob@x.io", PersonState::Shared, None)],
            vec![("plain", vec!["eve@x.io", "bob@x.io"]), ("locked", vec!["eve@x.io"]), ("elsewhere", vec![])],
        );
        assert!(shares_a_protected_record(Some(&v), "Eve@X.io", &items));
        assert!(!shares_a_protected_record(Some(&v), "bob@x.io", &items), "bob has only the plain one");
        assert!(!shares_a_protected_record(None, "eve@x.io", &items));
    }

    /// The window hands the card what it is in the middle of: the notice and
    /// the fingerprint being fetched, as well as Task 15's states.
    #[test]
    fn the_read_pane_is_handed_the_notice_and_the_fetch() {
        let item: VaultItem = serde_json::from_value(serde_json::json!({ "id": "r1", "name": "Acme VPN" })).expect("item");
        let mut pane = SharingPane::default();
        pane.notice = Some("no".into());
        pane.fingerprinting = Some("anna@x.io".into());
        let people = people_sharing(&pane, true, &item, &[]);
        assert_eq!(people.notice, Some("no"));
        assert_eq!(people.fingerprinting, Some("anna@x.io"));
        pane.say("Couldn't stop sharing Wi-Fi.".into(), Some("r2".into()));
        assert_eq!(people_sharing(&pane, true, &item, &[]).notice, None, "another record's sentence");
    }

    /// **The share toast** (mock 6a): the sentence, in the dark box, inside
    /// the pane's bottom right -- and nothing without one.
    #[test]
    fn the_share_toast_is_drawn_in_the_panes_corner() {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0))),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let pane_rect = egui::Rect::from_min_max(egui::pos2(400.0, 0.0), egui::pos2(900.0, 700.0));
        let mut pane = SharingPane::default();
        let frame = |pane: &SharingPane, copy: Option<egui::Rect>| {
            let output = ctx.run_ui(input(), |ui| draw_share_toast(ui, pane_rect, copy, pane));
            let mut texts = Vec::new();
            for clipped in &output.shapes {
                collect_texts(&clipped.shape, &mut texts);
            }
            texts
        };
        assert!(frame(&pane, None).is_empty());
        pane.toast = Some((shared_toast("anna@x.io"), 5.0));
        let texts = frame(&pane, None);
        let (_, at) = texts.iter().find(|(t, _)| *t == shared_toast("anna@x.io")).expect("the toast");
        assert!(pane_rect.contains_rect(*at), "{at:?}");
        assert!(at.center().x > pane_rect.center().x && at.center().y > pane_rect.center().y, "bottom right: {at:?}");

        // **Above a live copy confirmation, never over it**: the two share
        // the corner.
        let copy = egui::Rect::from_min_max(egui::pos2(740.0, 640.0), egui::pos2(880.0, 680.0));
        let texts = frame(&pane, Some(copy));
        let (_, stacked) = texts.iter().find(|(t, _)| *t == shared_toast("anna@x.io")).expect("the toast");
        assert!(stacked.bottom() < copy.top(), "{stacked:?} runs into the copy box {copy:?}");
        assert!(pane_rect.contains_rect(*stacked));
    }

    // ---- Sent's person shares (mock 6c) ------------------------------------------------

    /// **One row per (record, person)** (mock 6c), newest share first, with
    /// the person mark's subtitle "to anna@example.com · Shared" -- and a
    /// share this PC holds no record of last, with no date to show.
    #[test]
    fn sent_lists_one_row_for_each_record_and_person() {
        let v = view(
            vec![("anna@x.io", PersonState::Shared, None), ("bob@x.io", PersonState::WaitingAccept, None)],
            vec![("r1", vec!["anna@x.io", "bob@x.io"]), ("r2", vec!["anna@x.io"])],
        );
        let items: Vec<VaultItem> = ["r1", "r2"]
            .iter()
            .map(|id| serde_json::from_value(serde_json::json!({ "id": id, "name": format!("Record {id}") })).expect("item"))
            .collect();
        let mut history = SentHistory::default();
        history.record(crate::sent_history::SentRecord {
            kind: SentKind::Share,
            item_id: "r2".into(),
            recipient: "anna@x.io".into(),
            sent_at_unix_millis: NOW,
            ..Default::default()
        });
        let rows = share_rows(Some(&v), &[&items], &history, &crate::local_time::FixedOffset(0));
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].item_id, "r2", "the newest share first");
        assert_eq!(rows[0].subtitle, "to anna@x.io \u{b7} Shared");
        assert_eq!(rows[0].state_words(), "Shared");
        assert_eq!(rows[0].since.as_deref(), Some(day(NOW, &crate::local_time::FixedOffset(0)).as_str()));
        assert!(rows.iter().any(|r| r.subtitle == "to bob@x.io \u{b7} Waiting to accept"));
        assert_eq!(rows[2].since, None, "no local record, so no date and the end of the list");
        assert_eq!(parse_share_key(&rows[0].key), Some(("r2".to_string(), "anna@x.io".to_string())));
        assert_eq!(parse_share_key("send-id"), None, "a Send's id is not a share key");
        assert_eq!(share_rows(None, &[&items], &history, &crate::local_time::FixedOffset(0)), Vec::new());
    }

    /// **A record the vault does not hold is not listed**, and a person the
    /// view names twice on one record is one row: the key is the row.
    #[test]
    fn a_share_row_needs_its_record_and_is_listed_once() {
        let v = view(
            vec![("anna@x.io", PersonState::Shared, None)],
            vec![("r1", vec!["anna@x.io", "Anna@X.io"]), ("gone", vec!["anna@x.io"])],
        );
        let items: Vec<VaultItem> = vec![serde_json::from_value(serde_json::json!({ "id": "r1", "name": "Acme VPN" })).expect("item")];
        let rows = share_rows(Some(&v), &[&items], &SentHistory::default(), &crate::local_time::FixedOffset(0));
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].name, "Acme VPN", "the vault's name");
        assert_eq!(rows[0].pill(), theme::SharePillKind::Shared);

        // An ARCHIVED record's share is still live: found in the archive, it
        // is listed -- the live list is looked in first.
        let archived: Vec<VaultItem> = vec![serde_json::from_value(serde_json::json!({ "id": "gone", "name": "Old VPN" })).expect("item")];
        let rows = share_rows(Some(&v), &[&items, &archived], &SentHistory::default(), &crate::local_time::FixedOffset(0));
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(rows.iter().any(|r| r.item_id == "gone" && r.name == "Old VPN"));
    }

    // ---- Shared with me: records from people (mock 6d) ------------------------------

    /// A harness frame: `draw` in a `width`-wide window after one warm frame
    /// (a press is hit-tested against the widgets of the frame before it),
    /// and every string the measured frame painted.
    fn ui_frame(width: f32, events: Vec<egui::Event>, mut draw: impl FnMut(&mut egui::Ui)) -> Vec<(String, egui::Rect)> {
        let ctx = egui::Context::default();
        let input = |events: Vec<egui::Event>| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 800.0))),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input(Vec::new()), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(Vec::new()), |ui| draw(ui));
        let output = ctx.run_ui(input(events), |ui| draw(ui));
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_texts(&clipped.shape, &mut texts);
        }
        texts
    }

    /// **Mock 6d**: FROM PEOPLE above IMPORTED FROM SENDS, each received row
    /// "from denis@example.com · view-only", and a click opens the record.
    #[test]
    fn shared_with_me_lists_received_records_from_people_and_opens_them() {
        use crate::vault_window::send_ui::{RecordTiles, SendUiAction};
        let received = [ReceivedShareRow { key: "recv:r9".into(), item_id: "r9".into(), name: "Acme VPN".into(), from: "denis@x.io".into() }];
        let mut people = PeopleRows { received: &received, ..PeopleRows::none() };
        let mut selected = None;
        let texts = ui_frame(400.0, Vec::new(), |ui| {
            let _ = draw_received_group(ui, &mut people, 376.0, 62.0, &mut selected, RecordTiles::default());
        });
        assert!(texts.iter().any(|(t, _)| t == FROM_PEOPLE_EYEBROW));
        assert!(texts.iter().any(|(t, _)| t == "from denis@x.io \u{b7} view-only"), "{texts:?}");
        let row = texts.iter().find(|(t, _)| t == "Acme VPN").map(|(_, r)| r.center()).expect("the row");
        let mut opened = None;
        let _ = ui_frame(400.0, click(row), |ui| {
            if let Some(action) = draw_received_group(ui, &mut people, 376.0, 62.0, &mut selected, RecordTiles::default()) {
                opened = Some(action);
            }
        });
        assert!(matches!(opened, Some(SendUiAction::OpenRecord(ref id)) if id == "r9"));
        assert_eq!(selected.as_deref(), Some("recv:r9"), "the row is picked as it opens");
    }

    /// **Nobody shares anything: nothing is drawn**, not even the eyebrow --
    /// a Shared with me that only ever imported Sends looks as it always did.
    #[test]
    fn no_received_records_draw_no_group() {
        use crate::vault_window::send_ui::RecordTiles;
        let mut people = PeopleRows::none();
        let texts = ui_frame(400.0, Vec::new(), |ui| {
            let _ = draw_received_group(ui, &mut people, 376.0, 62.0, &mut None, RecordTiles::default());
        });
        assert!(texts.is_empty(), "{texts:?}");
    }

    /// **The row's name is the VAULT item's**, never the view's: the view
    /// says only which item and who from. An entry the vault does not hold
    /// (a view a frame older than a sync) is left out rather than drawn
    /// nameless, and no view is no rows.
    #[test]
    fn received_rows_are_named_by_the_vault_and_need_their_record() {
        let mut v = SharingView { available: true, ..SharingView::default() };
        v.received.push(crate::rest::sharing::ReceivedEntry { item_id: "r9".into(), from: "denis@x.io".into() });
        v.received.push(crate::rest::sharing::ReceivedEntry { item_id: "gone".into(), from: "denis@x.io".into() });
        let items: Vec<VaultItem> = vec![serde_json::from_value(serde_json::json!({ "id": "r9", "name": "Acme VPN" })).expect("item")];
        assert_eq!(
            received_share_rows(Some(&v), &items),
            vec![ReceivedShareRow { key: "recv:r9".into(), item_id: "r9".into(), name: "Acme VPN".into(), from: "denis@x.io".into() }]
        );
        assert!(received_share_rows(None, &items).is_empty());
    }

    /// **Paste an invite link** (official-path spec §4.1): Accept is greyed
    /// and deaf while the box is empty or a write is out, and a press with a
    /// link in the box asks once.
    #[test]
    fn the_paste_row_asks_to_accept_the_link_it_holds() {
        let mut people = PeopleRows { paste: Some(Zeroizing::new(String::new())), ..PeopleRows::none() };
        let texts = ui_frame(400.0, Vec::new(), |ui| draw_paste_and_waiting(ui, &mut people, 376.0));
        let accept = texts.iter().find(|(t, _)| t == PASTE_ACCEPT_LABEL).map(|(_, r)| r.center()).expect("Accept");
        let _ = ui_frame(400.0, click(accept), |ui| draw_paste_and_waiting(ui, &mut people, 376.0));
        assert_eq!(people.asked, None, "an empty box asks nothing");

        people.paste = Some(Zeroizing::new("https://vault.example/#/accept-organization?token=t".to_string()));
        people.busy = true;
        let _ = ui_frame(400.0, click(accept), |ui| draw_paste_and_waiting(ui, &mut people, 376.0));
        assert_eq!(people.asked, None, "a write is out");
        people.busy = false;
        let _ = ui_frame(400.0, click(accept), |ui| draw_paste_and_waiting(ui, &mut people, 376.0));
        assert_eq!(people.asked, Some(PeopleAsk::AcceptLink));
    }

    /// **An accepted sharer who has not finished is one quiet line** -- and
    /// with no box and nobody waited on, nothing is drawn at all.
    #[test]
    fn a_sharer_being_waited_on_is_one_line_and_nothing_is_drawn_without_one() {
        let waiting = ["denis@x.io".to_string()];
        let mut people = PeopleRows { waiting: &waiting, ..PeopleRows::none() };
        let texts = ui_frame(400.0, Vec::new(), |ui| draw_paste_and_waiting(ui, &mut people, 376.0));
        assert_eq!(texts.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(), ["Waiting for denis@x.io to finish sharing"]);
        let mut people = PeopleRows::none();
        let texts = ui_frame(400.0, Vec::new(), |ui| draw_paste_and_waiting(ui, &mut people, 376.0));
        assert!(texts.is_empty(), "{texts:?}");
        assert!(paste_and_waiting_height(&PeopleRows::none(), 6.0).abs() < f32::EPSILON);
    }

    /// **"Stop receiving from X?"** names the sharer and what goes, and
    /// Escape keeps.
    #[test]
    fn the_leave_question_names_the_sharer_and_escape_keeps() {
        let ctx = egui::Context::default();
        let input = |events: Vec<egui::Event>| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0))),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input(Vec::new()), |_ui| {});
        theme::apply(&ctx);
        let mut answer = None;
        // An area is sized, unseen, on the frame it first appears: the
        // second frame is the one that paints it.
        let _ = ctx.run_ui(input(Vec::new()), |ui| answer = draw_leave_confirm(ui.ctx(), "denis@x.io"));
        let output = ctx.run_ui(input(Vec::new()), |ui| answer = draw_leave_confirm(ui.ctx(), "denis@x.io"));
        assert_eq!(answer, None, "undecided until a press");
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_texts(&clipped.shape, &mut texts);
        }
        for said in [stop_receiving_title("denis@x.io"), stop_receiving_body("denis@x.io"), STOP_RECEIVING_LABEL.to_string(), KEEP_RECEIVING_LABEL.to_string()] {
            assert!(texts.iter().any(|(t, _)| *t == said), "{said:?} is not on the question: {texts:?}");
        }
        let escape = vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }];
        let _ = ctx.run_ui(input(escape), |ui| answer = draw_leave_confirm(ui.ctx(), "denis@x.io"));
        assert_eq!(answer, Some(false));
    }

    /// **A pasted link's answer**: accepted, a toast saying the records are on
    /// their way; refused, the engine's sentence -- and the write slot is
    /// free either way. A panicked worker reports as the paste.
    #[test]
    fn a_pasted_links_answer_is_said_and_frees_the_slot() {
        let mut pane = SharingPane { busy: true, ..SharingPane::default() };
        apply(&mut pane, SharingReport::LinkAccepted(Ok(())), 0.0);
        assert_eq!(pane.toast.as_ref().map(|(t, _)| t.as_str()), Some(LINK_ACCEPTED));
        assert!(!pane.is_busy());
        pane.busy = true;
        apply(&mut pane, SharingCall::AcceptLink(Zeroizing::new("x".into())).failed(PANICKED), 1.0);
        assert_eq!(pane.notice.as_deref(), Some(PANICKED));
        assert!(!pane.is_busy());
    }

    /// **A finished Stop receiving says so, by name**; a failed one says
    /// the sentence instead, and a panicked worker reports as a leave.
    #[test]
    fn a_finished_leave_is_a_toast_naming_the_sharer() {
        let mut pane = SharingPane { busy: true, ..SharingPane::default() };
        apply(&mut pane, SharingReport::Left { from: "denis@x.io".into(), result: Ok(()) }, 0.0);
        assert_eq!(pane.toast.as_ref().map(|(t, _)| t.as_str()), Some("Stopped receiving from denis@x.io."));
        assert!(!pane.is_busy());
        let call = SharingCall::Leave { organization_id: "org-denis".into(), from: "denis@x.io".into() };
        pane.busy = true;
        apply(&mut pane, call.failed(PANICKED), 1.0);
        assert_eq!(pane.notice.as_deref(), Some(PANICKED));
        assert!(!pane.is_busy());
    }

    /// **The paste row's and the waiting lines' measured height counts the
    /// gaps BETWEEN its rows**: `n` rows, `n - 1` gaps -- the gap under them
    /// is the list's to add.
    #[test]
    fn the_paste_and_waiting_height_counts_the_gaps_between_its_rows() {
        let waiting = ["e@x.io".to_string()];
        let one = PeopleRows { paste: Some(Zeroizing::new(String::new())), ..PeopleRows::none() };
        assert!((paste_and_waiting_height(&one, 6.0) - PASTE_ROW_HEIGHT).abs() < f32::EPSILON);
        let two = PeopleRows { paste: Some(Zeroizing::new(String::new())), waiting: &waiting, ..PeopleRows::none() };
        assert!((paste_and_waiting_height(&two, 6.0) - (PASTE_ROW_HEIGHT + WAITING_LINE_HEIGHT + 6.0)).abs() < f32::EPSILON);
        assert!((PASTE_ROW_HEIGHT - theme::BUTTON_HEIGHT).abs() < f32::EPSILON, "one inline field high");
    }
}

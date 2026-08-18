//! Putting a secret on the Windows clipboard **without** it being retained.
//!
//! Every "Copy" in this app -- `CTRL+B`, `CTRL+U`, `CTRL+T`, the
//! `CTRL+SHIFT+` chords, the click-to-copy rows, the preflight's "copy
//! instead" -- ends here. It exists because the ordinary way to copy text on
//! Windows leaves two traces the user never asked for:
//!
//! 1. **Clipboard history (`Win+V`).** Windows keeps a stack of recent
//!    clipboard entries. A password copied an hour ago is still one keypress
//!    away, long after the value has left the *current* clipboard.
//! 2. **Cloud Clipboard.** If the user has clipboard sync on, that same
//!    password is replicated to their other signed-in devices.
//!
//! Neither is defeated by overwriting or emptying the clipboard later: by then
//! the copy has already been taken. The only thing that prevents it is telling
//! Windows, *in the same clipboard operation that carries the secret*, not to
//! take it -- which is what [`plan`] builds and [`win32::put_secret`]
//! executes.
//!
//! **This is unconditional and has no setting.** "Yes, please keep my
//! passwords in `Win+V` and copy them to my phone" is not a preference a
//! password manager should offer.
//!
//! ## The three formats, and what each one actually does
//!
//! These are registered clipboard formats whose *names* Windows knows; see
//! Microsoft's "Clipboard Formats" topic, section "Cloud Clipboard and
//! Clipboard History Formats". They are obtained with `RegisterClipboardFormat`
//! like any other registered format -- the name is the contract, the numeric
//! id is whatever the system hands back.
//!
//! * [`EXCLUDE_FROM_MONITOR_PROCESSING`] -- any data placed under this format
//!   excludes **all** formats in the clipboard operation from clipboard
//!   history *and* from syncing to other devices. This is the broad one, and
//!   on its own it is what Bitwarden's desktop client sets.
//! * [`CAN_INCLUDE_IN_CLIPBOARD_HISTORY`] -- a serialized `DWORD`; zero
//!   prevents inclusion in clipboard history. Does **not** affect syncing.
//! * [`CAN_UPLOAD_TO_CLOUD_CLIPBOARD`] -- a serialized `DWORD`; zero prevents
//!   syncing to the user's other devices. Does **not** affect local history.
//!
//! All three are written, not just the first. They overlap on purpose: the
//! broad exclusion is documented in terms of "monitor processing", a phrase
//! that has meant slightly different things across Windows releases, while the
//! two `DWORD`s name the two behaviours this app cares about in so many words.
//! Setting all three costs two extra four-byte allocations and removes the
//! question.
//!
//! ## What is *not* guaranteed
//!
//! A clipboard *monitor* -- any process that has registered for
//! `WM_CLIPBOARDUPDATE`, which includes every third-party clipboard manager --
//! can still read the text. These formats are a request to the Windows
//! shell's own history and sync, not an access control. `PRIVACY.md` says as
//! much: a copied secret is readable by other software on the machine for as
//! long as it is there, which is why the autofill path types instead of
//! copying, and why the second half of this module exists to shorten "as long
//! as it is there".

use std::fmt;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

/// Prevents **every** format in this clipboard operation from entering
/// clipboard history or syncing to other devices. Any data at all under this
/// format is enough; [`plan`] writes a zero `DWORD` so all four entries have
/// the same shape.
pub const EXCLUDE_FROM_MONITOR_PROCESSING: &str = "ExcludeClipboardContentFromMonitorProcessing";

/// A `DWORD` of zero here keeps the clipboard item out of `Win+V` history.
/// Says nothing about syncing -- that is
/// [`CAN_UPLOAD_TO_CLOUD_CLIPBOARD`]'s job.
pub const CAN_INCLUDE_IN_CLIPBOARD_HISTORY: &str = "CanIncludeInClipboardHistory";

/// A `DWORD` of zero here keeps the clipboard item off the user's other
/// devices. Says nothing about local history -- that is
/// [`CAN_INCLUDE_IN_CLIPBOARD_HISTORY`]'s job.
pub const CAN_UPLOAD_TO_CLOUD_CLIPBOARD: &str = "CanUploadToCloudClipboard";

/// Which clipboard format one [`ClipEntry`] is placed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipFormat {
    /// `CF_UNICODETEXT`: the payload every paste target actually reads.
    UnicodeText,
    /// A format looked up by name through `RegisterClipboardFormat`. The
    /// three suppression formats above are the only users.
    Registered(&'static str),
}

/// One format-plus-bytes pair to hand to `SetClipboardData`.
///
/// `Debug` is hand-written and prints the byte **count**, never the bytes:
/// the payload of the [`ClipFormat::UnicodeText`] entry is the secret itself,
/// and a derived `Debug` on a type that can reach a `Zeroizing` is exactly
/// what `debug_leak_guard` refuses.
pub struct ClipEntry {
    pub format: ClipFormat,
    /// Wiped when the entry is dropped. Note that the copy handed to
    /// `GlobalAlloc` and then to `SetClipboardData` cannot be: once
    /// `SetClipboardData` succeeds the system owns that block, and freeing or
    /// scribbling on it would be a use-after-free. Only this crate's own copy
    /// is under our control, and this is it.
    pub bytes: Zeroizing<Vec<u8>>,
}

impl fmt::Debug for ClipEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClipEntry")
            .field("format", &self.format)
            .field("bytes", &format_args!("<{} bytes withheld>", self.bytes.len()))
            .finish()
    }
}

/// Everything that goes on the clipboard for one "copy a secret", as data.
///
/// Pure, so the composition is testable: the exact question "does a copy of a
/// password ask Windows not to keep it?" is answered by inspecting the return
/// value of this function, with no clipboard involved. The Win32 half
/// ([`win32::put_secret`]) does nothing but walk this list.
///
/// The text entry is first because paste targets read formats in the order
/// they were placed; the three suppression entries are metadata and their
/// order among themselves does not matter.
#[must_use]
pub fn plan(text: &str) -> Vec<ClipEntry> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    // `CF_UNICODETEXT` is NUL-terminated. Without this a paste reads past the
    // block and gets the secret plus whatever followed it in memory.
    wide.push(0);
    let mut text_bytes = Vec::with_capacity(wide.len() * 2);
    for unit in &wide {
        text_bytes.extend_from_slice(&unit.to_le_bytes());
    }
    // The intermediate `Vec<u16>` held the secret too, and it is about to go
    // out of scope as an ordinary allocation. Wipe it by hand rather than
    // leaving a second copy behind the one we are careful about.
    wide.iter_mut().for_each(|unit| *unit = 0);

    vec![
        ClipEntry { format: ClipFormat::UnicodeText, bytes: Zeroizing::new(text_bytes) },
        deny(EXCLUDE_FROM_MONITOR_PROCESSING),
        deny(CAN_INCLUDE_IN_CLIPBOARD_HISTORY),
        deny(CAN_UPLOAD_TO_CLOUD_CLIPBOARD),
    ]
}

/// A serialized `DWORD` of zero: "no" to whichever behaviour `name` governs.
fn deny(name: &'static str) -> ClipEntry {
    ClipEntry {
        format: ClipFormat::Registered(name),
        bytes: Zeroizing::new(0u32.to_le_bytes().to_vec()),
    }
}

/// **The one way a secret gets onto the clipboard in this app.**
///
/// Every copy affordance calls this instead of `egui::Context::copy_text`.
/// That is not a style preference: `copy_text` hands the string to eframe's
/// own clipboard, which writes `CF_UNICODETEXT` and nothing else, so the
/// suppression formats would have to be bolted on in a *second* clipboard
/// operation -- and a second operation is a second clipboard item, which is
/// precisely the thing `Win+V` would then keep.
///
/// Failure is logged and swallowed. Another process can legitimately hold the
/// clipboard open for a moment, and the honest consequence of failing here is
/// that the user's paste does not work and they press the key again -- not
/// that the app should fall over. **The value is never logged**, only whether
/// it landed.
pub fn copy_secret(text: &str) {
    match win32::put_secret(text) {
        Ok(sequence) => arm(sequence, Instant::now()),
        Err(error) => log::warn!("clipboard: the copy did not land ({error})"),
    }
}

// ---------------------------------------------------------------------------
// Taking it back off again
// ---------------------------------------------------------------------------

/// How long a copied secret is left on the clipboard before it is taken back.
///
/// **45 seconds, and the number is an argument rather than a habit.**
///
/// The floor is set by what the user is actually doing: copy a password, alt-tab
/// to a browser, wait for a sign-in page that is still loading, click the field,
/// paste. Thirty seconds is survivable but not comfortable -- a page that is
/// slow, a 2FA step that appears first, or a user who is not looking at the
/// screen all eat it -- and a password manager whose clipboard expires before
/// the paste has trained the user to copy twice, which is worse than not
/// clearing at all.
///
/// The ceiling is set by what the delay is *for*: the realistic exposure is the
/// user walking away, or a program that reads the clipboard on a poll. Past a
/// minute or two the value of clearing is mostly gone, and "clear it eventually"
/// is not a security property anyone can rely on.
///
/// Bitwarden's own desktop default is **Never**, and that is deliberately not
/// copied: it is the subject of repeated requests to change it, and "never" is
/// a default that only looks safe because nothing visibly breaks.
///
/// **This is the weakest of the four things that end a copied secret's life**,
/// and it is listed last on purpose. Lock, account switch and app exit are
/// moments where the user has *said* they are done; the timer is a guess. The
/// suppression formats in [`plan`] matter more than either, because they stop
/// the copy being retained at all rather than racing to catch up with it.
pub const CLEAR_AFTER: Duration = Duration::from_secs(45);

/// What this app last put on the clipboard, as the two facts needed to decide
/// whether to take it back: *when* it may go, and *how to tell it is still
/// there*.
///
/// **The secret itself is deliberately not here.** The obvious way to answer
/// "is what is on the clipboard still ours?" is to keep a copy and compare --
/// which would mean holding the password in memory for the whole 45 seconds
/// precisely so it can be un-held. `GetClipboardSequenceNumber` answers the
/// same question with an integer: Windows bumps it on every clipboard write by
/// anybody, so an unchanged number means nothing has been copied since, and a
/// changed one means something has -- whether by another app, another program's
/// paste-and-copy, or this app copying a second secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Armed {
    /// The clipboard's sequence number immediately after our write.
    pub sequence: u32,
    /// The earliest instant the timer may clear it.
    pub due: Instant,
}

/// What to do about an armed secret, right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing of ours is on the clipboard; there is nothing to clear and
    /// nothing to wait for.
    NothingArmed,
    /// **Somebody else has copied something since.** Leave it completely
    /// alone. This is the case worth getting right: a user who copies a
    /// password, then copies a paragraph out of a document, then locks the
    /// vault must keep their paragraph. Wiping it would be a worse bug than
    /// the leak this module fixes, and it would be blamed on the text editor.
    NotOurs,
    /// Still ours, but the deadline has not arrived. Carries how long is left,
    /// so a waiter can sleep exactly that long instead of polling.
    TooEarly(Duration),
    /// Still ours, and it may go.
    Clear,
}

/// **The decision, as a pure function.** Every path that clears the clipboard
/// -- the timer, locking, switching account, app exit -- ends in this, and
/// none of them re-decides anything for itself.
///
/// `deadline` is what separates the timer from the rest: the timer passes
/// [`Deadline::Respected`], because 45 seconds is the whole point of it, and
/// lock/switch/exit pass [`Deadline::Waived`], because the user has said they
/// are finished and there is nothing left to wait for. Neither can waive the
/// [`Verdict::NotOurs`] check, which is why that check is above the deadline
/// and not beside it.
#[must_use]
pub fn verdict(
    armed: Option<Armed>,
    now: Instant,
    sequence_now: u32,
    deadline: Deadline,
) -> Verdict {
    let Some(armed) = armed else { return Verdict::NothingArmed };
    if armed.sequence != sequence_now {
        return Verdict::NotOurs;
    }
    match deadline {
        Deadline::Waived => Verdict::Clear,
        Deadline::Respected if now >= armed.due => Verdict::Clear,
        Deadline::Respected => Verdict::TooEarly(armed.due - now),
    }
}

/// Whether [`verdict`] is allowed to say "not yet".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deadline {
    /// The timer: wait out [`CLEAR_AFTER`].
    Respected,
    /// Lock, account switch, app exit: the user is done, so there is nothing
    /// to wait for.
    Waived,
}

/// The one piece of shared state. The Windows clipboard is per-session global,
/// so what this app last put on it is global too -- a copy made from the vault
/// window has to be clearable from the tray, from a lock, and from a thread
/// none of them own.
static ARMED: Mutex<Option<Armed>> = Mutex::new(None);

/// Record that a secret has just landed, and start the clock.
///
/// **Re-copying replaces the record outright**, so the previous copy's waiter
/// can no longer clear anything: it wakes, reads *this* `Armed`, and either
/// finds the new deadline in the future ([`Verdict::TooEarly`], sleep again)
/// or finds a third copy's sequence number ([`Verdict::NotOurs`], give up).
/// There is no way for the first timer to wipe the second value early, because
/// there is no per-copy timer state for it to hold.
fn arm(sequence: u32, now: Instant) {
    let due = now + CLEAR_AFTER;
    *lock_armed() = Some(Armed { sequence, due });
    std::thread::spawn(move || wait_and_clear(due));
}

/// Sleep until the deadline, then act on [`verdict`] -- re-reading the shared
/// record each time, which is what makes a later copy silently take this
/// waiter over rather than needing it cancelled.
fn wait_and_clear(mut until: Instant) {
    loop {
        let remaining = until.saturating_duration_since(Instant::now());
        if !remaining.is_zero() {
            std::thread::sleep(remaining);
        }
        match take_if(Deadline::Respected) {
            Verdict::TooEarly(left) => until = Instant::now() + left,
            _ => return,
        }
    }
}

/// **Clear the clipboard, if and only if what is on it is still ours.**
///
/// This is what lock, account switch and app exit call. It waives the 45-second
/// deadline and nothing else: a paragraph the user copied out of a document
/// thirty seconds ago is still theirs and is still left alone.
pub fn clear_if_still_ours() {
    let _ = take_if(Deadline::Waived);
}

/// [`verdict`] plus its consequence: clear when told to, and forget the record
/// unless the answer was "not yet".
fn take_if(deadline: Deadline) -> Verdict {
    let mut armed = lock_armed();
    let answer = verdict(*armed, Instant::now(), win32::sequence_number(), deadline);
    match answer {
        Verdict::Clear => {
            if let Err(error) = win32::clear() {
                log::warn!("clipboard: could not clear it ({error})");
            }
            *armed = None;
        }
        // Disarmed as well: what we put there is gone, so there is nothing
        // left for a later lock or exit to consider.
        Verdict::NotOurs | Verdict::NothingArmed => *armed = None,
        Verdict::TooEarly(_) => {}
    }
    answer
}

/// A poisoned mutex here means a thread panicked between reading and writing
/// two integers, which cannot leave them inconsistent -- and refusing to clear
/// the clipboard because of it would be the wrong failure.
fn lock_armed() -> std::sync::MutexGuard<'static, Option<Armed>> {
    ARMED.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The Win32 half: the one part of this module no test in this crate touches.
///
/// **Deliberately tiny, and deliberately dumb.** Everything that could be got
/// wrong by thinking -- which formats, what bytes, what order -- is in
/// [`plan`], which is pure and tested. What is left here is `OpenClipboard`,
/// a loop over the plan, and `CloseClipboard`. There is no branch in it that
/// a test could meaningfully take.
///
/// **No test may call any of this.** The clipboard is per-session global
/// state: a test that set it would clobber whatever the person running
/// `cargo test` had copied, and two tests running in parallel would fight over
/// it. So the honest statement is that the Win32 calls below are exercised by
/// running the app and nothing else -- see the module's own tests for what
/// *is* checked without a clipboard.
///
/// `Result` rather than a panic: failing to copy is a thing that happens
/// (another process can hold the clipboard open) and it is not worth taking
/// the app down for.
pub mod win32 {
    use super::{plan, ClipFormat};
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardSequenceNumber, OpenClipboard,
        RegisterClipboardFormatW, SetClipboardData,
    };
    use windows::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
    };

    /// Windows' own answer to "has the clipboard changed since I last looked?".
    ///
    /// It increments on every clipboard *write* by anyone, and it is what
    /// makes "only clear it if it is still ours" a comparison of two integers
    /// rather than a comparison of the clipboard's contents against a copy of
    /// the secret we would then have to keep lying around.
    #[must_use]
    pub fn sequence_number() -> u32 {
        // Safety: no arguments, no handles, no failure mode.
        unsafe { GetClipboardSequenceNumber() }
    }

    /// Put `text` on the clipboard along with the three formats that ask
    /// Windows not to retain it, and answer with the sequence number the
    /// clipboard has afterwards.
    ///
    /// The whole thing is one clipboard operation, between a single
    /// `OpenClipboard`/`CloseClipboard` pair, because that is the unit the
    /// suppression formats apply to.
    pub fn put_secret(text: &str) -> windows::core::Result<u32> {
        let entries = plan(text);
        // Safety: `HWND::default()` (NULL) associates the clipboard with the
        // current task rather than a window, which is what a process with no
        // one obvious owner window wants.
        unsafe { OpenClipboard(HWND::default()) }?;
        let outcome = (|| -> windows::core::Result<()> {
            // Required: SetClipboardData is only legal after the clipboard has
            // been emptied by the process that opened it.
            unsafe { EmptyClipboard() }?;
            for entry in &entries {
                let format = match entry.format {
                    ClipFormat::UnicodeText => {
                        windows::Win32::System::Ole::CF_UNICODETEXT.0.into()
                    }
                    // Registering a name Windows already knows returns that
                    // name's existing id; registering one it does not creates
                    // a private format, which is why the names are pinned by
                    // a test.
                    ClipFormat::Registered(name) => unsafe {
                        RegisterClipboardFormatW(&HSTRING::from(name))
                    },
                };
                if format == 0 {
                    return Err(windows::core::Error::from_win32());
                }
                let handle = global_copy(&entry.bytes)?;
                // Safety: `handle` is a GMEM_MOVEABLE block, unlocked, and is
                // handed over exactly once. On success the system owns it and
                // must not be freed here; on failure we still own it.
                match unsafe { SetClipboardData(format, HANDLE(handle.0)) } {
                    Ok(_) => {}
                    Err(error) => {
                        let _ = unsafe { GlobalFree(handle) };
                        return Err(error);
                    }
                }
            }
            Ok(())
        })();
        // Closed on both paths: leaving the clipboard open would hang every
        // other process on the machine that tries to copy.
        let _ = unsafe { CloseClipboard() };
        outcome?;
        Ok(sequence_number())
    }

    /// Empty the clipboard. The caller decides *whether* to -- see
    /// [`super::verdict`] -- because "is what is on the clipboard still ours?"
    /// is a decision, and this is not where decisions live.
    pub fn clear() -> windows::core::Result<()> {
        unsafe { OpenClipboard(HWND::default()) }?;
        let outcome = unsafe { EmptyClipboard() };
        let _ = unsafe { CloseClipboard() };
        outcome
    }

    /// Copy `bytes` into a movable global block of the kind
    /// `SetClipboardData` requires. Ownership passes to the caller.
    fn global_copy(bytes: &[u8]) -> windows::core::Result<HGLOBAL> {
        // Safety: a non-zero size; the returned handle is checked by `?`.
        let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) }?;
        // Safety: `handle` was just allocated and is not locked.
        let target = unsafe { GlobalLock(handle) };
        if target.is_null() {
            let error = windows::core::Error::from_win32();
            let _ = unsafe { GlobalFree(handle) };
            return Err(error);
        }
        // Safety: `target` points at `bytes.len().max(1)` writable bytes and
        // cannot overlap `bytes`, which is a Rust allocation.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), target.cast::<u8>(), bytes.len()) };
        // Returns Err with ERROR_SUCCESS when the lock count reaches zero,
        // which is the normal case here, so the result is deliberately
        // dropped rather than propagated.
        let _ = unsafe { GlobalUnlock(handle) };
        Ok(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The whole point of the module, asserted on the plan.**
    ///
    /// Both directions: all three suppression formats are present *and* each
    /// carries the value that means "no", rather than merely being named.
    #[test]
    fn a_copied_secret_asks_windows_not_to_retain_it() {
        let entries = plan("hunter2");
        let named: Vec<&str> = entries
            .iter()
            .filter_map(|e| match e.format {
                ClipFormat::Registered(name) => Some(name),
                ClipFormat::UnicodeText => None,
            })
            .collect();
        assert_eq!(
            named,
            vec![
                EXCLUDE_FROM_MONITOR_PROCESSING,
                CAN_INCLUDE_IN_CLIPBOARD_HISTORY,
                CAN_UPLOAD_TO_CLOUD_CLIPBOARD,
            ],
            "a secret went on the clipboard without all three suppression formats, so it is \
             one Win+V away for as long as the history keeps it"
        );
        for entry in &entries {
            if let ClipFormat::Registered(name) = entry.format {
                assert_eq!(
                    entry.bytes.as_slice(),
                    &0u32.to_le_bytes(),
                    "`{name}` was written with something other than a zero DWORD; a one is an \
                     explicit REQUEST to retain, which is the opposite of what this does"
                );
            }
        }
    }

    /// **The format names are the contract**, verbatim. `RegisterClipboardFormat`
    /// happily registers a typo as a brand new private format that Windows has
    /// never heard of: the call succeeds, `SetClipboardData` succeeds, nothing
    /// logs, and the secret is retained anyway. Nothing but spelling these out
    /// catches that.
    #[test]
    fn the_format_names_are_exactly_the_ones_windows_knows() {
        assert_eq!(
            EXCLUDE_FROM_MONITOR_PROCESSING,
            "ExcludeClipboardContentFromMonitorProcessing"
        );
        assert_eq!(CAN_INCLUDE_IN_CLIPBOARD_HISTORY, "CanIncludeInClipboardHistory");
        assert_eq!(CAN_UPLOAD_TO_CLOUD_CLIPBOARD, "CanUploadToCloudClipboard");
    }

    /// **The text still arrives**, NUL-terminated and unaltered. A suppression
    /// plan that mangled the payload would be a far worse bug than the one
    /// this module fixes -- and the terminator is not decoration: `CF_UNICODETEXT`
    /// has no length prefix, so a missing NUL is a read past the block.
    #[test]
    fn the_unicode_text_entry_round_trips_the_secret_and_is_nul_terminated() {
        // Non-ASCII on purpose: a password is bytes, not letters, and a plan
        // that quietly went through `as u8` would pass on "hunter2".
        const SECRET: &str = "pä§§w0rd\u{1F511}";
        let entries = plan(SECRET);
        let text = entries
            .iter()
            .find(|e| e.format == ClipFormat::UnicodeText)
            .expect("the plan carries no text at all, so the copy would paste nothing");
        assert_eq!(
            entries.first().map(|e| e.format),
            Some(ClipFormat::UnicodeText),
            "the text is not the first format on the clipboard"
        );
        let units: Vec<u16> = text
            .bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(units.last(), Some(&0), "CF_UNICODETEXT payload is not NUL-terminated");
        assert_eq!(
            String::from_utf16(&units[..units.len() - 1]).expect("not valid UTF-16"),
            SECRET,
            "the value that reaches the clipboard is not the value that was copied"
        );
        // Control: the round-trip above would also pass on a plan that
        // ignored its argument and always emitted `SECRET`.
        let other = plan("something else entirely");
        let other_text =
            other.iter().find(|e| e.format == ClipFormat::UnicodeText).expect("no text entry");
        assert_ne!(
            other_text.bytes.as_slice(),
            text.bytes.as_slice(),
            "two different secrets produced the same clipboard payload"
        );
    }

    /// **An empty copy is still a copy**, and must still be suppressed. The
    /// interesting half is that the text entry is not empty either -- it is
    /// the lone NUL, because a zero-byte `CF_UNICODETEXT` block is what a
    /// paste reads past.
    #[test]
    fn an_empty_secret_still_gets_the_terminator_and_the_suppression() {
        let entries = plan("");
        assert_eq!(entries.len(), 4, "an empty copy skipped part of the plan: {entries:?}");
        assert_eq!(
            entries[0].bytes.as_slice(),
            &[0, 0],
            "the empty text entry is not a lone NUL: {:?}",
            entries[0]
        );
    }

    // ------------------------------------------------------------------
    // The decision to clear
    //
    // `verdict` is the whole of it, and it is pure, so every case below is
    // constructed rather than staged. **Nothing here opens the real
    // clipboard**: `win32::sequence_number` is passed in as an argument
    // precisely so these can run on a machine whose clipboard belongs to the
    // person running `cargo test`.
    // ------------------------------------------------------------------

    /// Two `Armed`s from one clock, so "our value is still there" and
    /// "something else was copied" differ in exactly the one field under test.
    fn armed_at(now: Instant, sequence: u32) -> Armed {
        Armed { sequence, due: now + CLEAR_AFTER }
    }

    /// **The property that makes this correct rather than merely present**,
    /// in both directions, off one clock.
    #[test]
    fn the_timer_clears_our_own_value_and_leaves_anything_else_alone() {
        let now = Instant::now();
        let ours = armed_at(now, 7);
        let fired = now + CLEAR_AFTER;

        assert_eq!(
            verdict(Some(ours), fired, 7, Deadline::Respected),
            Verdict::Clear,
            "our own copy was still on the clipboard when the timer fired and was left there"
        );
        // The live control: the SAME arming, the SAME instant, and the only
        // thing that changed is that the clipboard moved on -- which is what
        // a user copying a paragraph out of a document looks like from here.
        assert_eq!(
            verdict(Some(ours), fired, 8, Deadline::Respected),
            Verdict::NotOurs,
            "the timer wiped something the user copied from somewhere else"
        );
        // ...and the fixture really did change, rather than the two calls
        // differing for some reason the assertions cannot see.
        assert_ne!(ours.sequence, 8, "control: the two sequence numbers are the same");
    }

    /// **Early is not "clear anyway", it is "wait exactly this long".**
    /// `TooEarly` carrying the remaining time is what lets the waiter sleep
    /// rather than poll, and a re-copy move the deadline out from under it.
    #[test]
    fn before_the_deadline_the_answer_is_how_long_is_left() {
        let now = Instant::now();
        let ours = armed_at(now, 7);
        assert_eq!(
            verdict(Some(ours), now, 7, Deadline::Respected),
            Verdict::TooEarly(CLEAR_AFTER),
            "the whole interval was not left at the instant of the copy"
        );
        assert_eq!(
            verdict(Some(ours), now + Duration::from_secs(40), 7, Deadline::Respected),
            Verdict::TooEarly(Duration::from_secs(5)),
        );
        // One nanosecond either side of the deadline, so "clears at the
        // deadline" is a boundary rather than an approximation.
        let a_moment = Duration::from_nanos(1);
        assert_eq!(
            verdict(Some(ours), ours.due - a_moment, 7, Deadline::Respected),
            Verdict::TooEarly(a_moment),
        );
        assert_eq!(verdict(Some(ours), ours.due, 7, Deadline::Respected), Verdict::Clear);
    }

    /// **Locking waives the clock and nothing else.** Lock, account switch and
    /// app exit are moments where the user has said they are done, so waiting
    /// out the remaining 45 seconds would be silly -- but the value on the
    /// clipboard still has to be ours.
    #[test]
    fn locking_clears_immediately_but_still_only_what_is_ours() {
        let now = Instant::now();
        let ours = armed_at(now, 7);
        assert_eq!(
            verdict(Some(ours), now, 7, Deadline::Waived),
            Verdict::Clear,
            "locking the vault left the password it had just copied on the clipboard"
        );
        // The control that matters, and the reason `NotOurs` is checked
        // ABOVE the deadline and not beside it: waiving the wait must not
        // waive the ownership test.
        assert_eq!(
            verdict(Some(ours), now, 8, Deadline::Waived),
            Verdict::NotOurs,
            "locking the vault wiped a paragraph the user had copied from a document"
        );
        // ...and the same two arguments under `Respected` say something
        // different, so `Waived` is doing work rather than being ignored.
        assert_eq!(
            verdict(Some(ours), now, 7, Deadline::Respected),
            Verdict::TooEarly(CLEAR_AFTER),
            "control: `Waived` and `Respected` are the same answer, so the flag is dead"
        );
    }

    /// **Nothing armed is not the same as "not ours"**, and neither is a
    /// clear. If this app has copied nothing, a lock must not empty a
    /// clipboard it never touched.
    #[test]
    fn with_nothing_copied_there_is_nothing_to_clear_under_either_deadline() {
        let now = Instant::now();
        for deadline in [Deadline::Respected, Deadline::Waived] {
            assert_eq!(
                verdict(None, now, 7, deadline),
                Verdict::NothingArmed,
                "with nothing of ours copied, {deadline:?} still decided something"
            );
        }
        // Control: the same call with an arming present is not
        // `NothingArmed`, so the assertion above is about the `None` and not
        // about every input.
        assert_ne!(verdict(Some(armed_at(now, 7)), now, 7, Deadline::Waived), Verdict::NothingArmed);
    }

    /// **Re-copying restarts the clock, and the first copy's waiter cannot
    /// wipe the second value early.**
    ///
    /// The mechanism is that there is no per-copy timer state at all: a waiter
    /// re-reads the one shared `Armed`, so a second copy simply moves the
    /// deadline it will be judged against. Modelled here on the pure function,
    /// which is where the behaviour actually lives.
    #[test]
    fn a_second_copy_moves_the_deadline_the_first_waiter_will_be_judged_against() {
        let first_copy = Instant::now();
        let second_copy = first_copy + Duration::from_secs(30);
        // The first copy's waiter wakes 45s after the FIRST copy...
        let first_waiter_wakes = first_copy + CLEAR_AFTER;
        // ...but by then the record describes the second copy.
        let after_recopy = Armed { sequence: 9, due: second_copy + CLEAR_AFTER };
        assert_eq!(
            verdict(Some(after_recopy), first_waiter_wakes, 9, Deadline::Respected),
            Verdict::TooEarly(Duration::from_secs(30)),
            "the first copy's timer cleared the second copy's value 30 seconds early"
        );
        // And the second value does get cleared, at its own deadline -- the
        // re-copy moved the clock rather than switching the timer off.
        assert_eq!(
            verdict(Some(after_recopy), after_recopy.due, 9, Deadline::Respected),
            Verdict::Clear,
            "the re-copied value is never cleared at all"
        );
    }

    /// **`CLEAR_AFTER` is inside the range this was argued in**, so moving it
    /// to something indefensible is a decision someone has to take
    /// deliberately rather than a constant that drifted.
    #[test]
    fn the_interval_is_long_enough_to_paste_and_short_enough_to_matter() {
        assert!(
            CLEAR_AFTER >= Duration::from_secs(30),
            "under 30s the clipboard expires before a slow sign-in form is ready, which \
             trains the user to copy twice"
        );
        assert!(
            CLEAR_AFTER <= Duration::from_secs(90),
            "past 90s the delay has stopped being a control and is just a pause"
        );
    }

    /// **`Debug` must not print the secret.** `debug_leak_guard` refuses a
    /// *derived* `Debug` on a type that can reach a `Zeroizing`; this type
    /// hand-writes one instead of taking an exemption, so the thing worth
    /// asserting is what the hand-written one actually renders.
    #[test]
    fn the_debug_impl_withholds_the_bytes() {
        const SECRET: &str = "correct-horse-battery-staple";
        let entries = plan(SECRET);
        let rendered = format!("{entries:?}");
        assert!(
            !rendered.contains("correct-horse"),
            "the plan's Debug printed the secret in the clear: {rendered}"
        );
        // ...and it did not do that by rendering nothing at all.
        assert!(
            rendered.contains("UnicodeText") && rendered.contains("bytes withheld"),
            "control: the Debug impl says nothing useful either: {rendered}"
        );
        assert!(
            rendered.contains(EXCLUDE_FROM_MONITOR_PROCESSING),
            "control: format names are withheld too, so a log could not show which formats \
             were set: {rendered}"
        );
    }
}

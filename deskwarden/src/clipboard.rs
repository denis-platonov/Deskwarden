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
//! shell's own history and sync, not an access control. `PRIVACY.md` already
//! says a copied secret is readable by other software on the machine, and that
//! remains true.

use std::fmt;

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
        Ok(_) => {}
        Err(error) => log::warn!("clipboard: the copy did not land ({error})"),
    }
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

//! What to call an executable, and what to show beside its name.
//!
//! The user's words: *"show app icon and name as per application (not `*.exe`)
//! but the full path"*. `chrome.exe` is the string the match engine compares
//! and the string a launcher needs; it is not what anyone calls the program.
//! Windows already knows the answer -- every signed executable carries a
//! `VS_VERSIONINFO` resource whose `FileDescription` is exactly the name
//! Explorer, the task manager and the Start menu all show -- and the shell
//! knows its icon.
//!
//! # Why this is a module and not four lines in the edit form
//!
//! Both lookups are **I/O against a path out of a vault field**, on a thread
//! that is drawing a window. Three rules follow, and this module exists to be
//! the one place they are kept:
//!
//!  * **Resolved once, then cached.** An edit form re-runs its body on every
//!    frame; a version-info read per frame is a file open per frame.
//!  * **Never blocking the UI thread on a path that might be slow.** A drive
//!    letter is not proof of a local disk (`AppMatch::launchable_path`'s own
//!    doc says so: `subst` and a mapped network drive both produce `X:\`), and
//!    an unreachable network path can stall a file open for the SMB timeout --
//!    seconds, with the window frozen. So the probe runs on a **worker
//!    thread**, the form paints the file name until it answers, and the answer
//!    replaces it whenever it arrives.
//!  * **Failure is silent and total.** A missing, unreadable, resource-less or
//!    icon-less file degrades to the file name with no icon. There is no error
//!    to show: the user can see the path, and the path row is editable.
//!
//! The icon is the one call made on the UI thread, and only **after** the
//! worker has proved the file answers at all (see [`Probe::reachable`]). It is
//! deliberately not done on the worker: `SHGetFileInfoW` is a shell call whose
//! result is a GDI handle, and the existing [`crate::icon`] extractor is
//! written to be called from the thread that owns the window it draws into.

use crate::icon;
use eframe::egui;
use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};

/// The file name at the end of a Windows path, or `None` when there isn't one.
///
/// Both separators, because Windows accepts both -- the same reason
/// [`crate::app_match::AppMatch::launchable_path`] normalises before it splits.
/// A path ending in a separator names a directory and has no file name; so does
/// the empty string.
///
/// **This is also how the edit form derives `AppMatch::process` from a path the
/// user chose**, which is what keeps `launchable_path`'s file-name tie-back
/// satisfiable by construction. Public, and here rather than in the form, so
/// the two cannot answer differently.
pub fn file_name_of(path: &str) -> Option<&str> {
    let name = match path.rfind(['\\', '/']) {
        Some(cut) => &path[cut + 1..],
        None => path,
    };
    (!name.is_empty()).then_some(name)
}

/// What to call the app, given whatever the executable's version resource
/// yielded.
///
/// The order is `FileDescription`, then `ProductName`, then the path's file
/// name, then the matched `process` -- and it is the order Explorer's own
/// "Description" column uses, not an invention. `FileDescription` is the
/// per-binary name (`Google Chrome`), `ProductName` is the suite it belongs to
/// (`Google Chrome` too, usually, but `Microsoft Office` for a dozen different
/// binaries), so the specific one is preferred and the general one is the
/// fallback.
///
/// **A blank or whitespace-only resource string counts as absent.** Some
/// installers ship `FileDescription` as a single space; taking it would render
/// an app with no name at all and no way to tell why.
///
/// The last two steps cannot both fail: `process` is non-empty for every match
/// this app has ever written, and if it somehow were, the caller gets an empty
/// string and shows an empty name -- which is still not a panic and still not
/// a lie.
pub fn display_name(
    description: Option<&str>,
    product: Option<&str>,
    path: &str,
    process: &str,
) -> String {
    let usable = |s: Option<&str>| {
        s.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
    };
    usable(description)
        .or_else(|| usable(product))
        .or_else(|| file_name_of(path).map(str::to_string))
        .unwrap_or_else(|| process.to_string())
}

/// What one worker thread found out about one path.
pub struct Probe {
    /// `FileDescription` from the executable's version resource.
    description: Option<String>,
    /// `ProductName` from the same place.
    product: Option<String>,
    /// Whether the path could be stat'd **and named a file**.
    ///
    /// **This is the gate on the icon lookup, and that is its whole purpose.**
    /// The worker has just paid whatever the volume costs to answer; if it
    /// answered, the UI thread may make one shell call for the icon without
    /// risking the stall this module exists to avoid. If it did not -- the
    /// path is gone, the share is down, the `subst` target is unmounted -- the
    /// UI thread makes no call at all and the row simply has no icon.
    ///
    /// A **directory** is excluded as deliberately as an unreachable path is:
    /// `SHGetFileInfoW` answers for one perfectly well, with the shell's folder
    /// icon, and a folder icon beside the app's name is a picture of something
    /// that is not the app.
    file: bool,
    /// Whether the path named a directory. See [`AppIdentityCache::label`]:
    /// a directory's own last component is not the app's name either, so a
    /// path that resolves to one is labelled by `process` instead.
    directory: bool,
}

enum Entry {
    /// A worker is out. `placeholder` is what the form paints meanwhile -- the
    /// path's own file name, which is what the app was called before this
    /// module existed. It is stored rather than re-derived on each frame
    /// because the label is handed out as a borrow of the cache, and a borrow
    /// of the caller's `path` argument would not outlive the call.
    Pending {
        rx: mpsc::Receiver<Probe>,
        placeholder: String,
    },
    Ready {
        name: String,
        /// `None` means "there is no icon for this file", which includes "the
        /// file could not be reached". Never retried.
        icon: Option<egui::TextureHandle>,
    },
}

/// What the form gets back for one path: a name it can always paint, and an
/// icon it can paint if there is one.
pub struct AppLabel<'a> {
    pub name: &'a str,
    pub icon: Option<&'a egui::TextureHandle>,
    /// Whether the worker is still out, so the caller can keep the frames
    /// coming until it answers (egui repaints on input, and a channel is not
    /// input).
    pub pending: bool,
}

/// One resolved name and icon per executable path, for as long as the window
/// that owns this cache is open.
///
/// Held by the caller (the vault window's frame closure) rather than by the
/// draft, for the reason `EditDraft`'s own fields are all plain data: the
/// draft is `Clone`, and a channel and a GPU texture are neither cloneable nor
/// the form's state -- they are a cache of facts about the machine, which
/// survives switching between items and is thrown away with the window.
#[derive(Default)]
pub struct AppIdentityCache {
    entries: HashMap<String, Entry>,
    /// The path that has been asked for but not yet probed, and since when.
    ///
    /// **The whole of the debounce, and the reason it is one slot and not a
    /// second map.** The form asks about exactly one path per frame, and a
    /// path being typed is a path that changes between frames; a slot that
    /// holds the latest one and its arrival time is enough to answer "has this
    /// stopped moving?" and costs one `String` for the window's lifetime. A
    /// map would remember every prefix, which is the leak this exists to
    /// prevent.
    settling: Option<Settling>,
}

struct Settling {
    path: String,
    since: Instant,
    /// The path's file name, stored for the same reason `Entry::Pending` stores
    /// one: the label is handed out as a borrow of the cache.
    placeholder: String,
}

impl AppIdentityCache {
    /// The name and icon for `path`, starting the lookup if this is the first
    /// time it has been asked for.
    ///
    /// `process` is the match's own executable name, used as the last-resort
    /// label. An empty `path` is answered from `process` alone with no lookup
    /// and no thread -- which is the Microsoft Store case, where there is no
    /// image path to read anything out of.
    ///
    /// # A path is not probed the instant it is first seen
    ///
    /// The edit form's Program file box rewrites `path` on **every keystroke**,
    /// and this function used to start a lookup for every path it had not seen
    /// before. Typing a 53-character path therefore spawned 53 threads, made 53
    /// `fs::metadata` calls, and left 53 map entries that nothing ever removed
    /// -- measured, not supposed, by
    /// `typing_a_path_one_character_at_a_time_starts_no_probe`. Worse, a prefix
    /// that happens to name a directory (`C:\Windows`) is stat-able, so it also
    /// took the UI thread's `SHGetFileInfoW` call and a GPU texture: unbounded
    /// churn on the one shell call this module exists to ration.
    ///
    /// So a path must hold still for [`Self::SETTLE`] before it is probed.
    /// **Debounce rather than a "does this look like a file name?" test**,
    /// because every prefix of `chrome.exe` from `chrome.e` on looks exactly
    /// like one -- guessing at the shape of the string cannot tell a finished
    /// path from an unfinished one, and only time can. A path chosen through
    /// Browse... or the running-app picker is not delayed in any way the user
    /// can perceive: it arrives once and then holds still, and the answer is
    /// already only ever painted on the next [`Self::POLL_INTERVAL`] repaint,
    /// which is the same 150ms.
    ///
    /// Nothing is evicted. It does not need to be any more: the map now grows
    /// once per path the user actually settles on, not once per keystroke, and
    /// dropping a `Ready` entry would only make the next frame that asks for it
    /// spawn the probe again -- per-frame I/O, which is the thing this module
    /// was written to prevent.
    pub fn label<'a>(
        &'a mut self,
        ctx: &egui::Context,
        path: &str,
        process: &'a str,
    ) -> AppLabel<'a> {
        if path.is_empty() {
            return AppLabel { name: process, icon: None, pending: false };
        }

        // Two statements rather than an `entry().or_insert_with()`: the closure
        // would have to capture `ctx` while the map is already mutably
        // borrowed, and spelling the insert out keeps the "spawn exactly once"
        // rule visible.
        if !self.entries.contains_key(path) {
            // A path with no file name (`C:\Program Files\`) names a directory
            // by construction. There is no executable there to read a name out
            // of, `set_path` already refuses to derive `process` from it, and
            // it is what half a typed path looks like -- so no thread, no
            // entry, and no settle either.
            if let Some(name) = file_name_of(path) {
                if self.settling.as_ref().map(|s| s.path.as_str()) != Some(path) {
                    self.settling = Some(Settling {
                        path: path.to_string(),
                        since: Instant::now(),
                        placeholder: name.to_string(),
                    });
                }
                let settled = self
                    .settling
                    .as_ref()
                    .is_some_and(|s| s.since.elapsed() >= Self::SETTLE);
                if settled {
                    let placeholder = match self.settling.take() {
                        Some(s) => s.placeholder,
                        None => name.to_string(),
                    };
                    self.entries.insert(
                        path.to_string(),
                        Entry::Pending { rx: spawn_probe(path), placeholder },
                    );
                }
            }
        }

        // A pending entry that has answered becomes ready HERE, on the UI
        // thread, which is also the only place the icon may be fetched.
        if let Some(Entry::Pending { rx, .. }) = self.entries.get(path) {
            if let Ok(probe) = rx.try_recv() {
                let name = if probe.directory {
                    // The path names a FOLDER. Its last component is the
                    // folder's name, and painting "WINDOWS" where the app's
                    // name goes says something false about the binding; the
                    // executable this match is keyed on is the honest answer,
                    // and it is the one the user can check against the path in
                    // the box directly below.
                    process.to_string()
                } else {
                    display_name(
                        probe.description.as_deref(),
                        probe.product.as_deref(),
                        path,
                        process,
                    )
                };
                // Gated on the path having named a reachable FILE, and only
                // ever attempted once: a `None` here is remembered as "no
                // icon", never retried per frame.
                let icon = probe.file.then(|| load_icon(ctx, path)).flatten();
                self.entries.insert(path.to_string(), Entry::Ready { name, icon });
            }
        }

        match self.entries.get(path) {
            Some(Entry::Ready { name, icon }) => {
                AppLabel { name, icon: icon.as_ref(), pending: false }
            }
            // Still out. The file name is the honest placeholder: it is what
            // the app was called before this feature existed, and it does not
            // flicker through a spinner for the millisecond a local file takes.
            Some(Entry::Pending { placeholder, .. }) => {
                AppLabel { name: placeholder, icon: None, pending: true }
            }
            // Not probed yet: either still settling -- in which case the
            // placeholder is the same file name a `Pending` would show, so the
            // debounce is invisible -- or a path with no file name to probe.
            None => match &self.settling {
                Some(s) if s.path == path => {
                    AppLabel { name: &s.placeholder, icon: None, pending: true }
                }
                _ => AppLabel { name: process, icon: None, pending: false },
            },
        }
    }

    /// How long a caller should wait before drawing again while a lookup is
    /// out. A named constant rather than a number at the call site so the
    /// polling cadence is stated once; the picker's readiness probe uses the
    /// same 150ms.
    pub const POLL_INTERVAL: Duration = Duration::from_millis(150);

    /// How long a path must hold still before it is probed. See
    /// [`Self::label`]. One [`Self::POLL_INTERVAL`], deliberately: the answer
    /// cannot reach the screen faster than that repaint anyway, so a settle of
    /// the same length costs a Browse... choice nothing anyone can see, while
    /// being far longer than the gap between two keystrokes.
    pub const SETTLE: Duration = Self::POLL_INTERVAL;

    /// Pretend a probe already ran for `path` and answered `name` (and
    /// `icon`). **Test-only, and the only way a UI test can exercise a
    /// resolved name at all**: every real answer comes off a `VS_VERSIONINFO`
    /// resource on a file that exists on the machine running the test, so a
    /// test that wanted "this app is called Ledgerline Accounting Suite" would
    /// otherwise have to ship a signed binary.
    ///
    /// It writes the same `Entry::Ready` a real probe produces, through the
    /// same map [`Self::label`] reads, so a caller that stopped asking this
    /// cache -- or asked it about the wrong string -- still fails: the seeded
    /// name simply never reaches the screen.
    #[cfg(test)]
    pub fn seed_ready(&mut self, path: &str, name: &str, icon: Option<egui::TextureHandle>) {
        self.entries
            .insert(path.to_string(), Entry::Ready { name: name.to_string(), icon });
    }

    /// How many paths have been probed. Test-only: the leak this module's
    /// debounce exists to prevent is a *count*, and counting is the only way to
    /// see it.
    #[cfg(test)]
    fn probed(&self) -> usize {
        self.entries.len()
    }
}

/// Reads `path`'s version resource on a worker thread.
///
/// The channel is bounded at one and the send is allowed to fail: if the window
/// closed while the worker was out, the receiver is gone and there is nobody to
/// tell. Nothing is retried and nothing is joined -- the worker touches no
/// shared state and ends when its one file read does.
fn spawn_probe(path: &str) -> mpsc::Receiver<Probe> {
    let (tx, rx) = mpsc::sync_channel(1);
    let owned = path.to_string();
    std::thread::spawn(move || {
        let meta = std::fs::metadata(&owned);
        let file = meta.as_ref().map(|m| m.is_file()).unwrap_or(false);
        let directory = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let (description, product) = if file { version_names(&owned) } else { (None, None) };
        let _ = tx.send(Probe { description, product, file, directory });
    });
    rx
}

fn load_icon(ctx: &egui::Context, path: &str) -> Option<egui::TextureHandle> {
    let rgba = icon::extract_small_icon(path)?;
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [rgba.width as usize, rgba.height as usize],
        &rgba.rgba,
    );
    Some(ctx.load_texture(format!("app-identity:{path}"), image, egui::TextureOptions::default()))
}

/// `(FileDescription, ProductName)` out of `path`'s `VS_VERSIONINFO` resource,
/// or `(None, None)` for a file that has none, cannot be read, or whose
/// resource is malformed.
///
/// **Worker-thread only.** This is the call that can stall on a slow volume;
/// see the module doc.
fn version_names(path: &str) -> (Option<String>, Option<String>) {
    unsafe {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let size = GetFileVersionInfoSizeW(PCWSTR(wide.as_ptr()), None);
        if size == 0 {
            return (None, None);
        }
        let mut block = vec![0u8; size as usize];
        if GetFileVersionInfoW(
            PCWSTR(wide.as_ptr()),
            0,
            size,
            block.as_mut_ptr() as *mut core::ffi::c_void,
        )
        .is_err()
        {
            return (None, None);
        }

        // The resource's strings live under a language/codepage key, and a
        // binary may carry several. `\VarFileInfo\Translation` lists them; the
        // two hardcoded fallbacks are US-English/Unicode and US-English/Latin-1,
        // which is what a binary with a malformed or absent Translation table
        // in practice uses.
        let mut keys: Vec<String> = Vec::new();
        let mut buffer: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len: u32 = 0;
        let translation: Vec<u16> = r"\VarFileInfo\Translation"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        if VerQueryValueW(
            block.as_ptr() as *const core::ffi::c_void,
            PCWSTR(translation.as_ptr()),
            &mut buffer,
            &mut len,
        )
        .as_bool()
            && !buffer.is_null()
        {
            // Pairs of u16: language id, then codepage.
            let pairs = (len as usize) / 4;
            let words = std::slice::from_raw_parts(buffer as *const u16, pairs * 2);
            for pair in words.chunks_exact(2) {
                keys.push(format!("{:04x}{:04x}", pair[0], pair[1]));
            }
        }
        keys.push("040904b0".to_string());
        keys.push("040904e4".to_string());

        let mut description = None;
        let mut product = None;
        for key in &keys {
            if description.is_none() {
                description = version_string(&block, key, "FileDescription");
            }
            if product.is_none() {
                product = version_string(&block, key, "ProductName");
            }
            if description.is_some() && product.is_some() {
                break;
            }
        }
        (description, product)
    }
}

/// One `\StringFileInfo\<key>\<name>` value out of an already-loaded version
/// block.
///
/// # Safety
///
/// `block` must be a buffer filled by `GetFileVersionInfoW`.
unsafe fn version_string(block: &[u8], key: &str, name: &str) -> Option<String> {
    let sub: Vec<u16> = format!(r"\StringFileInfo\{key}\{name}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut buffer: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut len: u32 = 0;
    let found = VerQueryValueW(
        block.as_ptr() as *const core::ffi::c_void,
        PCWSTR(sub.as_ptr()),
        &mut buffer,
        &mut len,
    );
    if !found.as_bool() || buffer.is_null() || len == 0 {
        return None;
    }
    // `len` counts characters INCLUDING the terminating NUL, which must not
    // become part of the string.
    let chars = std::slice::from_raw_parts(buffer as *const u16, len as usize);
    let chars = match chars.iter().position(|&c| c == 0) {
        Some(nul) => &chars[..nul],
        None => chars,
    };
    let value = String::from_utf16_lossy(chars);
    (!value.trim().is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_name_is_the_last_component_under_either_separator() {
        assert_eq!(file_name_of(r"C:\Program Files\Google\Chrome\chrome.exe"), Some("chrome.exe"));
        assert_eq!(file_name_of("C:/Program Files/Google/chrome.exe"), Some("chrome.exe"));
        assert_eq!(file_name_of(r"C:\Apps/Mixed\Ledgerline.exe"), Some("Ledgerline.exe"));
        // No separator at all: the whole string is the name.
        assert_eq!(file_name_of("chrome.exe"), Some("chrome.exe"));
    }

    #[test]
    fn a_path_that_names_no_file_has_no_file_name() {
        // The positive control for the test above is that test itself: these
        // must NOT come back as `Some("")`, which is what a naive split gives
        // and which would then be written into `AppMatch::process`.
        assert_eq!(file_name_of(r"C:\Program Files\"), None);
        assert_eq!(file_name_of("C:/"), None);
        assert_eq!(file_name_of(""), None);
    }

    #[test]
    fn the_description_is_what_the_app_is_called() {
        // The two resource strings are DIFFERENT on purpose. They agree on a
        // real Chrome install, and a fixture that let them agree is a fixture
        // that re-derives its expectation from the thing under test: with
        // both reading "Google Chrome", a `display_name` that ignored
        // `FileDescription` entirely still passed. Measured, not supposed --
        // that mutation survived this test as first written.
        assert_eq!(
            display_name(
                Some("Google Chrome"),
                Some("Chromium Suite"),
                r"C:\c\chrome.exe",
                "chrome.exe"
            ),
            "Google Chrome",
            "the per-binary description must win over the product it belongs to"
        );
    }

    #[test]
    fn the_product_name_is_the_fallback_when_there_is_no_description() {
        // Paired with the test above so "prefers the description" cannot be
        // satisfied by a function that only ever reads one of the two.
        assert_eq!(
            display_name(None, Some("Microsoft Office"), r"C:\o\WINWORD.EXE", "WINWORD.EXE"),
            "Microsoft Office"
        );
    }

    #[test]
    fn a_blank_resource_string_is_treated_as_absent() {
        // Measured behaviour of real installers, not a hypothetical: a
        // `FileDescription` of one space would otherwise render a nameless app.
        assert_eq!(
            display_name(Some("   "), Some("Ledgerline"), r"C:\a\Ledgerline.exe", "Ledgerline.exe"),
            "Ledgerline"
        );
        assert_eq!(
            display_name(Some(""), Some("\t"), r"C:\a\Ledgerline.exe", "Ledgerline.exe"),
            "Ledgerline.exe",
            "both resource strings blank must fall through to the file name"
        );
    }

    #[test]
    fn a_resource_less_executable_falls_back_to_its_file_name() {
        assert_eq!(
            display_name(None, None, r"C:\Apps\Ledgerline\Ledgerline.exe", "Ledgerline.exe"),
            "Ledgerline.exe"
        );
    }

    #[test]
    fn a_match_with_no_path_at_all_is_named_by_its_process() {
        // Every match saved before `path` existed, and every Microsoft Store
        // match -- neither has an image to read a name out of.
        assert_eq!(display_name(None, None, "", "Speedtest.exe"), "Speedtest.exe");
        // And a path that names only a directory, which has no file name to
        // borrow either.
        assert_eq!(display_name(None, None, r"C:\Apps\", "Speedtest.exe"), "Speedtest.exe");
    }

    /// The name resolution is a **pure** function of what the resource said,
    /// which is what makes every test above possible.
    ///
    /// Surrounding whitespace is trimmed off a **label**, deliberately -- this
    /// is the one value in this feature that IS normalised, and it is safe to
    /// normalise precisely because nothing but a `ui.label` ever reads it. (The
    /// value that must never be touched is `AppMatch::args`, which is stored;
    /// see that field's own doc.)
    #[test]
    fn a_padded_description_is_shown_without_its_padding() {
        assert_eq!(display_name(Some(" Chrome "), None, "", "chrome.exe"), "Chrome");
    }

    /// Live, and deliberately weak -- it cannot assert a name, because what
    /// this file is called depends on the machine. What it does is run the real
    /// Win32 path on a real file and on a file that does not exist, so an
    /// unterminated buffer walk, a bad pointer cast or a panic on a
    /// resource-less file has somewhere to fail.
    #[test]
    fn reading_a_real_files_version_resource_neither_panics_nor_hangs() {
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let notepad = format!(r"{system_root}\System32\notepad.exe");
        if std::fs::metadata(&notepad).is_ok() {
            let (description, product) = version_names(&notepad);
            // Whatever it says, the name must be usable and must not be blank.
            let name = display_name(description.as_deref(), product.as_deref(), &notepad, "notepad.exe");
            assert!(!name.trim().is_empty(), "a real executable resolved to a blank name");
        }
        // A path that cannot exist: no panic, and nothing invented.
        let missing = format!(r"{system_root}\System32\deskwarden-no-such-file-9ce14529.exe");
        assert_eq!(version_names(&missing), (None, None));
    }

    // -- the debounce -------------------------------------------------------

    /// The path the tests below type, and the app it is bound to. Nothing on
    /// disk, so nothing here depends on what is installed.
    const TYPED: &str = r"C:\Deskwarden Test\Edge\msedge.exe";

    /// Every prefix of `path`, which is exactly what `label` is handed as the
    /// user types it: one call per frame, one character longer each time.
    fn keystrokes(path: &str) -> Vec<String> {
        (1..=path.chars().count())
            .map(|n| path.chars().take(n).collect::<String>())
            .collect()
    }

    /// A context, with no fonts and no frame -- `label` draws nothing, it only
    /// ever loads a texture, and only for a path that named a real file.
    fn ctx() -> egui::Context {
        egui::Context::default()
    }

    #[test]
    fn typing_a_path_one_character_at_a_time_starts_no_probe() {
        // The measurement this whole debounce exists for. Before it, this
        // asserted 0 and got 35: one worker thread, one `fs::metadata` and one
        // permanent map entry per keystroke, several of them naming real
        // directories and so also taking a shell call and a GPU texture.
        let ctx = ctx();
        let mut cache = AppIdentityCache::default();
        let typed = keystrokes(TYPED);
        assert!(typed.len() > 30, "the fixture must be long enough to be a leak: {}", typed.len());

        for prefix in &typed {
            let label = cache.label(&ctx, prefix, "chrome.exe");
            // ...and the form is never left with nothing to paint while it
            // waits: the placeholder is the same file name a started probe
            // would have shown, so the debounce is invisible on screen.
            assert_eq!(
                label.name,
                file_name_of(prefix).unwrap_or("chrome.exe"),
                "the box painted nothing useful while {prefix:?} settled"
            );
        }

        assert_eq!(
            cache.probed(),
            0,
            "typing {} characters started {} lookups -- one per keystroke is the leak",
            typed.len(),
            cache.probed()
        );
    }

    #[test]
    fn a_path_that_stops_moving_is_probed_exactly_once() {
        // The positive control, and the one that matters most: a debounce that
        // never fires would satisfy the test above and would mean no app on
        // this form ever got its name or its icon. It also stands in for the
        // Browse... and picker routes -- a path that arrives whole and then
        // holds still is precisely what those produce.
        let ctx = ctx();
        let mut cache = AppIdentityCache::default();

        assert!(cache.label(&ctx, TYPED, "chrome.exe").pending);
        assert_eq!(cache.probed(), 0, "a path was probed on the frame it first appeared");

        std::thread::sleep(AppIdentityCache::SETTLE + Duration::from_millis(50));
        let _ = cache.label(&ctx, TYPED, "chrome.exe");
        assert_eq!(cache.probed(), 1, "a settled path was never looked up");

        // ...and not again, on any number of further frames.
        for _ in 0..20 {
            let _ = cache.label(&ctx, TYPED, "chrome.exe");
        }
        assert_eq!(cache.probed(), 1, "a resolved path was looked up more than once");
    }

    #[test]
    fn a_path_that_names_no_file_is_never_probed_however_long_it_sits() {
        // A directory path is not an executable and never becomes one by
        // waiting. Distinct from the debounce above: this one is refused on its
        // shape, so it must not even take a settle slot's worth of a lookup.
        let ctx = ctx();
        let mut cache = AppIdentityCache::default();
        let label = cache.label(&ctx, r"C:\Program Files\", "chrome.exe");
        assert_eq!(label.name, "chrome.exe");
        assert!(!label.pending, "a path with no file name left the form waiting for an answer");

        std::thread::sleep(AppIdentityCache::SETTLE + Duration::from_millis(50));
        let _ = cache.label(&ctx, r"C:\Program Files\", "chrome.exe");
        assert_eq!(cache.probed(), 0, "a directory path was looked up");
    }

    /// Drives `cache` until `path` resolves, or gives up. Live -- the probe is
    /// a real worker on a real file system -- so it polls rather than sleeping
    /// a guessed amount.
    fn resolve(cache: &mut AppIdentityCache, ctx: &egui::Context, path: &str, process: &str) -> (String, bool) {
        std::thread::sleep(AppIdentityCache::SETTLE + Duration::from_millis(50));
        for _ in 0..200 {
            let label = cache.label(ctx, path, process);
            if !label.pending {
                return (label.name.to_string(), label.icon.is_some());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("{path:?} never resolved");
    }

    #[test]
    fn a_path_that_turns_out_to_be_a_folder_is_not_named_after_the_folder() {
        // A stored path ending in a directory is permanent, so the debounce
        // does not cover this one. Measured before the fix: `C:\Windows`
        // resolved to name "WINDOWS" with the shell's FOLDER icon beside it --
        // a picture of something that is not the app, over the name of
        // something that is not the app either.
        let ctx = ctx();
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        if std::fs::metadata(&system_root).is_err() {
            return;
        }
        let mut cache = AppIdentityCache::default();
        let (name, has_icon) = resolve(&mut cache, &ctx, &system_root, "Ledgerline.exe");
        assert_eq!(name, "Ledgerline.exe", "a folder's own name was shown as the app's");
        assert!(!has_icon, "a folder icon was drawn beside the app's name");
    }

    #[test]
    fn a_path_that_is_a_real_file_is_still_named_after_the_file() {
        // The positive control for the test above: without it, a `label` that
        // answered `process` for everything would pass it, and no app on this
        // form would ever show its real name again.
        let ctx = ctx();
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let notepad = format!(r"{system_root}\System32\notepad.exe");
        if std::fs::metadata(&notepad).is_err() {
            return;
        }
        let mut cache = AppIdentityCache::default();
        let (name, _) = resolve(&mut cache, &ctx, &notepad, "Ledgerline.exe");
        assert_ne!(
            name, "Ledgerline.exe",
            "a real executable was labelled by the match's process instead of by itself"
        );
        assert!(!name.trim().is_empty());
    }
}

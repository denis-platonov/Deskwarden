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
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
    RRF_RT_REG_SZ,
};
use windows::core::PWSTR;

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
    /// The `DisplayName` of the Microsoft Store package the path belongs to,
    /// when the path is a Store path and the package is registered for this
    /// user. `None` for everything else, which is almost every path.
    store_name: Option<String>,
    /// The path a reachable **file** was proved at, or `None`.
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
    ///
    /// **It is a path and not a `bool` because it is not always the path that
    /// was asked for.** A Microsoft Store path expires (see
    /// [`relocate_store_path`]), and when it does the worker proves the *same
    /// executable under the package's current directory* instead -- so the one
    /// shell call the UI thread is allowed to make has to be told where to
    /// point.
    file: Option<String>,
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
                    // The package's own `DisplayName` sits in the
                    // `ProductName` slot: it is the same kind of fact (the
                    // product this binary belongs to, per its publisher) and
                    // it is only ever reached when the binary's own resource
                    // said nothing. A Store app whose exe carries a
                    // `FileDescription` is still named by that.
                    let product = probe
                        .product
                        .as_deref()
                        .or(probe.store_name.as_deref());
                    display_name(probe.description.as_deref(), product, path, process)
                };
                // Gated on the path having named a reachable FILE, and only
                // ever attempted once: a `None` here is remembered as "no
                // icon", never retried per frame.
                let icon =
                    probe.file.as_deref().and_then(|found| load_icon(ctx, path, found));
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
        let directory = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let mut file = meta
            .as_ref()
            .map(|m| m.is_file())
            .unwrap_or(false)
            .then(|| owned.clone());
        let mut store_name = None;
        // Only a path that answered nothing pays for the Store lookup, so the
        // ordinary case -- a live file anywhere on the machine -- is exactly as
        // cheap as it was. A directory is not retried either: it was reached,
        // it is simply not an app.
        if file.is_none() && !directory {
            for (root, name) in relocate_store_path(&owned) {
                store_name = store_name.or(name);
                if std::fs::metadata(&root).map(|m| m.is_file()).unwrap_or(false) {
                    file = Some(root);
                    break;
                }
            }
        }
        let (description, product) = match &file {
            Some(found) => version_names(found),
            None => (None, None),
        };
        let _ = tx.send(Probe { description, product, store_name, file, directory });
    });
    rx
}

/// `key` names the texture and is the path the cache is keyed on; `path` is
/// where the pixels are read from. They differ for a relocated Store app: the
/// cache still answers about the path the item stores, while the icon has to
/// come off the executable that actually exists.
fn load_icon(ctx: &egui::Context, key: &str, path: &str) -> Option<egui::TextureHandle> {
    let rgba = icon::extract_small_icon(path)?;
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [rgba.width as usize, rgba.height as usize],
        &rgba.rgba,
    );
    Some(ctx.load_texture(format!("app-identity:{key}"), image, egui::TextureOptions::default()))
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


// -- Microsoft Store (MSIX) packages ---------------------------------------
//
// A Store app's executable lives at
// `%ProgramFiles%\WindowsApps\<PackageFullName>\<app>.exe`, and
// `<PackageFullName>` **carries the package version**:
//
// ```text
// AppleInc.iTunes_12139.10003.61011.0_x64__nzyj5cx40ttqa
// ^ name          ^ version           ^arch  ^ publisher id
// ```
//
// Every update installs into a *new* versioned directory and removes the old
// one. So the absolute path this app stores when the user picks a Store app is
// correct exactly until that app next updates, and then it names nothing --
// the app is still installed, still running, still matched by process name, and
// its path is a dead string. `fs::metadata` answers `ERROR_PATH_NOT_FOUND`, the
// version resource cannot be read, `SHGetFileInfoW` has no file to ask about,
// and the card falls all the way back to `iTunes.exe` with no icon. That is the
// reported defect, and it is not a permissions problem: measured on this
// machine, all 123 installed packages grant `BUILTIN\Users:(OI)(CI)(R)` on
// their install directory, and a live Store executable resolves its name, its
// version resource and its icon through the ordinary path with nothing special
// done for it. Only *enumerating* `WindowsApps` itself is denied, which is why
// the current directory cannot simply be looked for on disk.
//
// The three fields either side of the version -- name, architecture, publisher
// id -- do not change across updates, and the per-user package repository under
// `HKCU` lists every installed package by full name with its `PackageRootFolder`
// and the package's `DisplayName`. So a dead path is repaired by reading a
// registry key: no COM, no packaging API, and nothing that can block on a
// volume.

/// The `WindowsApps` package a Store executable sits under, AND everything
/// below it: `("AppleInc.iTunes_..._nzyj5cx40ttqa", r"bin\iTunes.exe")`.
///
/// `WindowsApps` is compared case-insensitively and against a whole path
/// component, so that `C:\My WindowsApps Backup\x.exe` is not mistaken for one.
///
/// **The tail is the half [`relocate_store_path`] used to throw away.** It kept
/// only `file_name_of(path)` and re-joined that to the new package root, which
/// is right for the flat layout and wrong for every nested one -- and MSIX
/// permits nesting, packages commonly use it, and the test above this one has
/// pinned `...\PKG\bin\iTunes.exe` as a shape this module recognises since the
/// day it was written. Measured against a real installed package on the
/// author's machine: `...\ShareX_21...\exiftool_files\perl.exe` relocated to
/// `...\ShareX_21...\perl.exe`, which names nothing, so `Probe::file` stayed
/// `None` and the card degraded to no icon. It *looked* repaired only because
/// the label comes from the registry's `DisplayName` whether or not any
/// candidate proved reachable.
///
/// Separators in the tail are normalised to `\`, because the root read out of
/// the registry always uses `\` and the two halves are joined.
fn windowsapps_package_and_tail(path: &str) -> Option<(&str, String)> {
    let normalised = path.replace('/', "\\");
    let lower = normalised.to_ascii_lowercase();
    let at = lower.find("\\windowsapps\\")? + r"\windowsapps\".len();
    // The component after it, which must be followed by something -- a path
    // that stops at the package directory names no executable.
    let rest = &path[at..];
    let cut = rest.find(['\\', '/'])?;
    let package = &rest[..cut];
    // ...and there must be a file name below it, not just a trailing slash.
    // That check is over `rest`, so it also rejects `PKG\sub\` -- which is
    // what stops an empty tail being handed back here.
    if package.is_empty() || file_name_of(rest).is_none() {
        return None;
    }
    Some((package, rest[cut + 1..].replace('/', "\\")))
}

/// `(name, architecture, publisher id)` out of a `PackageFullName`.
///
/// The form is `Name_Version_Arch__PublisherId`, and a package *name* may not
/// contain an underscore, so splitting on `_` is exact rather than a guess: the
/// empty fourth field is the doubled separator before the publisher id. A
/// **resource** package (`..._Arch_split.scale-100_Publisher`) puts a resource
/// id there instead and is rejected -- it holds satellite assets, never an
/// executable, so relocating a path into one would be wrong.
fn package_full_name_parts(full_name: &str) -> Option<(&str, &str, &str)> {
    let parts: Vec<&str> = full_name.split('_').collect();
    if parts.len() != 5 || !parts[3].is_empty() {
        return None;
    }
    let (name, arch, publisher) = (parts[0], parts[2], parts[4]);
    (!name.is_empty() && !arch.is_empty() && !publisher.is_empty()).then_some((
        name, arch, publisher,
    ))
}

/// Whether `candidate` is the same package as `(name, arch, publisher)`, at
/// whatever version.
///
/// **The architecture is compared and not ignored.** A family is name plus
/// publisher, and `Microsoft.VCLibs.140.00_..._x64__8wekyb3d8bbwe` and its
/// `_x86__` twin are one family with two install directories -- picking the
/// wrong one would relocate a 64-bit path into a 32-bit package.
fn same_package(candidate: &str, name: &str, arch: &str, publisher: &str) -> bool {
    match package_full_name_parts(candidate) {
        Some((n, a, p)) => {
            n.eq_ignore_ascii_case(name)
                && a.eq_ignore_ascii_case(arch)
                && p.eq_ignore_ascii_case(publisher)
        }
        None => false,
    }
}

/// Where the per-user package repository lives. Readable by the user who
/// installed the packages, which is the user whose vault this is.
const PACKAGE_REPOSITORY: &str = concat!(
    r"Software\Classes\Local Settings\Software\Microsoft\Windows",
    r"\CurrentVersion\AppModel\Repository\Packages"
);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Every registered install directory for the same package as `full_name`,
/// each with the package's `DisplayName` if it has one.
///
/// Usually one. More than one only while an update is staged, hence a list
/// rather than an answer -- the caller picks the one the executable is actually
/// in.
fn registered_roots(full_name: &str) -> Vec<(String, Option<String>)> {
    let Some((name, arch, publisher)) = package_full_name_parts(full_name) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for_each_registered_package(|candidate, key| {
        if same_package(candidate, name, arch, publisher) {
            // SAFETY: `key` is the open repository key the walker owns.
            if let Some(root) = unsafe { registry_string(key, candidate, "PackageRootFolder") } {
                let display = unsafe { registry_string(key, candidate, "DisplayName") };
                found.push((root, display));
            }
        }
        true
    });
    found
}

/// Calls `visit` with every installed package's full name and the open
/// repository key, until it returns `false` or the packages run out.
///
/// One walk, one place the registry handle is opened and closed, so the handle
/// cannot be leaked down one arm of a match.
fn for_each_registered_package(mut visit: impl FnMut(&str, HKEY) -> bool) {
    unsafe {
        // Bound to a local: `PCWSTR(wide(..).as_ptr())` would drop the buffer
        // at the end of the expression and pass a dangling pointer.
        let path = wide(PACKAGE_REPOSITORY);
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr()), 0, KEY_READ, &mut key).is_err() {
            return;
        }
        // A registry key name is at most 255 characters; a package full name is
        // far shorter. The buffer is reused, and `len` is reset every turn
        // because `RegEnumKeyExW` overwrites it with the length it wrote.
        let mut buffer = [0u16; 256];
        let mut index: u32 = 0;
        loop {
            let mut len = buffer.len() as u32;
            if RegEnumKeyExW(
                key,
                index,
                PWSTR(buffer.as_mut_ptr()),
                &mut len,
                None,
                PWSTR::null(),
                None,
                None,
            )
            .is_err()
            {
                break;
            }
            index += 1;
            let candidate = String::from_utf16_lossy(&buffer[..len as usize]);
            if !visit(&candidate, key) {
                break;
            }
        }
        let _ = RegCloseKey(key);
    }
}

/// One `REG_SZ` value under `key\subkey`, or `None` if it is absent, empty, or
/// not a string.
///
/// # Safety
///
/// `key` must be an open registry key.
unsafe fn registry_string(key: HKEY, subkey: &str, value: &str) -> Option<String> {
    let sub = wide(subkey);
    let val = wide(value);
    let mut size: u32 = 0;
    // First call sizes the value, in BYTES, including its terminating NUL.
    if RegGetValueW(
        key,
        PCWSTR(sub.as_ptr()),
        PCWSTR(val.as_ptr()),
        RRF_RT_REG_SZ,
        None,
        None,
        Some(&mut size),
    )
    .is_err()
        || size < 2
    {
        return None;
    }
    let mut buffer = vec![0u16; size as usize / 2 + 1];
    let mut got = size;
    if RegGetValueW(
        key,
        PCWSTR(sub.as_ptr()),
        PCWSTR(val.as_ptr()),
        RRF_RT_REG_SZ,
        None,
        Some(buffer.as_mut_ptr() as *mut core::ffi::c_void),
        Some(&mut got),
    )
    .is_err()
    {
        return None;
    }
    // `RegGetValueW` guarantees a terminator; the terminator must not become
    // part of the string.
    let chars = match buffer.iter().position(|&c| c == 0) {
        Some(nul) => &buffer[..nul],
        None => &buffer[..],
    };
    let text = String::from_utf16_lossy(chars);
    (!text.trim().is_empty()).then_some(text)
}

/// The same executable under the current install directory of the Microsoft
/// Store package `path` names an expired version of, plus that package's
/// `DisplayName`.
///
/// Empty for every path that is not a Store path, which is nearly all of them
/// -- so nothing but a dead `WindowsApps` path ever reaches the registry.
///
/// **This does not stat anything.** It is called from the worker thread and it
/// returns candidates; whether one of them is really there is the caller's
/// question, because the caller is the one allowed to pay for a file system.
fn relocate_store_path(path: &str) -> Vec<(String, Option<String>)> {
    relocate_store_path_with(path, registered_roots)
}

/// [`relocate_store_path`] with the registry lifted out, so all of it can be
/// tested without one.
///
/// **The seam exists because the join is the line that broke, and the only
/// thing that could reach it was a live test.** `relocate_store_path` reads the
/// per-user package repository, so every test of it depended on what happened
/// to be installed on the machine running it -- and the one test that killed
/// the flattening mutant `return`s when it finds no nested package. On CI, in a
/// container, or on any box whose Store packages are all flat, flattening this
/// join again left the whole suite green. `lookup` is `registered_roots` in
/// production, pinned by
/// `the_repair_asks_the_registry_through_the_one_seam_and_returns_its_answer`,
/// and a fixture in the tests, which is what makes the join itself provable
/// anywhere.
fn relocate_store_path_with(
    path: &str,
    lookup: impl Fn(&str) -> Vec<(String, Option<String>)>,
) -> Vec<(String, Option<String>)> {
    // **The TAIL, not the file name.** Keeping only `file_name_of(path)` and
    // re-joining it to the package root silently flattened every nested layout:
    // `...\PKG\exiftool_files\perl.exe` became `...\PKG\perl.exe`, which names
    // nothing at all, so nothing was relocated and the card lost its icon
    // anyway. See `windowsapps_package_and_tail`.
    let Some((package, tail)) = windowsapps_package_and_tail(path) else {
        return Vec::new();
    };
    lookup(package)
        .into_iter()
        .map(|(root, name)| {
            let root = root.trim_end_matches(['\\', '/']);
            (format!(r"{root}\{tail}"), name)
        })
        .collect()
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
            skipped(
                "a_path_that_turns_out_to_be_a_folder_is_not_named_after_the_folder",
                "%SystemRoot% does not exist on this machine",
            );
            return;
        }
        let mut cache = AppIdentityCache::default();
        let (name, has_icon) = resolve(&mut cache, &ctx, &system_root, "Ledgerline.exe");
        assert_eq!(name, "Ledgerline.exe", "a folder's own name was shown as the app's");
        assert!(!has_icon, "a folder icon was drawn beside the app's name");
    }


    // -- Microsoft Store paths ----------------------------------------------

    /// A real full name, kept in one place so every fixture below is a variation
    /// on a string Windows actually produces. This is the one from the bug
    /// report.
    const ITUNES: &str = "AppleInc.iTunes_12139.10003.61011.0_x64__nzyj5cx40ttqa";

    /// Just the package half of `windowsapps_package_and_tail`.
    ///
    /// This used to be a production function of its own, and became dead the
    /// moment `relocate_store_path` started asking for the tail as well --
    /// there is exactly one production caller and it needs both halves. Kept
    /// here because the "is this a Store path at all" tests below are about a
    /// question worth asking on its own, and pairing every one of them with a
    /// tail they do not care about would bury what they are asserting.
    fn windowsapps_package_of(path: &str) -> Option<&str> {
        windowsapps_package_and_tail(path).map(|(package, _)| package)
    }

    #[test]
    fn a_store_path_yields_the_package_directory_it_sits_in() {
        assert_eq!(
            windowsapps_package_of(&format!(r"C:\Program Files\WindowsApps\{ITUNES}\iTunes.exe")),
            Some(ITUNES)
        );
        // Windows accepts both separators, and the drive and the casing of
        // "WindowsApps" are not fixed -- a path out of a vault field is
        // whatever the user or the picker put there.
        assert_eq!(
            windowsapps_package_of(&format!("D:/Program Files/windowsapps/{ITUNES}/iTunes.exe")),
            Some(ITUNES)
        );
        // A nested path inside the package still names the package.
        assert_eq!(
            windowsapps_package_of(&format!(
                r"C:\Program Files\WindowsApps\{ITUNES}\bin\iTunes.exe"
            )),
            Some(ITUNES)
        );
    }

    /// **The subdirectory a repair used to drop.**
    ///
    /// `relocate_store_path` kept only `file_name_of(path)`, so a nested
    /// executable was re-joined straight to the package root and the repaired
    /// path named nothing. The test above already pinned `...\PKG\bin\x.exe` as
    /// a shape this module RECOGNISES -- it was recognised and then thrown
    /// away one function later, and no test looked at the part that was thrown.
    #[test]
    fn a_store_path_keeps_everything_below_the_package_directory() {
        assert_eq!(
            windowsapps_package_and_tail(&format!(
                r"C:\Program Files\WindowsApps\{ITUNES}\iTunes.exe"
            )),
            Some((ITUNES, "iTunes.exe".to_string())),
            "the flat case"
        );
        assert_eq!(
            windowsapps_package_and_tail(&format!(
                r"C:\Program Files\WindowsApps\{ITUNES}\bin\iTunes.exe"
            )),
            Some((ITUNES, r"bin\iTunes.exe".to_string())),
            "one directory deep -- the shape MSIX permits and packages use, and the shape a \
             repair that kept only the file name turned into a path naming nothing"
        );
        // Two deep, and the real one this was found on: ShareX ships
        // `exiftool_files\perl.exe`.
        assert_eq!(
            windowsapps_package_and_tail(&format!(
                r"C:\Program Files\WindowsApps\{ITUNES}\exiftool_files\perl\perl.exe"
            )),
            Some((ITUNES, r"exiftool_files\perl\perl.exe".to_string()))
        );
        // Forward slashes, which Windows accepts and a vault field may hold.
        // The tail is joined to a registry root that always uses `\`, so it is
        // normalised rather than passed through.
        assert_eq!(
            windowsapps_package_and_tail(&format!(
                "D:/Program Files/windowsapps/{ITUNES}/bin/iTunes.exe"
            )),
            Some((ITUNES, r"bin\iTunes.exe".to_string()))
        );
        // Positive control on the fixtures: the flat and the nested answers
        // must DIFFER, or an assertion satisfied by `file_name_of` alone would
        // read as an assertion about the tail.
        let flat = windowsapps_package_and_tail(&format!(
            r"C:\Program Files\WindowsApps\{ITUNES}\iTunes.exe"
        ))
        .map(|(_, tail)| tail);
        let nested = windowsapps_package_and_tail(&format!(
            r"C:\Program Files\WindowsApps\{ITUNES}\bin\iTunes.exe"
        ))
        .map(|(_, tail)| tail);
        assert_ne!(
            flat, nested,
            "the two fixtures produce the same tail, so this test cannot tell a repair that \
             keeps the subdirectory from one that drops it"
        );
    }

    /// Nothing the package rejects may come back with a tail: a `Some` here is
    /// a path that will be joined to a registry root and probed.
    #[test]
    fn a_path_with_no_file_below_the_package_has_no_tail() {
        for dead in [
            format!(r"C:\Program Files\WindowsApps\{ITUNES}\"),
            format!(r"C:\Program Files\WindowsApps\{ITUNES}\bin\"),
            format!(r"C:\Program Files\WindowsApps\{ITUNES}"),
            r"C:\Program Files\WindowsApps\".to_string(),
            r"C:\Windows\System32\notepad.exe".to_string(),
        ] {
            assert_eq!(windowsapps_package_and_tail(&dead), None, "{dead}");
        }
    }

    #[test]
    fn a_path_that_merely_contains_the_word_is_not_a_store_path() {
        // The positive control for the test above is that test; this one is the
        // reason the match is against a whole path COMPONENT. A substring test
        // would relocate this into whatever package it parsed out, which is a
        // registry walk and a wrong answer for an ordinary folder.
        assert_eq!(windowsapps_package_of(r"C:\My WindowsApps Backup\iTunes.exe"), None);
        assert_eq!(windowsapps_package_of(r"C:\WindowsAppsData\Thing\x.exe"), None);
        // A component that ENDS in the word, with a plausible package name
        // below it. Measured: without this line a `find("windowsapps\\")`
        // that dropped the leading separator passed this test, relocated
        // an ordinary backup folder into the registry, and survived every
        // other fixture here -- the two above only pin the case where the
        // word STARTS a component.
        assert_eq!(
            windowsapps_package_of(r"C:\Backup\MyWindowsApps\Led_1.0.0.0_x64__abcdefghijklm\x.exe"),
            None
        );
        // No package component at all, and no file below the package.
        assert_eq!(windowsapps_package_of(r"C:\Program Files\WindowsApps\"), None);
        assert_eq!(
            windowsapps_package_of(&format!(r"C:\Program Files\WindowsApps\{ITUNES}")),
            None
        );
        assert_eq!(
            windowsapps_package_of(&format!(r"C:\Program Files\WindowsApps\{ITUNES}\")),
            None
        );
        // ...and the ordinary case, which must never reach the registry.
        assert_eq!(windowsapps_package_of(r"C:\Windows\System32\notepad.exe"), None);
        assert_eq!(windowsapps_package_of(r"C:\Program Files\Google\Chrome\chrome.exe"), None);
    }

    #[test]
    fn a_package_full_name_splits_into_name_architecture_and_publisher() {
        // All three fields are DIFFERENT strings, and the version between them
        // is a fourth: a parser that returned the wrong field, or the whole
        // name, could not pass this.
        assert_eq!(
            package_full_name_parts(ITUNES),
            Some(("AppleInc.iTunes", "x64", "nzyj5cx40ttqa"))
        );
        assert_eq!(
            package_full_name_parts("NotepadPlusPlus_1.0.0.0_neutral__7njy0v32s6xk6"),
            Some(("NotepadPlusPlus", "neutral", "7njy0v32s6xk6"))
        );
    }

    #[test]
    fn a_resource_package_or_a_malformed_name_is_not_relocatable() {
        // A resource package puts a resource id where the empty field belongs.
        // It holds satellite assets and never an executable, so relocating into
        // one would move a path somewhere the app is not.
        assert_eq!(
            package_full_name_parts("AppleInc.iTunes_12139.1_x64_split.scale-100_nzyj5cx40ttqa"),
            None
        );
        // Not a full name at all: an ordinary directory that happens to sit
        // under WindowsApps, or a truncated one.
        assert_eq!(package_full_name_parts("iTunes"), None);
        assert_eq!(package_full_name_parts("AppleInc.iTunes_12139.1_x64"), None);
        assert_eq!(package_full_name_parts("_12139.1_x64__nzyj5cx40ttqa"), None);
        assert_eq!(package_full_name_parts("AppleInc.iTunes_12139.1_x64__"), None);
    }

    #[test]
    fn the_same_package_at_a_newer_version_is_still_the_same_package() {
        // The whole point: the version is the ONLY field allowed to differ,
        // because it is the only field an update changes.
        assert!(same_package(
            "AppleInc.iTunes_12140.20000.70000.0_x64__nzyj5cx40ttqa",
            "AppleInc.iTunes",
            "x64",
            "nzyj5cx40ttqa"
        ));
    }

    #[test]
    fn a_different_architecture_or_publisher_is_a_different_package() {
        // Paired with the test above, and each of these differs from it in
        // exactly ONE field -- so a `same_package` that compared only the name,
        // or only the publisher, fails here rather than passing both.
        assert!(
            !same_package(
                "AppleInc.iTunes_12140.2_x86__nzyj5cx40ttqa",
                "AppleInc.iTunes",
                "x64",
                "nzyj5cx40ttqa"
            ),
            "a 32-bit package was accepted as the current version of a 64-bit one"
        );
        assert!(
            !same_package(
                "AppleInc.iTunes_12140.2_x64__8wekyb3d8bbwe",
                "AppleInc.iTunes",
                "x64",
                "nzyj5cx40ttqa"
            ),
            "a different publisher's package of the same name was accepted"
        );
        assert!(!same_package(
            "AppleInc.iCloud_12140.2_x64__nzyj5cx40ttqa",
            "AppleInc.iTunes",
            "x64",
            "nzyj5cx40ttqa"
        ));
    }

    #[test]
    fn a_path_outside_windowsapps_never_reaches_the_registry() {
        // Not a performance nicety: it is the guarantee that adding this route
        // changed nothing for the paths that already worked, and for the dead
        // network share this module's worker exists to survive.
        assert!(relocate_store_path(r"C:\Program Files\Google\Chrome\chrome.exe").is_empty());
        assert!(relocate_store_path(r"\\dead-share\apps\Ledgerline.exe").is_empty());
        assert!(relocate_store_path("").is_empty());
    }

    /// A live test that found nothing to look at says so.
    ///
    /// **A silent `return` is indistinguishable from a pass.** The nested
    /// relocation test below was, until the tests above it existed, the ONLY
    /// thing that killed the flattening defect -- and on a machine with no
    /// nested Store package it returned green having asserted nothing. Printed
    /// rather than `ignored`, because whether there is anything to look at is
    /// not known until the registry has been walked, which is after the test
    /// has started. `cargo test -- --show-output` is where these appear.
    fn skipped(test: &str, why: &str) {
        println!("SKIPPED {test}: {why} -- this run asserted nothing about it");
    }

    /// A fixture `lookup` for [`relocate_store_path_with`], plus a counter, so
    /// a test can prove the seam was consulted rather than quietly bypassed.
    fn fixture_roots<'a>(
        roots: &[(&str, Option<&str>)],
        seen: &'a std::cell::RefCell<Vec<String>>,
    ) -> impl Fn(&str) -> Vec<(String, Option<String>)> + 'a {
        let owned: Vec<(String, Option<String>)> = roots
            .iter()
            .map(|(r, n)| (r.to_string(), n.map(str::to_string)))
            .collect();
        move |package: &str| {
            seen.borrow_mut().push(package.to_string());
            owned.clone()
        }
    }

    /// **The join keeps the tail, proved without a registry and without a
    /// particular machine's packages.**
    ///
    /// This is the line that shipped broken: `file_name_of(&tail)` instead of
    /// the tail, so `...\PKG\exiftool_files\perl.exe` was "repaired" to
    /// `...\PKG\perl.exe`, which names nothing. Its only killing test was live
    /// and skipped itself where no nested package was installed, which is every
    /// container and most machines.
    #[test]
    fn the_repair_keeps_everything_below_the_package_when_it_rejoins_the_root() {
        let seen = std::cell::RefCell::new(Vec::new());
        // Two roots, because an update being staged really does register two,
        // and a `map` that answered only the first would still pass a one-root
        // fixture.
        let roots = [
            (r"C:\Program Files\WindowsApps\ShareX_21.0.0.0_x64__egrzcvs15399j", Some("ShareX")),
            (r"D:\Apps\WindowsApps\ShareX_22.0.0.0_x64__egrzcvs15399j", None),
        ];
        let expired =
            r"C:\Program Files\WindowsApps\ShareX_1.0.0.0_x64__egrzcvs15399j\exiftool_files\perl.exe";
        let got = relocate_store_path_with(expired, fixture_roots(&roots, &seen));

        assert_eq!(
            seen.borrow().as_slice(),
            [r"ShareX_1.0.0.0_x64__egrzcvs15399j".to_string()],
            "the lookup was asked for something other than the expired path's own package, or \
             was not asked at all -- in which case everything below asserts nothing"
        );
        assert_eq!(got.len(), roots.len(), "a staged second root was dropped: {got:?}");

        for ((root, display), (path, name)) in roots.iter().zip(&got) {
            let wanted = format!(r"{root}\exiftool_files\perl.exe");
            let flattened = format!(r"{root}\perl.exe");
            // The control the live test also carries: the two shapes really are
            // different strings, so an assertion that the answer is one of them
            // is an assertion about something.
            assert_ne!(wanted, flattened, "control: the fixture is not nested");
            assert_eq!(
                path, &wanted,
                "the repair dropped the subdirectory. `{flattened}` names nothing, so \
                 `Probe::file` stays `None` and the card is left with no icon while the label \
                 still reads right, because the label comes from the registry either way"
            );
            assert_eq!(
                name.as_deref(),
                *display,
                "the package's DisplayName was not carried through the join"
            );
        }
    }

    #[test]
    fn a_flat_layout_and_a_trailing_separator_are_both_rejoined_without_damage() {
        // The other half of the same line: keeping the tail must not have
        // broken the flat case the old code got right, and the registry's roots
        // are not guaranteed to be free of a trailing `\`.
        let seen = std::cell::RefCell::new(Vec::new());
        let roots = [(r"C:\Program Files\WindowsApps\iTunes_12_x64__nzyj5cx40ttqa\", None)];
        let got = relocate_store_path_with(
            &format!(r"C:\Program Files\WindowsApps\{ITUNES}\iTunes.exe"),
            fixture_roots(&roots, &seen),
        );
        assert_eq!(seen.borrow().len(), 1, "the lookup was never consulted");
        assert_eq!(
            got.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
            [r"C:\Program Files\WindowsApps\iTunes_12_x64__nzyj5cx40ttqa\iTunes.exe"],
            "a flat relocation onto a root that ends in a separator came out malformed: {got:?}"
        );
    }

    #[test]
    fn a_forward_slashed_nested_path_is_rejoined_with_backslashes() {
        // `windowsapps_package_and_tail` normalises the tail because the root
        // out of the registry always uses `\`; this is that normalisation seen
        // from the join, which is where a mixed separator would actually hurt.
        let seen = std::cell::RefCell::new(Vec::new());
        let roots = [(r"C:\Program Files\WindowsApps\Led_2.0.0.0_x64__abcdefghijklm", None)];
        let got = relocate_store_path_with(
            "C:/Program Files/windowsapps/Led_1.0.0.0_x64__abcdefghijklm/bin/tools/led.exe",
            fixture_roots(&roots, &seen),
        );
        assert_eq!(seen.borrow().len(), 1, "the lookup was never consulted");
        assert_eq!(
            got.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
            [r"C:\Program Files\WindowsApps\Led_2.0.0.0_x64__abcdefghijklm\bin\tools\led.exe"],
            "a nested Store path written with forward slashes was rejoined wrong: {got:?}"
        );
    }

    #[test]
    fn a_path_that_is_not_a_store_path_never_reaches_the_lookup_at_all() {
        // `a_path_outside_windowsapps_never_reaches_the_registry` asserts the
        // ANSWER is empty, which a function that walked the registry and then
        // threw the result away would also satisfy. This asserts the walk did
        // not happen -- the guarantee that a dead network share costs nothing.
        let seen = std::cell::RefCell::new(Vec::new());
        let roots = [(r"C:\Program Files\WindowsApps\Led_2.0.0.0_x64__abcdefghijklm", None)];
        for outside in [
            r"C:\Program Files\Google\Chrome\chrome.exe",
            r"\\dead-share\apps\Ledgerline.exe",
            r"C:\My WindowsApps Backup\iTunes.exe",
            r"C:\Program Files\WindowsApps\Led_1.0.0.0_x64__abcdefghijklm",
            "",
        ] {
            let got = relocate_store_path_with(outside, fixture_roots(&roots, &seen));
            assert!(got.is_empty(), "{outside} was relocated onto {got:?}");
        }
        assert!(
            seen.borrow().is_empty(),
            "a path that is not a Store path reached the registry lookup: {:?}",
            seen.borrow()
        );
        // Positive control: the same fixture DOES get consulted for a real
        // Store path, so `is_empty()` above is a fact about these paths and not
        // about a lookup that is never called by anyone.
        let got = relocate_store_path_with(
            r"C:\Program Files\WindowsApps\Led_1.0.0.0_x64__abcdefghijklm\bin\led.exe",
            fixture_roots(&roots, &seen),
        );
        assert_eq!(seen.borrow().len(), 1, "control: the fixture is never consulted at all");
        assert_eq!(got.len(), 1, "control: the fixture produces no candidate for any path");
    }

    /// The brace-matched body of the function whose signature is `head`, with
    /// comments removed and every whitespace character squeezed out.
    ///
    /// Whitespace goes so that no reformatting of a one-line body can turn the
    /// pin below off; comments go so that documenting the body does not break
    /// it. A STRING literal in the body would be mangled by the comment
    /// stripping -- which fails the pin rather than passing it, and a body with
    /// a string literal in it is not a bare delegation anyway.
    fn compacted_body_of(production: &str, head: &str) -> String {
        assert_eq!(
            production.matches(head).count(),
            1,
            "{head:?} does not appear exactly once, so there is no one body to look at"
        );
        let at = production.find(head).expect("checked directly above");
        let open = at + head.len() - 1;
        assert!(
            production[open..].starts_with('{'),
            "{head:?} does not end at the brace that opens its body"
        );
        let mut depth = 0i32;
        let mut close = None;
        for (i, c) in production[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let close = close.unwrap_or_else(|| panic!("{head:?}'s body is never closed"));
        let body = &production[open + 1..close];

        let without_line_comments = body
            .lines()
            .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
            .collect::<Vec<_>>()
            .join("\n");
        let mut code = String::new();
        let mut rest = without_line_comments.as_str();
        while let Some(start) = rest.find("/*") {
            code.push_str(&rest[..start]);
            let end = rest[start..].find("*/").map_or(rest.len(), |e| start + e + 2);
            rest = &rest[end..];
        }
        code.push_str(rest);
        code.chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// **Production goes through the tested function, with the real registry --
    /// and hands back what it got.**
    ///
    /// The seam is only worth anything if `relocate_store_path` is a delegation
    /// and not a second copy of the join that can drift from the one every test
    /// above exercises. But "it still calls the seam" says nothing about what
    /// it RETURNS. This survived the whole suite on a machine with no nested
    /// Store package -- which is every CI box, every container, and the
    /// condition the live tests below skip themselves under (measured, 1655 +
    /// 142 green):
    ///
    /// ```text
    /// fn relocate_store_path(path: &str) -> Vec<(String, Option<String>)> {
    ///     relocate_store_path_with(path, registered_roots)
    ///         .into_iter()
    ///         .map(|(p, _)| (p, None))
    ///         .collect()
    /// }
    /// ```
    ///
    /// Every candidate loses its registry `DisplayName` in production, which is
    /// precisely the regression the DisplayName mutation was written to kill,
    /// and all five tests of the seam call `relocate_store_path_with` directly
    /// and cannot see it. A `truncate`, a `filter`, a `dedup` or a wrapper that
    /// re-derives its own answer from the seam's are the same story.
    ///
    /// So the delegation is pinned as the WHOLE body: whitespace-squeezed and
    /// comment-stripped, `relocate_store_path`'s body must be the call and
    /// nothing else. A post-process of any shape is a different string.
    ///
    /// This is a source pin, and a behavioural one would be better -- but there
    /// is nothing to route a pure test through. `relocate_store_path`'s only
    /// distinction from the seam IS the real registry; a fixture cannot be
    /// handed to it without becoming the seam again. Pinning the body is the
    /// strongest machine-independent statement available, and it is deliberately
    /// an EQUALITY rather than a containment.
    #[test]
    fn the_repair_asks_the_registry_through_the_one_seam_and_returns_its_answer() {
        let source = include_str!("app_identity.rs");
        let production = source
            .split_once(concat!("mod te", "sts {"))
            .expect("app_identity.rs must still have its test module")
            .0;
        assert!(production.len() < source.len(), "control: the split cut the test module off");

        let delegation = concat!("relocate_store_path", "_with(path, registered_roots)");
        assert_eq!(
            production.matches(delegation).count(),
            1,
            "{delegation:?} is not in the production half exactly once: `relocate_store_path` \
             either stopped delegating -- so the join every test above proves is not the join \
             production runs -- or it now calls the seam more than once"
        );
        // And the join lives in the tested function, not duplicated beside it.
        assert_eq!(
            production.matches(concat!("{root}", r"\{tail}")).count(),
            1,
            "the `root`/`tail` join appears more than once in the production half, so one copy \
             of it is untested"
        );

        // The delegation is the ENTIRE body, not merely present in it.
        let head = concat!(
            "fn relocate_store_path",
            "(path: &str) -> Vec<(String, Option<String>)> {"
        );
        let squeezed = concat!("relocate_store_path", "_with(path,registered_roots)");
        assert_eq!(
            compacted_body_of(production, head),
            squeezed,
            "`relocate_store_path` does something to the seam's answer before handing it back. \
             Every test of the join calls `relocate_store_path_with` directly, so a `map`, a \
             `filter`, a `truncate` or any other post-process out here is invisible to all of \
             them -- and dropping the registry `DisplayName` on the way through is a shipped \
             regression this suite has already had to kill once"
        );

        // Controls. First, the extractor really reads a body, comments and all.
        let fixture = concat!(
            "fn f(x: u8) -> u8 {",
            "\n    // a line comment\n    let y = g(x); /* inline */\n    y // trailing\n}"
        );
        assert_eq!(
            compacted_body_of(fixture, concat!("fn f", "(x: u8) -> u8 {")),
            "lety=g(x);y",
            "control: the body extractor cannot read a body with comments in it, so the \
             assertion above is about something other than this function"
        );
        // Second -- and this is the one that matters -- the extractor DISAGREES
        // with the survivor. Without this the equality above could be passing
        // against an extractor that returns the same string for everything.
        let survivor = concat!(
            "fn relocate_store_path(path: &str) -> Vec<(String, Option<String>)> {\n",
            "    relocate_store_path_with(path, registered_roots)\n",
            "        .into_iter()\n",
            "        .map(|(p, _)| (p, None))\n",
            "        .collect()\n}"
        );
        assert_ne!(
            compacted_body_of(survivor, head),
            squeezed,
            "control: the body pin cannot tell the measured survivor from the real delegation, \
             so it is not what stands between this crate and it"
        );
        assert!(
            compacted_body_of(survivor, head).starts_with(squeezed),
            "control: the survivor fixture is not the delegation plus a post-process, so it is \
             not the mutation this test claims to kill"
        );
        // Third, whitespace really is squeezed, so no reformatting turns this off.
        let reformatted = concat!(
            "fn relocate_store_path(path: &str) -> Vec<(String, Option<String>)> {\n\n",
            "    relocate_store_path_with(\n        path,\n        registered_roots,\n    )\n}"
        );
        assert_eq!(
            compacted_body_of(reformatted, head),
            concat!("relocate_store_path", "_with(path,registered_roots,)"),
            "control: the squeeze does not survive a line break, so this pin is a formatting \
             assertion wearing a behavioural one's clothes"
        );
    }

    /// An installed Store package that really has an executable in it, as
    /// `(package full name, install root, one .exe file name)`.
    ///
    /// Live: what is installed depends on the machine, so the tests below skip
    /// themselves when there is nothing to look at rather than asserting about
    /// a package that might not be there.
    fn an_installed_store_app() -> Option<(String, String, String)> {
        let mut answer = None;
        for_each_registered_package(|full_name, key| {
            if package_full_name_parts(full_name).is_none() {
                return true;
            }
            // SAFETY: `key` is the repository key the walker holds open.
            let Some(root) = (unsafe { registry_string(key, full_name, "PackageRootFolder") })
            else {
                return true;
            };
            let Ok(entries) = std::fs::read_dir(&root) else {
                return true;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.to_ascii_lowercase().ends_with(".exe")
                    && entry.path().is_file()
                    // Something with a real icon, which the stub launchers
                    // shipped beside some packages do not have.
                    && icon::extract_small_icon(&entry.path().to_string_lossy()).is_some()
                {
                    answer = Some((full_name.to_string(), root, name));
                    return false;
                }
            }
            true
        });
        answer
    }

    /// An installed Store package with an executable in a SUBDIRECTORY, as
    /// `(package full name, install root, tail relative to that root)`.
    ///
    /// `an_installed_store_app` only ever `read_dir`s the package root, which
    /// is why nothing covered the nested layout at all. This one descends,
    /// bounded to two levels below the root -- deep enough for the real cases
    /// (`exiftool_files\perl.exe`) and shallow enough not to walk a large
    /// package. No icon requirement: what the test below asserts is the shape
    /// of the repaired path, and demanding an icon would make it skip itself on
    /// machines where the only nested exe is a helper binary with none.
    fn an_installed_store_app_in_a_subdirectory() -> Option<(String, String, String)> {
        fn find(dir: &std::path::Path, prefix: &str, depth: usize) -> Option<String> {
            let entries = std::fs::read_dir(dir).ok()?;
            let mut dirs = Vec::new();
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let Ok(kind) = entry.file_type() else { continue };
                if kind.is_dir() {
                    dirs.push((entry.path(), format!("{prefix}{name}\\")));
                } else if !prefix.is_empty() && name.to_ascii_lowercase().ends_with(".exe") {
                    return Some(format!("{prefix}{name}"));
                }
            }
            if depth == 0 {
                return None;
            }
            dirs.into_iter()
                .find_map(|(path, prefix)| find(&path, &prefix, depth - 1))
        }

        let mut answer = None;
        for_each_registered_package(|full_name, key| {
            if package_full_name_parts(full_name).is_none() {
                return true;
            }
            // SAFETY: `key` is the repository key the walker holds open.
            let Some(root) = (unsafe { registry_string(key, full_name, "PackageRootFolder") })
            else {
                return true;
            };
            match find(std::path::Path::new(&root), "", 2) {
                Some(tail) => {
                    answer = Some((full_name.to_string(), root, tail));
                    false
                }
                None => true,
            }
        });
        answer
    }

    /// **The repair keeps the subdirectory, against a real installed package.**
    ///
    /// The pure fixtures above state the shape; this states that the shape is
    /// what a live registry root plus a live nested executable produce, and
    /// that the result is a path that EXISTS. Before the fix this asserted
    /// nothing because no test descended into a package at all: the repaired
    /// path named `<root>\perl.exe`, `Probe::file` stayed `None`, and the card
    /// degraded to no icon while the label still read "ShareX" -- because the
    /// label comes from the registry regardless of whether anything was found.
    #[test]
    fn an_expired_nested_store_path_is_relocated_with_its_subdirectory_intact() {
        let Some((full_name, root, tail)) = an_installed_store_app_in_a_subdirectory() else {
            skipped(
                "an_expired_nested_store_path_is_relocated_with_its_subdirectory_intact",
                "no installed Store package on this machine has an executable in a subdirectory",
            );
            return;
        };
        // The fixture really is nested, or this is the flat test again.
        assert!(
            tail.contains('\\'),
            "the fixture executable is not in a subdirectory: {tail}"
        );
        let expired = expired_path(&full_name, &tail);
        assert!(
            std::fs::metadata(&expired).is_err(),
            "the fixture must really be dead, or it proves nothing: {expired}"
        );

        let relocated = relocate_store_path(&expired);
        assert!(
            !relocated.is_empty(),
            "{expired}\n  relocated onto nothing at all"
        );
        let wanted = format!(r"{}\{tail}", root.trim_end_matches('\\'));
        assert!(
            relocated.iter().any(|(p, _)| p.eq_ignore_ascii_case(&wanted)),
            "{expired}\n  did not relocate onto {wanted}\n  got {relocated:?}"
        );
        // And the repaired path is REACHABLE, which is the whole point: the
        // flattened one named a file that does not exist, so the probe found
        // nothing and the card had no icon.
        assert!(
            relocated
                .iter()
                .any(|(p, _)| std::fs::metadata(p).is_ok()),
            "every relocated candidate names a file that is not there, so the probe finds \
             nothing and the card is left with no icon: {relocated:?}"
        );
        // The flattened answer the old code produced must NOT be among them --
        // the negative control, so this cannot pass against a function that
        // returns both.
        let flattened = format!(
            r"{}\{}",
            root.trim_end_matches('\\'),
            file_name_of(&tail).expect("the tail names a file")
        );
        assert_ne!(
            flattened.to_ascii_lowercase(),
            wanted.to_ascii_lowercase(),
            "control: the flattened path and the correct one are different strings"
        );
    }

    /// The path the user's item would hold after the app updated: the right
    /// package, the right executable, a version that is no longer installed.
    fn expired_path(full_name: &str, exe: &str) -> String {
        let (name, arch, publisher) = package_full_name_parts(full_name).expect("a full name");
        format!(r"C:\Program Files\WindowsApps\{name}_1.0.0.0_{arch}__{publisher}\{exe}")
    }

    #[test]
    fn an_expired_store_path_is_relocated_onto_the_installed_version() {
        let Some((full_name, root, exe)) = an_installed_store_app() else {
            skipped(
                "an_expired_store_path_is_relocated_onto_the_installed_version",
                "no installed Store package on this machine has an executable with an icon in its root",
            );
            return;
        };
        let expired = expired_path(&full_name, &exe);
        assert!(
            std::fs::metadata(&expired).is_err(),
            "the fixture must really be dead, or it proves nothing: {expired}"
        );
        let relocated = relocate_store_path(&expired);
        let wanted = format!(r"{}\{exe}", root.trim_end_matches('\\'));
        assert!(
            relocated.iter().any(|(p, _)| p.eq_ignore_ascii_case(&wanted)),
            "{expired}\n  did not relocate onto {wanted}\n  got {relocated:?}"
        );
    }

    #[test]
    fn an_expired_store_path_shows_the_apps_name_and_icon_again() {
        // The reported defect, end to end and through the same `label` the form
        // calls: a Store path whose version has gone is what shows `iTunes.exe`
        // with no icon.
        let Some((full_name, _, exe)) = an_installed_store_app() else {
            skipped(
                "an_expired_store_path_shows_the_apps_name_and_icon_again",
                "no installed Store package on this machine has an executable with an icon in its root",
            );
            return;
        };
        let ctx = ctx();
        let mut cache = AppIdentityCache::default();
        let expired = expired_path(&full_name, &exe);
        let (name, has_icon) = resolve(&mut cache, &ctx, &expired, "Ledgerline.exe");
        assert!(has_icon, "{expired}\n  resolved with no icon");
        assert_ne!(
            name, exe,
            "a Store app was still labelled by its file name after being relocated"
        );
        assert_ne!(name, "Ledgerline.exe", "the match's process name was shown instead");
        assert!(!name.trim().is_empty());
    }

    #[test]
    fn a_store_path_for_an_app_that_is_not_installed_still_degrades_quietly() {
        // The positive control for the two tests above, and the one that keeps
        // them honest: without it, a `label` that handed every WindowsApps path
        // some icon would pass them both. This package is Apple's real iTunes
        // full name -- if it is installed here the test has nothing to say, so
        // it steps aside rather than asserting something machine-dependent.
        let path = format!(r"C:\Program Files\WindowsApps\{ITUNES}\iTunes.exe");
        if !relocate_store_path(&path).is_empty() {
            skipped(
                "a_store_path_for_an_app_that_is_not_installed_still_degrades_quietly",
                "Apple iTunes really is installed here, so the fixture path is not dead",
            );
            return;
        }
        let ctx = ctx();
        let mut cache = AppIdentityCache::default();
        let (name, has_icon) = resolve(&mut cache, &ctx, &path, "Ledgerline.exe");
        assert_eq!(name, "iTunes.exe", "an uninstalled app was given a name it does not have");
        assert!(!has_icon, "an unreachable path was given an icon -- a generic one is a lie");
    }

    #[test]
    fn an_unreachable_store_path_does_not_make_the_form_wait_for_it() {
        // The guarantee the registry walk must not have cost: the UI thread
        // never blocks, and a path that resolves to nothing resolves to nothing
        // promptly. `resolve` panics if it never settles.
        let ctx = ctx();
        let mut cache = AppIdentityCache::default();
        let path = format!(r"C:\Program Files\WindowsApps\{ITUNES}\iTunes.exe");
        let started = Instant::now();
        let _ = resolve(&mut cache, &ctx, &path, "Ledgerline.exe");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "a dead Store path took {:?} to give up",
            started.elapsed()
        );
    }

    #[test]
    fn typing_a_store_path_one_character_at_a_time_still_starts_no_probe() {
        // `fa505c8`'s guarantee, re-measured on the path shape that now has a
        // registry walk behind it: every prefix of this path is a path that has
        // never been seen before, and several of them parse as plausible
        // package directories.
        let ctx = ctx();
        let mut cache = AppIdentityCache::default();
        let full = format!(r"C:\Program Files\WindowsApps\{ITUNES}\iTunes.exe");
        let typed = keystrokes(&full);
        assert!(typed.len() > 30, "the fixture must be long enough to be a leak");
        for prefix in &typed {
            let _ = cache.label(&ctx, prefix, "iTunes.exe");
        }
        assert_eq!(cache.probed(), 0, "typing a Store path started {} lookups", cache.probed());
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
            skipped(
                "a_path_that_is_a_real_file_is_still_named_after_the_file",
                "notepad.exe is not where %SystemRoot% says it should be",
            );
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
    // -----------------------------------------------------------------
    // The region BELOW the cut -- the half no source guard in this file
    // reads.
    // -----------------------------------------------------------------

    /// The `cfg` attribute that makes a module test-only, split so this
    /// constant is not itself one and so it cannot be found by a guard
    /// looking for the real attributes.
    const BELOW_CUT_GATE: &str = concat!("#[cfg(", "test)]");

    /// The literal the source guards in this file cut the file at. Split for
    /// the same reason: an unsplit copy would BE the first occurrence, and
    /// every production slice in this file would come back empty.
    const BELOW_CUT_MARKER: &str = concat!("mod te", "sts {");

    /// Column-0 lines that live below the cut but are the CONTENTS OF A
    /// STRING LITERAL rather than source. Each is controlled below: it must
    /// still occur in this file exactly once, so a stale entry here cannot
    /// quietly widen the hole this test exists to close.
    const BELOW_CUT_STRING_LINES: &[&str] = &[];

    /// `true` for `mod NAME {`, `pub mod NAME {` and `pub(crate) mod NAME {`,
    /// and for nothing else. Deliberately exact rather than a `starts_with`:
    /// `mod x { fn escape() {} }` on one line is not a module opener as far
    /// as this walk is concerned, and must fail it.
    fn below_cut_is_module_opener(line: &str) -> bool {
        let t = line.strip_prefix("pub(crate) ").unwrap_or(line);
        let t = t.strip_prefix("pub ").unwrap_or(t);
        let Some(rest) = t.strip_prefix("mod ") else {
            return false;
        };
        let Some(name) = rest.strip_suffix(" {") else {
            return false;
        };
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }

    /// What this file's below-the-cut region is walked under.
    ///
    /// The walk itself is [`crate::below_cut::walk`] and is NOT written here.
    /// It used to be, in fifteen near-identical copies, which is how the
    /// escaped-quote off-by-one in the brace matcher reached three files at
    /// once and how every fix since has had to be applied N times or silently
    /// fail to propagate. What the copies really disagreed about is this
    /// struct's worth of text, so that is what stayed local.
    ///
    /// `is_module_opener` is this file's OWN
    /// [`below_cut_is_module_opener`] and not
    /// [`crate::below_cut::is_module_opener`], deliberately: the
    /// `modules == column_zero_module_openers(..)` control below compares the
    /// walk's count against the other instance, so a one-edit widening of
    /// either predicate desynchronizes the two and reds the suite. Pointing
    /// the walk at the shared predicate would have made both sides move
    /// together and thrown that property away.
    const BELOW_CUT_RULES: crate::below_cut::WalkRules = crate::below_cut::WalkRules {
        gate: BELOW_CUT_GATE,
        gated_at_start: true,
        gate_at_column_zero: false,
        is_module_opener: below_cut_is_module_opener,
        string_lines: BELOW_CUT_STRING_LINES,
        top_level_item_note: "Every source guard in this file slices at the test-module marker and reads only what is above it, so an item down here is read by none of them: it can duplicate a call site pinned at exactly one, or reintroduce a construct banned by name, and the suite stays green.",
        ungated_module_note: "A `pub(crate) mod ext { impl super::T { .. } }` written down here is the same escape, one `mod` deep.",
    };

    /// `(visited, modules, closes, depth)` for the region below this file's
    /// cut, by the one shared walk.
    fn walk_below_the_cut(source: &str) -> (usize, usize, usize, usize) {
        let cut = source
            .find(BELOW_CUT_MARKER)
            .expect("the cut marker is controlled by the caller");
        crate::below_cut::walk(&source[cut..], &BELOW_CUT_RULES)
    }

    /// **Below the cut there is nothing but test-only modules, and the cut is
    /// where every guard in this file believes it is.**
    ///
    /// This file guards a great deal of behaviour by slicing its own source at
    /// the first `mod tests` module opener and counting needles in the half ABOVE that cut.
    /// Two things can silently empty those guards, and neither of them changes
    /// a single guard's own text:
    ///
    /// 1. **Anything appended below the test modules is invisible to all of
    ///    them.** It is not counted, not forbidden and not read. Confirmed by
    ///    mutation, in this crate, tonight: a second production item appended
    ///    under the test module of `main.rs` -- a duplicate startup handoff,
    ///    the exact defect a guard pins at "exactly one" -- left the whole
    ///    binary suite green with zero warnings. The bottom of a file is a
    ///    natural place to add a helper, and until this test existed nothing
    ///    stopped it.
    /// 2. **The cut can move UPWARDS.** The slice is a `find` of a literal, so
    ///    that literal appearing in a comment or a string above the real test
    ///    modules truncates the production half and blinds every guard to
    ///    everything after it. This file already contains that literal in prose
    ///    elsewhere; the guards survive it today only because the marker they
    ///    match is spelled to avoid it, which is an accident of spelling and
    ///    not a check.
    ///
    /// So the whole region from the cut to EOF is walked and required to be a
    /// sequence of `#[cfg(test)]`-gated, column-0 module blocks and nothing
    /// else, and the cut itself is pinned against a production anchor that must
    /// still be found (a positive control) immediately above it.
    ///
    /// This is a source-analysis test, which is the class that has failed in
    /// this codebase repeatedly, so every part of it carries its own control:
    /// the anchor's occurrence count, the module count, the close count, the
    /// number of lines actually visited, and the string-literal exceptions.
    /// A walk that visited nothing would fail on all five.
    #[test]
    fn nothing_but_gated_test_modules_lives_below_the_guards_cut() {
        let source = include_str!("app_identity.rs");

        // 1. The cut lands where the guards think it does.
        assert_eq!(
            source.matches(BELOW_CUT_MARKER).count(),
            1,
            "{BELOW_CUT_MARKER:?} occurs more than once in this file. The guards here `find` \
             the FIRST one, so a second copy in a comment or a string above the real test \
             module moves the cut upwards and empties every one of them"
        );
        let cut = source.find(BELOW_CUT_MARKER).unwrap_or_else(|| {
            panic!(
                "{BELOW_CUT_MARKER:?} is not in this file at all -- every source guard here \
                 slices at it, and a slice that cannot be made is a guard that reads nothing"
            )
        });
        assert!(
            cut > 0 && source.as_bytes()[cut - 1] == b'\n',
            "the cut landed in the MIDDLE of a line, so the marker was matched inside a \
             comment or a string literal rather than at a real declaration; that truncates \
             the production half and blinds every source guard in this file to everything \
             below the truncation point"
        );
        assert!(
            source[..cut].trim_end().ends_with(BELOW_CUT_GATE),
            "the module the cut lands on is not preceded by {BELOW_CUT_GATE:?}, so the region \
             below the cut is not test-only to begin with"
        );

        // 2. Positive control on where the cut is: the production half must
        //    still reach the LAST production item in the file. If the marker
        //    were matched earlier than the real test modules, this anchor
        //    would fall below the cut instead of just above it.
        const LAST_PRODUCTION_ITEM: &str = concat!("fn relocate_store_path", "_with(");
        assert_eq!(
            source.matches(LAST_PRODUCTION_ITEM).count(),
            1,
            "control: {LAST_PRODUCTION_ITEM:?} is not in this file exactly once, so it no \
             longer pins anything -- repoint it at the last production item above the test \
             modules"
        );
        let anchor = source.find(LAST_PRODUCTION_ITEM).expect("counted just above");
        assert!(
            anchor < cut,
            "the last production item this control knows about is BELOW the cut, which means \
             the cut moved up and the production half every guard in this file reads is \
             truncated"
        );
        assert!(
            cut - anchor < 4_000,
            "the cut is more than 4000 bytes past the last production item this control knows \
             about: either production was appended below the anchor (repoint the anchor) or \
             the cut moved down"
        );

        // 3. The walk. `lines()` strips the `\r` of this file's CRLF endings,
        //    so every comparison below is against the line's real text.
        let (visited, modules, closes, depth) = walk_below_the_cut(source);
        // 4. The walk is not vacuous, and it finished.
        assert!(
            visited > 100,
            "control: the walk visited only {visited} lines below the cut, which is not a test \
             module's worth -- the slice is empty or nearly so and this test proves nothing"
        );
        assert_eq!(
            depth, 0,
            "a test module below the cut is never closed by a column-0 `}}`, so the walk ran \
             off the end of the file inside it and stopped inspecting top-level lines"
        );
        assert_eq!(
            modules, 1,
            "the number of top-level test modules below the cut changed. That is fine -- but \
             this count is the control that proves the walk really visited them, so update it \
             deliberately rather than loosening it"
        );
        assert_eq!(
            closes, modules,
            "control: every module the walk opened must also have been closed at column 0"
        );

        // The opener count, cross-checked against a SECOND instance of the
        // opener predicate. `column_zero_module_openers` uses
        // `below_cut::is_module_opener`; the walk used this file's own
        // `below_cut_is_module_opener`. Widening either one alone
        // desynchronizes them and fails here, which is the property that
        // sharing a single predicate would have cost.
        assert_eq!(
            modules,
            crate::below_cut::column_zero_module_openers(&source[cut..]),
            "the walk opened {modules} modules but there are {} column-0 gated module openers \
             below the cut -- the walk's opener predicate and \
             `below_cut::is_module_opener` no longer agree",
            crate::below_cut::column_zero_module_openers(&source[cut..])
        );

        // Controls on the walk itself. Without these it could be a no-op that
        // visits lines and asserts nothing.
        let appended = format!("{source}\npub fn sneaked() {{}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&appended)).is_err(),
            "control: the walk accepted a `pub fn` appended below the test modules, which is \
             the exact mutation it exists to catch"
        );
        let indented = format!("{source}\n    struct Sneaked(u8);\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&indented)).is_err(),
            "control: the walk accepted an INDENTED top-level item appended below the test \
             modules, which a column-0-only filter would miss"
        );
        let ungated = format!("{source}\nmod shipped {{\n}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&ungated)).is_err(),
            "control: the walk accepted an UNGATED module below the cut, which ships"
        );

        // And the one the line walk could not catch: this file's own text with
        // its last module closed by an INDENTED brace, a `pub fn` at file
        // scope after it, and a column-0 `}` further down to rebalance the
        // count. Perfectly balanced source, no lexer trick -- every payload
        // line is indented, so the `depth == 1` branch skips it and the walk
        // ends with `closes == modules` and `depth == 0`. Measured SURVIVING
        // the whole suite at 2211 lib / 217 bin / 0 failed / 0 warnings in
        // both profiles, and shipping in the lib's LLVM IR. Only the
        // byte-offset close check kills it.
        let balanced = format!(
            "{}    }}\n    pub fn sneaked(x: u64) -> u64 {{ x }}\n    \
             #[allow(dead_code)]\n    mod filler {{\n}}\n",
            source
                .replace("\r\n", "\n")
                .strip_suffix("}\n")
                .expect("this file ends with a column-0 closing brace")
        );
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&balanced)).is_err(),
            "control: the walk accepted this file's last test module closed by an INDENTED \
             brace with a `pub fn` at file scope after it. That is the payload the byte-offset \
             close check exists for, and it is once again invisible"
        );
        for known in BELOW_CUT_STRING_LINES {
            assert_eq!(
                source.matches(known).count(),
                1,
                "control: the string-literal exception {known:?} is not in this file exactly \
                 once, so it is stale and is widening this check for nothing"
            );
        }
    }
}

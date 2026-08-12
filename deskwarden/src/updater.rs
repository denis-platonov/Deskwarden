use crate::signature::{is_trusted_signer, verify_authenticode};
use semver::Version;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// `Clone` so the tray-click handler can hand a copy to the background
/// download/verify/apply thread (see `main.rs`) while keeping its own copy for
/// the tray label, rather than moving the app's record of the available update
/// into a thread it can't get it back from.
#[derive(Clone, Debug)]
pub struct ReleaseInfo {
    pub version: Version,
    pub installer_download_url: String,
}

/// How long to wait for a TCP connection to establish before giving up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Total-time bound for the releases-API check.
///
/// The response is one small JSON document, so total elapsed time is the right
/// shape here: anything near this number is a broken path, not a slow one. The
/// check runs on a background thread (`main.rs`), so a 30s wait costs the user
/// nothing but a late tray badge.
const API_DEADLINE: Duration = Duration::from_secs(30);

/// No-progress bound for the installer download.
///
/// The download is a ~6 MB stream and its legitimate duration is unknown -- it
/// depends entirely on the user's link -- so *total* time is the wrong thing
/// to bound. v0.3.0's first fix bounded it anyway, at 600s, and that number is
/// the proof: too tight to allow a genuinely slow download, too loose to be a
/// bound anyone benefits from, and it left the tray pinned on "Updating to vX"
/// for ten minutes with repeat clicks swallowed (`main.rs`) where a stalled
/// download used to fail in fifteen seconds.
///
/// So this bounds the gap *between* reads instead: 15s with no byte arriving
/// is a dead transfer at any link speed worth downloading over, while a
/// slow-but-steady stream runs as long as it needs to. Deliberately *not*
/// paired with a whole-request deadline -- see
/// [`crate::http_agent::bounded_stall`]; adding one is exactly what made this
/// setting inert before.
///
/// Not bounded here: a server that dribbles one byte every 14s forever. That
/// is indistinguishable from a very slow link by any time-based rule, and this
/// is the shape that says so rather than pretending otherwise.
const DOWNLOAD_STALL_TIMEOUT: Duration = Duration::from_secs(15);

/// Agent for [`check_for_update`]: a small JSON response, bounded by total
/// time.
///
/// Separate from [`build_download_agent`] because the two requests need
/// different *kinds* of bound and ureq 2.12.1 can express only one per agent.
/// They used to share one agent with both settings applied, which meant one of
/// the settings silently did nothing; see [`crate::http_agent`].
pub fn build_api_agent() -> crate::http_agent::TotalBounded {
    crate::http_agent::bounded_total(CONNECT_TIMEOUT, API_DEADLINE)
}

/// Agent for [`download_and_verify`]: a multi-megabyte stream, bounded by time
/// without progress. See [`build_api_agent`] for why this is its own agent.
pub fn build_download_agent() -> crate::http_agent::StallBounded {
    crate::http_agent::bounded_stall(CONNECT_TIMEOUT, DOWNLOAD_STALL_TIMEOUT)
}

pub fn check_for_update(
    base_url: &str,
    current_version: &Version,
    agent: &crate::http_agent::TotalBounded,
) -> Result<Option<ReleaseInfo>, String> {
    let url = format!("{base_url}/repos/denis-platonov/deskwarden/releases/latest");
    let body: serde_json::Value = agent
        .get(&url)
        .call()
        .map_err(|e| format!("failed to reach GitHub releases API: {e}"))?
        .into_json()
        .map_err(|e| format!("failed to parse releases response: {e}"))?;

    let tag = body["tag_name"]
        .as_str()
        .ok_or("release response missing tag_name")?;
    let version_str = tag.strip_prefix('v').unwrap_or(tag);
    let latest_version =
        Version::parse(version_str).map_err(|e| format!("release tag '{tag}' is not valid semver: {e}"))?;

    let installer_url = body["assets"]
        .as_array()
        .and_then(|assets| {
            assets
                .iter()
                .find(|a| a["name"].as_str().map(|n| n.ends_with("-installer.exe")).unwrap_or(false))
        })
        .and_then(|asset| asset["browser_download_url"].as_str())
        .ok_or("release has no installer asset")?
        .to_string();

    if latest_version > *current_version {
        Ok(Some(ReleaseInfo { version: latest_version, installer_download_url: installer_url }))
    } else {
        Ok(None)
    }
}

/// File name a downloaded installer for `version` is stored under. Shared by
/// [`download_and_verify`] and [`cleanup_stale_downloads`] so the cleanup pass
/// can recognise exactly what the download pass writes, and nothing else.
fn installer_file_name(version: &Version) -> String {
    format!("deskwarden-{version}-installer.exe")
}

/// True for file names [`download_and_verify`] could have produced.
///
/// Matched by shape rather than by an exact version, because cleanup runs at
/// startup against leftovers from *previous* versions (whose numbers this
/// build has no list of). Anything else in the directory is left alone.
fn is_downloaded_installer(file_name: &str) -> bool {
    file_name.starts_with("deskwarden-") && file_name.ends_with("-installer.exe")
}

/// Deletes installers left behind in `dir` by earlier update attempts.
///
/// Called once at startup rather than after applying an update: `apply_update`
/// launches the installer and the app then exits immediately, so at that point
/// the file is the image of a *running* process and cannot be deleted. By the
/// next startup that installer has finished, so every downloaded installer
/// still sitting here is spent -- either it was applied (and this build is its
/// result) or the attempt failed -- and none of them are worth keeping.
///
/// Best-effort: a file that can't be removed (still locked by a slow
/// installer, say) is reported and skipped, never fatal. A missing directory
/// is success, not an error -- it just means nothing was ever downloaded.
pub fn cleanup_stale_downloads(dir: &Path) -> Result<usize, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(format!("could not read {}: {e}", dir.display())),
    };

    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !is_downloaded_installer(name) {
            continue;
        }
        match std::fs::remove_file(entry.path()) {
            Ok(()) => removed += 1,
            Err(e) => log::warn!(
                "could not delete stale update download {}: {e}",
                entry.path().display()
            ),
        }
    }
    Ok(removed)
}

/// Streams the release installer to `dest_dir` and refuses anything not signed
/// by the expected signer.
///
/// `agent` comes from [`build_download_agent`], and the type says so rather
/// than the comment: this is the one caller in the crate whose bound is "time
/// without progress" rather than "total time". That used to be prose only,
/// with both functions taking a bare `ureq::Agent`, so swapping the two
/// arguments at their `main.rs` call sites compiled -- and nothing tests
/// `main.rs`. The wrong direction is not cosmetic: [`build_api_agent`]'s 30s
/// *total* cap applied to a 6 MB stream aborts every legitimately slow
/// download, which is worse than the 600s cap this shape exists to remove.
pub fn download_and_verify(
    release: &ReleaseInfo,
    expected_thumbprint: &str,
    dest_dir: &Path,
    agent: &crate::http_agent::StallBounded,
) -> Result<PathBuf, String> {
    // Created here rather than assumed to exist: this is a dedicated cache
    // subdirectory (see `main.rs`), not the config directory the rest of the
    // app already creates at startup.
    std::fs::create_dir_all(dest_dir)
        .map_err(|e| format!("could not create {}: {e}", dest_dir.display()))?;
    let dest_path = dest_dir.join(installer_file_name(&release.version));

    let response = agent
        .get(&release.installer_download_url)
        .call()
        .map_err(|e| format!("failed to download installer: {e}"))?;
    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(&dest_path).map_err(|e| e.to_string())?;
    // A stalled transfer aborts here, part-written, and the partial file must
    // not be left behind -- same cleanup the signature-failure branch below
    // does, for the same reason. `cleanup_stale_downloads` would eventually
    // catch it at the next startup, but this path stopped being near-
    // unreachable when the bound went from a 600s total to a 15s stall: a
    // flaky link now produces a partial installer every retry, and each one
    // sits in the cache directory until the app is next restarted.
    if let Err(e) = std::io::copy(&mut reader, &mut file) {
        drop(file);
        let _ = std::fs::remove_file(&dest_path);
        return Err(e.to_string());
    }
    drop(file);

    let info = verify_authenticode(&dest_path)?;
    if !is_trusted_signer(&info, expected_thumbprint) {
        let _ = std::fs::remove_file(&dest_path);
        return Err("downloaded installer failed signature verification".to_string());
    }

    Ok(dest_path)
}

/// The Authenticode signer thumbprint an installer must carry before
/// [`apply_update`] will launch it.
///
/// The real certificate deskwarden's release builds will be signed with does
/// not exist yet at this point in the project, so there is no genuine value to
/// put here. This placeholder is intentionally not a plausible-looking
/// thumbprint: it can never match a real signature, so `is_trusted_signer`
/// -- and therefore both [`download_and_verify`] and [`apply_update`] -- fails
/// closed, refusing every update, until this constant is replaced with the
/// real one.
///
/// It lives HERE rather than in `main.rs` on purpose. [`apply_update`] is the
/// one function in this crate that turns a file on disk into a running
/// process, and if the signer it checks against were a parameter then the
/// check would be worth exactly as much as the caller wanted it to be worth:
/// any caller could pass the thumbprint of whatever it had just planted. A
/// compile-time constant cannot be argued with at a call site.
pub const EXPECTED_SIGNER_THUMBPRINT: &str = "PLACEHOLDER_SET_ONCE_SIGNPATH_CERT_ISSUED";

/// The launch-time trust decision, as a pure function of the signature so it
/// can be tested without a signed file on disk.
///
/// This pins WHAT the decision computes. It says nothing about whether the
/// launcher consults it, and that gap was not theoretical: the previous shape
/// of this module disclosed that deleting the `if !installer_is_launchable(..)`
/// branch from [`apply_update`] SURVIVED the whole suite, and rested its
/// defence on the dead-code warning the deletion leaves behind. **That backstop
/// did not exist.** Measured on this crate at `e7327f9`, replacing the entire
/// gating block with
///
/// ```ignore
/// let _launchable = installer_is_launchable(&info);
/// ```
///
/// -- which uses `info`, uses the function, and removes the gate -- survived at
/// 2168 lib / 217 bin / 4 ignored / 0 failed and ZERO warnings. Composed with a
/// one-line `pub fn zz_start(d: &Path, r: &ReleaseInfo)` forwarder in
/// `accounts.rs` (see [`apply_update`]'s note on the child-start guard's
/// `ALLOWED` list) it restored an arbitrary-directory, signature-free, jobless
/// process launcher on the crate's public surface, still at 2168 / 0 failed /
/// 0 warnings.
///
/// Substitution was killed; deletion and NEUTRALISATION were both free. The
/// difference is the whole lesson: a pin on a pure decision cannot see whether
/// the decision is in a GATING POSITION. That is now held separately, by
/// [`apply_update_with`] and the routing tests over it, and this function is
/// only one half of the pair.
fn installer_is_launchable(info: &crate::signature::SignatureInfo) -> bool {
    is_trusted_signer(info, EXPECTED_SIGNER_THUMBPRINT)
}

/// The one place in this crate that turns the downloaded installer into a
/// running process.
///
/// Split out of [`apply_update`] so that the launch is a VALUE
/// ([`UpdaterEnv::launch`]) a test can substitute and observe, rather than a
/// statement no test may ever execute. It takes a path because it is the
/// bottom of the funnel, not the top: the path it is given is constructed by
/// [`apply_update_with`] from [`installer_file_name`], and reaching this
/// function without going through the gate above it is what
/// [`the_only_process_start_in_this_module_is_the_launch_seam`] forbids.
fn launch_installer(installer_path: &Path) -> Result<(), String> {
    Command::new(installer_path)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES"])
        .spawn()
        .map_err(|e| format!("failed to launch installer: {e}"))?;
    Ok(())
}

/// The two outside-world operations [`apply_update`] performs, as `fn`
/// pointers, so that the ROUTING between them can be tested.
///
/// # Why this exists
///
/// `apply_update` cannot be driven to its spawn by any test that is allowed to
/// exist in this crate: doing so would mean shipping a genuinely
/// Authenticode-signed installer as a fixture and then RUNNING it. So for three
/// revisions the trust decision was lifted into the pure
/// [`installer_is_launchable`] and pinned over hand-built [`SignatureInfo`]
/// values, and the question "does the launcher still ask?" was disclosed as
/// untestable. It was not untestable. It was untested, and measured survivors
/// followed -- see [`installer_is_launchable`] for the numbers.
///
/// Behind this seam the launcher can be run end to end with **no real file, no
/// real signature and no real process**: the harness supplies a
/// [`SignatureInfo`] by hand through `verify` and records every path that
/// arrives at `launch`. The assertions are then about ROUTING -- that `launch`
/// is NOT reached for a wrong signer, an invalid signature, a missing
/// thumbprint or an unreadable file, and IS reached, with exactly the
/// constructed path, for the trusted one. That is the property deletion and
/// neutralisation both break and substitution also breaks, so one shape of test
/// now covers all three.
///
/// # `fn` pointers rather than `impl Fn`
///
/// Closures would be more convenient at the call site and would be the wrong
/// choice here, for the reason `VaultFrameEnv` in `vault_window/mod.rs`
/// records: a seam that is itself unpinned only MOVES the hole. A `fn` pointer
/// has an address, so [`production_holds_the_real_verify_and_the_real_launch`]
/// can assert that what [`UpdaterEnv::production`] hands over is the real
/// `verify_authenticode` and the real [`launch_installer`] BY IDENTITY, with
/// `std::ptr::fn_addr_eq`. A wrapper, a forwarder, a rename or a flag-gated
/// no-op is a different address and fails there, whatever it is spelled.
pub struct UpdaterEnv {
    /// [`crate::signature::verify_authenticode`] in production.
    verify: fn(&Path) -> Result<crate::signature::SignatureInfo, String>,
    /// [`launch_installer`] in production -- the module's only process start.
    launch: fn(&Path) -> Result<(), String>,
}

impl UpdaterEnv {
    /// The real world. The only constructor a shipping build compiles --
    /// pinned by [`production_is_the_only_updater_env_a_shipping_build_has`],
    /// which reads this file's production slice. The test-only substitute is
    /// written down in `mod tests` as an inherent impl, deliberately BELOW
    /// every source guard in this file, so that a test-gated item up here
    /// cannot truncate the slice those guards read.
    pub fn production() -> Self {
        Self { verify: verify_authenticode, launch: launch_installer }
    }
}

/// [`apply_update`]'s whole body, with the outside world as a parameter.
///
/// The gate is the point: `launch` is unreachable except through the `if`
/// below, and there is no other path to a process start in this module. See
/// [`UpdaterEnv`] for why this shape exists and
/// [`the_only_process_start_in_this_module_is_the_launch_seam`] for the guard
/// that keeps a second, ungated spawn from being written beside it.
fn apply_update_with(dest_dir: &Path, release: &ReleaseInfo, env: &UpdaterEnv) -> Result<(), String> {
    let installer_path = dest_dir.join(installer_file_name(&release.version));
    // The path is folded into every error on this path deliberately: the one
    // thing a caller must be able to see is WHICH file was about to be
    // launched, and `verify_authenticode`'s own errors are about the Win32
    // call rather than about the file.
    let info = (env.verify)(&installer_path)
        .map_err(|e| format!("refusing to launch {}: {e}", installer_path.display()))?;
    if !installer_is_launchable(&info) {
        return Err(format!(
            "refusing to launch {}: it is not signed by the expected signer",
            installer_path.display()
        ));
    }
    (env.launch)(&installer_path)
}

/// Launches the installer this module downloaded for `release`, and nothing
/// else.
///
/// # Why this does not take a path
///
/// It used to: `pub fn apply_update(installer_path: &Path)`, which spawned
/// whatever it was handed, with no job object and no further checks. That made
/// the updater a general-purpose, arbitrary-path process launcher standing
/// `pub` on the crate's surface -- and `updater.rs` is on the child-start
/// guard's `ALLOWED` list, so the guard reads none of it. Measured: a one-line
/// `pub fn zz_start(p: &Path) { crate::updater::apply_update(p) }` in
/// `accounts.rs` SURVIVED the whole suite at 2164 lib / 217 bin / 0 failed /
/// 0 warnings. A call to an existing `pub fn` is a QUIETER edit than the alias
/// lines the guard does catch, not a louder one.
///
/// So the capability is removed rather than guarded. The updater is the thing
/// that did the downloading; it knows where it wrote and under what name, so
/// it reconstructs the path from [`installer_file_name`] -- the same function
/// [`download_and_verify`] wrote it with and [`cleanup_stale_downloads`]
/// recognises it by. A caller chooses a directory and a version; it does not
/// choose a file.
///
/// # And it re-verifies
///
/// Naming the file is not on its own enough -- a caller can still name a
/// directory, and a directory is a place a file can be planted. So the
/// signature check is repeated here, immediately before the spawn, against
/// [`EXPECTED_SIGNER_THUMBPRINT`] rather than against anything the caller
/// supplied. [`download_and_verify`] already checks, but it checks at download
/// time and hands back a path; the gap between those two moments is exactly
/// where a swap goes. This is the check that is adjacent to the launch.
///
/// # And the check is now known to be CONSULTED
///
/// The body lives in [`apply_update_with`], over an [`UpdaterEnv`], and this is
/// a two-line wrapper over it holding production's env. That is not a
/// refactoring for its own sake: with the decision merely lifted into a pure
/// [`installer_is_launchable`], neutralising the gate to `let _launchable =
/// installer_is_launchable(&info);` was measured surviving the entire suite at
/// zero warnings. The routing tests behind the seam are what fail on it now.
pub fn apply_update(dest_dir: &Path, release: &ReleaseInfo) -> Result<(), String> {
    apply_update_with(dest_dir, release, &UpdaterEnv::production())
}

#[cfg(test)]
mod tests {
    use super::*;
    use semver::Version;

    #[test]
    fn reports_a_newer_release_as_available() {
        let mut server = mockito::Server::new();
        let body = r#"{
            "tag_name": "v1.2.0",
            "assets": [
                {"name": "deskwarden-installer.exe", "browser_download_url": "https://example.com/deskwarden-installer.exe"}
            ]
        }"#;
        let _m = server
            .mock("GET", "/repos/denis-platonov/deskwarden/releases/latest")
            .with_status(200)
            .with_body(body)
            .create();

        let current = Version::parse("1.1.0").unwrap();
        let agent = build_api_agent();
        let result = check_for_update(&server.url(), &current, &agent).unwrap();

        let release = result.expect("expected an available update");
        assert_eq!(release.version, Version::parse("1.2.0").unwrap());
        assert_eq!(release.installer_download_url, "https://example.com/deskwarden-installer.exe");
    }

    #[test]
    fn selects_the_installer_asset_even_when_a_bare_exe_is_also_present() {
        // Task 6's release workflow publishes both a bare `deskwarden.exe` and
        // a `*-installer.exe`. This test pins the selection logic to picking
        // the installer specifically, regardless of asset order.
        let mut server = mockito::Server::new();
        let body = r#"{
            "tag_name": "v1.2.0",
            "assets": [
                {"name": "deskwarden.exe", "browser_download_url": "https://example.com/deskwarden.exe"},
                {"name": "deskwarden-installer.exe", "browser_download_url": "https://example.com/deskwarden-installer.exe"}
            ]
        }"#;
        let _m = server
            .mock("GET", "/repos/denis-platonov/deskwarden/releases/latest")
            .with_status(200)
            .with_body(body)
            .create();

        let current = Version::parse("1.1.0").unwrap();
        let agent = build_api_agent();
        let result = check_for_update(&server.url(), &current, &agent).unwrap();

        let release = result.expect("expected an available update");
        assert_eq!(release.installer_download_url, "https://example.com/deskwarden-installer.exe");
    }

    #[test]
    fn reports_no_update_when_current_version_is_latest() {
        let mut server = mockito::Server::new();
        let body = r#"{
            "tag_name": "v1.1.0",
            "assets": [
                {"name": "deskwarden-installer.exe", "browser_download_url": "https://example.com/deskwarden-installer.exe"}
            ]
        }"#;
        let _m = server
            .mock("GET", "/repos/denis-platonov/deskwarden/releases/latest")
            .with_status(200)
            .with_body(body)
            .create();

        let current = Version::parse("1.1.0").unwrap();
        let agent = build_api_agent();
        let result = check_for_update(&server.url(), &current, &agent).unwrap();

        assert!(result.is_none());
    }

    #[test]
    fn reports_no_update_when_current_version_is_newer() {
        let mut server = mockito::Server::new();
        let body = r#"{
            "tag_name": "v1.0.0",
            "assets": [
                {"name": "deskwarden-installer.exe", "browser_download_url": "https://example.com/deskwarden-installer.exe"}
            ]
        }"#;
        let _m = server
            .mock("GET", "/repos/denis-platonov/deskwarden/releases/latest")
            .with_status(200)
            .with_body(body)
            .create();

        let current = Version::parse("1.1.0").unwrap();
        let agent = build_api_agent();
        let result = check_for_update(&server.url(), &current, &agent).unwrap();

        assert!(result.is_none());
    }

    /// A unique scratch directory, same `temp_dir()` + nanos pattern
    /// `session_store`/`logging`'s tests already use (no `tempfile`
    /// dev-dependency in this crate).
    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "deskwarden-updater-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn cleanup_removes_downloaded_installers_of_any_version() {
        let dir = scratch_dir("cleanup");
        std::fs::write(dir.join("deskwarden-0.1.0-installer.exe"), b"old").unwrap();
        std::fs::write(dir.join("deskwarden-0.2.0-installer.exe"), b"newer").unwrap();

        let removed = cleanup_stale_downloads(&dir).unwrap();

        assert_eq!(removed, 2);
        assert!(!dir.join("deskwarden-0.1.0-installer.exe").exists());
        assert!(!dir.join("deskwarden-0.2.0-installer.exe").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cleanup_leaves_unrelated_files_alone() {
        // The download directory is deliberately separate from the config
        // directory (which holds `session.bin` and the log), but the cleanup
        // pass still only ever deletes what it recognises as its own.
        let dir = scratch_dir("unrelated");
        std::fs::write(dir.join("session.bin"), b"secret").unwrap();
        std::fs::write(dir.join("deskwarden.log"), b"log").unwrap();
        std::fs::write(dir.join("deskwarden-0.1.0-installer.exe"), b"old").unwrap();

        let removed = cleanup_stale_downloads(&dir).unwrap();

        assert_eq!(removed, 1);
        assert!(dir.join("session.bin").exists());
        assert!(dir.join("deskwarden.log").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cleanup_treats_a_missing_directory_as_nothing_to_do() {
        let dir = std::env::temp_dir().join("deskwarden-updater-test-missing-dir-never-created");
        assert!(!dir.exists());
        assert_eq!(cleanup_stale_downloads(&dir).unwrap(), 0);
    }

    #[test]
    fn cleanup_recognises_exactly_what_download_and_verify_writes() {
        // Pins the two halves together: if the download file name ever
        // changes, this fails rather than silently leaving downloads to
        // accumulate forever.
        let version = Version::parse("1.2.3").unwrap();
        assert!(is_downloaded_installer(&installer_file_name(&version)));
    }

    #[test]
    fn errors_when_no_installer_asset_is_present() {
        let mut server = mockito::Server::new();
        let body = r#"{"tag_name": "v1.2.0", "assets": []}"#;
        let _m = server
            .mock("GET", "/repos/denis-platonov/deskwarden/releases/latest")
            .with_status(200)
            .with_body(body)
            .create();

        let current = Version::parse("1.1.0").unwrap();
        let agent = build_api_agent();
        let result = check_for_update(&server.url(), &current, &agent);

        assert!(result.is_err());
    }

    /// Pins that the *production* download agent -- not a test-built one --
    /// really is the non-pooling, stall-bounded shape.
    ///
    /// Connection reuse is the whole question: on a reused socket ureq has
    /// cleared the read timeout, and this agent deliberately carries no
    /// whole-request deadline to fall back on, so a pooled second request
    /// would be unbounded. `bounded_stall`'s `max_idle_connections(0)` is what
    /// makes that impossible; this asserts `build_download_agent` actually
    /// goes through it. Counted, not timed, so it cannot flake.
    #[test]
    fn the_production_download_agent_never_reuses_a_connection() {
        use std::io::{Read as _, Write as _};
        use std::net::{TcpListener, TcpStream};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        fn read_head(stream: &mut TcpStream) -> bool {
            let mut seen = Vec::new();
            let mut byte = [0u8; 1];
            while stream.read(&mut byte).unwrap_or(0) == 1 {
                seen.push(byte[0]);
                if seen.ends_with(b"\r\n\r\n") {
                    return true;
                }
            }
            false
        }

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepts = Arc::new(AtomicUsize::new(0));

        let counted = Arc::clone(&accepts);
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                counted.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    while read_head(&mut stream) {
                        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi");
                        let _ = stream.flush();
                    }
                });
            }
        });

        let agent = build_download_agent();
        let url = format!("http://127.0.0.1:{port}/installer.exe");
        for _ in 0..2 {
            agent.get(&url).call().unwrap().into_string().unwrap();
        }

        assert_eq!(
            accepts.load(Ordering::SeqCst),
            2,
            "the download agent pooled a connection, so its stall bound is not in force"
        );
    }

    /// A download that stalls part-way must not leave the partial installer
    /// behind.
    ///
    /// `cleanup_stale_downloads` would eventually collect it, but only at the
    /// *next* startup -- and this path stopped being near-unreachable when the
    /// bound became a 15s stall rather than a 600s total, so on a flaky link
    /// every retry now leaves another partial file in the cache directory for
    /// the rest of the session. The signature-failure branch already cleaned
    /// up after itself; this is the same courtesy on the branch that got
    /// common.
    ///
    /// Stalls the body rather than the head on purpose: a failure before the
    /// response arrives never reaches `File::create` and so proves nothing
    /// about the file. The stall bound here is the test's own 1s, not the
    /// production 15s, so this costs about a second.
    #[test]
    fn a_stalled_download_leaves_no_partial_installer_behind() {
        use std::io::{Read as _, Write as _};
        use std::net::{TcpListener, TcpStream};

        fn read_head(stream: &mut TcpStream) -> bool {
            let mut seen = Vec::new();
            let mut byte = [0u8; 1];
            while stream.read(&mut byte).unwrap_or(0) == 1 {
                seen.push(byte[0]);
                if seen.ends_with(b"\r\n\r\n") {
                    return true;
                }
            }
            false
        }

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_head(&mut stream);
            // Promise a megabyte, send ten bytes, then hold the socket open
            // and silent: a stalled transfer, not a closed one.
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\n\r\n0123456789");
            let _ = stream.flush();
            std::thread::sleep(Duration::from_secs(10));
        });

        let dir = scratch_dir("partial");
        let release = ReleaseInfo {
            version: Version::parse("9.9.9").unwrap(),
            installer_download_url: format!("http://127.0.0.1:{port}/installer.exe"),
        };
        let agent =
            crate::http_agent::bounded_stall(Duration::from_secs(1), Duration::from_secs(1));

        let result = download_and_verify(&release, "irrelevant-thumbprint", &dir, &agent);

        assert!(result.is_err(), "a transfer that stopped moving must not look like success");
        let partial = dir.join(installer_file_name(&release.version));
        assert!(
            !partial.exists(),
            "a stalled download left {} on disk; it would sit there until the next startup",
            partial.display()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `apply_update` starts a process, so every test of it has to be certain
    /// it cannot start one. These two are: the placeholder thumbprint can
    /// never match a real signature, and neither of the files below is a
    /// signed PE in the first place, so `verify_authenticode` fails before the
    /// spawn is reached on either path.
    #[test]
    fn apply_update_refuses_an_installer_it_cannot_verify() {
        let dir = scratch_dir("apply-unverifiable");
        let release = ReleaseInfo {
            version: Version::parse("9.9.9").unwrap(),
            installer_download_url: String::new(),
        };
        std::fs::write(dir.join(installer_file_name(&release.version)), b"not a signed PE").unwrap();

        let result = apply_update(&dir, &release);

        assert!(result.is_err(), "an installer that fails verification must not be launched");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The narrowing itself, asserted rather than left to the doc comment: the
    /// caller hands over a directory and a release, and the file that would be
    /// launched is the one `download_and_verify` writes -- not one the caller
    /// named.
    ///
    /// A directory holding a *differently* named executable therefore has
    /// nothing in it `apply_update` will touch, and the error says which file
    /// it looked for.
    #[test]
    fn apply_update_launches_only_the_file_the_download_pass_wrote() {
        let dir = scratch_dir("apply-constructs");
        let release = ReleaseInfo {
            version: Version::parse("9.9.9").unwrap(),
            installer_download_url: String::new(),
        };
        // A plausible decoy, sitting right beside where the real one would go.
        std::fs::write(dir.join("setup.exe"), b"not a signed PE").unwrap();

        let error = apply_update(&dir, &release).expect_err("nothing here is verifiable");

        let wanted = dir.join(installer_file_name(&release.version));
        assert!(
            error.contains(&wanted.display().to_string()),
            "apply_update reported {error}, which does not name {}; it is not constructing the \
             path from the release",
            wanted.display()
        );
        assert!(
            !error.contains("setup.exe"),
            "apply_update went looking at a file the caller merely left lying around: {error}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The signer is a constant, not an argument, so no call site can weaken
    /// it. If this ever becomes a parameter again, this stops compiling.
    #[test]
    fn the_launch_time_signer_check_is_not_something_a_caller_supplies() {
        assert_eq!(EXPECTED_SIGNER_THUMBPRINT, "PLACEHOLDER_SET_ONCE_SIGNPATH_CERT_ISSUED");
        // And the shape is pinned by the type system rather than by prose: a
        // function pointer of exactly this signature. If `apply_update` grows
        // back a `&Path` for the installer, or a caller-supplied thumbprint,
        // this line stops compiling.
        let narrowed: fn(&Path, &ReleaseInfo) -> Result<(), String> = apply_update;
        let _ = narrowed;
    }

    /// The decision [`apply_update`] makes immediately before it spawns, over
    /// signatures built by hand -- the only way to reach it, since a test that
    /// got there through a real file would need a real signed installer.
    #[test]
    fn the_launch_time_trust_decision_is_the_expected_signer() {
        use crate::signature::SignatureInfo;
        let info = |valid, thumbprint: Option<&str>| SignatureInfo {
            valid,
            thumbprint: thumbprint.map(str::to_string),
            subject_dn: Some("CN=Deskwarden".to_string()),
        };

        // The one accepted case: a valid signature carrying exactly the
        // constant, case-insensitively (thumbprints are hex).
        assert!(installer_is_launchable(&info(true, Some(EXPECTED_SIGNER_THUMBPRINT))));
        assert!(installer_is_launchable(&info(
            true,
            Some(&EXPECTED_SIGNER_THUMBPRINT.to_ascii_lowercase())
        )));

        // A valid signature by SOMEONE ELSE is the case that matters: it is
        // what a planted installer would carry, and it is what a check against
        // a caller-supplied thumbprint would have accepted.
        assert!(!installer_is_launchable(&info(true, Some("AABBCCDDEEFF00112233445566778899"))));
        // ...and an invalid or absent signature, whoever it names.
        assert!(!installer_is_launchable(&info(false, Some(EXPECTED_SIGNER_THUMBPRINT))));
        assert!(!installer_is_launchable(&info(true, None)));
        assert!(!installer_is_launchable(&info(false, None)));
    }

    /// Pins the two production numbers against the failure each was chosen to
    /// avoid, rather than leaving them to be re-tuned by feel.
    ///
    /// The 600s whole-request deadline this replaced is the reason: it pinned
    /// the tray's "Updating to vX" state -- and swallowed repeat clicks
    /// (`main.rs`) -- for ten minutes on a stalled download. A no-progress
    /// bound has to stay short enough that the tray recovers on a human
    /// timescale.
    #[test]
    fn the_download_stall_bound_stays_short_enough_for_the_tray_to_recover() {
        assert!(
            DOWNLOAD_STALL_TIMEOUT <= Duration::from_secs(60),
            "a stalled download must not pin the tray label for minutes"
        );
        // And long enough that a brief hiccup on a slow link isn't mistaken
        // for a dead transfer.
        assert!(DOWNLOAD_STALL_TIMEOUT >= Duration::from_secs(10));
        // The API check is bounded by *total* time, the download by time
        // without progress -- different quantities, so this is not a
        // "one must exceed the other" ordering claim. It is only a guard that
        // the two never collapse back into one shared number, which is the
        // arrangement that produced the inert setting in the first place.
        assert_ne!(API_DEADLINE, DOWNLOAD_STALL_TIMEOUT);
    }

    // ---------------------------------------------------------------------
    // THE GATE, AS ROUTING
    //
    // Everything above pins what the launcher COMPUTES. This section pins
    // that the launcher ASKS -- the property the previous shape of this
    // module disclosed as untestable and that a measured one-line
    // neutralisation (`let _launchable = installer_is_launchable(&info);`)
    // walked straight through at 2168 passed / 0 failed / 0 warnings.
    //
    // No fixture, no network, no signed file, no process: the harness hands
    // `apply_update_with` a `SignatureInfo` built by hand and records every
    // path that reaches the launch seam.
    // ---------------------------------------------------------------------

    use crate::signature::SignatureInfo;
    use std::sync::{Mutex, MutexGuard};

    /// What the substitute seam saw, for the whole PROCESS.
    ///
    /// # This used to be `thread_local!`, and that was the defect
    ///
    /// The previous shape of this harness kept `VERIFY_ANSWER`, `VERIFIED`
    /// and `LAUNCHED` in `thread_local!` storage and its doc called that a
    /// feature: "per-test state with no ordering assumption". It was a hole,
    /// and a measured one. Inserted into [`apply_update_with`] immediately
    /// BEFORE the `verify` call:
    ///
    /// ```ignore
    /// let zz_p = installer_path.clone();
    /// let zz_l = env.launch;
    /// let _ = std::thread::Builder::new().spawn(move || zz_l(&zz_p));
    /// ```
    ///
    /// -- which in production starts the real installer, unverified and
    /// ungated, on a background thread -- SURVIVED the entire suite at 2182
    /// lib / 217 bin / 0 failed and ZERO warnings. The launch happened on a
    /// thread that was not the test's, so every one of the six routing tests
    /// read an empty thread-local recorder and passed. The liveness control
    /// at the identical site, the same three lines with `let _ = zz_l(&zz_p);`
    /// in place of the `Builder` wrapper, was KILLED by all six at 2176/6.
    /// The ONLY difference between the survivor and the kill was which thread
    /// the call was on.
    ///
    /// So the recorder is global. **A launch on any thread in this process,
    /// however that thread was created, is written here.** The seam is a bare
    /// `fn` pointer by design (see [`UpdaterEnv`]) and cannot capture, so a
    /// `static` is the only place it can write; the per-test isolation the
    /// thread-locals used to give is now supplied by [`ROUTE_LOCK`] instead,
    /// which is a stronger property because it also serialises the
    /// harness-owned threads a mutant might create.
    struct Recorder {
        /// Which routing window is open -- or, on the odd bumps
        /// [`Session::drop`] makes, that NO window is open at all.
        ///
        /// # This tag used to be write-only, and that made the suite lie
        ///
        /// It was stamped onto every entry below and read by nothing:
        /// `Session::launched()`/`verified()` were `.map(|(_, p)| p.clone())`
        /// over the whole vector, and [`assert_no_late_launch`] only asked
        /// whether that vector was empty. Measured on `0cd9fe0`, replacing
        /// `let generation = r.generation;` with `let generation = 0;` in
        /// [`record_launch`] SURVIVED at 2192 / 0 failed / 0 warnings, while
        /// the liveness control at the identical statement
        /// (`r.launched.push((generation, path.to_path_buf()));` becoming
        /// `let _ = (generation, path);`) was KILLED at 2188/4. The second
        /// tuple element was load-bearing; the first was inert.
        ///
        /// The doc that used to stand here claimed the stamp "is what makes
        /// it red that window's assertions instead of vanishing". That was
        /// exactly backwards, and it cost this suite an intermittent red whose
        /// message was a FALSE ALARM about code signing. A launch from the
        /// detached-thread witness that misses its own [`Session::settle`]
        /// lands after `Session::drop` has cleared, after the NEXT
        /// `Session::open` has cleared, and is therefore stamped with the new
        /// window's generation -- so
        /// [`a_valid_signature_by_the_wrong_signer_never_reaches_the_launch_seam`]
        /// reported that an installer validly signed by someone else had
        /// reached the launch seam, in a run where nothing of the sort
        /// happened. Measured over 30 isolated `updater::` runs under
        /// concurrent compilation: 6 red, across three different victims.
        ///
        /// So the tag is READ now, in two places that between them leave a
        /// stray nowhere to be silently attributed:
        ///
        ///  * [`Session::launched`] and [`Session::verified`] go through
        ///    [`entries_of_window`], which panics -- naming the WINDOW, not
        ///    the signature -- on any entry stamped with a different one.
        ///  * [`Session::drop`] bumps the generation to a value no session
        ///    owns before it releases [`ROUTE_LOCK`], and waits out
        ///    [`CLOSE_GRACE`] while still holding it. A launch arriving in
        ///    that band carries the unowned generation and is left in place,
        ///    so the next [`Session::open`]'s emptiness assertion says so. It
        ///    is no longer erased, and it can no longer be inherited.
        generation: u64,
        /// What the substitute `verify` answers on its next call.
        answer: Option<Result<SignatureInfo, String>>,
        /// Every path the substitute `verify` was asked about.
        verified: Vec<(u64, PathBuf)>,
        /// Every path that reached the launch seam. **If this is ever
        /// non-empty when it should be empty, an unverified installer would
        /// have been started for real.**
        launched: Vec<(u64, PathBuf)>,
    }

    static RECORDER: Mutex<Recorder> = Mutex::new(Recorder {
        generation: 0,
        answer: None,
        verified: Vec::new(),
        launched: Vec::new(),
    });

    /// Held for the whole of one routing window, so exactly one window is
    /// open at a time and the global recorder above is unambiguous.
    static ROUTE_LOCK: Mutex<()> = Mutex::new(());

    /// Poisoning is recovered from deliberately: a routing test that fails
    /// panics while a `Session` is alive, and a poisoned recorder would then
    /// turn one real failure into five misleading ones.
    fn recorder() -> MutexGuard<'static, Recorder> {
        RECORDER.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The one write that says "a process would have started here".
    fn record_launch(path: &Path) {
        let mut r = recorder();
        let generation = r.generation;
        r.launched.push((generation, path.to_path_buf()));
    }

    fn substitute_verify(path: &Path) -> Result<SignatureInfo, String> {
        {
            let mut r = recorder();
            let generation = r.generation;
            r.verified.push((generation, path.to_path_buf()));
        }
        recorder()
            .answer
            .take()
            .expect("the test did not program a verify answer")
    }

    fn substitute_launch(path: &Path) -> Result<(), String> {
        record_launch(path);
        Ok(())
    }

    /// Panics if anything reached the seam AFTER the window that caused it
    /// had closed.
    ///
    /// Free rather than written inline in [`Session::open`] so that it can
    /// have a liveness control of its own. Nothing in this suite legitimately
    /// leaves a late launch behind, so an assertion written inline here is
    /// INERT -- and measurably so: neutralising it to `true || ..` survived
    /// the whole suite at 2188 / 217 / 0 failed / 0 warnings. A backstop with
    /// no control is a backstop nobody knows the shape of, so it is a
    /// function with a `#[should_panic]` test on it instead.
    fn assert_no_late_launch(launched: &[(u64, PathBuf)]) {
        assert!(
            launched.is_empty(),
            "a launch reached the seam AFTER the routing window that caused it had \
             closed: {launched:?}. In production that is a process start no assertion \
             was looking at"
        );
    }

    /// The directory every routing window hands to `apply_update_with`, TAGGED
    /// WITH THE WINDOW. Never created, never touched: nothing on the routing
    /// path reads the disk.
    ///
    /// The tag is what makes a late launch attributable, and it is the second
    /// half of the fix -- the generation STAMP alone is not enough. A stamp is
    /// read at the moment of recording, so a launch caused by window N but
    /// landing after window N+1 has opened is stamped N+1 and looks native.
    /// Measured: with the stamp alone, a detached launch delayed 900ms (past
    /// both [`SETTLE_QUIET`] and [`CLOSE_GRACE`]) still landed in the next
    /// window and reported a doubled launch vector there.
    ///
    /// The PATH, by contrast, is built by the window that caused the launch
    /// and travels with it. So a stray names the window it came from however
    /// late it arrives, and [`entries_of_window`] can say so.
    const ROUTING_DIR_PREFIX: &str = r"Z:\deskwarden-routing-test-never-created-";

    fn routing_dir(window: u64) -> PathBuf {
        PathBuf::from(format!("{ROUTING_DIR_PREFIX}{window}"))
    }

    /// Which window built `p`, for a path that came out of [`routing_dir`].
    /// `None` for anything else -- a real production path carries no tag, and
    /// then the generation stamp is all there is.
    fn window_of_path(p: &Path) -> Option<u64> {
        let s = p.to_string_lossy().into_owned();
        let rest = s.strip_prefix(ROUTING_DIR_PREFIX)?;
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    }

    /// Every path recorded under `window`, and a LOUD, SPECIFIC failure for
    /// anything that belongs to a different one -- by its generation stamp, or
    /// by the window its own path names.
    ///
    /// This is the read that makes [`Recorder::generation`] more than
    /// decoration. A stray cannot be quietly folded into the window that
    /// happens to be open when it lands: the panic names the window and says
    /// what went wrong, so the run is red for the reason it is actually red
    /// rather than red about code signing.
    fn entries_of_window(entries: &[(u64, PathBuf)], window: u64, what: &str) -> Vec<PathBuf> {
        let strays: Vec<&(u64, PathBuf)> = entries
            .iter()
            .filter(|(g, p)| *g != window || window_of_path(p).is_some_and(|w| w != window))
            .collect();
        assert!(
            strays.is_empty(),
            "a {what} from a previous window arrived late: {strays:?}, read inside routing \
             window {window}. It is NOT this window's -- attributing it here is how a stray \
             detached launch used to red an unrelated signature assertion with a message that \
             was a false alarm. Whatever else this run reports, something reached the seam \
             after the window that caused it had closed"
        );
        entries.iter().map(|(_, p)| p.clone()).collect()
    }

    /// How long the recorder must go unchanged before [`Session::settle`]
    /// calls a window settled.
    ///
    /// # This number is the witness, so it is pinned rather than felt
    ///
    /// The shape this replaced counted "12 consecutive unchanged 10ms polls",
    /// i.e. it could return as early as **120ms** -- while its own doc claimed
    /// the budget was 600ms, understating the gap by 5x. Nothing pinned it:
    /// measured on `0cd9fe0`, `if stable >= 12` -> `if stable >= 0` (still one
    /// 10ms sleep) SURVIVED at 2192 / 0 failed / 0 warnings. Only "at least
    /// one poll" was held by anything.
    ///
    /// 120ms was also not enough. Under concurrent compilation a detached
    /// thread routinely does not get scheduled inside it, which is what made
    /// the witness miss and the stray contaminate the next window.
    /// [`the_settle_window_waits_out_a_launch_it_did_not_start`] pins this
    /// behaviourally against a launch delayed well past the old budget, and
    /// [`the_settle_budget_is_not_a_token_one`] pins the number itself so a
    /// shrink is caught even on a lucky machine.
    const SETTLE_QUIET: Duration = Duration::from_millis(500);

    /// Poll interval for [`Session::settle`].
    const SETTLE_POLL: Duration = Duration::from_millis(10);

    /// Total bound on [`Session::settle_witnessing`]. Generous on purpose: it
    /// is not a budget anything is expected to approach, it is the point at
    /// which a witness that will never arrive stops pretending to wait.
    const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

    /// How long a closing window keeps [`ROUTE_LOCK`] after retiring its own
    /// generation, so that a launch still in flight lands under a generation
    /// NO session owns rather than under the next window's.
    const CLOSE_GRACE: Duration = Duration::from_millis(120);

    /// One routing window.
    ///
    /// Opening one takes [`ROUTE_LOCK`], asserts the recorder is EMPTY --
    /// anything in it arrived after the previous window closed, which is
    /// itself a launch nobody witnessed in time -- then bumps the generation
    /// and installs the programmed `verify` answer. Dropping one retires that
    /// generation, waits out [`CLOSE_GRACE`] still holding the lock, and only
    /// then releases it.
    struct Session {
        /// The generation this window owns. Everything it reads must carry
        /// this number; see [`entries_of_window`].
        generation: u64,
        _serial: MutexGuard<'static, ()>,
    }

    impl Session {
        fn open(answer: Option<Result<SignatureInfo, String>>) -> Self {
            let serial = ROUTE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let mut r = recorder();
            assert_no_late_launch(&r.launched);
            r.generation += 1;
            let generation = r.generation;
            r.answer = answer;
            r.verified.clear();
            r.launched.clear();
            drop(r);
            Session { generation, _serial: serial }
        }

        fn launched(&self) -> Vec<PathBuf> {
            entries_of_window(&recorder().launched, self.generation, "launch")
        }

        fn verified(&self) -> Vec<PathBuf> {
            entries_of_window(&recorder().verified, self.generation, "verify")
        }

        /// Wait for the recorder to stop changing before it is read.
        ///
        /// A launch on a thread the code under test created lands after
        /// `apply_update_with` has already returned, so reading the recorder
        /// the instant the call finishes would still miss it. This waits for
        /// [`SETTLE_QUIET`] of no change.
        ///
        /// **What this does not see, said plainly:** a launch delayed past
        /// [`SETTLE_QUIET`] after the last change. That is no longer a launch
        /// that VANISHES, though, which is the part that used to matter: it
        /// lands under a generation no window owns (see [`Session::drop`]) and
        /// reds the next [`Session::open`] with a message about a late launch,
        /// rather than being inherited by the next window and reported as a
        /// signature failure.
        fn settle(&self) {
            let mut last = recorder().launched.len();
            let mut quiet_since = std::time::Instant::now();
            while quiet_since.elapsed() < SETTLE_QUIET {
                std::thread::sleep(SETTLE_POLL);
                let now = recorder().launched.len();
                if now != last {
                    last = now;
                    quiet_since = std::time::Instant::now();
                }
            }
        }

        /// [`Session::settle`] for a window that is EXPECTING a launch: waits
        /// until at least `n` have been recorded, then for the usual quiet.
        ///
        /// A witness that can time out has to fail loudly when it does. The
        /// old settle could not: it returned quietly after ~120ms whether or
        /// not the launch it existed to witness had arrived, and the caller's
        /// `assert_eq!` then reported an empty vector -- which reads as "no
        /// launch happened", the opposite of what had happened. This panics,
        /// and says which.
        fn settle_witnessing(&self, n: usize) {
            let started = std::time::Instant::now();
            while recorder().launched.len() < n {
                assert!(
                    started.elapsed() < SETTLE_DEADLINE,
                    "the settle window timed out after {:?} still waiting for launch {n} of \
                     this routing window; only {} arrived. This is the witness FAILING, not \
                     the absence of a launch -- do not read it as one",
                    SETTLE_DEADLINE,
                    recorder().launched.len()
                );
                std::thread::sleep(SETTLE_POLL);
            }
            self.settle();
        }
    }

    /// What closing a routing window does to the recorder, as a function so
    /// [`a_closing_window_retires_its_generation_and_keeps_only_foreign_entries`]
    /// can drive it. A `Drop` body no test can call is a `Drop` body nothing
    /// knows the shape of, which is the same mistake
    /// [`assert_no_late_launch`] was pulled out of an inline assertion to
    /// avoid.
    fn retire_window(r: &mut Recorder, generation: u64) {
        r.answer = None;
        // Only THIS window's entries are cleared. A stray carrying another
        // generation is left where it is, for `Session::open`'s emptiness
        // assertion to find -- clearing it here is precisely what used to
        // erase the evidence.
        r.verified.retain(|(g, _)| *g != generation);
        r.launched.retain(|(g, _)| *g != generation);
        // Retire the generation. From here until the next `open` the recorder
        // stamps entries no session will ever claim.
        r.generation += 1;
    }

    impl Drop for Session {
        fn drop(&mut self) {
            {
                let mut r = recorder();
                retire_window(&mut r, self.generation);
            }
            // `_serial` is a field, so it is dropped AFTER this body: the lock
            // is still held here. A launch still in flight therefore lands in
            // this grace period, under the retired generation, and cannot be
            // swallowed by the next window's clear.
            std::thread::sleep(CLOSE_GRACE);
        }
    }

    impl UpdaterEnv {
        /// The test-only substitute.
        ///
        /// An inherent impl written from `mod tests` rather than a
        /// test-gated method beside [`UpdaterEnv::production`], for the
        /// reason `vault_window`'s seam records: every source guard in a file
        /// cuts its production slice at the FIRST test gate in the text, so a
        /// gated item up beside `production` would truncate the slice
        /// [`production_is_the_only_updater_env_a_shipping_build_has`] and
        /// [`the_only_process_start_in_this_module_is_the_launch_seam`] read,
        /// and blind both of them to everything below it.
        fn substitute(
            verify: fn(&Path) -> Result<SignatureInfo, String>,
            launch: fn(&Path) -> Result<(), String>,
        ) -> Self {
            Self { verify, launch }
        }
    }

    /// Everything one routing window observed.
    struct Routed {
        result: Result<(), String>,
        launched: Vec<PathBuf>,
        verified: Vec<PathBuf>,
        /// The installer path THIS window's directory produces -- what a
        /// correct launch must equal. Carried out of the window rather than
        /// recomputed by the caller, because the window number is part of it.
        expected: PathBuf,
    }

    /// Runs `apply_update_with` against the recording seam, with `verify`
    /// programmed to answer `answer`, and reports every path that reached
    /// either half of the seam ON ANY THREAD.
    fn route_recording(answer: Result<SignatureInfo, String>) -> Routed {
        let session = Session::open(Some(answer));

        // A directory that does not exist and is never created: nothing on
        // this path may touch the disk, because nothing on this path reads the
        // disk any more. Tagged with the window, so that whatever comes back
        // out of the seam names the window that sent it in.
        let dir = routing_dir(session.generation);
        let release = ReleaseInfo {
            version: Version::parse("9.9.9").unwrap(),
            installer_download_url: String::new(),
        };
        let env = UpdaterEnv::substitute(substitute_verify, substitute_launch);
        let result = apply_update_with(&dir, &release, &env);
        session.settle();
        Routed {
            result,
            launched: session.launched(),
            verified: session.verified(),
            expected: routed_path(session.generation),
        }
    }

    /// [`route_recording`] for the cases that only care about `launch`.
    fn route(answer: Result<SignatureInfo, String>) -> (Result<(), String>, Vec<PathBuf>) {
        let routed = route_recording(answer);
        (routed.result, routed.launched)
    }

    /// A `SignatureInfo` as `verify_authenticode` would return one.
    fn signature(valid: bool, thumbprint: Option<&str>) -> SignatureInfo {
        SignatureInfo {
            valid,
            thumbprint: thumbprint.map(str::to_string),
            subject_dn: Some("CN=Deskwarden".to_string()),
        }
    }

    /// The path a routing window builds, for the assertions below to compare
    /// against. Takes the window, because the window is IN the path -- see
    /// [`ROUTING_DIR_PREFIX`].
    fn routed_path(window: u64) -> PathBuf {
        routing_dir(window).join(installer_file_name(&Version::parse("9.9.9").unwrap()))
    }

    /// **The gate is consulted: a valid signature by SOMEONE ELSE is never
    /// launched.**
    ///
    /// This is the case that matters, because it is what a planted installer
    /// carries -- a real, valid Authenticode signature, by a signer that is
    /// not ours. `verify` succeeds, `installer_is_launchable` says no, and the
    /// question is whether anything downstream cares. Deleting the `if`, or
    /// neutralising it to a `let _`, makes `LAUNCHED` non-empty here.
    #[test]
    fn a_valid_signature_by_the_wrong_signer_never_reaches_the_launch_seam() {
        let (result, launched) =
            route(Ok(signature(true, Some("AABBCCDDEEFF00112233445566778899"))));

        assert!(
            launched.is_empty(),
            "an installer validly signed by someone other than the expected signer reached the \
             launch seam ({launched:?}); in production that is a process start"
        );
        let error = result.expect_err("a wrong-signer installer must not be a success");
        assert!(
            error.contains("not signed by the expected signer"),
            "the refusal does not say why: {error}"
        );
    }

    /// An INVALID signature naming the right signer is not launched either --
    /// a thumbprint match is not on its own a verdict.
    #[test]
    fn an_invalid_signature_never_reaches_the_launch_seam() {
        let (result, launched) = route(Ok(signature(false, Some(EXPECTED_SIGNER_THUMBPRINT))));

        assert!(
            launched.is_empty(),
            "an installer whose signature the OS rejected reached the launch seam ({launched:?})"
        );
        assert!(result.is_err());
    }

    /// No thumbprint at all -- an unsigned file, or one whose certificate
    /// could not be read -- is not launched.
    #[test]
    fn a_missing_thumbprint_never_reaches_the_launch_seam() {
        let (result, launched) = route(Ok(signature(true, None)));

        assert!(
            launched.is_empty(),
            "an installer carrying no signer thumbprint reached the launch seam ({launched:?})"
        );
        assert!(result.is_err());
    }

    /// `verify` returning `Err` -- the answer is UNKNOWN, not "untrusted" --
    /// is also a refusal, and the failure is propagated rather than swallowed
    /// into a default verdict.
    ///
    /// This is the mutant shaped as "the result is ignored rather than
    /// unused": a body that turns the `?` into an `unwrap_or(..)` of a
    /// fabricated trusted `SignatureInfo` still USES `verify`, still USES
    /// `installer_is_launchable`, and warns about nothing.
    #[test]
    fn an_unverifiable_installer_never_reaches_the_launch_seam() {
        let (result, launched) = route(Err("the file could not be read as a signed object".into()));

        assert!(
            launched.is_empty(),
            "an installer that could not be verified at all reached the launch seam ({launched:?})"
        );
        let error = result.expect_err("an unknown verdict is not a success");
        assert!(
            error.contains("the file could not be read as a signed object"),
            "the verifier's own failure was swallowed: {error}"
        );
    }

    /// **The counterpart, without which every assertion above is vacuous:**
    /// the trusted case IS launched, and with exactly the path the module
    /// constructed -- not one the caller named, and not some other file in the
    /// same directory.
    ///
    /// A gate that refuses everything passes the four tests above and is a
    /// broken updater. A launcher that launches a DIFFERENT path than the one
    /// it verified passes them too, and is the swap the re-verification exists
    /// to close; the path equality here is what says the file that was
    /// checked is the file that runs.
    #[test]
    fn the_trusted_installer_is_launched_and_it_is_the_file_that_was_verified() {
        let Routed { result, launched, verified, expected } =
            route_recording(Ok(signature(true, Some(EXPECTED_SIGNER_THUMBPRINT))));

        assert!(result.is_ok(), "the trusted installer was refused: {result:?}");
        assert_eq!(
            launched,
            vec![expected],
            "the launch seam did not receive exactly the one path the module constructed"
        );
        assert_eq!(
            verified, launched,
            "the file that was VERIFIED is not the file that was LAUNCHED; the gap between \
             those two paths is exactly where a swap goes"
        );
    }

    /// The gate is case-insensitive on the thumbprint (they are hex), the same
    /// way [`the_launch_time_trust_decision_is_the_expected_signer`] says --
    /// asserted here through the ROUTING rather than over the predicate.
    #[test]
    fn the_trusted_thumbprint_is_matched_case_insensitively_through_the_gate() {
        let Routed { result, launched, expected, .. } = route_recording(Ok(signature(
            true,
            Some(&EXPECTED_SIGNER_THUMBPRINT.to_ascii_lowercase()),
        )));

        assert!(result.is_ok(), "a lower-cased thumbprint was refused: {result:?}");
        assert_eq!(launched, vec![expected]);
    }

    /// **A launch on a thread this test did not create is still witnessed.**
    ///
    /// Without this, every `launched.is_empty()` above is a claim about ONE
    /// thread rather than about the process, which is exactly the hole the
    /// `std::thread::Builder::new().spawn(move || zz_l(&zz_p))` mutant walked
    /// through at a full 2182 / 0 failed / 0 warnings. This is the control
    /// that says the new recorder does not have that shape: the thread here
    /// is created by the harness rather than by production code, but the
    /// recorder cannot tell the difference and that is the point.
    #[test]
    fn a_launch_on_a_thread_the_test_does_not_own_is_witnessed() {
        let session = Session::open(None);
        let path = routed_path(session.generation);
        let handle = std::thread::Builder::new()
            .spawn(move || {
                let _ = substitute_launch(&path);
            })
            .expect("could not start the witness thread");
        handle.join().expect("the witness thread panicked");
        session.settle_witnessing(1);

        assert_eq!(
            session.launched(),
            vec![routed_path(session.generation)],
            "the recorder did not witness a launch made on another thread, so every \
             `launched.is_empty()` assertion in this module is a claim about one thread \
             rather than about this process"
        );
    }

    /// The same, DETACHED and never joined, and landing after the call that
    /// started it has already returned -- the exact shape of the survivor.
    /// [`Session::settle_witnessing`] is what closes the gap.
    ///
    /// This test used to be the suite's own flake. It waited on a plain
    /// `settle()` that returned after ~120ms whether or not the thread had run
    /// yet, and under concurrent compilation it often had not: the assertion
    /// then read an empty vector, and -- worse -- the launch landed later, in
    /// somebody else's window. `settle_witnessing` waits for the thing it is
    /// witnessing and fails as a TIMEOUT if it never comes, so this test now
    /// only goes red for its own reason.
    #[test]
    fn a_launch_on_a_detached_thread_is_witnessed_by_the_settle_window() {
        let session = Session::open(None);
        let path = routed_path(session.generation);
        let _ = std::thread::Builder::new().spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(5));
            let _ = substitute_launch(&path);
        });
        session.settle_witnessing(1);

        assert_eq!(
            session.launched(),
            vec![routed_path(session.generation)],
            "a launch on a detached thread was not witnessed inside the settle window"
        );
    }

    /// **The settle window is wide enough to be a witness.**
    ///
    /// [`a_launch_on_a_detached_thread_is_witnessed_by_the_settle_window`]
    /// waits for its launch, so it says nothing about the budget. This one
    /// does not wait: it calls the PLAIN [`Session::settle`] -- the one
    /// `route_recording` uses, and therefore the one that has to catch a
    /// mutant which slips a detached spawn into `apply_update_with` -- against
    /// a launch delayed past the 120ms the old shape actually allowed.
    ///
    /// With `SETTLE_QUIET` shrunk to anything under the delay below, this
    /// fails.
    #[test]
    fn the_settle_window_waits_out_a_launch_it_did_not_start() {
        let session = Session::open(None);
        let path = routed_path(session.generation);
        let _ = std::thread::Builder::new().spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let _ = substitute_launch(&path);
        });
        session.settle();

        assert_eq!(
            session.launched(),
            vec![routed_path(session.generation)],
            "a launch delayed 200ms was not witnessed by the plain settle window, so a mutant \
             that puts the real launch on a detached thread would be recorded by nobody in \
             `route_recording` -- which is the exact survivor this harness exists to kill"
        );
    }

    /// **A launch that lands well past the quiet window is still witnessed,
    /// because the witness WAITS for it.**
    ///
    /// The band this covers is the one the old shape lost things in: after
    /// `settle` would have given up, and before `Session::drop` clears. A
    /// launch landing there used to be erased outright and then blamed on the
    /// next window. Here the delay is deliberately longer than
    /// [`SETTLE_QUIET`], so a [`Session::settle_witnessing`] that stopped
    /// waiting for its count -- and merely settled -- would miss it and this
    /// would go red.
    #[test]
    fn a_launch_landing_past_the_quiet_window_is_still_waited_for() {
        let session = Session::open(None);
        let path = routed_path(session.generation);
        let _ = std::thread::Builder::new().spawn(move || {
            std::thread::sleep(SETTLE_QUIET + Duration::from_millis(400));
            let _ = substitute_launch(&path);
        });
        session.settle_witnessing(1);

        assert_eq!(
            session.launched(),
            vec![routed_path(session.generation)],
            "a launch that landed after the quiet window expired was not waited for. It is \
             not lost quietly any more, but a witness that gives up on the thing it is \
             witnessing is not a witness"
        );
    }

    /// And the number itself, so a shrink is caught on a machine lucky enough
    /// to schedule the thread above immediately.
    #[test]
    fn the_settle_budget_is_not_a_token_one() {
        assert!(
            SETTLE_QUIET >= Duration::from_millis(400),
            "the settle window's quiet period is {SETTLE_QUIET:?}. The shape this replaced \
             returned after 120ms while its doc claimed 600ms, and `if stable >= 12` -> \
             `if stable >= 0` was measured SURVIVING the whole suite: the budget was pinned \
             by nothing at all"
        );
        assert!(
            SETTLE_POLL <= Duration::from_millis(20),
            "the poll interval is coarser than the window it is sampling"
        );
        assert!(
            CLOSE_GRACE >= Duration::from_millis(100),
            "a closing window must hold the route lock long enough that an in-flight launch \
             lands under the RETIRED generation rather than under the next window's"
        );
        assert!(
            SETTLE_DEADLINE >= Duration::from_secs(5),
            "a witness that gives up in seconds is a flake, not a witness"
        );
    }

    /// **A launch that missed its window is named as a LATE LAUNCH, never
    /// counted as this window's.**
    ///
    /// This is the finding, reduced to an assertion. When the generation tag
    /// was write-only, a stray detached launch that landed after the recorder
    /// had been cleared was read as the NEXT window's launch -- and the next
    /// window happened to be
    /// [`a_valid_signature_by_the_wrong_signer_never_reaches_the_launch_seam`],
    /// so a run in which nothing was mis-signed reported that an installer
    /// validly signed by someone else had reached the launch seam. Here that
    /// is a different failure with a different message.
    ///
    /// Pure over the tag, so it is deterministic and touches no window: the
    /// payload of every launch in this module is a recorded no-op, never a
    /// real spawn.
    #[test]
    #[should_panic(expected = "arrived late")]
    fn a_launch_from_a_previous_window_is_never_attributed_to_this_one() {
        let entries = vec![(6, PathBuf::from(r"Z:\late-installer.exe"))];
        let _ = entries_of_window(&entries, 7, "launch");
    }

    /// **And the half the generation stamp cannot see: a launch whose STAMP
    /// says this window but whose PATH names the one before it.**
    ///
    /// This is precisely the shape of a launch caused by window N and landing
    /// after window N+1 has opened -- the stamp is read at recording time, so
    /// it says N+1 and looks native. Measured with the stamp alone in place: a
    /// detached launch delayed 900ms (past both `SETTLE_QUIET` and
    /// `CLOSE_GRACE`) landed in the next window and was reported there as that
    /// window's own doubled launch. The path tag is what tells them apart.
    #[test]
    #[should_panic(expected = "arrived late")]
    fn a_launch_whose_own_path_names_another_window_is_never_attributed_here() {
        let entries = vec![(7, routed_path(6))];
        let _ = entries_of_window(&entries, 7, "launch");
    }

    /// The control without which the two above are checks that always fire:
    /// entries carrying the open window's own generation are read normally, in
    /// order -- including one whose path carries this window's own tag, so the
    /// tag check is not simply refusing every tagged path.
    #[test]
    fn entries_of_the_open_window_are_read_in_order() {
        let entries = vec![
            (7, PathBuf::from(r"Z:\a.exe")),
            (7, PathBuf::from(r"Z:\b.exe")),
        ];
        assert_eq!(
            entries_of_window(&entries, 7, "launch"),
            vec![PathBuf::from(r"Z:\a.exe"), PathBuf::from(r"Z:\b.exe")]
        );
        assert!(entries_of_window(&[], 7, "launch").is_empty());
        assert_eq!(
            entries_of_window(&[(7, routed_path(7))], 7, "launch"),
            vec![routed_path(7)],
            "control: a path carrying this window's OWN tag was rejected, so the tag check \
             refuses everything and the assertion above it is vacuous"
        );
    }

    /// **A closing window retires its generation and clears only its OWN
    /// entries.**
    ///
    /// Both halves matter and both were absent. Clearing wholesale is what
    /// erased the evidence of a late launch; not retiring the generation is
    /// what let the next window inherit one. Everything here happens while the
    /// window is open -- so [`ROUTE_LOCK`] is held throughout and no other
    /// test can observe the scratch state -- and [`retire_window`] is the same
    /// code [`Session::drop`] runs, minus its sleep.
    #[test]
    fn a_closing_window_retires_its_generation_and_keeps_only_foreign_entries() {
        let session = Session::open(None);
        let owned = session.generation;
        let orphan = owned.wrapping_add(1_000);
        let left;
        let after;
        {
            let mut r = recorder();
            r.launched.push((owned, PathBuf::from(r"Z:\mine.exe")));
            r.launched.push((orphan, PathBuf::from(r"Z:\stray.exe")));
            retire_window(&mut r, owned);
            after = r.generation;
            left = r.launched.clone();
            // Restored before the lock is released, so this scratch state is
            // invisible to every other test.
            r.launched.clear();
            r.verified.clear();
        }
        drop(session);

        assert_eq!(
            left,
            vec![(orphan, PathBuf::from(r"Z:\stray.exe"))],
            "closing a window either kept its own entries or threw away a foreign one. \
             Throwing the foreign one away is how a late launch used to vanish without \
             anybody being told"
        );
        assert_ne!(
            after, owned,
            "a closing window left its own generation current, so a launch still in flight \
             lands under a generation that a window is about to claim"
        );
    }

    /// The liveness control for [`assert_no_late_launch`], which no other
    /// test in this file can reach: a well-behaved suite never leaves a late
    /// launch in the recorder, so the only way to know the check would fire
    /// is to hand it one.
    #[test]
    #[should_panic(expected = "AFTER the routing window")]
    fn a_late_launch_left_in_the_recorder_is_a_failure() {
        assert_no_late_launch(&[(7, PathBuf::from(r"Z:\late-installer.exe"))]);
    }

    // ---------------------------------------------------------------------
    // AND THE SEAM ITSELF
    //
    // A seam that is not pinned only moves the hole one level out: the tests
    // above observe what the HARNESS supplied, never what production supplies.
    // These two are what join them.
    // ---------------------------------------------------------------------

    /// **Both fields of the production [`UpdaterEnv`] are the real functions,
    /// compared BY ADDRESS.**
    ///
    /// The same hold `vault_window`'s
    /// `production_hands_the_window_the_real_functions` puts on its five spawn
    /// fields, and for the same measured reason: a wrapper written at module
    /// level -- `fn verify_when_enabled(p: &Path) -> Result<SignatureInfo,
    /// String> { if CHECKS_ENABLED { verify_authenticode(p) } else {
    /// Ok(trusted()) } }` -- still spells the real name, still leaves
    /// `production` defining nothing of its own, and is invisible to every
    /// routing test above, because those substitute this very pointer. It is a
    /// different address, so it fails here.
    ///
    /// What this does NOT cover, plainly: it says the pointer is the right
    /// FUNCTION, never what that function does. A hollowed-out
    /// `verify_authenticode` passes this and is `signature.rs`'s problem.
    ///
    /// And one profile caveat, measured rather than assumed: `fn_addr_eq`
    /// does not survive identical-code folding, so under this crate's release
    /// profile (`lto = true, codegen-units = 1`) a byte-identical twin of
    /// [`launch_installer`] compares EQUAL to it and would pass this pin. A
    /// probe crate with that profile measured exactly that; in debug, which is
    /// what `cargo test` builds and what every number in this file's ledger
    /// was measured under, the two are distinguished.
    #[test]
    fn production_holds_the_real_verify_and_the_real_launch() {
        let env = UpdaterEnv::production();

        // Typed `let`s rather than casts off the `fn` items, so each is a `fn`
        // POINTER of exactly the field's type before any address is taken: a
        // signature drift is a compile error here, not a silently different
        // address.
        let real_verify: fn(&Path) -> Result<SignatureInfo, String> = verify_authenticode;
        let real_launch: fn(&Path) -> Result<(), String> = launch_installer;

        assert!(
            std::ptr::fn_addr_eq(env.verify, real_verify),
            "`UpdaterEnv::production` hands the launcher something other than the real \
             `verify_authenticode`. A wrapper, a forwarder or a flag-gated pass-through still \
             SPELLS the name, and the routing tests cannot see it because they substitute this \
             pointer. This is the assertion it fails"
        );
        assert!(
            std::ptr::fn_addr_eq(env.launch, real_launch),
            "`UpdaterEnv::production` hands the launcher something other than the real \
             `launch_installer`"
        );

        // CONTROL: the comparison discriminates. A function of the right
        // SIGNATURE that is not the right function reads as different, so the
        // assertions above are not something every pair of `fn` pointers has.
        let decoy: fn(&Path) -> Result<(), String> = not_the_launcher;
        assert!(
            !std::ptr::fn_addr_eq(env.launch, decoy),
            "control: a different function of the same signature compares EQUAL to the \
             production launcher, so every assertion above is vacuous"
        );
        // ...and the real one really does compare equal to itself, so the
        // control is not passing because comparison always answers `false`.
        assert!(
            std::ptr::fn_addr_eq(real_launch, real_launch),
            "control: `fn_addr_eq` answers `false` for one function against itself"
        );
    }

    /// The decoy [`production_holds_the_real_verify_and_the_real_launch`]
    /// compares against: `launch`'s signature exactly, and nothing else.
    fn not_the_launcher(_: &Path) -> Result<(), String> {
        unreachable!("never called -- this exists to have an address")
    }

    // ---------------------------------------------------------------------
    // THE SOURCE GUARDS, AND WHAT ANCHORS THEM
    //
    // Both guards below read the file's PRODUCTION SLICE. Two separate
    // things have to hold for that to mean anything: the slice has to end
    // where production code ends, and what is read out of it has to be code
    // rather than text that merely looks like code. Each was a measured
    // survivor; see `production_slice` and `code_chars` for the numbers.
    // ---------------------------------------------------------------------

    /// The marker that ends the production half of this file.
    ///
    /// Deliberately the gate and the module opener TOGETHER, and deliberately
    /// required to be UNIQUE -- see [`production_slice`].
    ///
    /// Spelled through `concat!` so that the ATTRIBUTE ITSELF appears in this
    /// file exactly once, at the one real gate. [`CUT_GATE`] and the walk
    /// below count that attribute in the raw text, and a copy of it sitting
    /// inside this constant's own source would make the count 2 for no reason
    /// anyone could act on.
    const TEST_MODULE_MARKER: &str = concat!("\r\n#[cfg(", "test)]\r\nmod tests {\r\n");

    /// Just the module opener, counted separately so that a forgery of only
    /// half the marker is caught too.
    const TEST_MODULE_OPENER: &str = "\r\nmod tests {\r\n";

    /// The production slice of this file: everything above the test module.
    /// Both source guards below read it, so neither can be blinded by
    /// anything written inside the test module.
    ///
    /// # The cut is anchored by UNIQUENESS, because it was forgeable
    ///
    /// Two earlier cuts both failed, in opposite directions.
    ///
    /// Cutting at the first `cfg` gate in the text failed LOUDLY: a doc
    /// comment that merely mentions the gate in a code span truncates the
    /// slice to nothing and reds both guards. Annoying, but it fails.
    ///
    /// Cutting at the first module opener failed SILENTLY, which is worse,
    /// and it was measured. Inserted immediately above the test module:
    ///
    /// ```ignore
    /// const ZZ_LAYOUT: &str = r"
    /// mod tests {
    /// ";
    ///
    /// fn zz_second_launch(p: &Path) {
    ///     let _ = ZZ_LAYOUT;
    ///     let _ = Command::new(p).spawn();
    /// }
    /// ```
    ///
    /// plus a call to it as the second line of [`apply_update_with`], SURVIVED
    /// at 2182 lib / 217 bin / 0 failed / 0 warnings. The raw string's second
    /// line became the FIRST occurrence of the marker, so the cut moved up
    /// above the forged helper, both guards read a slice in which nothing had
    /// changed, and a plain `Command::new(p).spawn()` ran before verification.
    /// The liveness control at the identical site -- the same helper and the
    /// same call with only the four-line raw string removed -- was KILLED at
    /// 2181/1. The raw string was the whole difference.
    ///
    /// The lesson is that a cut chosen by "first occurrence" can be MOVED by
    /// production text, and production text is what the guard is supposed to
    /// be judging. So the cut is not chosen by position any more: the marker
    /// must occur EXACTLY ONCE in the file, and so must the bare module opener
    /// on its own. A forgery does not move the cut, it adds a second
    /// occurrence -- and a second occurrence is the failure. Forging the gate
    /// and the opener together does not help, because that is two occurrences
    /// of both.
    ///
    /// The cost is the same loudness the first cut had, and it is the right
    /// trade: this file may no longer write the exact byte sequence
    /// `<CRLF>mod tests {<CRLF>` anywhere except at its one real test module,
    /// not in a doc comment, not in a raw string, not in a test fixture. That
    /// is a rule a reader can check by eye, and breaking it reds two tests
    /// with a message that says so.
    fn production_slice() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/updater.rs");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));

        let markers = text.matches(TEST_MODULE_MARKER).count();
        assert_eq!(
            markers, 1,
            "updater.rs contains {markers} occurrences of the test-module marker, not 1. \
             The production slice both source guards read is cut at this marker, so a \
             SECOND one -- in a raw string, a doc comment or a fixture -- would let \
             production code choose where the guards stop reading. That was measured \
             surviving at a full 2182 / 0 failed / 0 warnings, so it is a failure here \
             rather than a matter of taste"
        );
        let openers = text.matches(TEST_MODULE_OPENER).count();
        assert_eq!(
            openers, 1,
            "updater.rs contains {openers} occurrences of the bare test-module opener, \
             not 1; half a forged marker moves the cut just as well as a whole one"
        );

        let cut = text
            .find(TEST_MODULE_MARKER)
            .expect("updater.rs no longer declares a test-gated `mod tests`");

        // # And the slice is a PREFIX, so what is BELOW it is part of the
        // # question
        //
        // Everything the two guards read is `text[..cut]`. The cut POSITION is
        // unforgeable (above), but nothing about that says the FILE ends
        // there, and a plain non-gated module appended after the test module's
        // closing brace ships, is invisible to all three statements at once,
        // and was measured surviving at a full 2192 / 0 failed / 0 warnings
        // with a live `Command::new(..).spawn()` in it.
        //
        // The walk is called from HERE, rather than only from its own test, on
        // purpose: a mutant that closes `mod tests` early and re-gates the
        // remainder deletes any single guard test, and a guard that only one
        // test calls is exactly that cheap to delete. Every source guard in
        // this file goes through `production_slice`, so hiding an item below
        // the cut now means excising the launch-seam guard as well.
        walk_below_the_cut(&text);

        text[..cut].to_string()
    }

    /// One character of this file that the COMPILER would see.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    struct CodeChar {
        ch: char,
        in_literal: bool,
    }

    /// `text` with every comment removed and every run of whitespace removed,
    /// as a sequence of characters each tagged with whether it came from
    /// inside a string or character literal.
    ///
    /// # Why the guards below read this rather than the raw text
    ///
    /// A guard that counts a needle in raw source counts it in prose and in
    /// string data too, which cuts both ways: a doc comment that mentions the
    /// needle reds the guard for nothing, and -- the direction that matters --
    /// a needle a guard is counting UP TO A LIMIT can be spent harmlessly
    /// inside a literal. Stripping comments and tagging literals is what makes
    /// a count a statement about code.
    ///
    /// It also makes whitespace irrelevant, which is what closes the UFCS and
    /// spacing families in one move: `Command :: new`, `Command\n    ::new` and
    /// `Command::new` all render identically here.
    ///
    /// Handles line comments, nested block comments, normal strings with
    /// escapes, raw strings with any number of hashes, byte and C string
    /// prefixes, and character literals -- distinguished from lifetimes by
    /// looking for the closing quote.
    fn code_chars(text: &str) -> Vec<CodeChar> {
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let at = |k: usize| -> char {
            if k < n {
                chars[k]
            } else {
                '\0'
            }
        };
        let mut out: Vec<CodeChar> = Vec::new();
        let push = |ch: char, in_literal: bool, out: &mut Vec<CodeChar>| {
            if !ch.is_whitespace() {
                out.push(CodeChar { ch, in_literal });
            }
        };
        let mut i = 0usize;
        while i < n {
            let c = chars[i];

            if c == '/' && at(i + 1) == '/' {
                while i < n && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            if c == '/' && at(i + 1) == '*' {
                let mut depth = 1usize;
                i += 2;
                while i < n && depth > 0 {
                    if chars[i] == '/' && at(i + 1) == '*' {
                        depth += 1;
                        i += 2;
                    } else if chars[i] == '*' && at(i + 1) == '/' {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                continue;
            }

            // A raw string, possibly behind a `b` or `c` prefix. Only if the
            // `r` does not continue an identifier, so `for` and `str` are not
            // mistaken for one.
            let raw_r = if c == 'r' {
                Some(i)
            } else if (c == 'b' || c == 'c') && at(i + 1) == 'r' {
                Some(i + 1)
            } else {
                None
            };
            if let Some(r) = raw_r {
                let fresh = out
                    .last()
                    .map_or(true, |p| !(p.ch.is_alphanumeric() || p.ch == '_'));
                let mut h = r + 1;
                while at(h) == '#' {
                    h += 1;
                }
                let hashes = h - r - 1;
                if fresh && at(h) == '"' {
                    for k in i..=h {
                        push(chars[k], true, &mut out);
                    }
                    i = h + 1;
                    while i < n {
                        if chars[i] == '"' {
                            let mut k = i + 1;
                            let mut got = 0usize;
                            while got < hashes && at(k) == '#' {
                                k += 1;
                                got += 1;
                            }
                            if got == hashes {
                                for m in i..k {
                                    push(chars[m], true, &mut out);
                                }
                                i = k;
                                break;
                            }
                        }
                        push(chars[i], true, &mut out);
                        i += 1;
                    }
                    continue;
                }
            }

            if c == '"' {
                push(c, true, &mut out);
                i += 1;
                while i < n {
                    let d = chars[i];
                    if d == '\\' {
                        push(d, true, &mut out);
                        if i + 1 < n {
                            push(chars[i + 1], true, &mut out);
                        }
                        i += 2;
                        continue;
                    }
                    push(d, true, &mut out);
                    i += 1;
                    if d == '"' {
                        break;
                    }
                }
                continue;
            }

            // A character literal, as opposed to a lifetime: `'\n'` or `'x'`.
            if c == '\'' && (at(i + 1) == '\\' || at(i + 2) == '\'') {
                push(c, true, &mut out);
                i += 1;
                while i < n {
                    let d = chars[i];
                    if d == '\\' {
                        push(d, true, &mut out);
                        if i + 1 < n {
                            push(chars[i + 1], true, &mut out);
                        }
                        i += 2;
                        continue;
                    }
                    push(d, true, &mut out);
                    i += 1;
                    if d == '\'' {
                        break;
                    }
                }
                continue;
            }

            push(c, false, &mut out);
            i += 1;
        }
        out
    }

    /// The code with every literal's contents ERASED: what is left is only
    /// what the programmer wrote as syntax. Counting identifiers here cannot
    /// be inflated or satisfied by string data.
    fn code_without_literals(cc: &[CodeChar]) -> String {
        cc.iter().filter(|c| !c.in_literal).map(|c| c.ch).collect()
    }

    /// The code with literals KEPT: used only for the one exact-equality pin
    /// below, where the literals are half the thing being pinned and equality
    /// cannot be forged by adding text elsewhere.
    fn code_with_literals(cc: &[CodeChar]) -> String {
        cc.iter().map(|c| c.ch).collect()
    }

    /// The whole of `fn <header> { .. }`, located by BRACE DEPTH over code
    /// characters, so a brace inside a comment or a string cannot move its
    /// end. Rendered with literals kept and whitespace removed.
    fn code_fn(cc: &[CodeChar], header: &str) -> Option<String> {
        let flat = code_with_literals(cc);
        // `flat` is one char per entry of `cc`, so a char index into one is a
        // char index into the other.
        let flat_chars: Vec<char> = flat.chars().collect();
        let needle: Vec<char> = header.chars().collect();
        let start = (0..flat_chars.len().saturating_sub(needle.len()) + 1)
            .find(|&s| flat_chars[s..s + needle.len()] == needle[..])?;
        let mut depth = 0usize;
        let mut seen_open = false;
        for k in start..cc.len() {
            if cc[k].in_literal {
                continue;
            }
            if cc[k].ch == '{' {
                depth += 1;
                seen_open = true;
            } else if cc[k].ch == '}' {
                depth -= 1;
                if seen_open && depth == 0 {
                    return Some(flat_chars[start..=k].iter().collect());
                }
            }
        }
        None
    }

    /// The header of the one function in this module allowed to start a
    /// process, and its whole body, pinned exactly.
    const LAUNCH_SEAM_HEADER: &str = "fnlaunch_installer(installer_path:&Path)->Result<(),String>";
    const LAUNCH_SEAM: &str = concat!(
        "fnlaunch_installer(installer_path:&Path)->Result<(),String>{",
        "Command::new(installer_path)",
        ".args([\"/VERYSILENT\",\"/SUPPRESSMSGBOXES\"])",
        ".spawn()",
        ".map_err(|e|format!(\"failedtolaunchinstaller:{e}\"))?;",
        "Ok(())}",
    );

    /// **[`UpdaterEnv::production`] is the only constructor a shipping build
    /// compiles.**
    ///
    /// [`production_holds_the_real_verify_and_the_real_launch`] pins the value
    /// `production()` builds. It is worth nothing if a shipping build has a
    /// SECOND constructor that some call site can reach instead -- a
    /// `pub fn permissive() -> Self` is one line, and the address test never
    /// looks at it. Every constructor of this type has to spell `-> Self` or
    /// `-> UpdaterEnv`; the production slice contains exactly one, counted over
    /// CODE rather than over text, so a doc comment or a string cannot add to
    /// or excuse the count.
    ///
    /// The test-only substitute is written in the test module, below the cut,
    /// and is therefore not compiled into a shipping build at all.
    #[test]
    fn production_is_the_only_updater_env_a_shipping_build_has() {
        let code = code_without_literals(&code_chars(&production_slice()));
        let constructors = code.matches("->Self").count() + code.matches("->UpdaterEnv").count();
        assert_eq!(
            constructors, 1,
            "the production half of updater.rs declares {constructors} functions returning an \
             `UpdaterEnv`, not 1. `production` is meant to be the only env a shipping build can \
             build; a second one is a launcher whose signature check is whatever its caller \
             picked"
        );
        assert!(
            code.contains("pubfnproduction()->Self"),
            "updater.rs's production slice no longer declares `production() -> Self`"
        );
    }

    /// **The module starts a process in exactly one place, that place is
    /// [`launch_installer`], and its body is exactly these bytes.**
    ///
    /// # This guard used to be a denylist, and the denylist kept losing
    ///
    /// It counted the strings `Command::new` and `.spawn()` in the production
    /// slice and required one of each. Every measured escape from it was a
    /// different SPELLING of the same act:
    ///
    ///  * `<Command>::new(&p).args([..]).status()` -- UFCS breaks the
    ///    `Command::new` substring and `.status()` is not `.spawn()`. SURVIVED
    ///    at 2182 / 0 failed / 0 warnings.
    ///  * `.output()` in place of `.status()`, identically.
    ///  * `type C = std::process::Command; C::new(p)`.
    ///  * `let f = Command::new;` -- the constructor as a value, never called
    ///    by that name.
    ///  * `std::thread::Builder::new().spawn(..)` -- which is not a process
    ///    start at all, but got a launch onto a thread no assertion watched.
    ///
    /// Widening the list is what lost four times. So the list is gone. What
    /// replaces it is three closed statements about the production slice, read
    /// as CODE (see [`code_chars`]) rather than as text:
    ///
    /// 1. **The seam's body is pinned exactly.** Not "contains a spawn" --
    ///    equals [`LAUNCH_SEAM`], arguments included. This is also the only
    ///    thing holding the installer's silent-install flags: deleting the
    ///    whole `.args([..])` line was measured SURVIVING the previous shape of
    ///    this suite at a full 2182 / 217 / 0 failed / 0 warnings, which would
    ///    have shipped an updater that pops an interactive installer UI, and
    ///    which says the seam's ARGUMENTS were never pinned by anything.
    /// 2. **`Command` is named exactly twice in the whole production slice**:
    ///    once by the `use` that imports it and once inside the pinned body.
    ///    This is not a list of spellings -- it is the observation that every
    ///    way to start a child through `std::process` has to NAME the type
    ///    somewhere, whatever punctuation surrounds the name and whichever of
    ///    `spawn`, `status` or `output` finishes the job. UFCS names it. An
    ///    alias names it. A `use .. as` names it. Taking the constructor as a
    ///    value names it.
    /// 3. **The production slice contains no `unsafe` and no `thread`.** The
    ///    only way left to start a process without naming `Command` is to call
    ///    Win32 directly, which needs `unsafe`; and the only way to get a call
    ///    onto a thread no routing assertion is watching is to name a thread.
    ///    Both are zero here and neither is anything this module has ever had a
    ///    use for, so both are cheap.
    ///
    /// # What this does NOT cover
    ///
    /// It reads THIS FILE only. `updater.rs` is on `job_object.rs`'s child-
    /// start `ALLOWED` list, so a spawn moved into another module and called
    /// from here is that guard's business, not this one's -- and a `pub`
    /// forwarder in a non-`ALLOWED` file has its own history there.
    #[test]
    fn the_only_process_start_in_this_module_is_the_launch_seam() {
        let slice = production_slice();
        let cc = code_chars(&slice);
        let code = code_without_literals(&cc);

        // 1. The seam, byte for byte.
        let body = code_fn(&cc, LAUNCH_SEAM_HEADER)
            .expect("updater.rs no longer declares `launch_installer` with its pinned header");
        assert_eq!(
            body, LAUNCH_SEAM,
            "the one process start in updater.rs is no longer exactly the pinned seam. Its \
             body, its arguments and its error mapping are all part of the pin: dropping \
             `/VERYSILENT` and `/SUPPRESSMSGBOXES` is an interactive installer on a user's \
             screen, and adding anything is a second thing happening at the one point in \
             this crate that turns a file into a running process"
        );

        // 2. The type is named twice: the import, and the seam.
        let named = code.matches("Command").count();
        assert_eq!(
            named, 2,
            "updater.rs's production code names `Command` {named} times, not 2 (the `use` \
             and the launch seam). Every way to start a child through `std::process` names \
             the type somewhere -- `<Command>::new`, `type C = Command`, `use .. as C`, \
             `let f = Command::new`, `.status()`, `.output()` -- so this count is the \
             statement, not a list of the spellings"
        );
        assert!(
            code.contains("usestd::process::Command;"),
            "updater.rs no longer imports `Command` by its own name, so the count above is \
             counting something else"
        );
        assert_eq!(
            body.matches("Command").count(),
            1,
            "the pinned seam does not name `Command`, so the two names counted above are \
             both somewhere else"
        );

        // 3. No Win32 process creation, and no threads.
        let unsafes = code.matches("unsafe").count();
        assert_eq!(
            unsafes, 0,
            "updater.rs's production code contains {unsafes} `unsafe` blocks. It has never \
             needed one, and `unsafe` is what a direct `CreateProcessW` would need -- the \
             one way left to start a process without naming `Command`"
        );
        let threads = code.matches("thread").count();
        assert_eq!(
            threads, 0,
            "updater.rs's production code names `thread` {threads} times. It must name it \
             none: a call moved onto a thread is a call the routing tests observe only by \
             the grace of a timing window, and \
             `std::thread::Builder::new().spawn(move || zz_l(&zz_p))` inserted above the \
             verify call was measured SURVIVING the whole suite at 2182 / 0 failed / 0 \
             warnings for exactly that reason"
        );
    }

    // ---------------------------------------------------------------------
    // AND THE SCANNER THE TWO GUARDS ABOVE STAND ON
    //
    // `code_chars` is now load-bearing for three counts and one equality. A
    // scanner that quietly returned an empty string would make all four
    // vacuous, so it is pinned here directly.
    // ---------------------------------------------------------------------

    fn erased(text: &str) -> String {
        code_without_literals(&code_chars(text))
    }

    fn kept(text: &str) -> String {
        code_with_literals(&code_chars(text))
    }

    #[test]
    fn the_scanner_drops_comments_and_whitespace() {
        assert_eq!(erased("let a = 1; // Command::new"), "leta=1;");
        assert_eq!(erased("/// Command::new\r\nlet a = 1;"), "leta=1;");
        assert_eq!(erased("/* Command::new */ let a = 1;"), "leta=1;");
        assert_eq!(erased("/* a /* b */ Command::new */ let a = 1;"), "leta=1;");
        assert_eq!(erased("Command :: new ( p )"), "Command::new(p)");
        assert_eq!(erased("Command\r\n    ::new(p)"), "Command::new(p)");
        assert_eq!(erased("<Command>::new(p)"), "<Command>::new(p)");
    }

    #[test]
    fn the_scanner_erases_literals_but_keeps_their_shape() {
        assert_eq!(erased("let s = \"Command::new\";"), "lets=;");
        assert_eq!(kept("let s = \"Command::new\";"), "lets=\"Command::new\";");
        assert_eq!(erased("let s = r\"Command::new\";"), "lets=;");
        assert_eq!(erased("let s = r#\"a \" Command::new\"#;"), "lets=;");
        assert_eq!(erased("let s = \"a \\\" Command::new\";"), "lets=;");
        assert_eq!(erased("let c = '\\'';let d = 1;"), "letc=;letd=1;");
        // A lifetime is not a character literal.
        assert_eq!(erased("fn f<'a>(x: &'a str) {}"), "fnf<'a>(x:&'astr){}");
        // `for` and `str` do not open a raw string.
        assert_eq!(erased("for x in y {}"), "forxiny{}");
    }

    #[test]
    fn the_scanner_finds_a_function_by_brace_depth_not_by_text() {
        let src = concat!(
            "fn f(a: u8) -> u8 {\r\n",
            "    // }\r\n",
            "    let s = \"}\";\r\n",
            "    if a > 0 { return 1; }\r\n",
            "    s.len() as u8\r\n",
            "}\r\n",
            "fn g() {}\r\n",
        );
        let cc = code_chars(src);
        assert_eq!(
            code_fn(&cc, "fnf(a:u8)->u8").unwrap(),
            "fnf(a:u8)->u8{lets=\"}\";ifa>0{return1;}s.len()asu8}"
        );
        assert_eq!(code_fn(&cc, "fnnosuchfn()"), None);
    }

    /// The scanner is not silently returning nothing: the real file's
    /// production slice renders to something substantial, and the needles the
    /// guards count are actually present in it.
    #[test]
    fn the_scanner_reads_this_file_as_code_rather_than_as_nothing() {
        let code = erased(&production_slice());
        assert!(
            code.len() > 1000,
            "updater.rs's production slice renders to {} characters of code; the scanner is \
             returning nothing and every count held over it is vacuous",
            code.len()
        );
        assert!(code.contains("fnlaunch_installer("));
        assert!(code.contains("fnapply_update_with("));
        assert!(code.contains("pubfnapply_update("));
    }

    // ---------------------------------------------------------------------
    // AND WHAT "PRODUCTION" MEANS, WHICH IS NO LONGER "A PREFIX"
    //
    // [`production_slice`] returns `text[..cut]`, and all three statements in
    // `the_only_process_start_in_this_module_is_the_launch_seam` plus the
    // constructor count in `production_is_the_only_updater_env_a_shipping_
    // build_has` read that prefix and nothing else. C2 made the cut POSITION
    // unforgeable -- the marker and the bare opener are each required to
    // occur exactly once -- and that is a real property, but it is a property
    // about where the prefix ENDS. Nothing said the FILE ends there.
    //
    // Measured on 0cd9fe0: a plain, non-gated
    //
    //     mod zz_below { pub fn go(p: &Path) -> Result<(), String> { .. } }
    //
    // appended after the test module's closing brace, containing a
    // `Command::new(p).spawn()` behind an unsatisfiable condition, plus a call
    // to it immediately above the `(env.verify)` call in `apply_update_with`,
    // SURVIVED at 2192 / 0 failed / 0 warnings -- and `cargo build --lib`
    // confirms it is genuinely compiled into a shipping build. Below the cut,
    // `Command`, `unsafe` and `thread` are all free, so ALL THREE statements
    // fall at once; and `job_object.rs`'s crate-wide child-start walk does not
    // help, because `updater.rs` is on its `ALLOWED` list. The liveness
    // control was the byte-identical module and call site with the module
    // placed ABOVE the marker: KILLED at 2191/1, on the `Command` count of 3.
    // The only difference between the two was which side of the cut it sat on.
    //
    // So production is defined here instead: everything above the cut, PLUS
    // the standing fact that below the cut there is nothing but test-gated
    // modules. The two-state walk that says so is the shape `breach.rs`,
    // `vault_export.rs` and `send.rs` already carry and that survived
    // adversarial review there, reused rather than reinvented.
    // ---------------------------------------------------------------------

    /// The `cfg` attribute that makes a module test-only, split so this
    /// constant is not itself one and cannot be found by a search for the real
    /// attribute. The same reason [`TEST_MODULE_MARKER`] is a `concat!`.
    const CUT_GATE: &str = concat!("#[cfg(", "test)]");

    /// Column-0 lines below the cut that are the CONTENTS OF A STRING LITERAL
    /// rather than source. Empty today, and controlled by the walk: a line
    /// that stops being one fails rather than being quietly forgiven.
    const BELOW_CUT_STRING_LINES: &[&str] = &[];

    /// `true` for `mod NAME {`, `pub mod NAME {` and `pub(crate) mod NAME {`,
    /// and for nothing else. Exact rather than a `starts_with`: a whole module
    /// written on one line is not a module opener here and must fail.
    fn below_cut_is_module_opener(line: &str) -> bool {
        let t = line.strip_prefix("pub(crate) ").unwrap_or(line);
        let t = t.strip_prefix("pub ").unwrap_or(t);
        let rest = match t.strip_prefix("mod ") {
            Some(rest) => rest,
            None => return false,
        };
        let name = match rest.strip_suffix(" {") {
            Some(name) => name,
            None => return false,
        };
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }

    /// The two-state walk from the cut to EOF over whatever text it is handed.
    /// Returns `(visited, modules, closes, depth)` so the caller can control it
    /// for non-vacuity.
    ///
    /// **Line-ending agnostic on purpose.** `lines()` strips a trailing
    /// carriage return, so every comparison is against the line's real text on
    /// a CRLF working tree and on an LF one alike.
    fn walk_below_the_cut(source: &str) -> (usize, usize, usize, usize) {
        let cut = source
            .find(CUT_GATE)
            .expect("the cut marker is controlled by the caller");
        let mut depth = 0usize;
        // The walked region BEGINS with the gate, so nothing inside it is
        // taken on trust: the first line seen is the attribute itself.
        let mut gated = false;
        let (mut modules, mut closes, mut visited) = (0usize, 0usize, 0usize);
        for line in source[cut..].lines() {
            visited += 1;
            if depth == 0 {
                // Between modules NOTHING is allowed but blanks, comments, the
                // gate and a module opener -- at ANY indentation, because an
                // indented `fn` at file scope is still a top-level item and a
                // column-0-only filter would walk straight past it.
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with("//") {
                    continue;
                }
                if trimmed == CUT_GATE {
                    gated = true;
                    continue;
                }
                assert!(
                    !line.starts_with(char::is_whitespace) && below_cut_is_module_opener(trimmed),
                    "top-level source below the cut: {line:?}. `production_slice` is a PREFIX \
                     of this file, so every guard here reads only the half above the cut: an \
                     item down here can name `Command`, write `unsafe`, name a thread or add \
                     a second `-> Self` and every one of those counts stays word-perfect. \
                     Move it above the test module."
                );
                assert!(
                    gated,
                    "the module {line:?} below the cut is not test-gated, so it SHIPS -- and it \
                     ships in the half of the file no guard here reads. That exact shape was \
                     measured surviving the whole suite at 2192 / 0 failed / 0 warnings with a \
                     live `Command::new(..).spawn()` inside it"
                );
                gated = false;
                depth = 1;
                modules += 1;
            } else if !line.is_empty() && !line.starts_with(char::is_whitespace) {
                // Inside a test module every item is indented, so the only
                // column-0 line is the module's own closing brace.
                if line == "}" {
                    depth = 0;
                    closes += 1;
                    continue;
                }
                assert!(
                    BELOW_CUT_STRING_LINES.contains(&line),
                    "a column-0 line inside a test module below the cut: {line:?}. Either a \
                     top-level item escaped the brace count, or this is the contents of a \
                     string literal and belongs in BELOW_CUT_STRING_LINES"
                );
            }
        }
        (visited, modules, closes, depth)
    }

    #[test]
    fn nothing_but_gated_test_modules_lives_below_the_guards_cut() {
        let source = include_str!("updater.rs");

        // 1. The cut lands where the guards think it does, and there is
        //    exactly one place it could land -- so it cannot move UP into a
        //    comment or a string and silently truncate the half they read.
        let seen = source.matches(CUT_GATE).count();
        assert_eq!(
            seen, 1,
            "the test gate occurs {seen} times in this file. `production_slice` cuts at the \
             FIRST, so a second occurrence is a cut that can move up and vacate every guard \
             below the truncation while their own text stays word-perfect"
        );
        let cut = source.find(CUT_GATE).expect("counted exactly one just above");
        assert!(
            cut > 0 && source.as_bytes()[cut - 1] == b'\n',
            "the cut landed in the MIDDLE of a line, so the gate was matched inside a comment \
             or a string literal rather than at a real attribute"
        );

        // 2. Positive control on WHERE the cut is: the production half still
        //    reaches the last production item in the file.
        const LAST_PRODUCTION_ITEM: &str =
            concat!("apply_update_with(dest_dir, release, ", "&UpdaterEnv::production())");
        assert_eq!(
            source.matches(LAST_PRODUCTION_ITEM).count(),
            1,
            "control: the anchor is not in this file exactly once, so it pins nothing -- \
             repoint it at the last production item above the test module"
        );
        let anchor = source.find(LAST_PRODUCTION_ITEM).expect("counted just above");
        assert!(
            anchor < cut,
            "the last production item this control knows about is BELOW the cut, so the cut \
             moved up and the production half every guard reads is truncated"
        );
        assert!(
            cut - anchor < 4_000,
            "the cut is more than 4000 bytes past the last production item this control knows \
             about: either production was appended below the anchor (repoint the anchor) or \
             the cut moved down"
        );

        // 3. The walk, over an LF copy and a CRLF copy of the same text, which
        //    must agree. Built both ways rather than compared against the bytes
        //    on disk: this repository stores LF blobs and only
        //    `core.autocrlf=true` makes a working tree CRLF, so a control that
        //    asserted "this file is CRLF" would pass here and fail on Linux.
        let lf = source.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        assert_ne!(
            lf, crlf,
            "control: the two copies are the same string, so comparing the walk over them \
             compares it with itself -- this file has no line endings at all"
        );
        assert_eq!(
            walk_below_the_cut(&lf),
            walk_below_the_cut(&crlf),
            "the walk gives a different answer on an LF copy of this file than on a CRLF one"
        );
        let on_disk = walk_below_the_cut(source);
        assert!(
            on_disk == walk_below_the_cut(&lf) || on_disk == walk_below_the_cut(&crlf),
            "this file's line endings are mixed: the walk over it agrees with neither the \
             all-LF nor the all-CRLF copy of its own text"
        );

        // 4. The walk is not vacuous, and it finished.
        let (visited, modules, closes, depth) = on_disk;
        assert!(
            visited > 100,
            "control: the walk visited only {visited} lines below the cut, which is not a test \
             module's worth -- the slice is empty and this test proves nothing"
        );
        assert_eq!(
            (modules, closes, depth),
            (1, 1, 0),
            "below the cut there is no longer exactly one opened-and-closed test module: \
             {modules} opened, {closes} closed, ending at depth {depth}"
        );

        // 5. Controls on the walk itself: it really refuses production code
        //    down there. Without these the walk could be a no-op that visits
        //    lines and asserts nothing.
        let with_an_appended_item = format!("{lf}\npub fn sneaked() {{}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&with_an_appended_item)).is_err(),
            "control: the walk accepted a `pub fn` appended below the test module, which is \
             the exact mutation it exists to catch"
        );
        // And an INDENTED one, which a column-0 filter would miss.
        let with_an_indented_item = format!("{lf}\n    struct Sneaked(u8);\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&with_an_indented_item)).is_err(),
            "control: the walk accepted an INDENTED top-level item appended below the test \
             module"
        );
        // And the measured survivor itself: an ungated module, which ships.
        let with_an_ungated_module = format!("{lf}\nmod zz_below {{\n}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&with_an_ungated_module)).is_err(),
            "control: the walk accepted an UNGATED module below the cut, which ships -- that \
             is the survivor, verbatim"
        );
        // And a module whose whole body is on one line, so the brace count
        // never sees an opener and would otherwise walk on at depth 0.
        let with_a_one_line_module = format!("{lf}\nmod zz_below {{ pub fn go() {{}} }}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&with_a_one_line_module)).is_err(),
            "control: the walk accepted a whole module written on ONE LINE below the cut"
        );
        // And a gate that is not THE gate: `#[cfg(not(test))]` ships.
        let with_an_inverted_gate =
            format!("{lf}\n#[cfg(not(test))]\nmod zz_below {{\n}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&with_an_inverted_gate)).is_err(),
            "control: the walk accepted `#[cfg(not(test))]` as a test gate, which is the one \
             attribute that means the OPPOSITE"
        );
    }
}

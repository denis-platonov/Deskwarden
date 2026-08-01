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

/// How long to wait between successive reads of a response body.
///
/// This bounds a *stalled* transfer, not a slow one: ureq applies
/// `timeout_read` per read, so a slow-but-steady download is never cut off by
/// it. Read the caveat on [`API_DEADLINE`] before treating it as this agent's
/// only protection -- on a reused keep-alive connection it is not applied at
/// all.
const READ_TIMEOUT: Duration = Duration::from_secs(15);

/// Whole-request deadline for the releases-API check.
///
/// Needed because `timeout_read` is silently dropped on connection reuse in
/// ureq 2.12.1: it is set on the socket only on the connect path
/// (`stream.rs:436`) and `Stream::reset()` clears it again (`stream.rs:265`)
/// before the connection is pooled, while `connect_socket` returns a pooled
/// connection without re-entering that path (`unit.rs:361-364`). The response
/// head is read under `unit.deadline` (`response.rs:574`), which comes only
/// from `request.timeout.or(agent.config.timeout)` (`request.rs:122`). So an
/// agent with `timeout_read` alone bounds its first request and nothing after
/// it -- the same defect that hung the vault path in v0.3.0 (see
/// `vault_bridge::REQUEST_DEADLINE`).
///
/// 30s: the response is a single small JSON document, so anything near this
/// is already a broken path, and the update check runs on a background thread
/// where a 30s wait costs the user nothing.
const API_DEADLINE: Duration = Duration::from_secs(30);

/// Whole-request deadline for the installer download.
///
/// Deliberately *not* the same number as [`API_DEADLINE`], and deliberately
/// not left unset. A whole-request deadline caps the entire transfer, body
/// included -- so the value that is right for a one-line JSON response would
/// abort a legitimate ~6 MB installer download on a slow link and trade a hang
/// for a broken updater. But leaving it unset is what caused the shipped hang,
/// so the download gets its own, generous bound instead of none.
///
/// 10 minutes sustains a ~6 MB installer at roughly 80 kbit/s end to end,
/// which is below any connection that could have reached GitHub to discover
/// the release in the first place. It is a "this will never finish" bound, not
/// a performance budget. `READ_TIMEOUT` still catches the common stall case
/// much sooner on a fresh connection; this is the backstop that also holds on
/// a pooled one. The download runs on a background thread, so the worst case
/// is a background update attempt that gives up after 10 minutes.
const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(600);

/// Builds a `ureq::Agent` with bounded connect/read timeouts, so a stalled
/// `api.github.com` (or any host on the network path to it) can't hang a
/// caller indefinitely the way the implicit default agent would. Mirrors the
/// bounded-wait pattern `bw_serve::READINESS_DEADLINE` already uses for the
/// local `bw serve` dependency -- just applied to an external host, which has
/// strictly more failure modes than localhost.
///
/// The whole-request deadline is *not* set here: it is set per request by
/// [`check_for_update`] and [`download_and_verify`], because one agent is
/// shared by a tiny API response and a multi-megabyte download and no single
/// number is correct for both.
pub fn build_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .build()
}

pub fn check_for_update(
    base_url: &str,
    current_version: &Version,
    agent: &ureq::Agent,
) -> Result<Option<ReleaseInfo>, String> {
    let url = format!("{base_url}/repos/denis-platonov/deskwarden/releases/latest");
    let body: serde_json::Value = agent
        .get(&url)
        .timeout(API_DEADLINE)
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

pub fn download_and_verify(
    release: &ReleaseInfo,
    expected_thumbprint: &str,
    dest_dir: &Path,
    agent: &ureq::Agent,
) -> Result<PathBuf, String> {
    // Created here rather than assumed to exist: this is a dedicated cache
    // subdirectory (see `main.rs`), not the config directory the rest of the
    // app already creates at startup.
    std::fs::create_dir_all(dest_dir)
        .map_err(|e| format!("could not create {}: {e}", dest_dir.display()))?;
    let dest_path = dest_dir.join(installer_file_name(&release.version));

    let response = agent
        .get(&release.installer_download_url)
        .timeout(DOWNLOAD_DEADLINE)
        .call()
        .map_err(|e| format!("failed to download installer: {e}"))?;
    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(&dest_path).map_err(|e| e.to_string())?;
    std::io::copy(&mut reader, &mut file).map_err(|e| e.to_string())?;
    drop(file);

    let info = verify_authenticode(&dest_path)?;
    if !is_trusted_signer(&info, expected_thumbprint) {
        let _ = std::fs::remove_file(&dest_path);
        return Err("downloaded installer failed signature verification".to_string());
    }

    Ok(dest_path)
}

pub fn apply_update(installer_path: &Path) -> Result<(), String> {
    Command::new(installer_path)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES"])
        .spawn()
        .map_err(|e| format!("failed to launch installer: {e}"))?;
    Ok(())
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
        let agent = build_agent();
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
        let agent = build_agent();
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
        let agent = build_agent();
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
        let agent = build_agent();
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
        let agent = build_agent();
        let result = check_for_update(&server.url(), &current, &agent);

        assert!(result.is_err());
    }
}

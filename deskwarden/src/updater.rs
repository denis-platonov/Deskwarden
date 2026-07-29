use crate::signature::{is_trusted_signer, verify_authenticode};
use semver::Version;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct ReleaseInfo {
    pub version: Version,
    pub installer_download_url: String,
}

pub fn check_for_update(base_url: &str, current_version: &Version) -> Result<Option<ReleaseInfo>, String> {
    let url = format!("{base_url}/repos/denis-platonov/deskwarden/releases/latest");
    let body: serde_json::Value = ureq::get(&url)
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

pub fn download_and_verify(
    release: &ReleaseInfo,
    expected_thumbprint: &str,
    dest_dir: &Path,
) -> Result<PathBuf, String> {
    let dest_path = dest_dir.join(format!("deskwarden-{}-installer.exe", release.version));

    let response = ureq::get(&release.installer_download_url)
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
        let result = check_for_update(&server.url(), &current).unwrap();

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
        let result = check_for_update(&server.url(), &current).unwrap();

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
        let result = check_for_update(&server.url(), &current).unwrap();

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
        let result = check_for_update(&server.url(), &current).unwrap();

        assert!(result.is_none());
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
        let result = check_for_update(&server.url(), &current);

        assert!(result.is_err());
    }
}

use std::path::{Path, PathBuf};
use std::process::Command;

/// Absolute path to Windows PowerShell, resolved under `%SystemRoot%`.
///
/// Deliberately not the bare command name `powershell`: `CreateProcess`'s
/// search order starts with the directory of the calling executable, and
/// deskwarden installs per-user into `%LOCALAPPDATA%\deskwarden` -- a
/// user-writable directory. Anything able to drop a file there (same user, no
/// privilege escalation needed) could otherwise substitute its own
/// `powershell.exe` and have it answer the one question the entire update
/// trust model rests on: "is this installer signed by us?". Naming the real
/// binary absolutely removes that substitution.
///
/// `%SystemRoot%` is read from the environment rather than hardcoded because
/// Windows genuinely is installable on other volumes, with `C:\Windows` as
/// the fallback for the (practically impossible) case of the variable being
/// missing -- a wrong-but-absolute path fails closed with "failed to run
/// powershell", which is the safe direction.
fn powershell_path() -> PathBuf {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    PathBuf::from(system_root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe")
}

#[derive(Debug, Clone)]
pub struct SignatureInfo {
    pub valid: bool,
    pub thumbprint: Option<String>,
    /// The signer certificate's subject DN, one RDN per line (`X500DistinguishedName.Format($true)`).
    /// `None` when there is no signer certificate at all.
    pub subject_dn: Option<String>,
}

/// Verifies a file's Authenticode signature using PowerShell's built-in
/// `Get-AuthenticodeSignature` cmdlet. Deliberately shells out rather than
/// binding raw `WinVerifyTrust`/WinTrust struct layouts directly: this
/// cmdlet ships with every stock Windows install (Microsoft.PowerShell.Security),
/// is stable, well-documented public surface, and avoids getting the
/// WINTRUST_DATA/WINTRUST_FILE_INFO FFI wrong in a security-critical path.
pub fn verify_authenticode(path: &Path) -> Result<SignatureInfo, String> {
    let path_str = path.to_str().ok_or("path is not valid UTF-8")?;
    let script = format!(
        "$sig = Get-AuthenticodeSignature -FilePath '{}'; \
         [PSCustomObject]@{{ Status = $sig.Status.ToString(); \
         Thumbprint = if ($sig.SignerCertificate) {{ $sig.SignerCertificate.Thumbprint }} else {{ $null }}; \
         SubjectDn = if ($sig.SignerCertificate) {{ $sig.SignerCertificate.SubjectName.Format($true) }} else {{ $null }} }} \
         | ConvertTo-Json -Compress",
        path_str.replace('\'', "''")
    );

    let output = Command::new(powershell_path())
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|e| format!("failed to run powershell: {e}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).map_err(|e| format!("failed to parse powershell output: {e}"))?;

    let status = parsed["Status"].as_str().unwrap_or("");
    let thumbprint = parsed["Thumbprint"].as_str().map(|s| s.to_string());
    let subject_dn = parsed["SubjectDn"].as_str().map(|s| s.to_string());

    Ok(SignatureInfo {
        valid: status == "Valid",
        thumbprint,
        subject_dn,
    })
}

pub fn is_trusted_signer(info: &SignatureInfo, expected_thumbprint: &str) -> bool {
    info.valid
        && info
            .thumbprint
            .as_deref()
            .map(|t| t.eq_ignore_ascii_case(expected_thumbprint))
            .unwrap_or(false)
}

/// Extracts every value of one Distinguished Name component (e.g. `"O"`,
/// `"CN"`) from a subject DN formatted via `X500DistinguishedName.Format($true)`
/// (one RDN per line, `Key=Value`). A Rust port of `Get-CertificateDnComponent`
/// in `installer/bootstrap-bw.ps1` -- kept in sync there by hand rather than
/// shared, since one runs at install time (PowerShell) and this runs at app
/// startup (Rust). Parsed line-by-line rather than split on commas for the
/// same reason that script gives: a value like `O="Bitwarden, Inc."` contains
/// a comma of its own. Surrounding quotes are stripped.
pub fn dn_component(subject_dn: &str, key: &str) -> Vec<String> {
    subject_dn
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let separator = line.find('=')?;
            let (k, v) = line.split_at(separator);
            if k.trim().eq_ignore_ascii_case(key) {
                Some(v[1..].trim().trim_matches('"').to_string())
            } else {
                None
            }
        })
        .collect()
}

/// True if `info` is a validly-chained signature whose subject DN names one
/// of `trusted_orgs` in its `O=` component -- an exact whole-component match,
/// never a substring search (see `$BitwardenSignerOrganizations` in
/// `installer/bootstrap-bw.ps1` for why: an unanchored substring match would
/// accept any validly-signed binary whose subject merely *contains* one of
/// these words, from an unrelated but legitimately-issued certificate).
pub fn is_trusted_organization(info: &SignatureInfo, trusted_orgs: &[&str]) -> bool {
    if !info.valid {
        return false;
    }
    let Some(dn) = info.subject_dn.as_deref() else {
        return false;
    };
    dn_component(dn, "O")
        .iter()
        .any(|org| trusted_orgs.iter().any(|t| t.eq_ignore_ascii_case(org)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusts_a_matching_thumbprint() {
        let info = SignatureInfo {
            valid: true,
            thumbprint: Some("ABCDEF0123456789".to_string()),
            subject_dn: None,
        };
        assert!(is_trusted_signer(&info, "abcdef0123456789"));
    }

    #[test]
    fn rejects_a_mismatched_thumbprint() {
        let info = SignatureInfo {
            valid: true,
            thumbprint: Some("ABCDEF0123456789".to_string()),
            subject_dn: None,
        };
        assert!(!is_trusted_signer(&info, "0000000000000000"));
    }

    #[test]
    fn rejects_an_invalid_signature_even_with_a_matching_thumbprint() {
        let info = SignatureInfo {
            valid: false,
            thumbprint: Some("ABCDEF0123456789".to_string()),
            subject_dn: None,
        };
        assert!(!is_trusted_signer(&info, "ABCDEF0123456789"));
    }

    #[test]
    fn rejects_a_missing_thumbprint() {
        let info = SignatureInfo { valid: true, thumbprint: None, subject_dn: None };
        assert!(!is_trusted_signer(&info, "ABCDEF0123456789"));
    }

    #[test]
    fn dn_component_extracts_a_quoted_value_containing_a_comma() {
        // The reason this isn't a naive split on ',': a bare split would tear
        // `O="Bitwarden, Inc."` in half at the comma inside the quotes.
        let dn = "CN=Bitwarden Inc.\r\nO=\"Bitwarden, Inc.\"\r\nC=US";
        assert_eq!(dn_component(dn, "O"), vec!["Bitwarden, Inc."]);
    }

    #[test]
    fn dn_component_matches_the_key_case_insensitively() {
        let dn = "cn=Example\r\no=Example Org";
        assert_eq!(dn_component(dn, "O"), vec!["Example Org"]);
    }

    #[test]
    fn dn_component_returns_empty_for_an_absent_key() {
        let dn = "CN=Example";
        assert!(dn_component(dn, "O").is_empty());
    }

    #[test]
    fn trusts_an_exact_organization_match() {
        let info = SignatureInfo {
            valid: true,
            thumbprint: None,
            subject_dn: Some("CN=bw.exe\r\nO=Bitwarden Inc.\r\nC=US".to_string()),
        };
        assert!(is_trusted_organization(&info, &["Bitwarden Inc.", "8bit Solutions LLC"]));
    }

    #[test]
    fn rejects_an_organization_that_only_contains_the_trusted_word() {
        // The point of the exact-component match: `OU=bitwarden-integration`
        // or `O=Not Bitwarden At All` must not slip through a substring check.
        let info = SignatureInfo {
            valid: true,
            thumbprint: None,
            subject_dn: Some("CN=evil.exe\r\nO=Not Bitwarden At All\r\nC=US".to_string()),
        };
        assert!(!is_trusted_organization(&info, &["Bitwarden Inc."]));
    }

    #[test]
    fn rejects_a_trusted_organization_on_an_invalid_signature() {
        // A validly-formed subject on a signature that doesn't actually chain
        // to a trusted root must still be refused.
        let info = SignatureInfo {
            valid: false,
            thumbprint: None,
            subject_dn: Some("CN=bw.exe\r\nO=Bitwarden Inc.\r\nC=US".to_string()),
        };
        assert!(!is_trusted_organization(&info, &["Bitwarden Inc."]));
    }

    #[test]
    fn rejects_a_missing_subject_dn() {
        let info = SignatureInfo { valid: true, thumbprint: None, subject_dn: None };
        assert!(!is_trusted_organization(&info, &["Bitwarden Inc."]));
    }

    #[test]
    fn resolves_powershell_by_absolute_path_under_the_system_root() {
        // The point of the fix: never a bare command name, whose
        // CreateProcess search order includes deskwarden's own (per-user,
        // user-writable) install directory.
        let path = powershell_path();
        assert!(path.is_absolute(), "{} is not absolute", path.display());
        assert!(path.ends_with("System32/WindowsPowerShell/v1.0/powershell.exe"));
        assert!(path.starts_with(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string())));
    }

    #[test]
    fn the_resolved_powershell_actually_exists() {
        // Windows-only crate, so this is a real assertion about the machine
        // the tests run on rather than an environment assumption: if the
        // canonical path is ever wrong, signature verification -- and with it
        // every update -- silently stops working, and this catches it.
        let path = powershell_path();
        assert!(path.exists(), "{} does not exist", path.display());
    }
}

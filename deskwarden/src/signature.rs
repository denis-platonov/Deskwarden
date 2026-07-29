use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct SignatureInfo {
    pub valid: bool,
    pub thumbprint: Option<String>,
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
         Thumbprint = if ($sig.SignerCertificate) {{ $sig.SignerCertificate.Thumbprint }} else {{ $null }} }} \
         | ConvertTo-Json -Compress",
        path_str.replace('\'', "''")
    );

    let output = Command::new("powershell")
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

    Ok(SignatureInfo {
        valid: status == "Valid",
        thumbprint,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusts_a_matching_thumbprint() {
        let info = SignatureInfo {
            valid: true,
            thumbprint: Some("ABCDEF0123456789".to_string()),
        };
        assert!(is_trusted_signer(&info, "abcdef0123456789"));
    }

    #[test]
    fn rejects_a_mismatched_thumbprint() {
        let info = SignatureInfo {
            valid: true,
            thumbprint: Some("ABCDEF0123456789".to_string()),
        };
        assert!(!is_trusted_signer(&info, "0000000000000000"));
    }

    #[test]
    fn rejects_an_invalid_signature_even_with_a_matching_thumbprint() {
        let info = SignatureInfo {
            valid: false,
            thumbprint: Some("ABCDEF0123456789".to_string()),
        };
        assert!(!is_trusted_signer(&info, "ABCDEF0123456789"));
    }

    #[test]
    fn rejects_a_missing_thumbprint() {
        let info = SignatureInfo { valid: true, thumbprint: None };
        assert!(!is_trusted_signer(&info, "ABCDEF0123456789"));
    }
}

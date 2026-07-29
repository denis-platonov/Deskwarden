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
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("failed to parse powershell output: {e}"))?;

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

/// Consumes a quoted DN value starting at `s[0] == '"'`.
///
/// `Some(value)` once the closing quote is seen (doubled `""` is an escaped
/// quote inside the value, which is how `CertNameToStr` emits one);
/// `None` while the quote is still open, meaning the value runs on into the
/// following line and the caller must keep feeding it.
fn close_quoted_value(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = s[1..].chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            if chars.peek() == Some(&'"') {
                chars.next();
                out.push('"');
            } else {
                return Some(out);
            }
        } else {
            out.push(c);
        }
    }
    None
}

/// Extracts every value of one Distinguished Name component (e.g. `"O"`,
/// `"CN"`) from a subject DN formatted via `X500DistinguishedName.Format($true)`
/// (one RDN per line, `Key=Value`). A Rust port of `Get-CertificateDnComponent`
/// in `installer/bootstrap-bw.ps1` -- kept in sync there by hand rather than
/// shared, since one runs at install time (PowerShell) and this runs at app
/// startup (Rust). Parsed line-by-line rather than split on commas for the
/// same reason that script gives: a value like `O="Bitwarden, Inc."` contains
/// a comma of its own. Surrounding quotes are stripped.
///
/// Quoting is *tracked across lines* rather than each line being parsed in
/// isolation, and that is a security property, not tidiness. `Format($true)`
/// separates RDNs with CRLF, but a certificate value may itself contain a
/// newline -- and `CertNameToStr` handles that by quoting the value, not by
/// escaping the newline. A naive line-at-a-time parser therefore reads
///
/// ```text
/// CN="Innocent
/// O=Bitwarden Inc."
/// ```
///
/// as two RDNs and reports a forged `O=Bitwarden Inc.` that no certificate
/// authority ever issued as an organization. Following the quote to its real
/// close keeps the whole thing as one `CN` value, where it belongs. (Reaching
/// this check at all would still require a CA to issue such a certificate
/// *and* the signature to chain validly, so it is theoretical -- but it is
/// also two lines of parser, so there is no reason to leave it open.)
pub fn dn_component(subject_dn: &str, key: &str) -> Vec<String> {
    let mut values = Vec::new();
    // `Some((this value's key matches, text accumulated so far))` while a
    // quoted value is still open across a line break.
    let mut open: Option<(bool, String)> = None;

    for raw_line in subject_dn.lines() {
        if let Some((matches, mut accumulated)) = open.take() {
            accumulated.push('\n');
            accumulated.push_str(raw_line);
            match close_quoted_value(&accumulated) {
                Some(value) => {
                    if matches {
                        values.push(value);
                    }
                }
                None => open = Some((matches, accumulated)),
            }
            continue;
        }

        let line = raw_line.trim();
        let Some(separator) = line.find('=') else {
            continue;
        };
        let (k, rest) = line.split_at(separator);
        let matches = k.trim().eq_ignore_ascii_case(key);
        let value = rest[1..].trim_start();

        if value.starts_with('"') {
            match close_quoted_value(value) {
                Some(value) => {
                    if matches {
                        values.push(value);
                    }
                }
                None => open = Some((matches, value.to_string())),
            }
        } else if matches {
            values.push(value.trim_end().to_string());
        }
    }

    values
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
        let info = SignatureInfo {
            valid: true,
            thumbprint: None,
            subject_dn: None,
        };
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
    fn dn_component_does_not_read_a_newline_inside_a_value_as_a_new_rdn() {
        // The injection this guards against: a CN whose value contains a
        // newline. `Format($true)` quotes such a value rather than escaping
        // the newline, so a line-at-a-time parser would see a second line
        // reading `O=Bitwarden Inc."` and report a Bitwarden organization
        // that is really just the tail of somebody's common name.
        let dn = "CN=\"Evil\r\nO=Bitwarden Inc.\"\r\nO=Definitely Not Bitwarden\r\nC=US";
        assert!(
            dn_component(dn, "O") == vec!["Definitely Not Bitwarden"],
            "a forged O= smuggled inside a CN was accepted: {:?}",
            dn_component(dn, "O")
        );
        assert_eq!(
            dn_component(dn, "CN"),
            vec!["Evil\nO=Bitwarden Inc."],
            "the whole quoted value should stay with its own key"
        );
    }

    #[test]
    fn a_forged_organization_inside_a_common_name_is_not_trusted() {
        // The end-to-end version of the parser test above, at the level the
        // decision is actually made.
        let info = SignatureInfo {
            valid: true,
            thumbprint: None,
            subject_dn: Some("CN=\"Evil\r\nO=Bitwarden Inc.\"\r\nO=Evil Corp\r\nC=US".to_string()),
        };
        assert!(!is_trusted_organization(&info, &["Bitwarden Inc."]));
    }

    #[test]
    fn dn_component_keeps_an_escaped_quote_inside_a_value() {
        // `CertNameToStr` doubles an embedded quote; the parser must treat
        // `""` as one literal quote rather than as the end of the value.
        let dn = "O=\"Say \"\"hello\"\", Inc.\"\r\nC=US";
        assert_eq!(dn_component(dn, "O"), vec!["Say \"hello\", Inc."]);
    }

    #[test]
    fn trusts_an_exact_organization_match() {
        let info = SignatureInfo {
            valid: true,
            thumbprint: None,
            subject_dn: Some("CN=bw.exe\r\nO=Bitwarden Inc.\r\nC=US".to_string()),
        };
        assert!(is_trusted_organization(
            &info,
            &["Bitwarden Inc.", "8bit Solutions LLC"]
        ));
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
        let info = SignatureInfo {
            valid: true,
            thumbprint: None,
            subject_dn: None,
        };
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
        assert!(path.starts_with(
            std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string())
        ));
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

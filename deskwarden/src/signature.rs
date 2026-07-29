use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Security::Cryptography::{
    CertCloseStore, CertFindCertificateInStore, CertFreeCertificateContext,
    CertGetCertificateContextProperty, CertNameToStrW, CryptMsgClose, CryptMsgGetParam,
    CryptQueryObject, CERT_CONTEXT, CERT_FIND_SUBJECT_CERT, CERT_INFO, CERT_NAME_STR_CRLF_FLAG,
    CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED, CERT_QUERY_FORMAT_FLAG_BINARY,
    CERT_QUERY_OBJECT_FILE, CERT_SHA1_HASH_PROP_ID, CERT_STRING_TYPE, CERT_X500_NAME_STR,
    CMSG_SIGNER_INFO, CMSG_SIGNER_INFO_PARAM, HCERTSTORE, PKCS_7_ASN_ENCODING, X509_ASN_ENCODING,
};
use windows::Win32::Security::WinTrust::{
    WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0,
    WINTRUST_FILE_INFO, WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
    WTD_STATEACTION_VERIFY, WTD_UI_NONE,
};

#[derive(Debug, Clone)]
pub struct SignatureInfo {
    pub valid: bool,
    pub thumbprint: Option<String>,
    /// The signer certificate's subject DN, one RDN per line (`CERT_X500_NAME_STR`
    /// with `CERT_NAME_STR_CRLF_FLAG`). `None` when there is no signer
    /// certificate at all.
    pub subject_dn: Option<String>,
}

/// A NUL-terminated UTF-16 copy of `path`, for the Win32 calls below.
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Verifies a file's Authenticode signature via `WinVerifyTrust` and reads
/// the signer certificate out of the embedded PKCS#7 message.
///
/// This used to shell out to PowerShell's `Get-AuthenticodeSignature`, which
/// looked like the lower-risk choice (no WinTrust FFI to get wrong) but is
/// not: that cmdlet lives in the `Microsoft.PowerShell.Security` module, and
/// when module autoloading is unavailable -- a restricted execution
/// environment, a sandbox, a clobbered `PSModulePath` -- the call fails with
/// "the module could not be loaded" rather than a verdict. A trust gate that
/// cannot answer is a trust gate that blocks the user, so the check now
/// depends only on the OS APIs the cmdlet itself wraps, and on no external
/// process at all.
///
/// `Err` means the answer is unknown (the file could not be read as a signed
/// object); `Ok(info)` with `valid: false` means the OS gave a verdict and it
/// was "not trusted". Both are refusals at the call site -- see `main`'s
/// startup check -- but only the first is worth telling the user to
/// investigate.
pub fn verify_authenticode(path: &Path) -> Result<SignatureInfo, String> {
    let path_w = wide(path);
    let valid = win_verify_trust(&path_w);
    // Read the certificate even when the verdict is "not trusted": the whole
    // point of the error path is telling the user *who* signed the thing
    // they're being warned about.
    let (thumbprint, subject_dn) = match signer_certificate(&path_w) {
        Ok(pair) => pair,
        Err(e) if !valid => return Err(e),
        Err(_) => (None, None),
    };

    Ok(SignatureInfo {
        valid,
        thumbprint,
        subject_dn,
    })
}

/// The trust verdict proper: does this file carry a signature that chains to
/// a trusted root and covers the file's current contents?
///
/// Revocation checking is deliberately off (`WTD_REVOKE_NONE`): it would make
/// every app start depend on reaching a CRL/OCSP responder, so a captive
/// portal or an offline laptop would turn into "deskwarden refuses to start".
fn win_verify_trust(path_w: &[u16]) -> bool {
    unsafe {
        let mut file_info = WINTRUST_FILE_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: PCWSTR(path_w.as_ptr()),
            hFile: HANDLE::default(),
            pgKnownSubject: std::ptr::null_mut(),
        };
        let mut data = WINTRUST_DATA {
            cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_NONE,
            dwUnionChoice: WTD_CHOICE_FILE,
            Anonymous: WINTRUST_DATA_0 {
                pFile: &mut file_info,
            },
            dwStateAction: WTD_STATEACTION_VERIFY,
            ..Default::default()
        };
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;

        let status = WinVerifyTrust(
            HWND::default(),
            &mut action,
            &mut data as *mut _ as *mut c_void,
        );

        // WinVerifyTrust allocates provider state on the VERIFY call and only
        // releases it when asked; skipping this leaks a handle per check.
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        let _ = WinVerifyTrust(
            HWND::default(),
            &mut action,
            &mut data as *mut _ as *mut c_void,
        );

        status == 0
    }
}

/// Pulls the signer certificate's SHA-1 thumbprint and subject DN out of the
/// file's embedded PKCS#7 signature.
///
/// The signer is found the way the OS does it -- take the issuer and serial
/// number from the message's signer info, then look *that* certificate up in
/// the message's own store -- rather than grabbing the first certificate
/// present, which would as happily hand back an intermediate CA.
fn signer_certificate(path_w: &[u16]) -> Result<(Option<String>, Option<String>), String> {
    unsafe {
        let mut store = HCERTSTORE::default();
        let mut msg: *mut c_void = std::ptr::null_mut();
        CryptQueryObject(
            CERT_QUERY_OBJECT_FILE,
            path_w.as_ptr() as *const c_void,
            CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED,
            CERT_QUERY_FORMAT_FLAG_BINARY,
            0,
            None,
            None,
            None,
            Some(&mut store),
            Some(&mut msg),
            None,
        )
        .map_err(|e| format!("the file carries no readable Authenticode signature ({e})"))?;

        // Everything below shares one exit path so the store/message handles
        // are always released.
        let result = (|| -> Result<(Option<String>, Option<String>), String> {
            let mut size = 0u32;
            CryptMsgGetParam(msg, CMSG_SIGNER_INFO_PARAM, 0, None, &mut size)
                .map_err(|e| format!("could not size the signer info ({e})"))?;
            let mut buffer = vec![0u8; size as usize];
            CryptMsgGetParam(
                msg,
                CMSG_SIGNER_INFO_PARAM,
                0,
                Some(buffer.as_mut_ptr() as *mut c_void),
                &mut size,
            )
            .map_err(|e| format!("could not read the signer info ({e})"))?;

            let signer = &*(buffer.as_ptr() as *const CMSG_SIGNER_INFO);
            let mut find = CERT_INFO::default();
            find.Issuer = signer.Issuer;
            find.SerialNumber = signer.SerialNumber;

            let cert = CertFindCertificateInStore(
                store,
                CERT_QUERY_ENCODING,
                0,
                CERT_FIND_SUBJECT_CERT,
                Some(&find as *const _ as *const c_void),
                None,
            );
            if cert.is_null() {
                return Err("the signature names a signer that is not in its own \
                            certificate store"
                    .to_string());
            }

            let out = (thumbprint_of(cert), subject_dn_of(cert));
            let _ = CertFreeCertificateContext(Some(cert));
            Ok(out)
        })();

        let _ = CryptMsgClose(Some(msg));
        let _ = CertCloseStore(store, 0);
        result
    }
}

/// The encoding Authenticode messages and certificates use.
const CERT_QUERY_ENCODING: windows::Win32::Security::Cryptography::CERT_QUERY_ENCODING_TYPE =
    windows::Win32::Security::Cryptography::CERT_QUERY_ENCODING_TYPE(
        X509_ASN_ENCODING.0 | PKCS_7_ASN_ENCODING.0,
    );

/// SHA-1 thumbprint as uppercase hex, matching the form
/// `EXPECTED_SIGNER_THUMBPRINT` is written in (and what certificate UIs show).
unsafe fn thumbprint_of(cert: *const CERT_CONTEXT) -> Option<String> {
    let mut size = 0u32;
    CertGetCertificateContextProperty(cert, CERT_SHA1_HASH_PROP_ID, None, &mut size).ok()?;
    let mut hash = vec![0u8; size as usize];
    CertGetCertificateContextProperty(
        cert,
        CERT_SHA1_HASH_PROP_ID,
        Some(hash.as_mut_ptr() as *mut c_void),
        &mut size,
    )
    .ok()?;
    Some(hash.iter().map(|b| format!("{b:02X}")).collect())
}

/// Subject DN, one RDN per line -- the same shape `dn_component` parses.
unsafe fn subject_dn_of(cert: *const CERT_CONTEXT) -> Option<String> {
    let subject = &(*(*cert).pCertInfo).Subject;
    let str_type = CERT_STRING_TYPE(CERT_X500_NAME_STR.0 | CERT_NAME_STR_CRLF_FLAG);

    // Returns the character count *including* the NUL terminator, so the
    // second call gets a buffer of exactly that size and the result is
    // trimmed back below.
    let len = CertNameToStrW(CERT_QUERY_ENCODING, subject, str_type, None);
    if len <= 1 {
        return None;
    }
    let mut buffer = vec![0u16; len as usize];
    let written = CertNameToStrW(CERT_QUERY_ENCODING, subject, str_type, Some(&mut buffer));
    if written == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(
        &buffer[..written.saturating_sub(1) as usize],
    ))
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
    fn an_unsigned_file_is_never_reported_as_valid() {
        // The test binary itself: built locally, so definitionally unsigned.
        // Whether that surfaces as Err (no signature to read) or Ok(valid:
        // false) is the OS's business; what must never happen is a `true`.
        let me = std::env::current_exe().expect("no path for the test binary");
        if let Ok(info) = verify_authenticode(&me) {
            assert!(!info.valid, "an unsigned binary was reported as trusted");
        }
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_verdict() {
        let missing = std::env::temp_dir().join("deskwarden-does-not-exist-xyz.exe");
        assert!(verify_authenticode(&missing).is_err());
    }

    #[test]
    fn verification_needs_no_external_process() {
        // A guard on the reason this module was rewritten: the check used to
        // shell out to PowerShell's Get-AuthenticodeSignature, which fails
        // wholesale wherever Microsoft.PowerShell.Security can't autoload.
        // Nothing here may spawn a process again.
        let source = include_str!("signature.rs");
        let body = source.split("mod tests").next().unwrap_or_default();
        assert!(
            !body.contains("Command::new"),
            "signature verification must not depend on an external process"
        );
    }
}

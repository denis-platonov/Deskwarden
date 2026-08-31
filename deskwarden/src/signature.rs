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

/// Organization (`O=`) values accepted as proof that a `bw.exe` really is
/// Bitwarden's own CLI.
///
/// # Why it lives here and not in `main.rs`
///
/// It used to be a private constant in `main.rs`, with exactly one reader:
/// the startup check that verifies whatever `bw_path::resolve_bw_exe` found.
/// There are two readers now. `bw_acquire::verify_is_bitwardens` checks a
/// freshly-downloaded copy **before installing it**, and it must reach the
/// same verdict as startup will reach on the same file at the next launch.
/// A second list is how those two come apart, and the direction they come
/// apart in is the bad one: the app installs a binary that its own startup
/// then refuses to run, or -- worse -- the reverse.
///
/// So the list moved to the module that owns the comparison
/// ([`is_trusted_organization`]) rather than to either caller. Neither caller
/// may carry its own; `bw_acquire`'s
/// `the_trusted_organizations_are_not_duplicated_in_this_module` pins that
/// for the new one.
///
/// It also used to mirror `$BitwardenSignerOrganizations` in
/// `installer/bootstrap-bw.ps1`, "kept in sync by hand, not shared code".
/// **That script is gone**, along with the second Authenticode mechanism and
/// the second hand-maintained DN parser that went with it, so there is no
/// hand-synchronised copy left anywhere. Pinning the *path* `bw.exe` is
/// resolved from is not enough on its own: the install tree's `bin`
/// directory is user-writable, so anything able to plant a file beside
/// `deskwarden.exe` can just as easily overwrite `bin\bw.exe`. This is the
/// check that actually matters -- whatever ends up at that path must be
/// signed by Bitwarden before it is handed the user's master password
/// (`login_ui::run_bw_with_password`) or session token.
///
/// **Entry 1, `"Bitwarden Inc."`, is verified.** Checked 2026-08-10 against
/// a real, currently-valid Bitwarden-signed `bw.exe`. Measured twice: once
/// with `Get-AuthenticodeSignature`, and once through
/// [`verify_authenticode`] itself, so what is recorded here is what
/// production reads rather than what a cmdlet formats.
///
/// ```text
/// valid      : true
/// O=         : Bitwarden Inc.       (CN= happens to be identical -- see below)
/// thumbprint : 80375A0C9630A51ECB7EC79B37A8174C8DACCCED
/// issuer     : CN=DigiCert Trusted G4 Code Signing RSA4096 SHA384 2021 CA1,
///              O="DigiCert, Inc.", C=US
/// notAfter   : 2027-07-30T16:59:59Z
/// ```
///
/// That certificate's subject DN is pinned verbatim as
/// `REAL_BITWARDEN_CLI_SUBJECT_DN` in `main.rs`'s test module, and the tests
/// there run the *production* comparison -- [`is_trusted_organization`]
/// against this very constant -- over it, so a well-meant retyping of the
/// string here fails the suite. Note the expiry: one certificate, on one
/// machine, that stops existing in 2027 -- and the organization spelling on
/// its replacement is not knowable today.
///
/// **The other four entries remain unverified** -- plausible spellings nobody
/// here has seen on a real Bitwarden certificate. They are kept rather than
/// trimmed: `8bit Solutions LLC` is Bitwarden's documented former legal name,
/// the punctuation variants cover the way DN spelling drifts between
/// issuances, and each is an *exact whole-`O=`-component* match on a name a
/// public CA had to validate before issuing -- so the breadth they add is
/// narrow, while dropping them would turn a legitimate older or
/// differently-punctuated Bitwarden certificate into a scary dialog, and a
/// dialog users learn to click through is worse than the entry.
/// `"Bitwarden"` alone is the weakest of the four and the first that should
/// go if this list is ever tightened.
///
/// Because four of the five entries are *still unverified* -- and the fifth
/// is verified only against a single certificate that expires in 2027 -- a
/// mismatch is deliberately **not** treated the same way
/// `updater::EXPECTED_SIGNER_THUMBPRINT` treats a bad update signature. See
/// `main::check_bw_signature` for the graded response at startup: an
/// unsigned or tamper-detected binary is refused outright, but "validly
/// signed by an organization this unverified list does not happen to name"
/// asks the user instead of killing a tray app with no console.
///
/// **`bw_acquire` does not grade.** A binary it just downloaded and cannot
/// prove is Bitwarden's is deleted, not queried about -- the startup grading
/// exists because a file the user already had may predate this list, and a
/// file this app fetched thirty seconds ago has no such excuse.
pub const TRUSTED_BW_SIGNER_ORGANIZATIONS: &[&str] = &[
    "Bitwarden Inc.",
    "Bitwarden, Inc.",
    "Bitwarden Inc",
    "Bitwarden",
    "8bit Solutions LLC",
];

/// True if `info` is a validly-chained signature whose subject DN names one
/// of `trusted_orgs` in its `O=` component -- an exact whole-component match,
/// never a substring search. The finding this preserves was first written
/// down against the installer's own copy of this check (now deleted, see
/// [`TRUSTED_BW_SIGNER_ORGANIZATIONS`]): an unanchored substring match would
/// accept any validly-signed binary whose subject merely *contains* one of
/// these words, from an unrelated but legitimately-issued certificate.
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
        // The production half. `BELOW_CUT_MARKER` rather than a bare literal:
        // written out, the needle was a second occurrence of the very string
        // it cuts at, and this guard's own line would have become a candidate
        // cut site. See `nothing_but_gated_test_modules_lives_below_the_guards_cut`.
        let body = source.split(BELOW_CUT_MARKER).next().unwrap_or_default();
        assert!(
            body.len() < source.len(),
            "the test-module marker was not found, so this guard is scanning its own fixtures"
        );
        assert!(
            !body.contains("Command::new"),
            "signature verification must not depend on an external process"
        );
    }
    // -----------------------------------------------------------------
    // The region BELOW the cut -- the half no source guard here reads.
    // -----------------------------------------------------------------

    /// The `cfg` attribute that makes a module test-only, split so this
    /// constant is not itself one and cannot be found by a guard looking for
    /// the real attribute.
    const BELOW_CUT_GATE: &str = concat!("#[cfg(", "test)]");

    /// The literal every source guard in this file cuts the file at. Split so
    /// it is not itself an occurrence of the thing it names -- unsplit it
    /// would be a SECOND occurrence in this file, and the uniqueness control
    /// below could not be written at all.
    const BELOW_CUT_MARKER: &str = concat!("mod te", "sts {");

    /// Column-0 lines below the cut that are the CONTENTS OF A STRING LITERAL
    /// rather than source. Each is controlled below: it must still occur in
    /// this file exactly once, so a stale entry cannot quietly widen the hole
    /// the walk exists to close.
    const BELOW_CUT_STRING_LINES: &[&str] = &[];

    /// `true` for `mod NAME {`, `pub mod NAME {` and `pub(crate) mod NAME {`,
    /// and for nothing else. Deliberately exact rather than a `starts_with`:
    /// a whole module written on one line is not a module opener as far as
    /// this walk is concerned, and must fail it.
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

    /// The two-state walk of everything from the cut to EOF, over whatever
    /// text it is handed. Returns `(visited, modules, closes, depth)` so the
    /// caller can control it for non-vacuity.
    ///
    /// **Line-ending agnostic on purpose.** `lines()` strips a trailing
    /// carriage return, so every comparison here is against the line's real
    /// text on a CRLF working tree and on an LF one alike. The blobs this
    /// repository stores are LF and only `core.autocrlf=true` makes a working
    /// tree CRLF, so a needle written with a carriage return in it would match
    /// nothing on a plain checkout -- green, and reading nothing.
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
        top_level_item_note: "Every source guard in this file slices at the test-module opener and reads only what is ABOVE it, so an item down here is read by none of them: it can shell out to the very process this module was rewritten to stop depending on, or reintroduce a construct banned by name, and the suite stays green.",
        ungated_module_note: "A `pub(crate) mod ext { .. }` written down here is the same escape, one `mod` deep.",
    };

    /// `(visited, modules, closes, depth)` for the region below this file's
    /// cut, by the one shared walk.
    fn walk_below_the_cut(source: &str) -> (usize, usize, usize, usize) {
        let cut = source
            .find(BELOW_CUT_MARKER)
            .expect("the cut marker is controlled by the caller");
        crate::below_cut::walk(&source[cut..], &BELOW_CUT_RULES)
    }

    /// **Below the cut there is nothing but gated test modules, and the cut is
    /// where every guard in this file believes it is.**
    ///
    /// Two things can silently empty every source guard in this file, and
    /// neither changes a single guard's own text:
    ///
    /// 1. **Anything appended below the test module is invisible to all of
    ///    them.** They read the half above the cut and nothing else. Measured
    ///    on the commit before this test existed, a one-line tuple struct
    ///    appended at EOF gives 1772 lib + 169 bin, 0 failed, 0 warnings.
    /// 2. **The cut can move UP.** The slice takes the FIRST occurrence of the
    ///    marker, so the marker appearing in a comment or a string above the
    ///    real test module truncates the production half and vacates every
    ///    guard downstream of the truncation -- silently, because the guards
    ///    whose needles still fall inside go on passing.
    ///
    /// The walk closes the first; the uniqueness and anchor controls close the
    /// second.
    #[test]
    fn nothing_but_gated_test_modules_lives_below_the_guards_cut() {
        let source: &str = include_str!("signature.rs");

        // 1. The cut lands where the guards think it does, and there is only
        //    one place it could land.
        let seen = source.matches(BELOW_CUT_MARKER).count();
        assert_eq!(
            seen, 1,
            "the cut marker occurs {seen} times in this file. Every guard here takes the FIRST \
             one, so a second occurrence -- in a comment, in a string, in a doc example -- is \
             a cut that can move up and truncate the production half all of them read"
        );
        let cut = source
            .find(BELOW_CUT_MARKER)
            .expect("counted exactly one just above");
        assert!(
            cut > 0 && source.as_bytes()[cut - 1] == b'\n',
            "the cut landed in the MIDDLE of a line, so the marker was matched inside a \
             comment or a string literal rather than at a real module opener"
        );
        assert!(
            source[..cut].trim_end().ends_with(BELOW_CUT_GATE),
            "the module the cut lands on is not preceded by the test gate, so the region below \
             the cut opens with a module that SHIPS"
        );

        // 2. Positive control on WHERE the cut is: the production half must
        //    still reach the last production item in the file. Were the marker
        //    matched above the real test module, this anchor would fall below
        //    the cut instead of just above it.
        const LAST_PRODUCTION_ITEM: &str = concat!(".any(|org| trusted_orgs.iter().any(|t| t.", "eq_ignore_ascii_case(org)))");
        assert_eq!(
            source.matches(LAST_PRODUCTION_ITEM).count(),
            1,
            "control: the anchor (the trusted-organisation comparison) is not in this file exactly once, \
             so it no longer pins anything -- repoint it at the last production item above the \
             test module"
        );
        let anchor = source
            .find(LAST_PRODUCTION_ITEM)
            .expect("counted just above");
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

        // 3. The walk, run over an LF copy of this file and a CRLF copy of the
        //    same text, which must agree. Built BOTH ways rather than compared
        //    against the bytes on disk on purpose: this repository stores LF
        //    blobs and only `core.autocrlf=true` makes a working tree CRLF, so
        //    a control that asserted "this file is CRLF" would itself be a
        //    check that passes on this machine and fails on Linux CI -- which
        //    is the defect being closed here, wearing the other hat.
        let lf = source.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        assert_ne!(
            lf, crlf,
            "control: the two copies are the same string, so comparing the walk over them \
             compares it with itself -- this file has no line endings at all"
        );
        let as_lf = walk_below_the_cut(&lf);
        let as_crlf = walk_below_the_cut(&crlf);
        assert_eq!(
            as_lf, as_crlf,
            "the walk gives a different answer on an LF copy of this file than on a CRLF one, \
             so something in it is sensitive to line endings"
        );
        // And the file as it really is on disk, whichever of the two that is.
        let as_on_disk = walk_below_the_cut(source);
        assert!(
            as_on_disk == as_lf || as_on_disk == as_crlf,
            "this file's line endings are mixed: the walk over it agrees with neither the \
             all-LF nor the all-CRLF copy of its own text"
        );

        // 4. The walk is not vacuous, and it finished.
        let (visited, modules, closes, depth) = as_on_disk;
        assert!(
            visited > 100,
            "control: the walk visited only {visited} lines below the cut, which is not a test \
             module's worth -- the slice is empty or nearly so and this test proves nothing"
        );
        assert_eq!(
            depth, 0,
            "a test module below the cut is never closed by a column-0 brace, so the walk ran \
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
        // An INDENTED top-level item, which a column-0-only filter would miss.
        // The payload is an indented, GATED module opener and not a
        // `struct`: a struct is refused whether or not indentation is
        // checked, because it is not a module opener either way, so it left
        // the indentation rule unmeasured. This shape the opener predicate
        // accepts, so only the indentation rule can refuse it -- and the
        // trailing column-0 `}` makes the payload one the walk would
        // otherwise ACCEPT, so deleting the rule reds this control.
        let indented =
            format!("{source}\n{BELOW_CUT_GATE}\n    mod sneaked_indented {{\n}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&indented)).is_err(),
            "control: the walk accepted an INDENTED, gated module opener appended below \
             the test modules, which a column-0-only filter would miss"
        );
        // A column-0 line INSIDE the last test module that this file does
        // not name in its string-literal allowance. The line is planted by
        // dropping the file's final column-0 `}` and writing it back after
        // the payload, so the braces still balance and the module's real
        // close is still the last line -- the ONLY thing that refuses it is
        // the allowance being an exact list rather than a permission.
        // Measured: without this the `string_lines` rule was held by one
        // test in the whole crate, so a mutation plus deleting that test
        // were the two edits that opened it.
        let unlisted = format!(
            "{}zz_not_source\n}}\n",
            source
                .replace("\r\n", "\n")
                .strip_suffix("}\n")
                .expect("this file ends with a column-0 closing brace")
        );
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&unlisted)).is_err(),
            "control: the walk accepted a column-0 line inside a test module that this \
             file's string-literal allowance does not name, so the allowance is a \
             permission and not a list"
        );
        // Liveness control at the IDENTICAL site: the SAME planting, walked
        // with this file's own rules except that the planted line is named in
        // the allowance, is ACCEPTED. So the refusal above is about the
        // allowance and not about the planting having broken the region.
        // This file's real `BELOW_CUT_STRING_LINES` is empty, so the naming
        // has to be done here rather than read off the constant.
        let naming_it = crate::below_cut::WalkRules {
            string_lines: &["zz_not_source"],
            ..BELOW_CUT_RULES
        };
        let cut_of_unlisted =
            unlisted.find(BELOW_CUT_MARKER).expect("the marker survives the planting");
        assert!(
            crate::below_cut::try_walk(&unlisted[cut_of_unlisted..], &naming_it).is_ok(),
            "control: the walk refuses the planted region even when the line IS named in \
             the allowance, so the refusal above is not measuring the allowance"
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
        // both profiles, and shipping in the lib's DEBUG LLVM IR. Only the
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

//! **An organisation invite link, read** (official-path spec §4.1) -- what
//! Shared with me's Paste an invite link hands the daemon, which accepts with
//! it on Bitwarden's own route, `POST /organizations/{organizationId}/users/
//! {organizationUserId}/accept {token}`.
//!
//! # The format
//!
//! Bitwarden's server writes the link in `HandlebarsMailService.
//! BuildInvitationUrl` (bitwarden/server main, src/Core/Platform/Mail/
//! HandlebarsMailService.cs, lines 875-893):
//!
//! `{vault}/#/accept-organization?organizationId=…&organizationUserId=…&email=…&organizationName=…&token=…&initOrganization=True|False&orgUserHasExistingUser=True|False`
//!
//! -- the query inside the FRAGMENT; `email`, `organizationName` and `token`
//! through .NET's `WebUtility.UrlEncode`, where a space is `+`. NodeWarden
//! writes the same. Vaultwarden writes `…/#/accept-organization/?…` with
//! `email` first and `%20` for a space. So this reads the query wherever it
//! stands -- the fragment's, or the URL's own -- under a path that names
//! `accept-organization`, with keys in any case and values form-decoded.
//! Both ids must be GUIDs, as the official web vault's
//! `DirectOrganizationInvite.fromUrlParams` requires, which also keeps any
//! other text out of a route's path.
//!
//! # The token
//!
//! It is what accepts the invitation, so it is held in a `Zeroizing<String>`,
//! and [`InviteLink`]'s `Debug` prints no part of it -- nor the email or the
//! ids, which nothing that debugs a link needs. Nothing here logs.
//!
//! Pure: no I/O, no clock, no window.

use zeroize::Zeroizing;

/// One invite link, read. See the module docs.
pub struct InviteLink {
    /// A GUID, lower-cased.
    pub organization_id: String,
    /// The invitation's org-user id: a GUID, lower-cased.
    pub organization_user_id: String,
    /// The address invited, trimmed and lower-cased -- what the daemon
    /// compares with the signed-in account's before it asks the server.
    pub email: String,
    /// The organisation's name, as the link carries it. Plaintext anyone
    /// could have set: shown as a claim, never trusted.
    pub organization_name: Option<String>,
    pub token: Zeroizing<String>,
}

/// **Hand-written**: the token accepts the invitation, and the email and the
/// ids say whom it is for. Only the organisation's name -- the one part a
/// person reading a log could want, and plaintext on every server -- is shown.
impl std::fmt::Debug for InviteLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InviteLink").field("organization_name", &self.organization_name).finish_non_exhaustive()
    }
}

/// Why a pasted text is not a link this can accept with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkError {
    /// No `accept-organization` path with a query: some other text or page.
    NotAnInviteLink,
    /// The link lacks this key, or it is empty (an email without an `@`
    /// counts as missing).
    Missing(&'static str),
    /// This id is not a GUID.
    NotAnId(&'static str),
}

impl LinkError {
    /// The sentence Shared with me shows. A link cut short and a link with a
    /// bad id read alike: either way, the fix is to paste the whole link.
    #[must_use]
    pub fn sentence(&self) -> &'static str {
        match self {
            Self::NotAnInviteLink => {
                "That isn't an organisation invite link. Paste the whole link from the invitation email."
            }
            Self::Missing(_) | Self::NotAnId(_) => {
                "That invite link is incomplete. Paste the whole link from the invitation email."
            }
        }
    }
}

/// A GUID as Bitwarden writes one: 8-4-4-4-12 hex digits.
fn is_guid(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => *b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// **Reads a pasted invite link.** See the module docs for the formats.
///
/// # Errors
///
/// [`LinkError`]: not an invite link, a part missing, or an id that is not a
/// GUID.
pub fn parse(text: &str) -> Result<InviteLink, LinkError> {
    let text = text.trim().trim_start_matches('<').trim_end_matches('>').trim();
    let (outside, fragment) = match text.split_once('#') {
        Some((outside, fragment)) => (outside, Some(fragment)),
        None => (text, None),
    };
    // The fragment's query first (Bitwarden, Vaultwarden, NodeWarden), else
    // the URL's own.
    let (path, query) = match fragment.and_then(|fragment| fragment.split_once('?')) {
        Some(found) => found,
        None => outside.split_once('?').ok_or(LinkError::NotAnInviteLink)?,
    };
    if !path.to_ascii_lowercase().contains("accept-organization") {
        return Err(LinkError::NotAnInviteLink);
    }
    let (mut organization_id, mut organization_user_id, mut email, mut organization_name, mut token) = (None, None, None, None, None);
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        let value = value.trim();
        match key.to_ascii_lowercase().as_str() {
            "organizationid" => organization_id = Some(value.to_ascii_lowercase()),
            "organizationuserid" => organization_user_id = Some(value.to_ascii_lowercase()),
            "email" => email = Some(value.to_lowercase()),
            "organizationname" => organization_name = Some(value.to_string()).filter(|name| !name.is_empty()),
            "token" => token = Some(Zeroizing::new(value.to_string())),
            _ => {}
        }
    }
    let organization_id = organization_id.filter(|id| !id.is_empty()).ok_or(LinkError::Missing("organizationId"))?;
    let organization_user_id = organization_user_id.filter(|id| !id.is_empty()).ok_or(LinkError::Missing("organizationUserId"))?;
    let email = email.filter(|email| email.split_once('@').is_some_and(|(local, domain)| !local.is_empty() && !domain.is_empty()));
    let email = email.ok_or(LinkError::Missing("email"))?;
    let token = token.filter(|token| !token.is_empty()).ok_or(LinkError::Missing("token"))?;
    if !is_guid(&organization_id) {
        return Err(LinkError::NotAnId("organizationId"));
    }
    if !is_guid(&organization_user_id) {
        return Err(LinkError::NotAnId("organizationUserId"));
    }
    Ok(InviteLink { organization_id, organization_user_id, email, organization_name, token })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORG: &str = "6f8a3c2e-1b4d-4e5f-9a7b-0c1d2e3f4a5b";
    const OU: &str = "a1b2c3d4-e5f6-4789-8abc-def012345678";

    /// **A link exactly as Bitwarden's server writes it**
    /// (`HandlebarsMailService.BuildInvitationUrl`): the query in the
    /// fragment, `email` and `organizationName` through .NET's
    /// `WebUtility.UrlEncode` (a space is `+`), a DataProtector token, the
    /// booleans as C# prints them.
    #[test]
    fn a_link_from_a_bitwarden_email_is_read() {
        let link = format!(
            "https://vault.bitwarden.com/#/accept-organization?organizationId={ORG}&organizationUserId={OU}\
             &email=Anna%40Example.com&organizationName=Shared+by+denis%40example.com\
             &token=BwOrgUserInviteToken_CfDJ8Kx-Q9vR3pZ_e2A7hT0wLmN1yF4uB6cD8gH0jK2lM4nP6qS8tU0vW2xY4zA6bC8dE0fG2hI4\
             &initOrganization=False&orgUserHasExistingUser=True"
        );
        let read = parse(&link).expect("a Bitwarden link");
        assert_eq!(read.organization_id, ORG);
        assert_eq!(read.organization_user_id, OU);
        assert_eq!(read.email, "anna@example.com", "lower-cased");
        assert_eq!(read.organization_name.as_deref(), Some("Shared by denis@example.com"));
        assert_eq!(
            read.token.as_str(),
            "BwOrgUserInviteToken_CfDJ8Kx-Q9vR3pZ_e2A7hT0wLmN1yF4uB6cD8gH0jK2lM4nP6qS8tU0vW2xY4zA6bC8dE0fG2hI4"
        );
    }

    /// **Vaultwarden's link** (`mail.rs` `send_invite`): a `/` before the `?`,
    /// `email` first, `%20` rather than `+`, a JWT token, and the extra keys
    /// it adds.
    #[test]
    fn a_link_from_a_vaultwarden_email_is_read() {
        let link = format!(
            "https://vw.example.org/#/accept-organization/?email=anna%40example.com&organizationName=Shared%20by%20denis%40example.com\
             &organizationId={ORG}&organizationUserId={OU}&token=eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ4In0.c2ln\
             &initOrganization=false&orgUserHasExistingUser=true"
        );
        let read = parse(&link).expect("a Vaultwarden link");
        assert_eq!(read.organization_name.as_deref(), Some("Shared by denis@example.com"));
        assert_eq!(read.token.as_str(), "eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ4In0.c2ln");
        assert_eq!(read.email, "anna@example.com");
    }

    /// **NodeWarden's link** (`invite-mail.ts` `invitationUrl`): Bitwarden's
    /// format, with NodeWarden's own token.
    #[test]
    fn a_link_from_a_nodewarden_email_is_read() {
        let token = "0f1e2d3c-4b5a-4968-8776-655443322110a9b8c7d6-e5f4-4321-8fed-cba987654321";
        let link = format!(
            "https://nw.example.net/#/accept-organization?organizationId={ORG}&organizationUserId={OU}&email=anna%40example.com\
             &organizationName=Shared+by+denis%40example.com&token={token}&initOrganization=False&orgUserHasExistingUser=False"
        );
        assert_eq!(parse(&link).expect("a NodeWarden link").token.as_str(), token);
    }

    /// **Keys in any case, and a query outside the fragment**: a link a mail
    /// client rewrote, or one copied from a web vault that routes by path.
    #[test]
    fn keys_in_any_case_and_a_query_outside_the_fragment_are_read() {
        let link = format!(
            "https://vault.example.com/accept-organization?ORGANIZATIONID={}&OrganizationUserId={OU}&EMAIL=anna%40example.com&TOKEN=t-1",
            ORG.to_uppercase()
        );
        let read = parse(&link).expect("read");
        assert_eq!(read.organization_id, ORG, "a GUID is compared lower-cased");
        assert_eq!(read.organization_name, None, "the name is not required");
        assert_eq!(read.token.as_str(), "t-1");
    }

    /// **What a paste carries around the link** -- spaces, a newline, angle
    /// brackets some mail clients add -- is not part of it.
    #[test]
    fn a_pasted_link_is_trimmed() {
        let link = format!("  <https://v.example/#/accept-organization?organizationId={ORG}&organizationUserId={OU}&email=a%40b.c&token=t>\r\n");
        assert_eq!(parse(&link).expect("read").email, "a@b.c");
    }

    /// **Not an invite link**: text, another vault page, an
    /// `accept-organization` with no query.
    #[test]
    fn what_is_not_an_invite_link_is_refused() {
        for text in ["", "hello", "https://vault.example/#/vault?organizationId=x", "https://vault.example/#/accept-organization"] {
            assert!(matches!(parse(text), Err(LinkError::NotAnInviteLink)), "{text:?}");
        }
        assert_eq!(
            LinkError::NotAnInviteLink.sentence(),
            "That isn't an organisation invite link. Paste the whole link from the invitation email."
        );
    }

    /// **A link cut short** is refused for what it lacks.
    #[test]
    fn a_link_missing_a_part_is_refused() {
        let whole = format!("https://v.example/#/accept-organization?organizationId={ORG}&organizationUserId={OU}&email=a%40b.c&token=t");
        for (cut, part) in [("&token=t", "token"), ("&email=a%40b.c", "email")] {
            assert_eq!(parse(&whole.replace(cut, "")).err(), Some(LinkError::Missing(part)), "{part}");
        }
        assert_eq!(parse(&whole.replace("a%40b.c", "nobody")).err(), Some(LinkError::Missing("email")), "an email has an @");
        assert_eq!(parse(&whole.replace("token=t", "token=")).err(), Some(LinkError::Missing("token")));
        assert_eq!(
            LinkError::Missing("token").sentence(),
            "That invite link is incomplete. Paste the whole link from the invitation email."
        );
    }

    /// **Both ids must be GUIDs**, as the official web vault requires
    /// (`DirectOrganizationInvite.fromUrlParams`'s `isId`) -- and as a path
    /// segment needs: nothing else reaches a route.
    #[test]
    fn ids_that_are_not_guids_are_refused() {
        let with = |org: &str, ou: &str| format!("https://v.example/#/accept-organization?organizationId={org}&organizationUserId={ou}&email=a%40b.c&token=t");
        assert_eq!(parse(&with("../x", OU)).err(), Some(LinkError::NotAnId("organizationId")));
        assert_eq!(parse(&with(ORG, "a1b2c3d4")).err(), Some(LinkError::NotAnId("organizationUserId")));
        assert_eq!(parse(&with(ORG, "a1b2c3d4-e5f6-4789-8abc-def01234567z")).err(), Some(LinkError::NotAnId("organizationUserId")));
    }

    /// **A link's `Debug` prints no token, no email and no id** -- the token
    /// accepts the invitation, and nothing that debugs a link needs the rest.
    #[test]
    fn a_links_debug_prints_no_token_and_no_address() {
        let link = format!(
            "https://v.example/#/accept-organization?organizationId={ORG}&organizationUserId={OU}&email=anna%40example.com&token=TOKEN-NEEDLE-31b"
        );
        let printed = format!("{:?}", parse(&link).expect("read"));
        for secret in ["TOKEN-NEEDLE-31b", "anna@example.com", ORG, OU] {
            assert!(!printed.contains(secret), "{printed}");
        }
    }
}

//! **Bitwarden's URI match rules: does one saved website match the page in
//! the address bar?**
//!
//! The account picker's ranking asks this, and nothing else does. It decides
//! which row of the picker is on top -- never whether anything is typed. That
//! is what makes reading the address bar safe at all: a wrong answer here costs
//! the user one arrow key, because the fill still waits for their key press.
//! `match_engine` is untouched by it and still matches no browser unattended.
//!
//! # The six rules, transcribed from Bitwarden's clients rather than invented
//!
//! `LoginUriView.matchesUri` in Bitwarden's `libs/common`:
//!
//! * **Base domain** (`0`, and the absent key): the registrable domains --
//!   eTLD+1, by the public-suffix list -- are equal. `localhost` and IP
//!   addresses have no registrable domain, so they compare by host ALONE:
//!   Bitwarden's `Utils.getDomain` returns the hostname without a port, so a
//!   login saved for `localhost:3000` is offered on `localhost:8080` too. (The
//!   Host rule below does compare the port.)
//!   Bitwarden asks the list with private suffixes ON (`allowPrivateDomains`),
//!   which is what keeps `alice.github.io` and `bob.github.io` apart, and the
//!   `psl` crate's compiled list carries that section.
//!   Its `DomainMatchBlacklist` applies here and to no other rule: a login
//!   saved for `google.com` is not offered on `script.google.com`.
//! * **Host** (`1`): host and port equal. The scheme is not compared, exactly
//!   as Bitwarden's `getHost` does not: `http://example.com` saved matches
//!   `https://example.com` in the address bar.
//! * **Starts with** (`2`) and **Exact** (`3`): the page URL as a string,
//!   against the saved string as typed. Case-sensitive, as Bitwarden is.
//! * **Regular expression** (`4`): the saved pattern, case-insensitive (the
//!   `i` flag Bitwarden compiles it with), against the page URL. A pattern this
//!   crate's `regex` cannot compile -- including JavaScript-only syntax such as
//!   look-behind -- matches nothing. Rust's `\d` and `\w` are Unicode classes,
//!   not ASCII as in JavaScript; that is harmless for URLs, which are ASCII once
//!   parsed.
//! * **Never** (`5`): nothing.
//!
//! A saved website without a scheme is read as `http://` for the two host
//! rules, which is Bitwarden's `getUrl` rule. Those two rules also ignore a
//! saved website whose scheme is not `http` or `https` (`androidapp://...`).
//!
//! # What is not here, on purpose
//!
//! The account's own "default URI match detection" setting and equivalent
//! domains (`google.com` and `youtube.com`) are out of scope by the owner's
//! decision: an entry with no match type is base domain, always.
//!
//! **Nothing in this module logs**, and `the_matcher_writes_no_log_line` holds
//! it: both of its inputs are websites the user visits.

use url::{Host, Url};

/// Bitwarden's six match rules, by the names its clients show.
///
/// The wire numbers are NOT spelled here. They are read through
/// [`crate::vault_window::detail_edit::UriMatchChoice::from_wire`], the edit
/// form's own reader, so `0`..`5` are transcribed in one place in this crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MatchType {
    BaseDomain,
    Host,
    StartsWith,
    Exact,
    RegularExpression,
    Never,
}

impl MatchType {
    /// What a URI entry's `match` value asks for. `None` is "no match type":
    /// the key is absent or `null`, and the matcher reads it as base domain.
    ///
    /// **A value this build cannot name is [`MatchType::Never`]**, not base
    /// domain. A rule written by a later Bitwarden, or by hand, that this
    /// build does not understand must not pre-pick a login on a guess about
    /// what it meant.
    pub fn from_wire(value: Option<&serde_json::Value>) -> Option<Self> {
        use crate::vault_window::detail_edit::UriMatchChoice as Choice;
        match Choice::from_wire(value) {
            Choice::Default => None,
            Choice::BaseDomain => Some(Self::BaseDomain),
            Choice::Host => Some(Self::Host),
            Choice::StartsWith => Some(Self::StartsWith),
            Choice::Exact => Some(Self::Exact),
            Choice::RegularExpression => Some(Self::RegularExpression),
            Choice::Never | Choice::Unrecognised => Some(Self::Never),
        }
    }
}

/// One saved website of a login, and the rule it was saved with: what
/// [`matches`] needs and nothing more. Carried by `app::ItemFacts`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedUri {
    /// The website exactly as saved -- not parsed, not trimmed. Exact and
    /// starts-with compare this string.
    pub uri: String,
    /// `None` when the entry carries no match type.
    pub match_type: Option<MatchType>,
}

impl SavedUri {
    /// The entry as the matcher needs it, or `None` for an entry with no
    /// `uri` at all.
    pub fn of(entry: &crate::vault_bridge::UriEntry) -> Option<Self> {
        Some(Self {
            uri: entry.uri.clone()?,
            match_type: MatchType::from_wire(entry.other.get("match")),
        })
    }
}

/// The largest compiled program a saved pattern may become, in bytes.
///
/// `regex` runs in time linear in the input whatever the pattern, so there is
/// no catastrophic backtracking to defend against; what is left is a pattern
/// whose compiled form is huge. A megabyte is far past any URL rule a person
/// writes, and a pattern past it matches nothing, like an invalid one.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// Whether the website `saved`, with the rule `match_type`, matches `page`.
pub fn matches(saved: &str, match_type: Option<MatchType>, page: &Url) -> bool {
    // An empty entry matches nothing under any rule. Bitwarden's
    // `if (!this.uri) return false` -- without it, "starts with" an empty
    // string would match every page.
    if saved.trim().is_empty() {
        return false;
    }
    match match_type.unwrap_or(MatchType::BaseDomain) {
        MatchType::BaseDomain => match (saved_url(saved), base_domain(page)) {
            (Some(saved), Some(page_domain)) => {
                base_domain(&saved) == Some(page_domain) && !blacklisted(&saved, page)
            }
            _ => false,
        },
        MatchType::Host => match (saved_url(saved), host_and_port(page)) {
            (Some(saved), Some(page)) => host_and_port(&saved) == Some(page),
            _ => false,
        },
        MatchType::StartsWith => page.as_str().starts_with(saved),
        MatchType::Exact => page.as_str() == saved,
        MatchType::RegularExpression => regex::RegexBuilder::new(saved)
            .case_insensitive(true)
            .size_limit(REGEX_SIZE_LIMIT)
            .build()
            .is_ok_and(|pattern| pattern.is_match(page.as_str())),
        MatchType::Never => false,
    }
}

/// A saved website as a web URL for the two host rules.
///
/// It is parsed as written first, and only when that fails -- or yields a URL
/// with no host, which is what `localhost:3000` does (scheme `localhost`, path
/// `3000`) -- is it retried with `http://` in front. A test for the substring
/// `://` was the first version and was wrong: `example.com/r?u=https://other.com`
/// contains one, and is a scheme-less website with a redirect in its query.
///
/// **A scheme that is not `http` or `https` yields `None`**: `androidapp://com.example.app`
/// parses with a host, and must not match a page on `example.app`. A scheme-less
/// string has been given `http://` by then, so it is never caught by this.
fn saved_url(saved: &str) -> Option<Url> {
    let saved = saved.trim();
    let url = match Url::parse(saved) {
        Ok(url) if url.has_host() => url,
        _ => Url::parse(&format!("http://{saved}")).ok()?,
    };
    matches!(url.scheme(), "http" | "https").then_some(url)
}

/// Bitwarden's `DomainMatchBlacklist`: saved base domain -> page hosts that the
/// base-domain rule must NOT match it with, though the registrable domains are
/// equal. `script.google.com` runs arbitrary user scripts, so a login saved for
/// `google.com` is not offered there. Only the base-domain rule asks.
const DOMAIN_MATCH_BLACKLIST: &[(&str, &[&str])] = &[("google.com", &["script.google.com"])];

/// Whether `page`'s host is excluded for the website `saved` (already known to
/// share `page`'s registrable domain).
fn blacklisted(saved: &Url, page: &Url) -> bool {
    let (Some(domain), Some(host)) = (base_domain(saved), host_and_port(page).map(|(host, _)| host)) else {
        return false;
    };
    DOMAIN_MATCH_BLACKLIST
        .iter()
        .any(|(listed, hosts)| *listed == domain && hosts.contains(&host.as_str()))
}

/// The host, lowercased and without a trailing dot, and the port as the URL
/// spells it -- `None` for the scheme's default, so `https://x` and
/// `https://x:443` are one place, as they are to a browser.
fn host_and_port(url: &Url) -> Option<(String, Option<u16>)> {
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    Some((host, url.port()))
}

/// What base domain compares: the registrable domain, or -- for `localhost`
/// and an IP address, which have none -- the host, WITHOUT a port.
///
/// Bitwarden's `Utils.getDomain` returns the hostname for those, and a
/// hostname has no port, so `localhost:3000` and `localhost:8080` are one
/// domain to the base-domain rule. (The Host rule is the one that compares
/// ports: see [`host_and_port`].)
///
/// **The IP arm is not an optimisation.** The public-suffix list knows nothing
/// about addresses: asked about `192.168.1.10` it answers `1.10`, so every
/// address ending in `.1.10` would be one "domain".
fn base_domain(url: &Url) -> Option<String> {
    match url.host()? {
        Host::Ipv4(_) | Host::Ipv6(_) => host_and_port(url).map(|(host, _)| host),
        Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.');
            if domain.eq_ignore_ascii_case("localhost") {
                return host_and_port(url).map(|(host, _)| host);
            }
            psl::domain_str(domain).map(|registrable| registrable.to_ascii_lowercase())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(text: &str) -> Url {
        Url::parse(text).expect("the fixture page is a URL")
    }

    fn base(saved: &str, at: &str) -> bool {
        matches(saved, Some(MatchType::BaseDomain), &page(at))
    }

    #[test]
    fn base_domain_matches_a_subdomain_of_the_saved_domain() {
        assert!(base("https://google.com", "https://accounts.google.com/signin"));
        assert!(base("https://accounts.google.com", "https://mail.google.com/"));
        assert!(!base("https://google.com", "https://google.org/"), "another TLD is another site");
    }

    #[test]
    fn no_match_type_is_base_domain() {
        assert!(matches("https://google.com", None, &page("https://accounts.google.com/")));
        assert!(!matches("https://google.com", None, &page("https://example.com/")));
    }

    #[test]
    fn base_domain_uses_the_public_suffix_list_and_not_the_last_two_labels() {
        assert!(base("https://bbc.co.uk", "https://www.bbc.co.uk/account"));
        assert!(
            !base("https://bbc.co.uk", "https://other.co.uk/"),
            "`co.uk` is a public suffix: two sites under it are two sites"
        );
        assert!(base("https://example.co.jp", "https://a.b.example.co.jp/"));
    }

    #[test]
    fn base_domain_keeps_private_suffixes_apart() {
        assert!(
            !base("https://alice.github.io", "https://bob.github.io/"),
            "`github.io` is a private suffix: alice's login must not be offered on bob's page"
        );
        assert!(base("https://alice.github.io", "https://alice.github.io/blog/"));
    }

    #[test]
    fn a_saved_website_without_a_scheme_is_read_as_http() {
        assert!(base("bbc.co.uk", "https://www.bbc.co.uk/"));
        assert!(matches("accounts.google.com", Some(MatchType::Host), &page("https://accounts.google.com/")));
    }

    #[test]
    fn base_domain_compares_ip_addresses_by_address_and_not_by_port() {
        assert!(base("192.168.1.10", "https://192.168.1.10/login"));
        assert!(!base("192.168.1.10", "https://192.168.1.11/"));
        assert!(
            !base("10.0.1.10", "https://192.168.1.10/"),
            "the suffix list reads both of these as `1.10`; an address is not a domain"
        );
        assert!(base("192.168.1.10:8443", "https://192.168.1.10:8443/"));
        assert!(
            base("192.168.1.10:8443", "https://192.168.1.10:9443/"),
            "Bitwarden's getDomain returns the hostname, which has no port"
        );
        assert!(base("https://[::1]:8080", "http://[::1]:8080/"));
        assert!(base("https://[::1]:8080", "http://[::1]:9090/"));
        assert!(!base("https://[::1]:8080", "http://[::2]:8080/"));
    }

    #[test]
    fn base_domain_compares_localhost_by_host_and_not_by_port() {
        assert!(base("http://localhost:3000", "http://localhost:3000/login"));
        assert!(
            base("http://localhost:3000", "http://localhost:8080/"),
            "Bitwarden's getDomain returns the hostname, which has no port"
        );
        assert!(base("localhost", "https://localhost/"));
        assert!(base("localhost:3000", "http://localhost:8080/"));
    }

    #[test]
    fn host_still_compares_the_port_for_localhost_and_ip_addresses() {
        let host = |saved: &str, at: &str| matches(saved, Some(MatchType::Host), &page(at));
        assert!(host("http://localhost:3000", "http://localhost:3000/login"));
        assert!(!host("http://localhost:3000", "http://localhost:8080/"));
        assert!(host("192.168.1.10:8443", "https://192.168.1.10:8443/"));
        assert!(!host("192.168.1.10:8443", "https://192.168.1.10:9443/"));
    }

    #[test]
    fn a_saved_website_is_parsed_as_written_before_http_is_put_in_front() {
        assert!(
            base("example.com/r?u=https://other.com", "https://www.example.com/"),
            "a `://` in the query does not make the website a URL with a scheme"
        );
        assert!(
            !base("example.com/r?u=https://other.com", "https://other.com/"),
            "and it is example.com that was saved, not other.com"
        );
        assert!(base("localhost:3000", "http://localhost:3000/"));
        assert!(base("example.com:8443", "https://example.com/"));
    }

    #[test]
    fn google_com_is_not_offered_on_script_google_com_under_base_domain() {
        assert!(!base("https://google.com", "https://script.google.com/home"));
        assert!(!base("google.com", "https://script.google.com/"));
        assert!(
            !base("https://accounts.google.com", "https://script.google.com/"),
            "the blacklist is keyed by the saved website's base domain, whatever its subdomain"
        );
        assert!(base("https://google.com", "https://mail.google.com/"), "control: the rest of google.com still matches");
        assert!(
            !base("https://script.google.com", "https://script.google.com/"),
            "as in Bitwarden, the key is the saved base domain: a login for script.google.com itself needs the Host rule"
        );
        assert!(
            base("https://google.com", "https://other.script.google.com/"),
            "control: Bitwarden's set holds exactly the one host, as written"
        );
    }

    #[test]
    fn the_blacklist_belongs_to_base_domain_alone() {
        let page = page("https://script.google.com/home");
        assert!(matches("https://script.google.com", Some(MatchType::Host), &page));
        assert!(matches("https://script.google.com", Some(MatchType::StartsWith), &page));
        assert!(matches("^https://script\\.google\\.com/", Some(MatchType::RegularExpression), &page));
        assert!(matches("https://script.google.com/home", Some(MatchType::Exact), &page));
    }

    #[test]
    fn base_domain_and_host_ignore_a_saved_website_that_is_not_http_or_https() {
        let at = page("https://example.app/");
        for rule in [None, Some(MatchType::BaseDomain), Some(MatchType::Host)] {
            assert!(!matches("androidapp://com.example.app", rule, &at), "{rule:?}");
            assert!(!matches("androidapp://example.app", rule, &at), "{rule:?}");
            assert!(!matches("ftp://example.app/", rule, &at), "{rule:?}");
        }
        assert!(matches("example.app", Some(MatchType::BaseDomain), &at), "control: no scheme is http");
        assert!(matches("http://example.app", Some(MatchType::Host), &at), "control: http is a web scheme");
    }

    #[test]
    fn base_domain_does_not_match_a_bare_public_suffix() {
        assert!(
            !base("https://co.uk", "https://bbc.co.uk/"),
            "a public suffix has no registrable domain, so it matches nothing"
        );
    }

    #[test]
    fn internationalised_domains_compare_in_their_ascii_form() {
        assert!(base("https://bücher.de", "https://shop.xn--bcher-kva.de/"));
        assert!(base("https://xn--bcher-kva.de", "https://bücher.de/"));
    }

    #[test]
    fn host_needs_the_same_host_and_port_and_ignores_the_scheme() {
        let host = |saved: &str, at: &str| matches(saved, Some(MatchType::Host), &page(at));
        assert!(host("https://accounts.google.com", "https://accounts.google.com/x"));
        assert!(!host("https://accounts.google.com", "https://mail.google.com/"));
        assert!(host("example.com:8443", "https://example.com:8443/"));
        assert!(!host("example.com:8443", "https://example.com/"));
        assert!(host("http://example.com", "https://example.com/"), "Bitwarden's host rule has no scheme in it");
        assert!(host("https://EXAMPLE.com", "https://example.com/"));
    }

    #[test]
    fn starts_with_compares_the_page_url_as_a_string() {
        let starts = |saved: &str, at: &str| matches(saved, Some(MatchType::StartsWith), &page(at));
        assert!(starts("https://example.com/app", "https://example.com/app/login"));
        assert!(!starts("https://example.com/app", "https://example.com/other"));
        assert!(!starts("https://example.com/App", "https://example.com/app/"), "case-sensitive, as Bitwarden is");
    }

    #[test]
    fn exact_needs_the_whole_url() {
        let exact = |saved: &str, at: &str| matches(saved, Some(MatchType::Exact), &page(at));
        assert!(exact("https://example.com/login", "https://example.com/login"));
        assert!(!exact("https://example.com/login", "https://example.com/login?next=1"));
    }

    #[test]
    fn a_regular_expression_matches_case_insensitively() {
        let re = |saved: &str, at: &str| matches(saved, Some(MatchType::RegularExpression), &page(at));
        assert!(re(r"^https://(www\.)?example\.com/", "https://www.example.com/login"));
        assert!(re(r"^HTTPS://EXAMPLE\.COM/", "https://example.com/"));
        assert!(!re(r"^https://example\.com/", "https://example.org/"));
    }

    #[test]
    fn an_invalid_regular_expression_matches_nothing() {
        let re = |saved: &str| matches(saved, Some(MatchType::RegularExpression), &page("https://example.com/"));
        assert!(!re("("), "an unbalanced group is not a pattern");
        assert!(!re("(?<=a)b"), "JavaScript's look-behind is not this crate's syntax");
        assert!(re("example"), "control: a valid pattern does match this page");
    }

    #[test]
    fn never_matches_nothing() {
        assert!(!matches("https://example.com", Some(MatchType::Never), &page("https://example.com/")));
    }

    #[test]
    fn an_empty_or_unreadable_saved_website_matches_nothing() {
        for rule in [None, Some(MatchType::Host), Some(MatchType::StartsWith), Some(MatchType::RegularExpression)] {
            assert!(!matches("", rule, &page("https://example.com/")), "{rule:?}");
            assert!(!matches("   ", rule, &page("https://example.com/")), "{rule:?}");
        }
        assert!(!base("http://exa mple.com", "https://example.com/"));
    }

    #[test]
    fn the_match_value_is_read_with_the_edit_forms_own_reader() {
        use serde_json::json;
        assert_eq!(MatchType::from_wire(None), None);
        assert_eq!(MatchType::from_wire(Some(&json!(null))), None);
        assert_eq!(MatchType::from_wire(Some(&json!(0))), Some(MatchType::BaseDomain));
        assert_eq!(MatchType::from_wire(Some(&json!(1))), Some(MatchType::Host));
        assert_eq!(MatchType::from_wire(Some(&json!(2))), Some(MatchType::StartsWith));
        assert_eq!(MatchType::from_wire(Some(&json!(3))), Some(MatchType::Exact));
        assert_eq!(MatchType::from_wire(Some(&json!(4))), Some(MatchType::RegularExpression));
        assert_eq!(MatchType::from_wire(Some(&json!(5))), Some(MatchType::Never));
        assert_eq!(
            MatchType::from_wire(Some(&json!(7))),
            Some(MatchType::Never),
            "a rule this build cannot read must not pre-pick anything"
        );
        assert_eq!(MatchType::from_wire(Some(&json!("exact"))), Some(MatchType::Never));
    }

    #[test]
    fn a_saved_uri_carries_its_entrys_match_value() {
        let entry: crate::vault_bridge::UriEntry =
            serde_json::from_str(r#"{"uri":"https://example.com","match":3}"#).unwrap();
        assert_eq!(
            SavedUri::of(&entry),
            Some(SavedUri { uri: "https://example.com".to_string(), match_type: Some(MatchType::Exact) })
        );
        let no_uri: crate::vault_bridge::UriEntry = serde_json::from_str(r#"{"match":3}"#).unwrap();
        assert_eq!(SavedUri::of(&no_uri), None, "an entry with no website is not a website");
    }

    #[test]
    fn the_matcher_writes_no_log_line() {
        let source = include_str!("uri_match.rs");
        let production = source
            .split_once(concat!("#[cfg(", "test)]"))
            .map_or(source, |(above, _)| above);
        assert!(production.len() < source.len(), "control: the test gate was not found");
        assert!(
            !production.contains(concat!("log", "::")),
            "uri_match logs: both of its inputs are websites the user visits"
        );
    }
}

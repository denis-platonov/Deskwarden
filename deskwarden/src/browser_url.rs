//! **The page a browser window is showing, read from its address bar.**
//!
//! In a browser the executable is the same for every site and the window title
//! is written by the page, so neither may choose a credential (see
//! `match_engine`). The address bar is the one string in a browser window a
//! page cannot write: it is the browser's own chrome. When the user presses
//! `CTRL+ALT+B` in a browser, the account picker reads it once and ranks the
//! logins saved for that website first (`app_candidates`, `uri_match`).
//!
//! **It only ever decides which row is highlighted.** Nothing is typed until
//! the user presses a key on the card.
//!
//! # A page must not be able to pose as the address bar
//!
//! UI Automation exposes a web page's own elements in the same tree as the
//! browser's toolbar, with the page's `id` as the `AutomationId` and its
//! `aria-label` as the `Name`. A page that planted `<input id="urlbar-input"
//! value="https://bank.example">` would otherwise have the bank's login
//! pre-picked on the attacker's page -- which is the phishing that matching by
//! website is supposed to stop. So a candidate field counts only if it is part
//! of the browser's chrome: see [`is_browser_chrome`].
//!
//! # Two halves, in this crate's idiom
//!
//! [`parse`], [`family_of_class`], [`Family::address_field`] and
//! [`is_browser_chrome`] are pure and tested. `read` is the observation half --
//! one window-class lookup and one UI Automation query, in
//! `injector::ui_automation` beside the crate's other UI Automation helpers --
//! and decides nothing a test could not decide.
//!
//! **The address is never logged**, by this module or by its caller;
//! `the_address_is_never_logged` holds this file to it.

/// Which browser's chrome a window is drawn by, told by its top-level window
/// class rather than by its executable name.
///
/// **Not by the executable**, because `app::BROWSER_IMAGE_NAMES` is the one
/// place this crate names a browser -- `app` and `match_engine` both pin that
/// -- and a second table of image names here would be the "two enumerations
/// that must agree" defect those pins exist for. The window class is the
/// engine's own, and the engine is what decides which address field to look
/// for: every Chromium browser draws its window as `Chrome_WidgetWin_1`, every
/// Gecko browser as `MozillaWindowClass`. The caller has already asked
/// `app::browser_window` whether this IS a browser; Electron apps share
/// Chromium's class and are excluded there, not here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// Chrome, Edge, Brave, Opera, Vivaldi, Thorium, Chromium.
    Chromium,
    /// Firefox, LibreWolf, Floorp, Waterfox, Zen.
    Firefox,
}

/// The family a top-level window class belongs to, or `None` -- which is
/// every other browser, Internet Explorer's `IEFrame` included, and means "no
/// address is read".
pub fn family_of_class(class_name: &str) -> Option<Family> {
    if class_name.starts_with("Chrome_WidgetWin_") {
        Some(Family::Chromium)
    } else if class_name == "MozillaWindowClass" {
        Some(Family::Firefox)
    } else {
        None
    }
}

/// A UI Automation control type an address field may report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    /// `UIA_EditControlTypeId`.
    Edit,
    /// `UIA_ComboBoxControlTypeId`.
    ComboBox,
}

/// How one family's address field is found in its UI Automation tree: a
/// descendant of the window whose control type is one of [`Self::kinds`] AND
/// that has ANY of the listed property values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddressField {
    /// The control types the field may report. At least one must match.
    pub kinds: &'static [FieldKind],
    /// `UIA_ClassNamePropertyId` values.
    pub class_names: &'static [&'static str],
    /// `UIA_AutomationIdPropertyId` values.
    pub automation_ids: &'static [&'static str],
    /// `UIA_NamePropertyId` values.
    pub names: &'static [&'static str],
}

impl Family {
    /// **Exactly which properties are relied on, per family**, as measured on
    /// 2026-10-01 against Chrome, Edge, Brave and Firefox on Windows 11.
    ///
    /// * **Chromium.** The omnibox is an Edit whose `ClassName` is the views
    ///   class: `OmniboxViewViews` in Chrome and Edge, `BraveOmniboxViewViews`
    ///   in Brave. A class name is neither translated nor build-specific, so it
    ///   is the key. The English accessible name "Address and search bar" (all
    ///   three) is the fallback for a Chromium browser that renamed the class;
    ///   in another UI language it is translated and simply does not match.
    ///   **The `AutomationId` is NOT used**: it is `view_<n>`, a views
    ///   counter -- `view_1012` in Chrome and Brave, `view_1017` in Edge on the
    ///   same day -- which identifies nothing. Opera and Vivaldi draw parts of
    ///   their own toolbar; where neither key matches there, the picker ranks
    ///   as it did before this existed.
    /// * **Firefox.** The URL bar's input element has the id `urlbar-input`,
    ///   which Gecko reports as the `AutomationId`. It reports the control as
    ///   a **ComboBox**, not an Edit (it has a drop-down of suggestions), so
    ///   both kinds are accepted. Its name is the search engine's placeholder
    ///   ("Search with Google or enter address"), which changes with the
    ///   engine and the language, so it is not used.
    pub fn address_field(self) -> AddressField {
        match self {
            Family::Chromium => AddressField {
                kinds: &[FieldKind::Edit],
                class_names: &["OmniboxViewViews", "BraveOmniboxViewViews"],
                automation_ids: &[],
                names: &["Address and search bar"],
            },
            Family::Firefox => AddressField {
                kinds: &[FieldKind::ComboBox, FieldKind::Edit],
                class_names: &[],
                automation_ids: &["urlbar-input"],
                names: &[],
            },
        }
    }
}

/// How many ancestors, the window itself included, may stand between a
/// candidate field and the browser window.
///
/// Measured on 2026-10-01, counting the window: Firefox's URL bar is 4 steps
/// below its window, the Chromium omnibox 8 (Chrome, Edge and Brave alike). A
/// web page's own fields were 13 or more in every browser, because a page is
/// drawn inside a content pane the browser nests several levels down -- and no
/// page can make its elements shallower than the pane it is drawn in. 10
/// leaves room on both sides. A browser that one day nests its omnibox deeper than 10 loses the
/// pre-pick and ranks as before, which is the safe direction to fail in.
pub const MAX_FIELD_DEPTH: usize = 10;

/// **Whether a candidate field is the browser's own, and not a page's.**
///
/// `depth` is how many ancestors the walk up from the field took to reach the
/// browser window (the window included), or `None` if it did not reach it
/// within [`MAX_FIELD_DEPTH`]. `crosses_a_document` is whether any ancestor on
/// the way was a UI Automation `Document` -- which is what a web page is
/// exposed as, and which the toolbar never sits inside.
///
/// Both are required. A page's element is inside a Document in every case
/// measured but one (a Firefox tab whose page was exposed without its
/// Document, 13 deep), and the depth bound is what refuses that one.
pub fn is_browser_chrome(depth: Option<usize>, crosses_a_document: bool) -> bool {
    !crosses_a_document && depth.is_some_and(|depth| depth <= MAX_FIELD_DEPTH)
}

/// The longest address-bar text that is parsed at all, in bytes.
///
/// A sign-in page's address is a few hundred characters. The field can hold a
/// pasted `data:` URL of megabytes, and nothing on that path is a website a
/// login was saved for.
pub const MAX_ADDRESS_LEN: usize = 8192;

/// The address-bar text as a URL, or `None` when it is not a web address.
///
/// * trimmed;
/// * `https://` is put in front of text with no `://` -- browsers hide the
///   scheme, so `example.com/login` is what the field often holds;
/// * only `http` and `https` are kept: `chrome://settings`, `about:blank`,
///   `file:///` and `edge://newtab` are not websites;
/// * text with whitespace in it is a search the user is typing, not an
///   address;
/// * the host must be `localhost`, an IP address, or a name with a dot in it
///   -- `intranet` and `printer` are single words the user typed, which a
///   browser would search for;
/// * anything the `url` crate does not parse is `None`.
///
/// **Never trusted for anything but choosing a highlighted row.**
pub fn parse(text: &str) -> Option<url::Url> {
    let text = text.trim();
    if text.is_empty() || text.len() > MAX_ADDRESS_LEN || text.chars().any(char::is_whitespace) {
        return None;
    }
    let url = if text.contains("://") {
        url::Url::parse(text).ok()?
    } else {
        url::Url::parse(&format!("https://{text}")).ok()?
    };
    if url.scheme() != "http" && url.scheme() != "https" {
        return None;
    }
    let looks_like_a_site = match url.host()? {
        url::Host::Ipv4(_) | url::Host::Ipv6(_) => true,
        url::Host::Domain(name) => name.eq_ignore_ascii_case("localhost") || name.contains('.'),
    };
    looks_like_a_site.then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> Option<String> {
        parse(text).map(|url| url.to_string())
    }

    #[test]
    fn a_bare_address_is_read_as_https() {
        assert_eq!(parsed("example.com/login"), Some("https://example.com/login".to_string()));
        assert_eq!(parsed("example.com"), Some("https://example.com/".to_string()));
    }

    #[test]
    fn the_text_is_trimmed() {
        assert_eq!(parsed("  https://example.com/a  "), Some("https://example.com/a".to_string()));
    }

    #[test]
    fn http_and_https_are_kept_as_they_are() {
        assert_eq!(parsed("http://example.com/"), Some("http://example.com/".to_string()));
        assert_eq!(parsed("https://example.com/"), Some("https://example.com/".to_string()));
    }

    #[test]
    fn other_schemes_are_not_websites() {
        for text in ["chrome://settings", "edge://newtab", "about:blank", "file:///C:/x.html", "ftp://example.com/"] {
            assert_eq!(parsed(text), None, "{text}");
        }
    }

    #[test]
    fn a_search_is_not_an_address() {
        assert_eq!(parsed("how to reset my password"), None);
        assert_eq!(parsed("example.com login"), None, "a space anywhere makes it a search");
        assert_eq!(parsed("intranet"), None, "one word without a dot is what a browser searches for");
        assert_eq!(parsed("intranet/login"), None);
    }

    #[test]
    fn localhost_and_ip_addresses_are_addresses() {
        assert_eq!(parsed("localhost:3000/login"), Some("https://localhost:3000/login".to_string()));
        assert_eq!(parsed("http://localhost/"), Some("http://localhost/".to_string()));
        let ip = parse("192.168.1.1:8443/admin").expect("an IPv4 address with a port");
        assert_eq!(ip.host(), Some(url::Host::Ipv4(std::net::Ipv4Addr::new(192, 168, 1, 1))));
        assert_eq!(ip.port(), Some(8443));
        assert!(parse("[::1]:8080").is_some(), "an IPv6 literal has no dot and is still an address");
    }

    #[test]
    fn ports_are_kept() {
        let url = parse("example.com:8443/x").expect("a host with a port");
        assert_eq!(url.port(), Some(8443));
        assert_eq!(parse("https://example.com:443/").unwrap().port(), None, "the default port is no port");
    }

    #[test]
    fn an_internationalised_domain_becomes_its_ascii_form() {
        assert_eq!(parse("bücher.de").unwrap().host_str(), Some("xn--bcher-kva.de"));
    }

    #[test]
    fn empty_and_oversized_text_is_nothing() {
        assert_eq!(parsed(""), None);
        assert_eq!(parsed("   "), None);
        let long = format!("example.com/{}", "a".repeat(MAX_ADDRESS_LEN));
        assert_eq!(parsed(&long), None);
        let fits = format!("example.com/{}", "a".repeat(100));
        assert!(parse(&fits).is_some(), "control: a long-but-fitting address still parses");
    }

    #[test]
    fn the_family_is_told_by_the_window_class() {
        assert_eq!(family_of_class("Chrome_WidgetWin_1"), Some(Family::Chromium));
        assert_eq!(family_of_class("MozillaWindowClass"), Some(Family::Firefox));
        assert_eq!(family_of_class("IEFrame"), None, "Internet Explorer is not supported");
        assert_eq!(family_of_class("Notepad"), None);
        assert_eq!(family_of_class(""), None);
    }

    /// The properties are the documentation: this is the list a person
    /// checking a new browser build by hand compares against.
    #[test]
    fn each_family_looks_for_its_own_address_field() {
        let chromium = Family::Chromium.address_field();
        assert_eq!(chromium.kinds, &[FieldKind::Edit]);
        assert_eq!(chromium.class_names, &["OmniboxViewViews", "BraveOmniboxViewViews"]);
        assert!(chromium.automation_ids.is_empty(), "`view_<n>` is a views counter, not an id");
        assert_eq!(chromium.names, &["Address and search bar"]);
        let firefox = Family::Firefox.address_field();
        assert_eq!(firefox.kinds, &[FieldKind::ComboBox, FieldKind::Edit]);
        assert_eq!(firefox.automation_ids, &["urlbar-input"]);
        assert!(
            firefox.class_names.is_empty() && firefox.names.is_empty(),
            "Firefox's field name is the search engine's placeholder and is not a key"
        );
    }

    #[test]
    fn the_browsers_own_field_is_shallow_and_outside_any_document() {
        assert!(is_browser_chrome(Some(4), false), "Firefox's URL bar, as measured");
        assert!(is_browser_chrome(Some(8), false), "the Chromium omnibox, as measured");
        assert!(is_browser_chrome(Some(MAX_FIELD_DEPTH), false));
    }

    #[test]
    fn a_pages_field_is_never_the_address_bar() {
        assert!(!is_browser_chrome(Some(5), true), "inside a Document is a web page, at any depth");
        assert!(!is_browser_chrome(Some(13), false), "a page exposed without its Document is still too deep");
        assert!(!is_browser_chrome(Some(MAX_FIELD_DEPTH + 1), false));
        assert!(!is_browser_chrome(None, false), "a walk that never reached the window proves nothing");
    }

    #[test]
    fn the_address_is_never_logged() {
        let source = include_str!("browser_url.rs");
        let production = source
            .split_once(concat!("#[cfg(", "test)]"))
            .map_or(source, |(above, _)| above);
        assert!(production.len() < source.len(), "control: the test gate was not found");
        assert!(
            !production.contains(concat!("log", "::")),
            "browser_url logs: the address is the website the user is on"
        );
    }
}

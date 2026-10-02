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
//! [`parse`], [`family_of_class`], [`Family::address_field`],
//! [`is_browser_chrome`], [`reading_order`], [`document_verdict`] and
//! [`prefer_document`] are pure and tested. `read` is
//! the observation half -- one window-class lookup, one UI Automation query for
//! the address field and, in a Chromium browser whose omnibox showed an elided
//! address, one for the page's `Document`, all in `injector::ui_automation`
//! beside the crate's other UI Automation helpers -- and decides nothing a test
//! could not decide.
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
///
/// Any `Chrome_WidgetWin_*` is Chromium: `_1` is the class every measured
/// browser uses, but the suffix is a counter of Chromium's own and a build may
/// number its top-level window otherwise. `MozillaWindowClass` is the one
/// Gecko class.
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

    /// Whether [`read`] may ask the window's first `Document` for the page's
    /// full address when the address field showed it elided (see
    /// [`prefer_document`]).
    ///
    /// **Chromium only, as measured on 2026-10-01.** Chrome's first `Document`
    /// under the window held the active tab's URL. Firefox's first `Document`
    /// was a DIFFERENT tab's page -- so in the Firefox family the `Document`
    /// says nothing about the page in front of the user, and is never read.
    /// Firefox trims `https://` and `http://` from its `urlbar-input` too
    /// (`browser.urlbar.trimURLs`, `trimHttps`), so a trimmed Firefox address
    /// keeps its exact host and a guessed scheme: a [`PageAddress`] that is not
    /// `complete`.
    pub fn reads_the_page_document(self) -> bool {
        match self {
            Family::Chromium => true,
            Family::Firefox => false,
        }
    }
}

/// How many ancestors, the window itself included, may stand between a
/// candidate field and the browser window, **counted in UI Automation's RAW
/// view** -- the `RawViewWalker` that `injector::ui_automation`'s `ancestry`
/// walks. The control and content views leave out elements the raw view
/// counts, so a depth read through either is smaller and does not compare
/// with this bound; a hand measurement must use the raw view too.
///
/// Measured by hand on 2026-10-01 in the raw view, counting the window as the
/// first step:
/// - Chrome and Edge, normal tabbed windows: the `OmniboxViewViews` field is
///   at depth 8, with no `Document` above it.
/// - Firefox, normal windows: the `urlbar-input` field is at depth 4, with no
///   `Document` above it.
/// - Firefox popup windows and Chrome app/PWA windows: not yet measured.
///   `examples/address_bar_probe.rs` is the hand check for both.
///
/// A web page's own fields were 13 or more in every browser, because a page is
/// drawn inside a content pane the browser nests several levels down -- and no
/// page can make its elements shallower than the pane it is drawn in. 10
/// leaves room on both sides. A browser that one day nests its omnibox deeper than 10 loses the
/// pre-pick and ranks as before, which is the safe direction to fail in.
pub const MAX_FIELD_DEPTH: usize = 10;

/// **Whether a candidate field is the browser's own, and not a page's.**
///
/// `depth` is how many ancestors the walk up from the field took to reach the
/// browser window (the window included), or `None` if it did not reach it
/// within [`MAX_FIELD_DEPTH`]. Only `1..=MAX_FIELD_DEPTH` counts: the window
/// itself is the first step, so `Some(0)` describes a walk that never left the
/// field and proves nothing. `crosses_a_document` is whether any ancestor on
/// the way was a UI Automation `Document` -- which is what a web page is
/// exposed as, and which the toolbar never sits inside.
///
/// Both are required. A page's element is inside a Document in every case
/// measured but one (a Firefox tab whose page was exposed without its
/// Document, 13 deep), and the depth bound is what refuses that one.
pub fn is_browser_chrome(depth: Option<usize>, crosses_a_document: bool) -> bool {
    !crosses_a_document && depth.is_some_and(|depth| (1..=MAX_FIELD_DEPTH).contains(&depth))
}

/// **The order the examined candidates are read in**: the indices of those
/// [`is_browser_chrome`] keeps, SHALLOWEST first, ties in tree order. Each
/// entry of `walks` is one candidate's `(depth, crosses_a_document)`, in tree
/// order.
///
/// Not "the first kept in tree order", because Firefox lists the page's
/// content BEFORE its toolbar: a page field that slipped past the depth bound
/// would be read ahead of the real URL bar. Shallowest first closes that,
/// because a page's field can never be shallower than the content pane it is
/// drawn in, and the toolbar is shallower than that pane (4 against 13 or
/// more in Firefox on 2026-10-01). `injector::ui_automation`'s
/// `address_field_text` tries the indices in this order and reads the first
/// that gives a value.
pub fn reading_order(walks: &[(Option<usize>, bool)]) -> Vec<usize> {
    let mut kept: Vec<(usize, usize)> = walks
        .iter()
        .enumerate()
        .filter(|(_, (depth, crosses_a_document))| is_browser_chrome(*depth, *crosses_a_document))
        .filter_map(|(index, (depth, _))| depth.map(|depth| (depth, index)))
        .collect();
    // (depth, index): sorting the pairs is depth first, then tree order.
    kept.sort_unstable();
    kept.into_iter().map(|(_, index)| index).collect()
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
/// * `https://` is put in front of text that does not start with a scheme --
///   browsers hide it, so `example.com/login` is what the field often holds.
///   Text has a scheme only when what precedes its FIRST `://` is a valid one
///   (`[A-Za-z][A-Za-z0-9+.-]*`): `accounts.example.com/signin?continue=https://mail.example.com`
///   has a `://` too, but it is in the query of a scheme-less address. Trying
///   `Url::parse` first would be wrong the other way, because `localhost:3000`
///   and `example.com:8443/x` parse, with `localhost` and `example.com` as the
///   scheme;
/// * only `http` and `https` are kept: `chrome://settings`, `about:blank`,
///   `file:///` and `edge://newtab` are not websites;
/// * text with whitespace in it is a search the user is typing, not an
///   address;
/// * the host must be `localhost`, an IP address, or a name with a dot in it
///   -- `intranet` and `printer` are single words the user typed, which a
///   browser would search for. A trailing dot is not a dot: `intranet.` is
///   still a dotless name, while `example.com.` and `localhost.` are fine;
/// * anything the `url` crate does not parse is `None`;
/// * userinfo is dropped: a user who typed `user:pass@host` has put a password
///   in the field, and a `Url` that kept it would print it under `Debug`.
///
/// **Never trusted for anything but choosing a highlighted row.**
pub fn parse(text: &str) -> Option<url::Url> {
    let text = text.trim();
    if text.is_empty() || text.len() > MAX_ADDRESS_LEN || text.chars().any(char::is_whitespace) {
        return None;
    }
    let mut url = if has_scheme(text) {
        url::Url::parse(text).ok()?
    } else {
        url::Url::parse(&format!("https://{text}")).ok()?
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    if url.scheme() != "http" && url.scheme() != "https" {
        return None;
    }
    let looks_like_a_site = match url.host()? {
        url::Host::Ipv4(_) | url::Host::Ipv6(_) => true,
        url::Host::Domain(name) => {
            let name = name.trim_end_matches('.');
            name.eq_ignore_ascii_case("localhost") || name.contains('.')
        }
    };
    looks_like_a_site.then_some(url)
}

/// **The page a browser window is showing, and how much of its address is
/// known.**
///
/// The address bar does not always hold the whole address. Chrome's omnibox
/// shows `x.com` for `https://www.x.com/`, and Firefox trims `https://` and
/// `http://` too (`browser.urlbar.trimURLs`, `trimHttps`). [`parse`] puts
/// `https://` in front of such text, so the `url` it gives is right about the
/// host -- which the field shows exactly, `www.` aside -- and a GUESS about the
/// scheme, the `www.` and, in Chrome, the path.
///
/// `app_candidates::ranked` needs to know which: its rule that a login saved
/// for this very site, whose own rule refused the page, is not offered by the
/// title either, holds only for a refusal the guess cannot have caused. Exact
/// `http://x.com/login` "refusing" a guessed `https://x.com/` says nothing
/// about the real page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageAddress {
    /// The page's address, as well as it is known.
    pub url: url::Url,
    /// **Whether `url` is the browser's whole address**: the field's text had
    /// a scheme of its own (Edge; Chrome with "Always show full URLs"; Firefox
    /// with trimming off), or Chrome's page `Document` agreed with the field
    /// and supplied the address ([`DocumentVerdict::Agrees`]). `false` when
    /// the scheme was put in front of elided text by [`parse`] -- Firefox's
    /// trimmed text, or a Chromium field whose `Document` was unusable.
    pub complete: bool,
}

/// **The observation half**: the address in the browser window `hwnd`, or
/// `None` -- a browser this module has no field for, no field found, a field
/// that is a page's rather than the browser's, a UI Automation failure, or
/// text that is not a web address. None of those is an error the user sees;
/// each leaves the account picker ranking as it did before.
///
/// Called by `app::handle_no_match` once per `CTRL+ALT+B` press, and only
/// for a window `app::browser_window` says is a browser. Never on a focus
/// change, never in the background. No test can run it -- it needs a real
/// window and a real UI Automation tree -- so `examples/address_bar_probe.rs`
/// is its hand check.
///
/// # Chrome's omnibox holds elided text; the page's `Document` holds the URL
///
/// Measured live with UI Automation on 2026-10-01: the `ValuePattern` value of
/// Chrome's `OmniboxViewViews` is the DISPLAY text, `mail.google.com/...` --
/// no `https://`, and Chrome elides `www.` and `http://` too. Edge's omnibox
/// holds the full `https://...`. Chrome's first `Document` under the window
/// has a `ValuePattern` value with the full URL of the active tab,
/// `https://mail.google.com/...`; the browser sets it, and it is the document
/// URL. So in the Chromium family ([`Family::reads_the_page_document`]), and
/// only when the field's text came without a scheme, the `Document` is read as
/// well and [`prefer_document`] decides whether it may restore the scheme, the
/// `www.` and the path. **The address field stays the authority for the
/// host**: a `Document` that is a web page on another host never supplies an
/// address -- it only withdraws the field's, because then the field is holding
/// what the user is typing, not the page (see [`document_verdict`]). In Firefox
/// the first `Document` was another tab's page, so it is never read there.
///
/// The answer says whether it is the browser's whole address or a guess built
/// on elided text: [`PageAddress::complete`].
pub fn read(hwnd: isize) -> Option<PageAddress> {
    let class = crate::injector::target::class_name_of(hwnd)?;
    let family = family_of_class(&class)?;
    let text = crate::injector::ui_automation::address_field_text(hwnd, family.address_field())
        .ok()
        .flatten()?;
    let omnibox = parse(&text)?;
    let had_scheme = has_scheme(text.trim());
    let document = (family.reads_the_page_document() && !had_scheme)
        .then(|| crate::injector::ui_automation::page_document_text(hwnd).ok().flatten())
        .flatten();
    prefer_document(omnibox, had_scheme, document.as_deref())
}

/// What the window's first `Document` says about an elided address field.
/// See [`document_verdict`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentVerdict {
    /// The same page, spelled in full: this is the page's address.
    Agrees(url::Url),
    /// Nothing the address field can be checked against -- no `Document`, an
    /// empty one, one that is not a web page (`chrome://`, `devtools://`), or
    /// one on the field's own host behind another port. The field's text
    /// stands.
    Unusable,
    /// **A web page on another host.** The field is not showing the page: the
    /// user is typing another address into it. Nothing is the page's address.
    Disagrees,
}

/// **Whether the window's first `Document` confirms, cannot speak to, or
/// contradicts the address field's elided text.**
///
/// `omnibox` is the address field's text after [`parse`]; `document_text` is
/// the value of the window's first `Document`, when one was read (see [`read`]
/// for why, and for the 2026-10-01 measurement this rests on). Only asked when
/// the field's text had no scheme -- see [`prefer_document`].
///
/// * [`DocumentVerdict::Unusable`] when there is no document text, or it does
///   not itself start with a scheme, or [`parse`] refuses it: `http` or
///   `https` only (`chrome://`, `devtools://` and `about:` are not pages),
///   within [`MAX_ADDRESS_LEN`], no whitespace. Such a `Document` says nothing
///   about which web page is in front of the user, so it neither confirms nor
///   contradicts the field.
/// * [`DocumentVerdict::Agrees`] when its host, lowercased and without a
///   trailing dot, **equals** the address field's, or equals `www.` followed
///   by it -- the one prefix Chrome elides -- and its port is the field's.
///   Equality, never "ends with" or "contains":
///   `evil-mail.google.com.attacker.net` is not `mail.google.com`. Userinfo is
///   dropped, as [`parse`] drops it.
/// * The same host on another port is [`DocumentVerdict::Unusable`]: Chrome
///   shows a non-default port in the elided text, so that is another server
///   on the field's host -- a stale `Document` -- and the field, whose host is
///   right either way, stands.
/// * [`DocumentVerdict::Disagrees`] when it is a web page on any other host.
///
/// # Why a disagreeing page withdraws the address instead of being ignored
///
/// The `Document` is the page the browser is showing; the address field is
/// what it is showing ABOVE that page -- and while the user edits it, that is
/// whatever they have typed so far. Typing `bank.com` over
/// `https://news.example/` leaves an elided `bank.com` in the field and
/// `https://news.example/` in the `Document`: reading the field then would
/// rank the bank's login first on the news site. Ignoring the `Document` (the
/// first version of this function) did exactly that.
///
/// **The field is still the authority for the host.** A disagreeing
/// `Document` never supplies an address -- a page on another host could be a
/// side panel, docked devtools showing a page, or a document left over from
/// the previous page as easily as the real one -- it only takes the field's
/// away. The cost of a false alarm (a side panel on another `https` host) is
/// the picker ranking by the window title as it did before any address was
/// read, which is the safe direction to fail in.
pub fn document_verdict(omnibox: &url::Url, document_text: Option<&str>) -> DocumentVerdict {
    let Some(text) = document_text.map(str::trim) else {
        return DocumentVerdict::Unusable;
    };
    if !has_scheme(text) {
        return DocumentVerdict::Unusable;
    }
    let Some(document) = parse(text) else {
        return DocumentVerdict::Unusable;
    };
    let (Some(shown), Some(page)) = (comparable_host(omnibox), comparable_host(&document)) else {
        return DocumentVerdict::Unusable;
    };
    let same_host = page == shown || page.strip_prefix("www.") == Some(shown.as_str());
    if !same_host {
        DocumentVerdict::Disagrees
    } else if document.port() == omnibox.port() {
        DocumentVerdict::Agrees(document)
    } else {
        DocumentVerdict::Unusable
    }
}

/// **The page's address: the address field's, the page `Document`'s when
/// that is the same page spelled in full, or `None` when the `Document` shows
/// the field is holding typed text.**
///
/// `omnibox` is the address field's text after [`parse`]; `omnibox_had_scheme`
/// is whether that text started with a scheme; `document_text` is the value of
/// the window's first `Document`, when one was read.
///
/// * The field's text had a scheme (Edge, or Chrome with "Always show full
///   URLs"): it is already the full address, the `Document` is not consulted
///   at all, and `omnibox` is the answer, `complete`.
/// * Otherwise the [`document_verdict`] decides: `Agrees` gives the
///   document's URL, `complete` -- restoring what the elision hid: the scheme
///   (`http` for a plain-HTTP site the field showed bare), the `www.`, and the
///   full path and query; `Unusable` keeps `omnibox`, NOT complete, because
///   its scheme is [`parse`]'s guess; `Disagrees` is `None`, and the picker
///   ranks by the title as it did before.
///
/// So the `Document` can restore what the field elided, or withdraw the
/// field's address, and nothing else: it never names a host the field did not.
/// The Firefox family reaches here with no document text, so its trimmed text
/// is `Unusable`'s case: the field's host, not complete.
pub fn prefer_document(
    omnibox: url::Url,
    omnibox_had_scheme: bool,
    document_text: Option<&str>,
) -> Option<PageAddress> {
    if omnibox_had_scheme {
        return Some(PageAddress { url: omnibox, complete: true });
    }
    match document_verdict(&omnibox, document_text) {
        DocumentVerdict::Agrees(document) => Some(PageAddress { url: document, complete: true }),
        DocumentVerdict::Unusable => Some(PageAddress { url: omnibox, complete: false }),
        DocumentVerdict::Disagrees => None,
    }
}

/// A URL's host as [`document_verdict`] compares it: lowercased, without a
/// trailing dot.
fn comparable_host(url: &url::Url) -> Option<String> {
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// Whether `text` starts with a URL scheme: what precedes its first `://` is
/// `[A-Za-z][A-Za-z0-9+.-]*`. Checked by hand, because this crate has no regex
/// need here and a dependency for one character class is not worth it.
fn has_scheme(text: &str) -> bool {
    let Some((scheme, _)) = text.split_once("://") else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
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
    fn a_scheme_less_address_may_carry_a_url_in_its_query_or_fragment() {
        let url = parse("accounts.example.com/signin?continue=https://mail.example.com/x")
            .expect("a sign-in page with a redirect in its query");
        assert_eq!(url.host_str(), Some("accounts.example.com"));
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.query(), Some("continue=https://mail.example.com/x"));
        let fragment = parse("example.com/#https://x").expect("a URL in the fragment");
        assert_eq!(fragment.host_str(), Some("example.com"));
        assert_eq!(parse("http://example.com/r?u=https://other.com").unwrap().scheme(), "http");
        // Controls: these parse with a bogus scheme under "try Url::parse first".
        assert_eq!(parse("localhost:3000/login").unwrap().host_str(), Some("localhost"));
        assert_eq!(parse("example.com:8443/x").unwrap().host_str(), Some("example.com"));
        for text in ["javascript:alert(1)", "chrome://settings", "file:///c:/x"] {
            assert_eq!(parsed(text), None, "{text}");
        }
    }

    #[test]
    fn a_scheme_is_letters_digits_plus_dot_and_minus_after_a_letter() {
        assert!(has_scheme("https://x"));
        assert!(has_scheme("a+b.c-d1://x"));
        for text in ["example.com", "://x", "1http://x", "-a://x", "a b://x", "example.com/p?u=https://x", "a/b://x"] {
            assert!(!has_scheme(text), "{text}");
        }
    }

    #[test]
    fn a_dotless_name_with_a_trailing_dot_is_still_dotless() {
        assert_eq!(parsed("intranet."), None);
        assert_eq!(parsed("intranet./login"), None);
        assert_eq!(parsed("example.com."), Some("https://example.com./".to_string()));
        assert!(parse("localhost.").is_some());
        assert!(parse("localhost.:3000/x").is_some());
    }

    #[test]
    fn typed_userinfo_is_not_kept() {
        let url = parse("user:pass@bank.com").expect("an address with userinfo");
        assert_eq!(url.host_str(), Some("bank.com"));
        assert_eq!(url.username(), "");
        assert_eq!(url.password(), None);
        assert!(!format!("{url:?}").contains("pass@") && !format!("{url:?}").contains("\"pass\""),"Debug must not print the typed password");
        let with_scheme = parse("https://user:pass@bank.com/x").unwrap();
        assert_eq!((with_scheme.username(), with_scheme.password()), ("", None));
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
        assert!(!is_browser_chrome(Some(0), false), "zero steps is not a walk up to the window");
        assert!(is_browser_chrome(Some(1), false), "control: the lower bound itself is inside");
    }

    #[test]
    fn the_shallowest_kept_candidate_is_read_first() {
        assert_eq!(
            reading_order(&[(Some(7), false), (Some(4), false)]),
            vec![1, 0],
            "Firefox lists the page before the toolbar: the shallower field is the toolbar's"
        );
        assert_eq!(reading_order(&[(Some(4), false), (Some(7), false)]), vec![0, 1]);
    }

    #[test]
    fn equally_deep_candidates_are_read_in_tree_order() {
        assert_eq!(reading_order(&[(Some(8), false), (Some(8), false)]), vec![0, 1]);
        assert_eq!(reading_order(&[(Some(9), false), (Some(8), false), (Some(8), false)]), vec![1, 2, 0]);
    }

    #[test]
    fn a_refused_candidate_is_never_in_the_reading_order() {
        assert_eq!(
            reading_order(&[(None, true), (Some(13), false), (Some(4), false), (Some(0), false), (Some(3), true), (None, false)]),
            vec![2],
            "only the kept candidate is read, however shallow the refused ones are"
        );
        assert!(reading_order(&[]).is_empty());
        assert!(reading_order(&[(None, true)]).is_empty());
    }

    /// What Chrome's omnibox held on 2026-10-01: display text, no scheme.
    fn elided(text: &str) -> url::Url {
        assert!(!has_scheme(text), "fixture: an elided omnibox text has no scheme");
        parse(text).expect("the fixture omnibox text parses")
    }

    fn preferred(omnibox: &str, document: Option<&str>) -> Option<String> {
        prefer_document(elided(omnibox), false, document).map(|page| page.url.to_string())
    }

    /// Whether the answer for an elided field is the whole address.
    fn complete(omnibox: &str, document: Option<&str>) -> Option<bool> {
        prefer_document(elided(omnibox), false, document).map(|page| page.complete)
    }

    fn verdict(omnibox: &str, document: Option<&str>) -> DocumentVerdict {
        document_verdict(&elided(omnibox), document)
    }

    fn kept(url: &str) -> Option<String> {
        Some(url.to_string())
    }

    #[test]
    fn a_document_on_the_same_host_restores_the_full_address() {
        assert_eq!(
            preferred("mail.google.com/mail/u/0/", Some("https://mail.google.com/mail/u/0/#inbox")),
            kept("https://mail.google.com/mail/u/0/#inbox")
        );
        assert_eq!(
            preferred("MAIL.google.com./x", Some("https://mail.google.com/x?a=1")),
            kept("https://mail.google.com/x?a=1"),
            "hosts compare lowercased and without a trailing dot"
        );
        assert_eq!(
            verdict("mail.google.com/x", Some("  https://mail.google.com/x/full  ")),
            DocumentVerdict::Agrees(parse("https://mail.google.com/x/full").unwrap()),
            "the verdict carries the document's own address, trimmed"
        );
    }

    #[test]
    fn a_document_may_restore_the_www_chrome_elided() {
        assert_eq!(
            preferred("example.com/login", Some("https://www.example.com/login?next=%2F")),
            kept("https://www.example.com/login?next=%2F")
        );
    }

    #[test]
    fn a_document_restores_http_for_a_plain_http_site_the_omnibox_showed_bare() {
        assert_eq!(elided("example.com/login").scheme(), "https", "control: the bare text reads as https");
        assert_eq!(
            preferred("example.com/login", Some("http://example.com/login")),
            kept("http://example.com/login")
        );
        assert_eq!(
            preferred("localhost:3000/login", Some("http://localhost:3000/login")),
            kept("http://localhost:3000/login")
        );
    }

    /// **The user is typing another address over the page.** The field holds
    /// what they typed, the `Document` the page they are still on: ranking by
    /// the field would put the bank's login on top on the news site.
    #[test]
    fn typing_another_address_over_a_page_is_no_address() {
        assert_eq!(verdict("bank.com", Some("https://news.example/")), DocumentVerdict::Disagrees);
        assert_eq!(preferred("bank.com", Some("https://news.example/")), None);
        assert_eq!(
            preferred("bank.com", Some("https://bank.com/")),
            kept("https://bank.com/"),
            "control: once the page IS the bank, the field and the page agree"
        );
    }

    #[test]
    fn a_web_page_on_another_host_withdraws_the_address() {
        for document in [
            "https://accounts.google.com/",
            "https://google.com/",
            "https://evil.example/mail.google.com/",
            "https://mail.google.com.evil.example/",
            "http://accounts.google.com/",
        ] {
            assert_eq!(verdict("mail.google.com/x", Some(document)), DocumentVerdict::Disagrees, "{document}");
            assert_eq!(preferred("mail.google.com/x", Some(document)), None, "{document}");
        }
        assert_eq!(
            preferred("www.example.com/x", Some("https://example.com/x")),
            None,
            "only a `www.` the omnibox elided is the same host, never one it showed and the document lacks"
        );
    }

    /// A side panel or docked devtools showing a page on another `https` host
    /// comes first in tree order too, and is told apart from typed text by
    /// nothing. It withdraws the address as typed text does: the picker ranks
    /// by the title, as it did before any address was read. Accepted, because
    /// that is the safe direction -- the other would let a `Document` the
    /// field never named decide the page.
    #[test]
    fn a_side_panel_on_another_host_also_withdraws_the_address() {
        assert_eq!(preferred("mail.google.com/x", Some("https://www.google.com/search?q=x")), None);
    }

    #[test]
    fn another_port_on_the_same_host_keeps_the_field() {
        assert_eq!(verdict("example.com:8443/x", Some("https://example.com:9443/x")), DocumentVerdict::Unusable);
        assert_eq!(
            preferred("example.com:8443/x", Some("https://example.com:9443/x")),
            kept("https://example.com:8443/x"),
            "another port on the same host is a stale document; the field's host is right either way"
        );
    }

    #[test]
    fn a_document_whose_host_merely_contains_the_omnibox_host_is_another_host() {
        for document in [
            "https://evil-mail.google.com.attacker.net/",
            "https://www.mail.google.com.attacker.net/",
            "https://xmail.google.com/",
            "https://wwwmail.google.com/",
        ] {
            assert_eq!(preferred("mail.google.com/", Some(document)), None, "{document}");
        }
    }

    #[test]
    fn a_document_that_is_not_a_web_page_keeps_the_field() {
        for document in [
            "chrome://settings/",
            "devtools://devtools/bundled/inspector.html?mail.google.com",
            "about:blank",
            "file:///C:/mail.google.com/x.html",
            "mail.google.com/x",
            "",
            "   ",
            "https://mail.google.com/a b",
            "https://news.example/a b",
        ] {
            assert_eq!(verdict("mail.google.com/x", Some(document)), DocumentVerdict::Unusable, "{document:?}");
            assert_eq!(preferred("mail.google.com/x", Some(document)), kept("https://mail.google.com/x"), "{document:?}");
        }
        let oversized = format!("https://news.example/{}", "a".repeat(MAX_ADDRESS_LEN));
        assert_eq!(verdict("mail.google.com/x", Some(&oversized)), DocumentVerdict::Unusable);
        assert_eq!(preferred("mail.google.com/x", Some(&oversized)), kept("https://mail.google.com/x"));
    }

    #[test]
    fn a_documents_userinfo_is_not_kept() {
        let url = prefer_document(elided("bank.com/x"), false, Some("https://user:pass@bank.com/x/full"))
            .expect("the same host agrees")
            .url;
        assert_eq!(url.path(), "/x/full", "control: the document's address was taken");
        assert_eq!((url.username(), url.password()), ("", None));
    }

    #[test]
    fn an_omnibox_that_showed_a_scheme_is_not_second_guessed() {
        let full = parse("https://mail.google.com/x").unwrap();
        for document in [Some("https://mail.google.com/x/elsewhere"), Some("https://news.example/"), None] {
            assert_eq!(
                prefer_document(full.clone(), true, document),
                Some(PageAddress { url: full.clone(), complete: true }),
                "Edge, or Chrome showing full URLs: the field's own text is the whole address \
                 ({document:?})"
            );
        }
    }

    #[test]
    fn no_document_keeps_the_omnibox_address() {
        assert_eq!(verdict("mail.google.com/x", None), DocumentVerdict::Unusable);
        assert_eq!(preferred("mail.google.com/x", None), kept("https://mail.google.com/x"));
    }

    /// **Complete only when the browser said the whole address.** A scheme
    /// [`parse`] put in front of elided text is a guess: Chrome showed `x.com`
    /// for what may be `http://www.x.com/`.
    #[test]
    fn an_address_built_on_elided_text_is_not_complete() {
        assert_eq!(complete("x.com", None), Some(false), "no Document: Firefox's trimmed text, or Chrome's");
        assert_eq!(complete("x.com", Some("devtools://devtools/x")), Some(false), "an unusable Document");
        assert_eq!(complete("x.com:8443/", Some("https://x.com:9443/")), Some(false), "a stale Document");
        assert_eq!(complete("x.com", Some("http://www.x.com/")), Some(true), "the Document agreed and said it all");
        assert_eq!(complete("x.com", Some("https://news.example/")), None, "control: typed text is no address");
        let full = parse("http://x.com/login").unwrap();
        assert!(
            prefer_document(full, true, None).is_some_and(|page| page.complete),
            "the field's own scheme: the whole address"
        );
    }

    #[test]
    fn only_the_chromium_family_reads_the_page_document() {
        assert!(Family::Chromium.reads_the_page_document());
        assert!(
            !Family::Firefox.reads_the_page_document(),
            "Firefox's first Document was another tab's page on 2026-10-01"
        );
    }

    /// `read` asks for the document in one place, behind both conditions.
    #[test]
    fn the_page_document_is_fetched_once_and_only_for_an_elided_chromium_address() {
        let source = include_str!("browser_url.rs").replace('\r', "");
        let production = source
            .split_once(concat!("#[cfg(", "test)]"))
            .map_or(source.as_str(), |(above, _)| above);
        assert!(production.len() < source.len(), "control: the test gate was not found");
        assert_eq!(
            production.matches(concat!("page_document_", "text(")).count(),
            1,
            "the page's Document is read from a second place in this module"
        );
        // Whitespace is removed from both sides, so a rustfmt reflow of the
        // call (a different indent, the `.then` joined onto the line above)
        // cannot turn this red while the two conditions are still in front of
        // the read. What is pinned is the order of the tokens, not their layout.
        let squashed = |text: &str| -> String { text.chars().filter(|c| !c.is_whitespace()).collect() };
        let production = squashed(production);
        let needle = squashed(concat!(
            "(family.reads_the_page_document() && !had_scheme)\n",
            "        .then(|| crate::injector::ui_automation::page_document_",
            "text(hwnd)"
        ));
        assert!(
            needle.len() > 40 && !needle.contains(char::is_whitespace),
            "control: the needle was not squashed, so the comparison below is layout-bound again"
        );
        assert!(
            production.contains(&needle),
            "the Document read is no longer behind the family and the elided-text conditions"
        );
        // Controls on the comparison itself: a reflowed spelling of the same
        // call still matches, and the call with the elided-text condition
        // dropped does not.
        let reflowed = squashed(
            "(family.reads_the_page_document()\n    && !had_scheme).then(||\n  \
             crate::injector::ui_automation::page_document_text(hwnd)",
        );
        let unconditional = squashed(
            "(family.reads_the_page_document()).then(|| \
             crate::injector::ui_automation::page_document_text(hwnd)",
        );
        assert_eq!(reflowed, needle, "control: whitespace still decides the comparison");
        assert!(!unconditional.contains(&needle), "control: the needle ignores `!had_scheme`");
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

# Browser URL matching: the picker pre-picks the login whose website is in the address bar

**Status:** approved by the owner 2026-10-01. Deskwarden only; NodeWarden is untouched.

## Problem

In a browser, Deskwarden matches nothing automatically, by design (`match_engine.rs`): the
process (`chrome.exe`) is the same for every site and the window title is written by the page,
so neither may choose a credential. `CTRL+ALT+B` therefore opens the account picker
(`picker_prompt`, ranked by `app_candidates`) with guesses from the title, and the user searches
for the login every time a new site is filled.

The address bar is browser chrome, not page content: a page cannot write it. It is the one
string in a browser window that identifies the site.

## Owner's decisions

1. **A URL match only pre-picks.** `CTRL+ALT+B` opens the picker with matching logins on top
   and the best one selected; Enter fills. Nothing is typed without the user's key press. No
   unattended fill, no fill card, no matching outside the hotkey.
2. **Bitwarden's own match rules**, per saved URI: base domain (the default when an entry has
   no match type), host, starts with, exact, regular expression, never. Base domain uses the
   public-suffix list, so `accounts.google.com` matches a login saved as `google.com` and
   `bbc.co.uk` does not match `other.co.uk`.
3. **Read the address bar through UI Automation.** No browser extension (that is the deferred
   Chrome-extension project).

## Design

### `browser_url` (observation, untestable half)

- Called only when `CTRL+ALT+B` is pressed and the foreground image is in
  `app::BROWSER_IMAGE_NAMES`. Never on focus changes, never in the background.
- Asks UI Automation for the foreground browser window's address field. Chromium family
  (Chrome, Edge, Brave, Opera, Vivaldi, Thorium, Chromium): the Edit control named
  "Address and search bar". Firefox family (Firefox, LibreWolf, Floorp, Waterfox, Zen): the
  URL bar's Edit control (`urlbar-input`). Internet Explorer: not supported (returns nothing).
  Locate by stable properties (AutomationId / ClassName / ControlType), with name-based lookup
  as the fallback; document exactly which properties are used per family.
- Returns the field's text, or nothing. A failure, a missing control, or an unsupported
  browser is "no URL", never an error the user sees.
- **Chrome's omnibox holds elided text** (measured live with UI Automation, 2026-10-01). The
  `ValuePattern` value of Chrome's `OmniboxViewViews` is the display text, e.g.
  `mail.google.com/...`: no `https://`, and Chrome also elides `www.` and `http://`. Edge's
  omnibox holds the full `https://...`. Chrome's first `ControlType.Document` under the window
  has a `ValuePattern` value with the full URL of the active tab; the browser sets it. In
  Firefox the first `Document` under the window was a different tab's page.
  So, in the Chromium family only, and only when the omnibox text had no scheme, `read` also
  takes the first `Document`'s value (`FindFirst`, descendants, same session and timeouts) and
  `browser_url::prefer_document` (pure) uses it as the page URL only if it parses as `http` or
  `https` (userinfo dropped, the same 8192-byte cap) and its host, lowercased and without a
  trailing dot, equals the omnibox host or `www.` + it, and its port is the omnibox's.
  Otherwise the omnibox-derived URL stands. The omnibox stays the authority for the host; the
  `Document` only restores the scheme, the `www.` and the full path. Omnibox text that already
  had a scheme (Edge, Chrome with "Always show full URLs") never triggers the `Document` read.
  The `Document` is never read in the Firefox family, and its text is never logged.
- Lives beside `injector::ui_automation` and keeps the same apartment handling.

### `browser_url::parse` (pure)

- Turns the field text into a URL: trims it; adds `https://` when the text has no scheme;
  accepts only `http`/`https`; rejects text with spaces (a search), text without a dot or
  `localhost`, and anything that does not parse with the `url` crate.
- Never treats the text as trusted for anything but choosing which picker row is highlighted.

### `uri_match` (pure, the matcher)

- `matches(saved: &str, match_type: Option<MatchType>, page: &Url) -> bool`, following
  Bitwarden's semantics:
  - **Base domain** (`0`, and `None`): the registrable domains (eTLD+1 via the public-suffix
    list) are equal. IP addresses and `localhost` compare by host alone, never the port
    (Bitwarden's `Utils.getDomain` returns the hostname without one). Two Bitwarden details:
    its `DomainMatchBlacklist` (`google.com` never matches a page on `script.google.com`)
    applies to this rule only, and a saved website whose scheme is not http or https
    (`androidapp://...`) matches nothing under this rule or the host rule.
  - **Host** (`1`): host and port equal.
  - **Starts with** (`2`): the page URL string starts with the saved string.
  - **Exact** (`3`): the URL strings are equal.
  - **Regular expression** (`4`): the saved pattern matches the page URL (the `regex` crate,
    already a dependency); an invalid pattern matches nothing.
  - **Never** (`5`): never matches.
- Saved URIs without a scheme are parsed as `http://` for host comparisons, as Bitwarden does.
- The match type is read from the URI entry's `match` value (`UriEntry::other["match"]`).
  The account's default match setting (Bitwarden's "default URI match detection") is out of
  scope: `None` means base domain.
- Public-suffix data: a small, dependency-free crate with the list compiled in (`psl`
  preferred). Cargo.toml changes, so the `job_object.rs` manifest pin is re-pinned.

### Picker ranking (`app_candidates`)

- `candidates` gains the page URL (`Option<&Url>`) and a new top rank, `RANK_URL`, above
  `RANK_URI_HOST`, for items with any saved URI that `uri_match::matches` accepts.
- `ItemFacts` carries what the matcher needs: each URI string with its match type. No secret is
  added to `ItemFacts`.
- In a browser, the existing exe-stem rank (`chrome` against item domains/names) is not applied:
  it is noise there.
- Inside `RANK_URL`, the stricter rule first, then vault order: Exact, StartsWith, Regex, Host,
  base domain. An item with several matching websites counts by its strictest one.
- The picker already focuses its first row on every card, so a `RANK_URL` login is pre-selected
  by being first; nothing about the focus changes. The five-minute fill recall (`fill_recall`)
  still wins when it applies, so a multi-page sign-in keeps its record.

### Safety

- The URL only decides which row is highlighted; the user still presses a key to fill, so a
  wrong or spoofed reading costs a key press, never a leak.
- `match_engine` is unchanged: browsers still match nothing unattended, by process or title.
- The URL text is not logged.

## Testing

- `browser_url::parse`: scheme added, `http`/`https` only, search text rejected, `localhost`,
  IP addresses, ports, internationalised domains.
- `browser_url::prefer_document`: an agreeing host, a `www.`-prefixed document host, an `http`
  document for an elided `http` site, a mismatching host, a non-web scheme (`chrome://`,
  `devtools://`), a document host that merely contains the omnibox host, an omnibox that had a
  scheme, and no document.
- `uri_match`: every match type; base-domain edge cases (`co.uk`, `github.io`-style private
  suffixes, subdomains, IPs, `localhost`, ports); an invalid regex; `None` = base domain;
  `Never`.
- `app_candidates`: URL-matched items rank first; no exe-stem rank in a browser; a non-match
  falls back to today's ranking; no URL = today's behaviour, except that a browser's
  executable name no longer ranks anything.
- The picker already focuses its first row on every card, so a URL-ranked login is pre-selected
  by being first, and nothing about the focus changes; recall still wins.
- The UI Automation read is a thin, documented observation layer and is not unit-tested; it is
  checked by hand in Chrome, Edge and Firefox.
- Hand check, 2026-10-01: Chrome, Edge and Firefox, against 12 live windows. Every address bar
  was read correctly. Depths, counted in the raw view (`RawViewWalker`) with the window as the
  first step: the Chrome and Edge `OmniboxViewViews` field at 8 and the Firefox `urlbar-input`
  field at 4 in normal windows, with no `Document` above either. Firefox popup windows and
  Chrome app/PWA windows are not yet measured.

## Non-goals

- Unattended fill or a fill card in browsers.
- The account's default URI match setting, equivalent domains, and Bitwarden's
  "global equivalent domains" (google.com ↔ youtube.com).
- A browser extension.

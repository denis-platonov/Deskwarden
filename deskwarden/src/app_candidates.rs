//! Which vault items plausibly belong to the foreground window.
//!
//! **Deliberately looser than [`crate::match_engine::MatchEngine`], and
//! deliberately separate from it.** The engine answers one item or nothing,
//! from an `AppMatch` the user configured; that is what a *fill* is allowed to
//! act on unattended. This answers a ranked list from guesses, and nothing it
//! returns is ever typed without the user picking it -- which is what makes a
//! loose matcher safe here and would make it dangerous there.
//!
//! **In a browser there is one more signal, and it outranks the rest**: the
//! page address `crate::browser_url` read out of the address bar, held against
//! every saved website by Bitwarden's own rules (`crate::uri_match`). It puts
//! the logins saved for that website on top, where the card's first row --
//! which the card focuses when it opens -- is the one Enter takes. It never
//! types anything by itself, for this module's reason above.
//!
//! Pure, and takes `&[VaultItem]` rather than a cache, so the whole of it is
//! testable with fixtures and no window, no vault and no clock.


/// One row of the picker. **Display strings and an id -- never a password.**
/// The secret is fetched at dispatch by the component that already holds it;
/// a copy carried here would be a second, non-zeroizing home for it that lived
/// as long as the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    pub username: String,
}

/// Below this length a title word matches too much to mean anything: "on",
/// "to" and "a" appear in most window titles and most item names.
const MIN_TITLE_WORD: usize = 4;

/// Strongest first. The number is a sort key, not a confidence score, and
/// nothing downstream may treat it as one.
///
/// `RANK_URL` is the only rank a page can earn, and the only one that is not
/// a guess: the user saved that website on that login.
const RANK_URL: u8 = 0;
const RANK_URI_HOST: u8 = 1;
const RANK_NAME: u8 = 2;
const RANK_TITLE: u8 = 3;

/// The exe name without its extension, lowercased: `Slack.exe` -> `slack`.
fn stem(exe_name: &str) -> String {
    exe_name
        .rsplit_once('.')
        .map(|(before, _)| before)
        .unwrap_or(exe_name)
        .to_ascii_lowercase()
}

fn ranked(
    exe_name: &str,
    title: &str,
    item: &crate::app::ItemFacts,
    page: Option<&url::Url>,
) -> Option<u8> {
    if let Some(page) = page {
        if item
            .uris
            .iter()
            .any(|saved| crate::uri_match::matches(&saved.uri, saved.match_type, page))
        {
            return Some(RANK_URL);
        }
    }

    // **No executable-name rank in a browser.** `chrome` against item names
    // and domains is noise there: it surfaces a "Chrome sync" login, or any
    // login saved at `chrome.google.com`, on every website the user visits.
    // The browser's executable says nothing about the page, which is the
    // whole reason `match_engine` refuses browsers.
    let stem = match crate::app::browser_window(exe_name) {
        crate::app::BrowserWindow::Yes => String::new(),
        crate::app::BrowserWindow::No => stem(exe_name),
    };
    let name = item.name.to_ascii_lowercase();

    if !stem.is_empty() {
        for saved in &item.uris {
            let Some(domain) = crate::favicon::domain_from_uri(&saved.uri) else { continue };
            if domain.to_ascii_lowercase().contains(&stem) {
                return Some(RANK_URI_HOST);
            }
        }
        if !name.is_empty() && name.contains(&stem) {
            return Some(RANK_NAME);
        }
    }

    if !name.is_empty() {
        for word in title.to_ascii_lowercase().split(|c: char| !c.is_alphanumeric()) {
            if word.len() >= MIN_TITLE_WORD && name.contains(word) {
                return Some(RANK_TITLE);
            }
        }
    }

    None
}

/// The ranked candidates for this window, strongest first. Ties keep the
/// vault's own order, so the list is stable between presses.
/// Takes [`crate::app::ItemFacts`] rather than `VaultItem`s, and that is the
/// point rather than a convenience: a `VaultItem` carries a password, so a
/// matcher over a slice of them meant the daemon holding every password in
/// the vault to answer which windows look familiar. It needs a name and some
/// URIs.
///
/// `page` is the browser's address, when one was read; `None` everywhere
/// else, and then nothing about the URL rank applies.
pub fn candidates(
    exe_name: &str,
    title: &str,
    items: &[crate::app::ItemFacts],
    page: Option<&url::Url>,
) -> Vec<Candidate> {
    let mut scored: Vec<(u8, Candidate)> = Vec::new();
    for item in items {
        // No id, nothing to fill from later: not a candidate, however well it
        // reads.
        if item.id.is_empty() {
            continue;
        }
        let Some(rank) = ranked(exe_name, title, item, page) else { continue };
        scored.push((
            rank,
            Candidate {
                id: item.id.clone(),
                name: item.name.clone(),
                username: item.username.clone(),
            },
        ));
    }
    scored.sort_by_key(|(rank, _)| *rank);
    scored.into_iter().map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_bridge::{LoginData, UriEntry, VaultItem};

    /// **Returns the projection, built through the production mapping.**
    ///
    /// `ItemFacts::of` is what the daemon runs, so a fixture that hand-built
    /// an `ItemFacts` would be a second mapping able to disagree with it --
    /// and these tests would then pass over facts the real picker never sees.
    fn item(id: &str, name: &str, user: &str, uri: Option<&str>) -> crate::app::ItemFacts {
        saved_with(id, name, user, uri, None)
    }

    /// [`item`], with the URI saved under a Bitwarden match value -- the
    /// number exactly as the wire carries it in `uris[].match`.
    fn saved_with(
        id: &str,
        name: &str,
        user: &str,
        uri: Option<&str>,
        match_value: Option<serde_json::Value>,
    ) -> crate::app::ItemFacts {
        let mut other = serde_json::Map::new();
        if let Some(value) = match_value {
            other.insert("match".to_string(), value);
        }
        crate::app::ItemFacts::of(&VaultItem {
            id: id.to_string(),
            name: name.to_string(),
            fields: Vec::new(),
            login: Some(LoginData {
                username: Some(user.to_string()),
                uris: uri
                    .map(|u| vec![UriEntry { uri: Some(u.to_string()), other: other.clone() }])
                    .unwrap_or_default(),
                ..Default::default()
            }),
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: None,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        })
    }

    fn page(text: &str) -> url::Url {
        url::Url::parse(text).expect("the fixture page is a URL")
    }

    fn ids(found: &[Candidate]) -> Vec<&str> {
        found.iter().map(|c| c.id.as_str()).collect()
    }

    #[test]
    fn a_uri_host_matching_the_exe_stem_outranks_a_name_match() {
        let items = vec![
            item("n", "Slack notes", "notes@example.com", None),
            item("u", "Work chat", "me@example.com", Some("https://slack.com/login")),
        ];
        let found = candidates("slack.exe", "", &items, None);
        assert_eq!(
            found.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            vec!["u", "n"],
            "the URI host is the strongest signal and must sort first"
        );
    }

    #[test]
    fn an_item_with_no_connection_to_the_app_is_not_a_candidate() {
        let items = vec![item("x", "Electricity bill", "me@example.com", Some("https://edf.fr"))];
        assert!(
            candidates("slack.exe", "Slack", &items, None).is_empty(),
            "a loose matcher that returns everything is the same as no matcher"
        );
    }

    #[test]
    fn a_title_word_matches_when_the_exe_name_does_not() {
        let items = vec![item("t", "Jira", "me@example.com", None)];
        let found = candidates("chrome.exe", "Jira - board - Google Chrome", &items, None);
        assert_eq!(found.len(), 1, "the title is the only signal a browser window has");
        assert_eq!(found[0].name, "Jira");
    }

    #[test]
    fn short_title_words_do_not_match_because_they_match_everything() {
        let items = vec![item("s", "Amazon", "me@example.com", None)];
        assert!(
            candidates("chrome.exe", "on to a - Google Chrome", &items, None).is_empty(),
            "two-letter words would make every item a candidate for every window"
        );
    }

    #[test]
    fn an_item_with_no_id_is_skipped_because_nothing_can_be_filled_from_it() {
        let mut orphan = item("ignored", "Slack", "me@example.com", None);
        orphan.id = String::new();
        assert!(candidates("slack.exe", "", &[orphan], None).is_empty());
    }

    #[test]
    fn the_username_is_carried_so_two_accounts_for_one_app_can_be_told_apart() {
        let items = vec![
            item("a", "Slack", "work@example.com", None),
            item("b", "Slack", "home@example.com", None),
        ];
        let found = candidates("slack.exe", "", &items, None);
        assert_eq!(found.len(), 2);
        let users: Vec<_> = found.iter().map(|c| c.username.as_str()).collect();
        assert!(users.contains(&"work@example.com") && users.contains(&"home@example.com"));
    }

    /// **The feature, in one assertion.** The title names one login and the
    /// address bar another; the address bar wins, because the page cannot
    /// write it and can write the title.
    #[test]
    fn a_login_saved_for_the_page_in_the_address_bar_ranks_first() {
        let items = vec![
            item("t", "Example portal", "title@example.com", None),
            item("u", "Work login", "url@example.com", Some("https://example.com")),
        ];
        let found = candidates(
            "chrome.exe",
            "Example portal - Google Chrome",
            &items,
            Some(&page("https://accounts.example.com/login")),
        );
        assert_eq!(
            ids(&found),
            vec!["u", "t"],
            "the login saved for this website must be the first row, which is the row the card \
             focuses when it opens"
        );
    }

    /// The URL rank is Bitwarden's rule for the entry, not "the host looks
    /// similar": `Never` and a missed `Exact` earn nothing from the page.
    #[test]
    fn the_url_rank_follows_each_entrys_own_match_rule() {
        let at = page("https://example.com/login?next=1");
        let never = saved_with("n", "Never here", "a@x", Some("https://example.com"), Some(serde_json::json!(5)));
        let exact = saved_with("e", "Exact elsewhere", "b@x", Some("https://example.com/login"), Some(serde_json::json!(3)));
        let host = saved_with("h", "Host", "c@x", Some("example.com"), Some(serde_json::json!(1)));
        let found = candidates("msedge.exe", "", &[never, exact, host], Some(&at));
        assert_eq!(
            ids(&found),
            vec!["h"],
            "`Never` must never rank and `Exact` must need the whole URL; only the host rule \
             matched this page"
        );
    }

    /// **The executable's name is not a signal in a browser.** Today's
    /// matcher ranked a "Chrome sync" login, and every login saved at a
    /// `chrome.` host, on every page.
    #[test]
    fn in_a_browser_the_executable_name_is_not_a_signal() {
        let items = vec![
            item("n", "Chrome sync", "me@example.com", None),
            item("u", "Web store", "me@example.com", Some("https://chrome.google.com/webstore")),
        ];
        assert!(
            candidates("chrome.exe", "", &items, None).is_empty(),
            "the exe stem `chrome` ranked a login in a browser, with no page read"
        );
        assert!(candidates("chrome.exe", "", &items, Some(&page("https://example.org/"))).is_empty());
        // Control: the same rule still works for a native app, so the
        // emptiness above is the browser rule and not a broken stem.
        let native = vec![item("s", "Slack notes", "me@example.com", None)];
        assert_eq!(ids(&candidates("slack.exe", "", &native, None)), vec!["s"]);
    }

    /// A page no login is saved for falls back to the title, as before.
    #[test]
    fn a_page_no_login_is_saved_for_falls_back_to_the_title() {
        let items = vec![item("t", "Jira", "me@example.com", Some("https://jira.example.com"))];
        let found = candidates(
            "firefox.exe",
            "Jira - board - Mozilla Firefox",
            &items,
            Some(&page("https://example.org/")),
        );
        assert_eq!(ids(&found), vec!["t"], "a page that matches nothing must not hide the title's guess");
    }
}

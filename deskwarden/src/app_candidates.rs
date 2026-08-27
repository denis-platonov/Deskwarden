//! Which vault items plausibly belong to the foreground window.
//!
//! **Deliberately looser than [`crate::match_engine::MatchEngine`], and
//! deliberately separate from it.** The engine answers one item or nothing,
//! from an `AppMatch` the user configured; that is what a *fill* is allowed to
//! act on unattended. This answers a ranked list from guesses, and nothing it
//! returns is ever typed without the user picking it -- which is what makes a
//! loose matcher safe here and would make it dangerous there.
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
const RANK_URI_HOST: u8 = 0;
const RANK_NAME: u8 = 1;
const RANK_TITLE: u8 = 2;

/// The exe name without its extension, lowercased: `Slack.exe` -> `slack`.
fn stem(exe_name: &str) -> String {
    exe_name
        .rsplit_once('.')
        .map(|(before, _)| before)
        .unwrap_or(exe_name)
        .to_ascii_lowercase()
}

fn ranked(exe_name: &str, title: &str, item: &crate::app::ItemFacts) -> Option<u8> {
    let stem = stem(exe_name);
    let name = item.name.to_ascii_lowercase();

    if !stem.is_empty() {
        {
            for uri in &item.uris {
                let Some(domain) = crate::favicon::domain_from_uri(uri) else { continue };
                if domain.to_ascii_lowercase().contains(&stem) {
                    return Some(RANK_URI_HOST);
                }
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
pub fn candidates(exe_name: &str, title: &str, items: &[crate::app::ItemFacts]) -> Vec<Candidate> {
    let mut scored: Vec<(u8, Candidate)> = Vec::new();
    for item in items {
        // No id, nothing to fill from later: not a candidate, however well it
        // reads.
        if item.id.is_empty() {
            continue;
        }
        let Some(rank) = ranked(exe_name, title, item) else { continue };
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
        crate::app::ItemFacts::of(&VaultItem {
            id: id.to_string(),
            name: name.to_string(),
            fields: Vec::new(),
            login: Some(LoginData {
                username: Some(user.to_string()),
                uris: uri
                    .map(|u| {
                        vec![UriEntry {
                            uri: Some(u.to_string()),
                            other: serde_json::Map::new(),
                        }]
                    })
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

    #[test]
    fn a_uri_host_matching_the_exe_stem_outranks_a_name_match() {
        let items = vec![
            item("n", "Slack notes", "notes@example.com", None),
            item("u", "Work chat", "me@example.com", Some("https://slack.com/login")),
        ];
        let found = candidates("slack.exe", "", &items);
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
            candidates("slack.exe", "Slack", &items).is_empty(),
            "a loose matcher that returns everything is the same as no matcher"
        );
    }

    #[test]
    fn a_title_word_matches_when_the_exe_name_does_not() {
        let items = vec![item("t", "Jira", "me@example.com", None)];
        let found = candidates("chrome.exe", "Jira - board - Google Chrome", &items);
        assert_eq!(found.len(), 1, "the title is the only signal a browser window has");
        assert_eq!(found[0].name, "Jira");
    }

    #[test]
    fn short_title_words_do_not_match_because_they_match_everything() {
        let items = vec![item("s", "Amazon", "me@example.com", None)];
        assert!(
            candidates("chrome.exe", "on to a - Google Chrome", &items).is_empty(),
            "two-letter words would make every item a candidate for every window"
        );
    }

    #[test]
    fn an_item_with_no_id_is_skipped_because_nothing_can_be_filled_from_it() {
        let mut orphan = item("ignored", "Slack", "me@example.com", None);
        orphan.id = String::new();
        assert!(candidates("slack.exe", "", &[orphan]).is_empty());
    }

    #[test]
    fn the_username_is_carried_so_two_accounts_for_one_app_can_be_told_apart() {
        let items = vec![
            item("a", "Slack", "work@example.com", None),
            item("b", "Slack", "home@example.com", None),
        ];
        let found = candidates("slack.exe", "", &items);
        assert_eq!(found.len(), 2);
        let users: Vec<_> = found.iter().map(|c| c.username.as_str()).collect();
        assert!(users.contains(&"work@example.com") && users.contains(&"home@example.com"));
    }
}

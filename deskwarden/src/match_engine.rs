use crate::app_match::AppMatch;
use crate::window_watch::{is_host_process, ForegroundEvent};
use std::collections::HashMap;

/// The saved matches, indexed for the two questions a foreground window can be
/// asked.
///
/// **Two tables, and which one a window may consult is decided by the window,
/// not by the match.** See [`MatchEngine::lookup`] -- that split is the whole
/// safety argument for matching on a title at all.
pub struct MatchEngine {
    by_process: HashMap<String, (String, AppMatch)>,
    by_title: HashMap<String, (String, AppMatch)>,
}

impl MatchEngine {
    pub fn new() -> Self {
        Self { by_process: HashMap::new(), by_title: HashMap::new() }
    }

    /// Rebuilds both lookup tables.
    ///
    /// A match contributes to the process table unless its process is a window
    /// host ([`crate::window_watch::is_host_process`]), and to the title table
    /// whenever it recorded a title. Most matches are in both; the two are
    /// built independently because they answer for different windows.
    ///
    /// **Host-named entries are still dropped from the process table, and a
    /// host-named entry that recorded no title is dropped entirely.** That is
    /// the repair path for matches already sitting in the user's vault. Before
    /// the foreground watcher learned to attribute a hosted window, saving a
    /// match for a Store app recorded `ApplicationFrameHost.exe` -- the
    /// reported bug: one entry that fires on every Store app the user focuses,
    /// which is not "too eager", it is wrong by construction. Such an entry
    /// cannot be *narrowed* into a correct one, because the name carries no
    /// information about which app was meant, and no title was captured
    /// alongside it to repair it from. Every such entry predates the `title`
    /// field, so this is exactly the set of old bad values.
    ///
    /// **Dropped here, and nowhere else.** This app does not rewrite the
    /// user's vault behind their back, so the field stays exactly as they
    /// saved it; what changes is that autofill stops acting on it.
    ///
    /// **The user is told by `picker_ui::existing_host_match_notice`, not by
    /// anything this type hands back.** A `Vec` of the dropped pairs and an
    /// `unmatchable_hosts()` accessor lived here for exactly that purpose and
    /// never acquired a caller -- complete, correct and unreachable, which a
    /// `pub` function in a lib crate produces no warning for. `MatchEngine` is
    /// owned by `main`'s event loop and the picker is a separate window with
    /// no reference to it, so the report had nowhere to go. The picker derives
    /// the same fact where it is actually usable: straight off the target
    /// item's own `deskwarden:app-match` field, on the screen where Save
    /// replaces it. Deleting the accessor therefore cost nothing but its own
    /// tests. The `log::warn!` below is what remains, and it is a trace for
    /// the developer, not a surface for the user.
    ///
    /// So a user whose match went quiet is told **when they next open "Add
    /// app..." on that item**, and not at the moment it goes quiet. Closing
    /// that gap needs a channel out of `main`'s loop -- a tray balloon, say --
    /// which is a decision about `main.rs`, not about this file.
    pub fn rebuild(&mut self, entries: &[(String, AppMatch)]) {
        self.by_process = entries
            .iter()
            .filter(|(_, m)| !is_host_process(&m.process))
            .map(|(item_id, m)| (m.process.to_lowercase(), (item_id.clone(), m.clone())))
            .collect();

        self.by_title = entries
            .iter()
            .filter(|(_, m)| !m.title.is_empty())
            .map(|(item_id, m)| (m.title.to_lowercase(), (item_id.clone(), m.clone())))
            .collect();

        // Logged here rather than at the four `rebuild` call sites in `main`,
        // because there are four of them and a warning that only three carry
        // is a warning that goes missing on the fourth path.
        for (item_id, m) in entries
            .iter()
            .filter(|(_, m)| is_host_process(&m.process) && m.title.is_empty())
        {
            let process = &m.process;
            log::warn!(
                "ignoring the app match on vault item {item_id}: {process} owns the top-level \
                 window for every Microsoft Store app, so this match would fire on all of them, \
                 and it recorded no window title to identify the app by instead. The vault is \
                 unchanged -- re-add the app from \"Add app...\" to replace it"
            );
        }
    }

    /// Drops every match, so nothing can be looked up until a rebuild.
    ///
    /// Pairs with `VaultCache::clear`: an empty cache and a populated engine
    /// are an inconsistent pair (review 13's Minor 3). Left populated after
    /// a lock the user then declined to unlock, a matched process still
    /// raises the autofill prompt, and the fill then finds nothing --
    /// `handle_match` looks the item up in the now-empty cache, misses, and
    /// falls through to a `bridge.get_item` with an id belonging to an
    /// account the app is no longer signed into. Clearing both together
    /// means a locked app is simply inert, which is what "locked" should
    /// look like.
    pub fn clear(&mut self) {
        self.by_process.clear();
        self.by_title.clear();
    }

    /// The item matched by the foreground window, if any.
    ///
    /// **The two tables are disjoint alternatives, chosen by the window.** A
    /// window whose process could be identified is answered from the process
    /// table and the stored titles are never consulted; a window owned by a
    /// known host ([`crate::window_watch::is_host_process`]) -- which is to say
    /// a window with no identifiable process at all -- is answered from the
    /// title table and the stored process names are never consulted.
    ///
    /// **Why the title is not simply a second thing every window may match
    /// on.** Keeper's "Application Title or Program Name" is one free-text
    /// value compared against either, which is what the user pointed at and
    /// what motivated storing the title -- but "match if either hits" makes
    /// every saved title a live needle against every window on the desktop,
    /// and window titles are the most attacker- and accident-controlled
    /// strings on a machine. A saved title of `Mabl` would then match a
    /// *browser tab* named Mabl, and a page can name its own tab. Credentials
    /// offered to the wrong application is the failure mode this whole file
    /// exists to make impossible, so it is made impossible by construction
    /// rather than unlikely by heuristic: a title can only ever be reached
    /// through a window that `ApplicationFrameHost.exe` owns, and no browser,
    /// no installer and no user application is on that list -- it has exactly
    /// one entry, added on measured evidence.
    ///
    /// The narrowness is the point, and it is aimed precisely at the case that
    /// has no other answer: a *suspended* Microsoft Store app. Its frame has
    /// no `Windows.UI.Core.CoreWindow` child, so nothing can name its
    /// executable (see [`crate::window_watch::attribute_window`]); the title on
    /// the frame is the only identity it has left.
    ///
    /// Two independent guards keep the host itself unmatchable: `rebuild`
    /// keeps host-named entries out of the process table, and this function
    /// never consults the process table for a host-owned window in the first
    /// place. Either alone would do; neither is load-bearing on the other.
    ///
    /// A title is matched whole and case-insensitively, never as a substring:
    /// a stored `Settings` must not match a window called `Settings for
    /// Something Else`. The cost is that an app which renames its window loses
    /// the match until it is re-added, which fails closed.
    ///
    /// **Takes the whole event rather than the two strings it reads**, and
    /// [`Self::lookup_parts`] below is private for the same reason: the one
    /// production call site is inside `main`'s event loop, where no test can
    /// see it, and `lookup(&event.exe_name, "")` is a mutation that compiles,
    /// leaves every test in this file green, and silently switches the title
    /// table off. There is now nothing there to get wrong -- the caller has
    /// one value to hand over and no opportunity to assemble it.
    pub fn lookup(&self, event: &ForegroundEvent) -> Option<(&str, &AppMatch)> {
        self.lookup_parts(&event.exe_name, &event.title)
    }

    /// [`Self::lookup`]'s decision, on the two strings it actually reads --
    /// private, so that a `ForegroundEvent` is the only way in from outside
    /// this module.
    fn lookup_parts(&self, exe_name: &str, title: &str) -> Option<(&str, &AppMatch)> {
        if is_host_process(exe_name) {
            if title.is_empty() {
                return None;
            }
            return self
                .by_title
                .get(&title.to_lowercase())
                .map(|(id, m)| (id.as_str(), m));
        }
        self.by_process
            .get(&exe_name.to_lowercase())
            .map(|(id, m)| (id.as_str(), m))
    }
}

impl Default for MatchEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_match::{AppMatch, TriggerMode};

    const HOST: &str = "ApplicationFrameHost.exe";

    fn entry(item_id: &str, process: &str, trigger: TriggerMode) -> (String, AppMatch) {
        (item_id.to_string(), AppMatch::for_process(process, trigger))
    }

    /// A match as the picker now saves it: the attributed process AND the
    /// title the window carried at the moment it was picked.
    fn captured(item_id: &str, process: &str, title: &str) -> (String, AppMatch) {
        (
            item_id.to_string(),
            AppMatch {
                process: process.to_string(),
                title: title.to_string(),
                path: format!(r"C:\Apps\{process}"),
                trigger: TriggerMode::Prompt,
            },
        )
    }

    /// A window as `window_watch` reports it, which is the only thing the
    /// public [`MatchEngine::lookup`] accepts.
    fn window(exe_name: &str, title: &str) -> ForegroundEvent {
        ForegroundEvent {
            hwnd: 0x1234,
            pid: 4242,
            exe_name: exe_name.to_string(),
            title: title.to_string(),
        }
    }

    #[test]
    fn the_public_lookup_reads_both_of_the_events_names() {
        // Everything below drives `lookup_parts` directly, so this is the one
        // test that pins the wrapper `main` actually calls -- and specifically
        // that it forwards the TITLE. Changing the delegation to
        // `self.lookup_parts(&event.exe_name, "")` gives
        //     "the public lookup dropped the event's title"
        // while leaving every other test in this file green.
        let mut engine = MatchEngine::new();
        engine.rebuild(&[captured("keepsolid", "KeepSolid.exe", "KeepSolid")]);

        assert_eq!(
            engine.lookup(&window(HOST, "KeepSolid")).map(|(id, _)| id),
            Some("keepsolid"),
            "the public lookup dropped the event's title"
        );
        // The other name, so the wrapper cannot pass by reading only the title:
        // swapping the two arguments fails here.
        assert_eq!(
            engine.lookup(&window("KeepSolid.exe", "")).map(|(id, _)| id),
            Some("keepsolid"),
            "the public lookup dropped the event's process name"
        );
        assert_eq!(engine.lookup(&window("chrome.exe", "KeepSolid")).map(|(id, _)| id), None);
    }

    #[test]
    fn empty_engine_matches_nothing() {
        let engine = MatchEngine::new();
        assert!(engine.lookup_parts("anything.exe", "Anything").is_none());
    }

    #[test]
    fn matches_exact_process_name() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("1", "RockstarGamesLauncher.exe", TriggerMode::Prompt)]);

        let (id, m) = engine.lookup_parts("RockstarGamesLauncher.exe", "").unwrap();
        assert_eq!(id, "1");
        assert_eq!(m.trigger, TriggerMode::Prompt);
    }

    #[test]
    fn clear_drops_every_match_so_a_locked_app_is_inert() {
        // Review 13's Minor 3: after lock recovery is dismissed the cache is
        // empty, so the engine must be too -- otherwise a matched process
        // still raises the autofill prompt for an account the app is no
        // longer signed into, and the fill can only fail.
        //
        // Both tables, because either one left populated reaches
        // `handle_match` on its own. Deleting the `by_title.clear()` line
        // gives "a locked app must be inert -- the title table survived".
        let mut engine = MatchEngine::new();
        engine.rebuild(&[
            entry("1", "RockstarGamesLauncher.exe", TriggerMode::Prompt),
            captured("2", "Speedtest.exe", "Speedtest"),
        ]);

        engine.clear();

        assert!(engine.lookup_parts("RockstarGamesLauncher.exe", "").is_none());
        assert!(
            engine.lookup_parts(HOST, "Speedtest").is_none(),
            "a locked app must be inert -- the title table survived"
        );
    }

    #[test]
    fn matches_case_insensitively() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("1", "RockstarGamesLauncher.exe", TriggerMode::Auto)]);

        assert!(engine.lookup_parts("rockstargameslauncher.EXE", "").is_some());
    }

    #[test]
    fn returns_none_for_unrelated_process() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("1", "mabl.exe", TriggerMode::Hotkey)]);

        assert!(engine.lookup_parts("notepad.exe", "").is_none());
    }

    /// The user's stored match: `ApplicationFrameHost.exe`, saved when they
    /// pointed at KeepSolid (a Store app), which then fired on Speedtest --
    /// and on every other Store app -- because that one exe owns the window
    /// for all of them.
    #[test]
    fn a_match_stored_against_the_frame_host_is_not_loaded() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("keepsolid", HOST, TriggerMode::Prompt)]);

        // Deleting the `filter` in `rebuild` gives
        //     "the frame host is still matched, so every Store app fills this item"
        // Note the empty title: a host-owned window with no title reaches
        // neither table, which is the second guard, tested on its own below.
        assert!(
            engine.lookup_parts(HOST, "").is_none(),
            "the frame host is still matched, so every Store app fills this item"
        );
        // And with a title, because an old entry recorded none: there is
        // nothing in the title table for it either.
        assert!(engine.lookup_parts(HOST, "Speedtest").is_none());
        assert!(engine.lookup_parts(HOST, "KeepSolid").is_none());
    }

    #[test]
    fn dropping_a_host_entry_does_not_drop_the_good_ones_beside_it() {
        // The positive control for the test above, and the mutation it kills:
        // a `rebuild` that filtered out everything (or simply stopped
        // building the table) would satisfy "the host is not matched" while
        // making the whole feature inert. Inverting the filter's sense gives
        //     "a real app stopped being matched"
        let mut engine = MatchEngine::new();
        engine.rebuild(&[
            entry("keepsolid", HOST, TriggerMode::Prompt),
            entry("ledgerline", "Ledgerline.exe", TriggerMode::Auto),
        ]);

        assert!(
            engine.lookup_parts("Ledgerline.exe", "").is_some(),
            "a real app stopped being matched"
        );
        assert!(engine.lookup_parts(HOST, "").is_none());
    }

    #[test]
    fn replacing_a_host_entry_with_a_real_one_makes_the_replacement_live() {
        // What the deleted `unmatchable_hosts` report used to be asserted
        // through, said in terms of the only thing `rebuild` actually changes:
        // the lookup tables. Deleting the `self.by_process = ...` assignment
        // (so a rebuild keeps the previous table) gives
        //     "the replacement must actually be live"
        // and inverting the filter's sense gives
        //     "the bad entry survived its own replacement"
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("keepsolid", HOST, TriggerMode::Prompt)]);
        assert!(
            engine.lookup_parts(HOST, "").is_none(),
            "precondition: the host entry was never loaded"
        );

        engine.rebuild(&[entry("keepsolid", "KeepSolid.exe", TriggerMode::Prompt)]);

        assert!(
            engine.lookup_parts("KeepSolid.exe", "").is_some(),
            "the replacement must actually be live"
        );
        assert!(
            engine.lookup_parts(HOST, "").is_none(),
            "the bad entry survived its own replacement"
        );
    }

    #[test]
    fn rebuild_replaces_previous_entries() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("1", "mabl.exe", TriggerMode::Hotkey)]);
        engine.rebuild(&[entry("2", "notepad.exe", TriggerMode::Auto)]);

        assert!(engine.lookup_parts("mabl.exe", "").is_none());
        assert!(engine.lookup_parts("notepad.exe", "").is_some());
    }

    #[test]
    fn rebuild_replaces_the_previous_titles_too() {
        // The title table's half of `rebuild_replaces_previous_entries`.
        // Changing `self.by_title = ...` to an `extend` gives
        //     "the previous title survived a rebuild"
        let mut engine = MatchEngine::new();
        engine.rebuild(&[captured("1", "Speedtest.exe", "Speedtest")]);
        engine.rebuild(&[captured("2", "KeepSolid.exe", "KeepSolid")]);

        assert!(
            engine.lookup_parts(HOST, "Speedtest").is_none(),
            "the previous title survived a rebuild"
        );
        assert!(engine.lookup_parts(HOST, "KeepSolid").is_some());
    }

    // ---- The title table: what it is for, and everything it must not do ----

    /// **The case with no other answer.** The user added KeepSolid while it was
    /// on screen, so the match records `KeepSolid.exe` and the title
    /// `KeepSolid`. They then minimise it; Windows suspends the app, its
    /// `CoreWindow` goes with it, and the frame can no longer be attributed to
    /// any executable at all -- `window_watch` reports the window under the
    /// host's own name. Only the title is left to recognise it by.
    #[test]
    fn a_suspended_store_app_is_matched_by_the_title_its_frame_still_carries() {
        // Deleting the `by_title` branch from `lookup` (or the `by_title`
        // assignment from `rebuild`) gives
        //     "a suspended Store app has nothing but its title to be matched by"
        let mut engine = MatchEngine::new();
        engine.rebuild(&[captured("keepsolid", "KeepSolid.exe", "KeepSolid")]);

        let (id, m) = engine
            .lookup_parts(HOST, "KeepSolid")
            .expect("a suspended Store app has nothing but its title to be matched by");
        assert_eq!(id, "keepsolid");
        assert_eq!(m.process, "KeepSolid.exe");
    }

    #[test]
    fn the_same_store_app_is_matched_by_its_process_once_it_is_awake() {
        // The other half of the same saved match: restored, the frame resolves
        // to `KeepSolid.exe` and the ordinary process path answers. Both must
        // work off ONE saved value, which is why `rebuild` files a captured
        // match into both tables rather than choosing between them. Changing
        // the `by_process` filter to also exclude entries that carry a title
        // gives "a match that recorded a title stopped matching by process".
        let mut engine = MatchEngine::new();
        engine.rebuild(&[captured("keepsolid", "KeepSolid.exe", "KeepSolid")]);

        assert!(
            engine.lookup_parts("KeepSolid.exe", "KeepSolid").is_some(),
            "a match that recorded a title stopped matching by process"
        );
    }

    /// **The over-matching this design refuses.** A saved title is not a second
    /// needle every window is tested against: a browser can be made to say
    /// anything in its title bar by the page it is showing.
    #[test]
    fn a_title_saved_for_one_app_does_not_match_an_ordinary_window_that_wears_it() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[
            captured("mabl", "mabl.exe", "Mabl"),
            captured("keepsolid", "KeepSolid.exe", "KeepSolid"),
        ]);

        // Changing `lookup` to fall back to `by_title` when the process misses
        // -- Keeper's "title OR process" shape -- gives
        //     left: Some(("mabl", ..))  right: None
        assert_eq!(
            engine.lookup_parts("chrome.exe", "Mabl").map(|(id, _)| id),
            None,
            "a page that names its own tab must not be able to claim a saved match"
        );
        // Positive control on the same engine and the same stored title: the
        // ONLY difference is which process is asking. Without this, the
        // assertion above passes against an engine that matches nothing.
        assert_eq!(engine.lookup_parts(HOST, "Mabl").map(|(id, _)| id), Some("mabl"));
    }

    #[test]
    fn a_host_owned_window_is_never_answered_from_the_process_table() {
        // The second, independent guard: even if a host name somehow reached
        // the process table, a host-owned window would not read it. Built by
        // hand rather than through `rebuild` (which filters it out) so this
        // tests `lookup` alone. Deleting the `is_host_process` branch from
        // `lookup` gives
        //     left: Some("keepsolid")  right: None
        let mut engine = MatchEngine::new();
        engine
            .by_process
            .insert(HOST.to_lowercase(), ("keepsolid".to_string(), AppMatch::for_process(HOST, TriggerMode::Prompt)));

        assert_eq!(engine.lookup_parts(HOST, "Speedtest").map(|(id, _)| id), None);
        // Positive control: the table really does hold that entry, so the
        // `None` above is the routing and not an empty engine.
        assert_eq!(
            engine.by_process.get(&HOST.to_lowercase()).map(|(id, _)| id.as_str()),
            Some("keepsolid")
        );
    }

    #[test]
    fn a_title_is_matched_whole_and_not_as_a_substring() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[captured("settings", "Settings.exe", "Settings")]);

        // Loosening `lookup`'s title comparison to `contains` gives
        //     left: Some("settings")  right: None
        assert_eq!(engine.lookup_parts(HOST, "Settings for Ledgerline").map(|(id, _)| id), None);
        assert_eq!(engine.lookup_parts(HOST, "Settings").map(|(id, _)| id), Some("settings"));
    }

    #[test]
    fn a_title_is_matched_case_insensitively() {
        // Window titles are display text and their casing is not stable
        // enough to key on -- the rest of this crate's name comparisons are
        // case-insensitive for the same reason. Deleting either
        // `to_lowercase` gives "the stored title stopped matching its own
        // window".
        let mut engine = MatchEngine::new();
        engine.rebuild(&[captured("keepsolid", "KeepSolid.exe", "KeepSolid")]);

        assert!(
            engine.lookup_parts(HOST, "keepsolid").is_some(),
            "the stored title stopped matching its own window"
        );
    }

    #[test]
    fn a_host_owned_window_with_no_title_matches_nothing() {
        // There is no identity left at all, and "" must not become a key that
        // an entry could ever be filed under. Deleting the `title.is_empty()`
        // guard would let an entry whose title is "" answer for every
        // unnameable frame on the desktop; `rebuild`'s own filter is the other
        // half of that, tested below.
        let mut engine = MatchEngine::new();
        engine.rebuild(&[captured("keepsolid", "KeepSolid.exe", "KeepSolid")]);

        assert!(engine.lookup_parts(HOST, "").is_none());
    }

    #[test]
    fn a_match_that_recorded_no_title_puts_nothing_in_the_title_table() {
        // Every match saved before the `title` field existed. Deleting
        // `rebuild`'s `!m.title.is_empty()` filter files it under "", which
        // the guard above then has to catch on its own.
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("ledgerline", "Ledgerline.exe", TriggerMode::Auto)]);

        assert!(engine.by_title.is_empty(), "an empty title became a key: {:?}", engine.by_title);
        // Positive control: a captured title does become one.
        engine.rebuild(&[captured("keepsolid", "KeepSolid.exe", "KeepSolid")]);
        assert_eq!(engine.by_title.len(), 1);
    }
}

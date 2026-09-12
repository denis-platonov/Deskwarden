use crate::app::{browser_window, BrowserWindow};
use crate::app_match::AppMatch;
use crate::window_watch::{is_host_process, ForegroundEvent};
use std::collections::HashMap;

/// True when `exe_name` names a web browser, as
/// [`crate::app::BROWSER_IMAGE_NAMES`] defines one.
///
/// **A thin call into `app`, and deliberately not a second list.** The names
/// live in `app.rs` because that is where the first consumer landed
/// ([`crate::app::disposition`]'s browser suppressor), and
/// `the_browser_list_is_the_only_place_a_browser_is_named` pins them as the
/// single source. "Two enumerations that must agree" is the shape this
/// codebase keeps finding defects in, and a copy of the list here would
/// compile, ship, and disagree with that one on the first browser either of
/// them forgot -- so the engine asks rather than remembers.
/// `the_browser_rule_asks_app_rather_than_keeping_a_second_list` pins that this
/// file names no browser at all outside its own tests.
///
/// The direction of the dependency is worth stating, because it looks
/// backwards: `match_engine` sits under `app` and this is a call upward. It is
/// a call to a **pure function over one `&str`** -- no window, no cache, no
/// settings -- so it carries none of `app`'s state with it, and the
/// alternative (a duplicate list, or moving the list into a file this pass
/// does not own) is worse in both directions.
fn is_browser_process(exe_name: &str) -> bool {
    browser_window(exe_name) == BrowserWindow::Yes
}

/// The saved matches, indexed for the two questions a foreground window can be
/// asked.
///
/// **Two tables, and which one a window may consult is decided by the window,
/// not by the match.** See [`MatchEngine::lookup`] -- that split is the whole
/// safety argument for matching on a title at all.
///
/// **A third rule sits across both: a web browser is not an application
/// identity, so a browser is in neither table.** See
/// [`MatchEngine::rebuild`] and [`MatchEngine::lookup`]; the short version is
/// that `msedge.exe` is one process for every site the user has ever visited,
/// so a binding keyed on it is a binding to the whole web.
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
    /// only when it recorded a title **and** recorded that the window it came
    /// off was owned by a host frame ([`crate::app_match::AppMatch::hosted`]).
    /// A hosted match is in both -- awake it is matched by its process, asleep
    /// by its title -- and an ordinary desktop app is in the process table
    /// alone.
    ///
    /// **`hosted`, not "has a title", is the gate (review 31's Important 1).**
    /// The filter here used to be `!m.title.is_empty()` while the picker
    /// recorded a title for every row, which made an ordinary desktop app's
    /// title a live needle for any frame `ApplicationFrameHost.exe` owns: a
    /// Store-packaged app whose title follows its content (a reader, a chat
    /// client) could wear `Ledgerline - Invoices` and be handed the bank's
    /// credentials. The picker no longer records a title for a row that was not
    /// hosted, so the data is fixed at the source; this filter is what makes
    /// the titles ALREADY saved by that commit -- indistinguishable from
    /// legitimate ones, and not ours to rewrite -- inert where they sit.
    ///
    /// Two matches that recorded the same title collide, and the last one in
    /// `entries` order wins silently (that order follows the vault item list,
    /// so it is at least deterministic). Unlike the host case below this is not
    /// logged: after the narrowing above, a collision needs two *hosted* rows
    /// with byte-identical titles, which is two Store apps that present under
    /// one name -- rare enough that a log line on every rebuild would be noise,
    /// and the user-visible symptom (the wrong item offered for one Store app)
    /// is the same thing the picker's Save flow already lets them correct.
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
    /// # A web browser is dropped from the process table too, and for the same
    /// reason said about a different kind of process
    ///
    /// The owner's report: *"But I still see that popup for Edge everytime"*,
    /// and then *"Microsoft (tivity) fils msedge"*. They had bound a vault item
    /// to the process `msedge.exe`, and **a browser is one process for every
    /// site you visit**. So that binding matched every Edge window -- every
    /// tab, every page, forever -- and the prompt appeared constantly, for the
    /// wrong reason, offering an item that was right only by coincidence.
    ///
    /// That is [`crate::window_watch::HOST_PROCESSES`]'s defect in a second
    /// shape. `ApplicationFrameHost.exe` is one process for every Store app;
    /// `msedge.exe` is one process for every web site. In both cases the name
    /// that was saved **carries no information about which thing the user
    /// meant**, so honouring it literally is not honouring their intent -- and
    /// in both cases it cannot be narrowed into a correct entry, because there
    /// is nothing in the stored value to narrow it *with*.
    ///
    /// **What is deliberately NOT done: the host's repair does not transfer.**
    /// A host-named entry can be rescued by a title, because
    /// [`Self::lookup`] routes a host-owned window to the title table and
    /// nothing else. A browser cannot be rescued that way, and the refusal is
    /// the argument at [`Self::lookup`]'s own doc, written before any of this:
    /// a page can name its own tab, so a browser title is the most
    /// attacker-controlled string on the desktop. Putting browsers on the host
    /// list -- the obvious "make it symmetrical" edit -- would route every
    /// browser window straight into the title table and hand exactly that
    /// string the job of choosing a credential. It is refused.
    ///
    /// So a browser binding contributes to **neither** table, and there is no
    /// title-shaped escape hatch. What the user keeps is the whole of the
    /// browser path this app already has and already prefers: `CTRL+ALT+B` in
    /// the browser window reaches [`crate::app::Open::NoMatch`]'s account
    /// picker (see [`crate::app::Trigger::Hotkey`], which exists so that a
    /// chord press is never answered with silence), the item is filled from
    /// there, and [`crate::fill_recall`] then offers that same item back at
    /// that same window for five minutes -- which is the multi-page browser
    /// sign-in the owner described, solved without a binding at all.
    ///
    /// **A browser that is not on the list keeps matching**, exactly as
    /// [`crate::app::BROWSER_IMAGE_NAMES`] says of its own consumer: the list
    /// is finite and honest, and the unrecognised browser gets today's
    /// behaviour rather than a guess. The failure direction is the recoverable
    /// one -- the user sees the over-eager prompt they can report, rather than
    /// a match that went quiet for a reason no list explains.
    ///
    /// **Nothing is written to the vault here either.** The `msedge.exe` in
    /// the owner's item stays exactly as they saved it. The `log::warn!` below
    /// is a developer's trace, not a surface for the user; the surface is a
    /// browser-shaped sibling of `picker_ui::existing_host_match_notice`, and
    /// that window belongs to a different pass.
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
            .filter(|(_, m)| !is_host_process(&m.process) && !is_browser_process(&m.process))
            .map(|(item_id, m)| (m.process.to_lowercase(), (item_id.clone(), m.clone())))
            .collect();

        self.by_title = entries
            .iter()
            .filter(|(_, m)| m.hosted && !m.title.is_empty())
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

        // The browser half, logged in the same place and for the same reason:
        // four `rebuild` call sites in `main`, and a warning that only three of
        // them carry is a warning that goes missing on the fourth.
        //
        // Unconditional, where the host loop above is gated on an empty title.
        // That asymmetry is the whole browser argument in one line: a host
        // entry that captured a title still has an identity left to be matched
        // by, and a browser entry never does -- its title is whatever the page
        // currently says it is.
        for (item_id, m) in entries.iter().filter(|(_, m)| is_browser_process(&m.process)) {
            let process = &m.process;
            log::warn!(
                "ignoring the app match on vault item {item_id}: {process} is a web browser, and \
                 a browser runs one process for every site you visit -- so this match would fire \
                 on every tab and every page rather than on the site you meant. The vault is \
                 unchanged. Press the fill shortcut in that window and pick the item instead; \
                 the pick is remembered for the rest of the sign-in"
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
    /// **A web browser gets the same pair of guards, and for a reason this
    /// very paragraph already wrote down.** The argument above refuses a saved
    /// title as a second needle because "a saved title of `Mabl` would then
    /// match a *browser tab* named Mabl, and a page can name its own tab". It
    /// was making a point about titles; it was also, without saying so, the
    /// complete argument against a browser being an app match. A browser
    /// window has exactly two strings on it -- the image name, which is the
    /// same for every site on the web, and the title, which the site itself
    /// writes -- and neither is an identity a credential may be chosen by. So
    /// the process table is not consulted for a browser window either, and
    /// `rebuild` keeps browsers out of it.
    ///
    /// **This is a narrowing and not an inversion of the rule above.** The
    /// shape refused there is "match if the process hits OR the title hits",
    /// which turns every saved title into a live needle against every window
    /// on the desktop and can only ever *add* matches. The shape added here is
    /// "a browser matches nothing", which can only ever *remove* them. Nothing
    /// becomes matchable that was not matchable before; a set of windows that
    /// were matched for a reason that was never true stops being matched. No
    /// window gains a new way to claim a credential, which is the property the
    /// paragraph above exists to protect, and it is untouched.
    ///
    /// A browser is never also a host, so the two branches cannot disagree;
    /// the browser guard sits directly in front of the table it guards rather
    /// than being folded into the host branch, so that deleting either one is
    /// a visible, separate deletion.
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
        // **The browser guard, and it is the whole of the owner's Edge
        // defect.** One process, every site on the web; nothing saved against
        // that name can say which site was meant. `rebuild` keeps such an
        // entry out of `by_process` and this refuses to read `by_process` for
        // such a window -- two guards that agree by hand, exactly as the host
        // pair above does, because each one is the other's only backstop the
        // day the other is refactored away as redundant.
        //
        // Note what it does NOT do: it does not fall through to `by_title`.
        // That would be the title-as-second-needle shape the doc above
        // refuses, aimed at the one process whose title a stranger writes.
        if is_browser_process(exe_name) {
            return None;
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

    /// A match as the picker saves it for a **hosted** row: the attributed
    /// process, the title the frame carried at the moment it was picked, and
    /// the `hosted` flag that says the title may be matched on.
    fn hosted(item_id: &str, process: &str, title: &str) -> (String, AppMatch) {
        (
            item_id.to_string(),
            AppMatch {
                process: process.to_string(),
                title: title.to_string(),
                path: format!(r"C:\Apps\{process}"),
                hosted: true,
                args: String::new(),
                sequence: String::new(),
                trigger: TriggerMode::Prompt,
            },
        )
    }

    /// A match carrying a title but NOT the hosted flag -- which is exactly the
    /// shape the previous commit wrote for every ordinary desktop app, and the
    /// shape sitting in users' vaults right now. The title must be inert.
    fn titled_but_not_hosted(item_id: &str, process: &str, title: &str) -> (String, AppMatch) {
        let (id, m) = hosted(item_id, process, title);
        (id, AppMatch { hosted: false, ..m })
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
        engine.rebuild(&[hosted("keepsolid", "KeepSolid.exe", "KeepSolid")]);

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
        // A third process carrying the right title matches nothing.
        //
        // **This used to say `chrome.exe`, and that spelling has stopped
        // testing anything.** A browser is now refused by `lookup_parts`
        // before either table is read, so the `None` would arrive whatever the
        // wrapper did with its two arguments -- the assertion would pass
        // against the very mutation the rest of this test exists to kill.
        // `notepad.exe` is an ordinary process, so the answer still comes from
        // the routing under test.
        assert_eq!(engine.lookup(&window("notepad.exe", "KeepSolid")).map(|(id, _)| id), None);
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
            hosted("2", "Speedtest.exe", "Speedtest"),
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

        // Deleting the `filter` in `rebuild` does NOT fail this test, and
        // the comment that said it did was wrong for two years' worth of
        // reviews. `lookup_parts` short-circuits on `is_host_process(exe)`
        // before it ever reads `by_process`, so a host entry that reached
        // that table is unreachable THROUGH THIS FUNCTION. What this test
        // kills is the removal of `lookup_parts`'s own branch -- the second
        // guard, tested on its own in
        // `a_host_owned_window_is_never_answered_from_the_process_table`.
        // The filter is asserted about directly, on the table it builds, by
        // `rebuild_keeps_the_frame_host_out_of_the_process_table`.
        //
        // Note the empty title: a host-owned window with no title reaches
        // neither table.
        assert!(
            engine.lookup_parts(HOST, "").is_none(),
            "the frame host is still matched, so every Store app fills this item"
        );
        // And with a title, because an old entry recorded none: there is
        // nothing in the title table for it either.
        assert!(engine.lookup_parts(HOST, "Speedtest").is_none());
        assert!(engine.lookup_parts(HOST, "KeepSolid").is_none());
    }

    /// **`rebuild` keeps the frame host out of the process table**, said
    /// about the table and not through `lookup_parts`.
    ///
    /// The invariant nothing else in this file can state. `rebuild` filters
    /// `is_host_process` out of `by_process` and `lookup_parts` refuses
    /// `is_host_process` before reading `by_process`; either one alone gives
    /// the user the right answer today, so every test that goes in through
    /// `lookup_parts` is satisfied by the second and says nothing about the
    /// first. Deleting `rebuild`'s `filter` outright leaves the whole suite
    /// green -- measured, 1564 passed and 0 failed -- which is what three
    /// comments in this module used to claim it did not.
    ///
    /// It matters because the two agree on `is_host_process` by hand. The
    /// day `lookup_parts` stops being private, or grows a caller that reads
    /// `by_process` directly, or its branch is refactored away as
    /// "redundant", the filter is the only thing left -- and if the filter
    /// has meanwhile rotted, the user's `ApplicationFrameHost.exe` match
    /// fires on every Store app again, which is the reported bug.
    ///
    /// Built the way `a_host_owned_window_is_never_answered_from_the_process_
    /// table` builds its fixture -- reading `by_process` directly, because
    /// the public path cannot see this -- and with a real entry beside the
    /// host one, so that "filter everything out" fails it too.
    #[test]
    fn rebuild_keeps_the_frame_host_out_of_the_process_table() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[
            entry("keepsolid", HOST, TriggerMode::Prompt),
            entry("ledgerline", "Ledgerline.exe", TriggerMode::Auto),
        ]);

        // Deleting the `filter` gives
        //     left: ["applicationframehost.exe", "ledgerline.exe"]
        //     right: ["ledgerline.exe"]
        // and inverting its sense gives
        //     left: ["applicationframehost.exe"]  right: ["ledgerline.exe"]
        let mut keys: Vec<&str> = engine.by_process.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["ledgerline.exe"],
            "the process table is {keys:?}. {HOST} owns the top-level window for every \
             Microsoft Store app, so an entry keyed on it there is a match on all of \
             them -- and the only other thing standing between that entry and the user \
             is one `if` in the private `lookup_parts`"
        );
    }

    #[test]
    fn dropping_a_host_entry_does_not_drop_the_good_ones_beside_it() {
        // The positive control for the test above, and the mutation it kills:
        // a `rebuild` that filtered out everything (or simply stopped
        // building the table) would satisfy "the host is not matched" while
        // making the whole feature inert. Inverting the filter's sense --
        // `filter(|(_, m)| is_host_process(&m.process))` -- gives
        //     "a real app stopped being matched"
        // and NOT the second assertion below, which holds either way for the
        // reason given in the test above: `lookup_parts` never reaches
        // `by_process` for a host exe. The inversion's other half -- that the
        // host entry is now IN the table -- is what
        // `rebuild_keeps_the_frame_host_out_of_the_process_table` sees.
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
        engine.rebuild(&[hosted("1", "Speedtest.exe", "Speedtest")]);
        engine.rebuild(&[hosted("2", "KeepSolid.exe", "KeepSolid")]);

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
        engine.rebuild(&[hosted("keepsolid", "KeepSolid.exe", "KeepSolid")]);

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
        engine.rebuild(&[hosted("keepsolid", "KeepSolid.exe", "KeepSolid")]);

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
            hosted("mabl", "mabl.exe", "Mabl"),
            hosted("keepsolid", "KeepSolid.exe", "KeepSolid"),
        ]);

        // Changing `lookup` to fall back to `by_title` when the process misses
        // -- Keeper's "title OR process" shape -- gives
        //     left: Some(("mabl", ..))  right: None
        //
        // **The probe is `notepad.exe` and no longer `chrome.exe`.** The
        // narrative is still the browser tab -- that is what the doc argues
        // about -- but a browser is now refused before either table is read,
        // so probing with one would make this assertion hold for a reason that
        // has nothing to do with the fallback it is written against: the
        // "title OR process" mutation would survive it. An editor showing a
        // document called `Mabl` wears the same string for the same reason and
        // still goes through the routing under test.
        assert_eq!(
            engine.lookup_parts("notepad.exe", "Mabl").map(|(id, _)| id),
            None,
            "a window that wears a saved title must not be able to claim that saved match"
        );
        // ...and the browser itself, which is now refused twice over: once by
        // this rule and once by its own. Asserted beside the line above rather
        // than instead of it, so neither reason can quietly become the only
        // one.
        assert_eq!(
            engine.lookup_parts("msedge.exe", "Mabl").map(|(id, _)| id),
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
        engine.rebuild(&[hosted("settings", "Settings.exe", "Settings")]);

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
        engine.rebuild(&[hosted("keepsolid", "KeepSolid.exe", "KeepSolid")]);

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
        engine.rebuild(&[hosted("keepsolid", "KeepSolid.exe", "KeepSolid")]);

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
        engine.rebuild(&[hosted("keepsolid", "KeepSolid.exe", "KeepSolid")]);
        assert_eq!(engine.by_title.len(), 1);
    }

    /// **Review 31's Important 1.** The title table used to take a title from
    /// every match that had one, and the picker recorded one for every row --
    /// so an ORDINARY DESKTOP app's title was a live needle for any frame the
    /// host owns. A Store-packaged app whose title is content-derived (a
    /// reader, a chat client, a browser showing a file or contact name) could
    /// wear it and claim a match saved for something else entirely.
    ///
    /// Deleting `rebuild`'s `m.hosted` filter gives
    ///     left: Some("bank")  right: None
    #[test]
    fn a_title_saved_for_an_ordinary_desktop_app_is_not_a_needle_for_a_store_frame() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[titled_but_not_hosted("bank", "Ledgerline.exe", "Ledgerline - Invoices")]);

        assert_eq!(
            engine.lookup_parts(HOST, "Ledgerline - Invoices").map(|(id, _)| id),
            None,
            "a hosted frame wearing a desktop app's title claimed that app's match"
        );
        // Both sides are case-folded, so the obvious near-miss is not a defence.
        assert_eq!(
            engine.lookup_parts("APPLICATIONFRAMEHOST.EXE", "ledgerline - invoices").map(|(id, _)| id),
            None
        );
        // Positive controls, on the same engine and the same strings, so the
        // `None`s above cannot be an engine that matches nothing: the desktop
        // app still matches by its own process name...
        assert_eq!(
            engine.lookup_parts("Ledgerline.exe", "").map(|(id, _)| id),
            Some("bank"),
            "narrowing the title table must not cost the process table its entry"
        );
        // ...and the SAME title, recorded off a hosted row, is still a needle.
        engine.rebuild(&[hosted("bank", "Ledgerline.exe", "Ledgerline - Invoices")]);
        assert_eq!(
            engine.lookup_parts(HOST, "Ledgerline - Invoices").map(|(id, _)| id),
            Some("bank")
        );
    }

    /// The rule that makes the titles ALREADY in users' vaults safe without a
    /// migration: the flag's absence means "do not match on this title". Old
    /// JSON has no `hosted` key, `#[serde(default)]` makes it `false`, and such
    /// a match contributes nothing to the title table.
    #[test]
    fn a_title_from_a_vault_written_before_the_hosted_flag_existed_is_inert() {
        let stored = r#"{"process":"Ledgerline.exe","title":"Ledgerline - Invoices","path":"C:\\Apps\\Ledgerline.exe","trigger":"prompt"}"#;
        let m = AppMatch::from_field_value(stored).expect("the shipped four-key shape must parse");
        assert!(!m.hosted, "an absent `hosted` key must not read as true");

        let mut engine = MatchEngine::new();
        engine.rebuild(&[("bank".to_string(), m)]);

        assert!(engine.by_title.is_empty(), "an old title became a needle: {:?}", engine.by_title);
    }

    // ---- A web browser is not an application identity -------------------

    /// **THE DEFECT, in the owner's words:** *"But I still see that popup for
    /// Edge everytime"*, then *"Microsoft (tivity) fils msedge"*.
    ///
    /// They bound a vault item to `msedge.exe`. A browser is one process for
    /// every site, so the binding matched every Edge window there has ever
    /// been -- every tab, every page -- and the prompt fired constantly. The
    /// item it offered was right only by coincidence.
    ///
    /// Deleting either guard gives, for the first title below,
    ///     left: Some("m365")  right: None
    #[test]
    fn a_process_only_browser_binding_matches_no_window_at_all() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("m365", "msedge.exe", TriggerMode::Prompt)]);

        for title in [
            // The site they meant...
            "Sign in to your account - Microsoft Edge",
            // ...and every other site, which is the defect.
            "Your bank - Microsoft Edge",
            "Breaking news - Microsoft Edge",
            "New tab - Microsoft Edge",
            "",
        ] {
            assert_eq!(
                engine.lookup_parts("msedge.exe", title).map(|(id, _)| id),
                None,
                "msedge.exe still matches {title:?}. A browser is one process for every site \
                 you visit, so this binding fires on all of them and the item it offers is \
                 right only by coincidence"
            );
        }
    }

    /// **`rebuild` keeps a browser out of the process table**, said about the
    /// table rather than through `lookup_parts` -- the invariant that function
    /// cannot state, exactly as
    /// `rebuild_keeps_the_frame_host_out_of_the_process_table` cannot be
    /// stated through it either.
    ///
    /// It matters because the two guards agree on `is_browser_process` by
    /// hand. The day `lookup_parts` grows a caller that reads `by_process`
    /// directly, or its browser branch is refactored away as "redundant", this
    /// filter is the only thing left -- and if it has meanwhile rotted, the
    /// owner's Edge popup is back.
    ///
    /// Built with a real app beside the browser one, so that a `rebuild` which
    /// filtered out everything fails it too.
    #[test]
    fn rebuild_keeps_a_browser_out_of_the_process_table() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[
            entry("m365", "msedge.exe", TriggerMode::Prompt),
            entry("ledgerline", "Ledgerline.exe", TriggerMode::Auto),
        ]);

        // Deleting the `is_browser_process` half of the filter gives
        //     left: ["ledgerline.exe", "msedge.exe"]
        //     right: ["ledgerline.exe"]
        let mut keys: Vec<&str> = engine.by_process.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["ledgerline.exe"],
            "the process table is {keys:?}. msedge.exe is one process for every site on the \
             web, so an entry keyed on it there is a match on all of them -- and the only \
             other thing standing between that entry and the user is one `if` in the private \
             `lookup_parts`"
        );
    }

    /// The second, independent guard: even if a browser name somehow reached
    /// the process table, a browser window would not read it. Built by hand
    /// rather than through `rebuild` (which filters it out) so this tests
    /// `lookup_parts` alone -- the same construction
    /// `a_host_owned_window_is_never_answered_from_the_process_table` uses,
    /// for the same reason.
    #[test]
    fn a_browser_window_is_never_answered_from_the_process_table() {
        let mut engine = MatchEngine::new();
        engine.by_process.insert(
            "msedge.exe".to_string(),
            ("m365".to_string(), AppMatch::for_process("msedge.exe", TriggerMode::Prompt)),
        );

        // Deleting the `is_browser_process` branch from `lookup_parts` gives
        //     left: Some("m365")  right: None
        assert_eq!(engine.lookup_parts("msedge.exe", "Anything").map(|(id, _)| id), None);
        // Positive control: the table really does hold that entry, so the
        // `None` above is the routing and not an empty engine.
        assert_eq!(
            engine.by_process.get("msedge.exe").map(|(id, _)| id.as_str()),
            Some("m365")
        );
    }

    /// **Every name `app` calls a browser is refused, and nothing near one
    /// is.**
    ///
    /// Driven off [`crate::app::BROWSER_IMAGE_NAMES`] rather than off a list
    /// written here, which is the point: this file keeps no second opinion
    /// about what a browser is, so a name added there is covered here on the
    /// same commit.
    #[test]
    fn every_browser_the_app_recognises_is_refused_and_nothing_else_is() {
        assert!(!crate::app::BROWSER_IMAGE_NAMES.is_empty());
        for name in crate::app::BROWSER_IMAGE_NAMES {
            let mut engine = MatchEngine::new();
            engine.rebuild(&[entry("item", name, TriggerMode::Prompt)]);
            assert!(engine.by_process.is_empty(), "{name} reached the process table");
            assert!(
                engine.lookup_parts(name, "A Page").is_none(),
                "{name} still matches, so a binding to it fires on every site"
            );
            // Case-folded, because Windows hands the same image back with
            // different capitalisation depending on how it was launched -- and
            // a case-sensitive guard is a guard with a trivial bypass.
            assert!(engine.lookup_parts(&name.to_uppercase(), "A Page").is_none(), "{name}");
        }

        // **A browser this build does not recognise keeps matching**, which is
        // the deliberate failure direction and the positive control for every
        // assertion above: a guard that refused everything would pass them all
        // and leave the whole feature inert. The unrecognised browser gets
        // today's behaviour -- the over-eager prompt, which is a thing the user
        // can see and report -- rather than a match that went quiet for a
        // reason no list explains.
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("pale", "palemoon.exe", TriggerMode::Prompt)]);
        assert_eq!(engine.lookup_parts("palemoon.exe", "A Page").map(|(id, _)| id), Some("pale"));

        // And not a substring, a prefix or a suffix of a listed name, which is
        // how a real application loses the ability to be matched at all.
        for near in ["notchrome.exe", "chrome.exe.exe", "chrome", "msedgewebview2.exe"] {
            let mut engine = MatchEngine::new();
            engine.rebuild(&[entry("real", near, TriggerMode::Prompt)]);
            assert_eq!(
                engine.lookup_parts(near, "A Window").map(|(id, _)| id),
                Some("real"),
                "{near} is not a browser this build names, and a real app must stay matchable"
            );
        }
    }

    /// **The escape hatch that is deliberately absent, and the reason it is.**
    ///
    /// The obvious way to "rescue" a browser binding is the one the frame host
    /// already has: require a title needle. `lookup`'s own doc refuses it, and
    /// refused it before any of this was written -- *"a page can name its own
    /// tab"*. A browser is the one process on the desktop whose window title
    /// is written by a stranger.
    ///
    /// So a browser entry carrying a title -- which is exactly the shape one
    /// shipped commit wrote, for every row including browser rows -- must
    /// reach neither table. Adding a "browsers may match on their title"
    /// branch to `lookup_parts` fails the second assertion; adding browsers to
    /// `window_watch::HOST_PROCESSES`, the other tempting symmetry, fails the
    /// first.
    #[test]
    fn a_browser_binding_never_becomes_a_title_needle() {
        let stored = r#"{"process":"msedge.exe","title":"Ledgerline - Invoices","path":"C:\\Apps\\msedge.exe","trigger":"prompt"}"#;
        let m = AppMatch::from_field_value(stored).expect("the shipped four-key shape must parse");
        assert_eq!(m.title, "Ledgerline - Invoices", "the premise: it carries a title");

        let mut engine = MatchEngine::new();
        engine.rebuild(&[("bank".to_string(), m)]);

        assert!(
            engine.by_title.is_empty(),
            "a browser's title became a needle: {:?}. The page chose that string",
            engine.by_title
        );
        assert!(engine.by_process.is_empty(), "a browser reached the process table");
        // The page wearing its own saved title still gets nothing...
        assert_eq!(engine.lookup_parts("msedge.exe", "Ledgerline - Invoices"), None);
        // ...and neither does a host frame wearing it, which is the crossover
        // `a_title_saved_for_an_ordinary_desktop_app_is_not_a_needle_for_a_store_frame`
        // guards for ordinary apps and this guards for browsers.
        assert_eq!(engine.lookup_parts(HOST, "Ledgerline - Invoices"), None);
    }

    #[test]
    fn dropping_a_browser_entry_does_not_drop_the_good_ones_beside_it() {
        // The positive control for the refusals above, in the shape
        // `dropping_a_host_entry_does_not_drop_the_good_ones_beside_it` uses:
        // a `rebuild` that filtered out everything, or a `lookup_parts` that
        // answered `None` unconditionally, would satisfy every "the browser is
        // not matched" assertion while making the whole feature inert.
        // Inverting the browser filter's sense gives
        //     "a real app stopped being matched"
        let mut engine = MatchEngine::new();
        engine.rebuild(&[
            entry("m365", "msedge.exe", TriggerMode::Prompt),
            entry("ledgerline", "Ledgerline.exe", TriggerMode::Auto),
            hosted("keepsolid", "KeepSolid.exe", "KeepSolid"),
        ]);

        assert!(
            engine.lookup_parts("Ledgerline.exe", "").is_some(),
            "a real app stopped being matched"
        );
        assert!(
            engine.lookup_parts(HOST, "KeepSolid").is_some(),
            "a suspended Store app stopped being matched by its title"
        );
        assert!(engine.lookup_parts("msedge.exe", "Anything").is_none());
    }

    /// **This file keeps no second opinion about what a browser is.**
    ///
    /// The sibling of `app`'s own
    /// `the_browser_list_is_the_only_place_a_browser_is_named`, which pins the
    /// list as the single source but can only see `app.rs` and `main.rs`. This
    /// says the same thing about the file that has just become the list's
    /// second consumer: the shipping half of `match_engine.rs` names no
    /// browser at all and asks [`crate::app::browser_window`] instead.
    ///
    /// Pasting the list in here -- the change that makes the dependency arrow
    /// point the tidy way -- would compile, ship, and disagree with `app`'s
    /// copy on the first browser either of them forgot. That is the "two
    /// enumerations that must agree" shape this codebase keeps finding defects
    /// in, and it fails here instead.
    ///
    /// **Comments are stripped before the count, where `app`'s version counts
    /// the whole production half.** The property being pinned is "no
    /// executable statement here names a browser", and the docs above
    /// deliberately do name one: the owner reported `msedge.exe` by name, and
    /// a doc that would not say which browser broke is a worse doc. A sentence
    /// cannot disagree with a list at runtime; a second `const` can. The
    /// stripper is itself driven by `named_in_code`'s positive control below,
    /// so "strip everything" cannot be how this test passes.
    #[test]
    fn the_browser_rule_asks_app_rather_than_keeping_a_second_list() {
        let source = include_str!("match_engine.rs");
        // Everything from the first test module on is test text -- the tests
        // above name browsers as sample windows, quite properly. The claim is
        // about the half of the file that ships.
        let boundary = source.find(concat!("mod ", "tests {")).expect(
            "match_engine.rs's test module is gone, so this pin no longer knows where the \
             shipping half of the file ends",
        );
        let production = &source[..boundary];
        assert!(
            production.contains(concat!("fn is_browser", "_process(exe_name: &str)")),
            "the boundary landed before `is_browser_process`, so `production` does not contain \
             the rule this test is about"
        );
        let code = strip_comment_lines(production);
        assert!(
            code.contains(concat!("browser", "_window(exe_name)")),
            "the stripper removed the one call this file makes, so a zero count below would \
             mean nothing"
        );
        for name in crate::app::BROWSER_IMAGE_NAMES {
            assert_eq!(
                code.matches(*name).count(),
                0,
                "{name} is named by code in match_engine.rs, which must ask \
                 `app::browser_window` rather than keep a second opinion about what a browser is"
            );
        }
    }

    /// Every line whose first non-whitespace characters are `//` removed --
    /// which covers `///`, `//!` and a plain `//` alike. Block comments are
    /// not handled because this file has none and a stripper that guessed at
    /// them would be the untested thing the pin above depends on.
    fn strip_comment_lines(source: &str) -> String {
        source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The positive control for the stripper the pin above rests on: a
    /// stripper that removed everything, or that removed nothing, would make
    /// that test meaningless in one direction or fail it in the other.
    #[test]
    fn named_in_code() {
        let planted = concat!(
            "/// A doc naming chrome", ".exe\n",
            "    // and a comment naming chrome", ".exe\n",
            "    const X: &str = \"chrome", ".exe\";\n"
        );
        let stripped = strip_comment_lines(planted);
        assert_eq!(
            stripped.matches("chrome.exe").count(),
            1,
            "the stripper kept {stripped:?}"
        );
        assert!(stripped.contains("const X"), "the stripper ate the code: {stripped:?}");
    }
}

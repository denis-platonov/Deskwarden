//! The vault window's **Password health** screen: which of the passwords in
//! this vault are reused, and which are weak. Computed here, on this machine,
//! from the snapshot the window already holds. **Nothing in this file opens a
//! socket, spawns a process or writes a log line.**
//!
//! # Where it lives, and why it is not in Preferences
//!
//! Preferences holds settings. This is data *about the vault's contents*, and
//! a finding the user cannot click through to the item is a dead end -- so it
//! is drawn in the vault window, in the **item-list column**, with the detail
//! pane left standing beside it. Clicking a finding selects that item, and
//! the pane on the right fills in with the ordinary read view: its password,
//! its Edit button, its generator. The report stays up the whole time, so
//! working through a list of twelve reused logins is twelve clicks and not
//! twelve round trips through the sidebar.
//!
//! It is a **screen**, not a [`super::sidebar::SidebarFilter`], for the same
//! reason `Sends` is not one. A `SidebarFilter` is a per-item predicate
//! (`scope_contains(&self, item)`) evaluated against one item at a time, and
//! **reuse is not a property of an item** -- it is a property of a pair of
//! them. There is no predicate over a single `VaultItem` that can answer it.
//! And the item list is a fixed-pitch virtualized list (`ScrollArea::
//! show_rows`, one tile per row); group headings inside it would put the
//! scroll maths out of register with its own scrollbar, which is a defect
//! that file already carries a comment about. So: its own pane, its own
//! scroll area, group headings that are allowed to be a different height
//! from a row.
//!
//! # The weakness rule, and why it is this one
//!
//! **A password is weak here exactly when [`crate::password_strength::rate`]
//! says [`Strength::Weak`].** That is: fewer than 8 characters, or 8 to 11
//! characters drawn from fewer than three of the four character types.
//!
//! It is deliberately not a rule of this module's own. The detail pane
//! already prints "Strength: Weak" beside the very same password, from that
//! very same function; a second rule here would let this screen list a
//! password the pane calls Fair, or stay silent about one the pane calls
//! Weak, and the user would have no way to tell which of the two was lying.
//! One rule, two surfaces.
//!
//! **There is no score, and that is the point.** A user cannot act on
//! "43/100". What they are shown instead is the two facts that produced the
//! verdict -- the character count and the character types actually present,
//! e.g. *"9 characters, lowercase letters and digits"* -- which names the
//! thing to change. [`crate::password_strength::CharClasses`] is where those
//! types are decided, and `rate` counts the same value, so the explanation
//! cannot drift from the rating it explains.
//!
//! What this rule deliberately does NOT flag: a long single-class password
//! ("horsehorsehorsehorse"). Twenty lowercase characters is a wider search
//! space than eight mixed ones, and flagging it would be crying wolf at the
//! passphrase style this app's own generator can produce.
//!
//! # Items with no password are excluded, not counted as weak
//!
//! A card reported as "weak password" is a bug, not a finding. The gate is
//! [`password_of`]: an item is considered only if it has a `login` object
//! carrying a **non-empty** password. Cards, secure notes, identities, SSH
//! keys, and logins whose password field is empty or absent are all outside
//! the report entirely -- they are not weak, they are not safe, they are not
//! anything, because there is nothing to say about a password that does not
//! exist. See [`HealthReport::with_password`], which is the count the empty
//! state reads.
//!
//! # The grouping key: what it is, and exactly how long it lives
//!
//! Reuse is exact, not heuristic: two items are reused together iff their
//! passwords are byte-for-byte equal. Comparing passwords means holding
//! them, so the comparison is done on **SHA-256 digests, never on the
//! plaintext**.
//!
//! * The key is `Zeroizing<[u8; 32]>` -- the SHA-256 of the password, in a
//!   buffer that is wiped when it drops.
//! * It lives in one local `Vec` inside [`report_for`] and **nowhere else**.
//!   It is not returned, not stored in [`HealthReport`], not put in any
//!   struct that outlives the call, and not cached between frames. When
//!   `report_for` returns, every key has been dropped and wiped.
//! * **There is deliberately no `HashMap<String, Vec<Item>>` of live
//!   passwords.** That is the obvious implementation and it is the one thing
//!   this module must not do: it would hold every password in this vault, in
//!   plaintext, on the heap, for the length of the computation, and free them
//!   unwiped.
//! * The `Vec` is never sorted in place -- an *index* vector is sorted
//!   instead (see [`report_for`]) -- so the sort's own move buffer never
//!   receives a copy of a digest that nothing would wipe.
//!
//! A SHA-256 digest of a password is not the password, but it is a rainbow
//! table's index and it narrows the password to one candidate. It is treated
//! as a secret for that reason: it is wiped, and **it is never formatted,
//! logged, or put in a `Debug`**. Nothing in this file logs at all.
//!
//! # Cost on a large vault
//!
//! [`report_for`] is **O(n log n)** in `n`, the number of items carrying a
//! password: one SHA-256 of a short string per item, then one sort of `n`
//! indices comparing 32 bytes each, then one linear walk over the sorted
//! order to cut it into runs. It is explicitly **not** quadratic -- the naive
//! "compare every password against every other" is O(n^2) comparisons over
//! plaintext, which is both the slow shape and the unsafe one, and it is
//! ruled out by construction here because no two plaintexts are ever compared
//! at all.
//!
//! # Breach data, when the user has not asked for it
//!
//! Breach checking (`crate::breach`, Have I Been Pwned's k-anonymity range
//! API) is **opt-in and off by default**, and `PRIVACY.md` says making that
//! call on the user's behalf "is not the developer's decision to make".
//!
//! So this screen does not make it. It performs **no** breach lookup, in
//! either state of the setting -- not when it is off, and not when it is on.
//! Opening a report over a 1600-item vault is not consent to 1600 outbound
//! requests, and the setting the user actually agreed to governs the
//! per-password badge on the detail pane, one item at a time, when they look
//! at it. What this screen does instead is **say so out loud**: a footer line
//! that names the state of the setting and where the setting is, so
//! "breached" being absent from this report is never mistaken for "none of
//! your passwords are breached". See [`breach_note`].

use super::sidebar;
use crate::password_strength::{rate, CharClasses, Strength};
use crate::theme;
use crate::vault_bridge::VaultItem;
use eframe::egui::{self, CornerRadius, Margin};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// The rail row that opens this screen. Spelled once, so the sidebar and the
/// tests that press it cannot drift apart -- exactly as
/// [`sidebar::SENDS_ROW_LABEL`] is.
pub const HEALTH_ROW_LABEL: &str = "Password health";

/// The pane's own title, at the top of the column.
pub const HEALTH_PANE_TITLE: &str = "Password health";

/// Where the breach setting lives, named the way the user will find it.
///
/// The row's label is spelled here as its own literal because `prefs_ui`'s
/// `BREACH_LABEL` is private to that module. `the_breach_setting_is_named_
/// where_it_actually_is` reads `prefs_ui.rs` off disk and fails if the two
/// ever disagree, so renaming the preference and leaving this sentence
/// behind reds the suite rather than sending the user hunting.
pub const BREACH_SETTING_LOCATION: &str =
    "Preferences > General > \"Check passwords against known breaches\"";

/// One item a finding is about: enough to draw its row and to select it, and
/// **nothing else**.
///
/// Deliberately not a `VaultItem` and not a borrow of one. Both the id and
/// the name are already on screen elsewhere in this window and neither is a
/// secret; carrying the item itself would put a `Zeroizing` password inside
/// the report, which is the long-lived plaintext this module exists to avoid.
/// `Debug` is derived for exactly that reason -- there is nothing here that
/// came from a password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundItem {
    pub id: String,
    pub name: String,
}

/// A set of two or more items sharing one password.
///
/// Never one item: a group of one is not reuse, and [`report_for`] does not
/// emit one -- see `a_password_used_once_is_not_a_group`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReuseGroup {
    /// In vault order, so the group reads the same way twice running.
    pub items: Vec<FoundItem>,
}

/// One item whose password [`rate`] calls [`Strength::Weak`], with the two
/// facts that made it so.
///
/// `chars` and `classes` are what the row actually says. They are carried
/// rather than recomputed at the draw site because recomputing means holding
/// the password again, at a second place, for the length of a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeakItem {
    pub item: FoundItem,
    /// Characters, not bytes: a password of five emoji is five characters,
    /// and telling the user it is twenty would be telling them something
    /// they cannot check.
    pub chars: usize,
    pub classes: CharClasses,
}

/// Everything this screen says about one vault, computed once.
///
/// **No `Zeroizing` anywhere in it, by construction**, which is what makes it
/// safe to hold for the length of a frame and safe to derive `Debug` on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HealthReport {
    /// Groups of items sharing a password, ordered by where the group's first
    /// member sits in the vault.
    pub reused: Vec<ReuseGroup>,
    /// Items whose password is weak, in vault order.
    pub weak: Vec<WeakItem>,
    /// How many items carried a non-empty password at all -- the denominator
    /// this report is over.
    ///
    /// **The empty state reads this, and that is why it exists.** "No reused
    /// passwords" over a vault of 300 logins is a result; the same words over
    /// a vault of nothing but cards is a non-answer wearing a result's
    /// clothes, and the two must not look alike. See [`Summary`].
    pub with_password: usize,
}

impl HealthReport {
    /// How many distinct items have at least one finding against them -- the
    /// number on the rail's badge.
    ///
    /// Distinct, because a password can be both reused and weak and one item
    /// counted twice would put a badge on the rail that no list on this
    /// screen adds up to.
    pub fn flagged_items(&self) -> usize {
        let mut ids: Vec<&str> = self
            .reused
            .iter()
            .flat_map(|group| group.items.iter())
            .chain(self.weak.iter().map(|weak| &weak.item))
            .map(|found| found.id.as_str())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids.len()
    }
}

/// The password this item is judged on, or `None` if it has none.
///
/// **The single gate keeping cards, notes, SSH keys, identities and
/// password-less logins out of the report.** An empty string is `None` here
/// and not a zero-length password: a login whose password field was never
/// filled in has nothing to be weak about, and reporting it would be the
/// "a card is weak" bug in a different hat.
///
/// Returns a borrow. It does not clone, and must not be made to: a clone is a
/// second plaintext copy on the heap, and the only reason this function
/// exists is so that there is exactly one place that decides what counts.
fn password_of(item: &VaultItem) -> Option<&str> {
    item.login
        .as_ref()
        .and_then(|login| login.password.as_ref())
        .map(|password| password.as_str())
        .filter(|password| !password.is_empty())
}

/// The whole report, over the live vault.
///
/// See the module docs for the grouping key's lifetime and for the
/// complexity. In short: one SHA-256 per password into a local `Vec` of
/// `Zeroizing` digests, an index sort, and one linear pass. Nothing here
/// keeps a plaintext password anywhere but in the borrow it reads.
pub fn report_for(items: &[VaultItem]) -> HealthReport {
    // (digest, index into `items`). The ONLY place a password-derived value
    // lives, and it dies at the end of this function -- see the module docs.
    let mut keyed: Vec<(Zeroizing<[u8; 32]>, usize)> = Vec::new();
    let mut weak = Vec::new();

    for (index, item) in items.iter().enumerate() {
        let Some(password) = password_of(item) else {
            // Not weak, not safe, not counted. See `password_of`.
            continue;
        };
        // Wiped on drop, and filled by copy so the `GenericArray` `Sha256`
        // returns is the only unwiped copy -- it is stack-resident and 32
        // bytes, exactly as the SHA-1 module documents for its own digest.
        let mut digest = Zeroizing::new([0u8; 32]);
        digest.copy_from_slice(Sha256::digest(password.as_bytes()).as_slice());
        keyed.push((digest, index));

        if rate(password) == Strength::Weak {
            weak.push(WeakItem {
                item: found(item),
                chars: password.chars().count(),
                classes: CharClasses::of(password),
            });
        }
    }
    let with_password = keyed.len();

    // **The indices are sorted, not `keyed` itself.** `Vec::sort_by` moves
    // its elements through a merge buffer, and a digest copied into that
    // buffer is a copy nothing wipes -- `Zeroizing`'s drop runs on the
    // element, not on the scratch space it was moved through. Sorting a
    // `Vec<usize>` and looking the digests up leaves every digest where it
    // was allocated, so each is wiped exactly once, on drop, from the one
    // place it ever lived.
    let mut order: Vec<usize> = (0..keyed.len()).collect();
    order.sort_by(|a, b| {
        keyed[*a]
            .0
            .as_slice()
            .cmp(keyed[*b].0.as_slice())
            // Ties broken by vault position, so a group's members come out in
            // the order the vault holds them and two runs of this function
            // over one vault cannot disagree about it.
            .then(keyed[*a].1.cmp(&keyed[*b].1))
    });

    let mut reused = Vec::new();
    let mut run_start = 0usize;
    while run_start < order.len() {
        let mut run_end = run_start + 1;
        while run_end < order.len()
            && keyed[order[run_end]].0.as_slice() == keyed[order[run_start]].0.as_slice()
        {
            run_end += 1;
        }
        // A run of one is a password used once, which is not a finding.
        if run_end - run_start >= 2 {
            reused.push(ReuseGroup {
                items: order[run_start..run_end]
                    .iter()
                    .map(|slot| found(&items[keyed[*slot].1]))
                    .collect(),
            });
        }
        run_start = run_end;
    }

    // Sorted by digest until now, which is an order with no meaning to a
    // reader. Re-ordered by where each group's first member sits in the
    // vault, so the screen reads top-to-bottom the way the vault does. The
    // key is the member's index, which the tie-break above already made the
    // smallest one in the group.
    reused.sort_by_key(|group| {
        group
            .items
            .first()
            .and_then(|first| items.iter().position(|item| item.id == first.id))
            .unwrap_or(usize::MAX)
    });

    HealthReport { reused, weak, with_password }
}

/// The non-secret half of an item: what a finding row draws and clicks.
fn found(item: &VaultItem) -> FoundItem {
    FoundItem { id: item.id.clone(), name: item.name.clone() }
}

/// The sentence under a weak item: the two facts that produced the verdict.
///
/// "9 characters, lowercase letters and digits" -- something to act on. Never
/// a score; see the module docs.
pub fn weak_detail(weak: &WeakItem) -> String {
    let unit = if weak.chars == 1 { "character" } else { "characters" };
    let names = weak.classes.names();
    let classes = match names.len() {
        // Unreachable for a real finding -- an empty password is excluded by
        // `password_of` long before it gets here -- but stated rather than
        // left to produce a sentence with a hole in it.
        0 => "no characters at all".to_string(),
        1 => format!("{} only", names[0]),
        _ => {
            let (last, rest) = names.split_last().expect("len >= 2 in this arm");
            format!("{} and {}", rest.join(", "), last)
        }
    };
    format!("{} {unit}, {classes}", weak.chars)
}

/// The heading over one reuse group.
pub fn reuse_group_heading(group: &ReuseGroup) -> String {
    format!("One password, {} items", group.items.len())
}

/// What the top of the screen says, before any list.
///
/// Three states, and **the clean one is a result, not an absence.** A blank
/// panel and a grey "Nothing here yet" both read as a failed load, and "no
/// reused passwords" is the answer the user came for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Summary {
    /// No item in this vault carries a password at all -- so there is nothing
    /// to report, and saying "no reused passwords" would be claiming a result
    /// this app has not got. Its own state, deliberately.
    NothingToCheck,
    /// Passwords were checked and none of them is reused or weak.
    AllClear,
    /// Findings, with the number of distinct items they are about.
    Findings(usize),
}

/// Decides between the three. Pure, so the empty state can be asserted
/// without a rendered frame.
pub fn summary_of(report: &HealthReport) -> Summary {
    if report.with_password == 0 {
        return Summary::NothingToCheck;
    }
    match report.flagged_items() {
        0 => Summary::AllClear,
        flagged => Summary::Findings(flagged),
    }
}

impl Summary {
    /// The headline. One wording per state, here rather than three literals
    /// at the draw site, so the states can be asserted without a frame.
    pub fn headline(self) -> String {
        match self {
            Summary::NothingToCheck => "No saved passwords to check".to_string(),
            Summary::AllClear => "No reused or weak passwords".to_string(),
            Summary::Findings(1) => "1 item needs attention".to_string(),
            Summary::Findings(n) => format!("{n} items need attention"),
        }
    }

    /// The line under the headline. Always present: the headline alone does
    /// not say what it was measured over, and "no reused passwords" over a
    /// vault the app read three logins out of is a much smaller claim than it
    /// looks.
    pub fn detail(self, report: &HealthReport) -> String {
        let n = report.with_password;
        let noun = if n == 1 { "password" } else { "passwords" };
        match self {
            Summary::NothingToCheck => {
                "Cards, notes, SSH keys and logins with an empty password are not checked."
                    .to_string()
            }
            Summary::AllClear => format!("Checked {n} {noun}."),
            Summary::Findings(_) => format!("Out of {n} {noun} checked."),
        }
    }
}

/// The footer that says what this report could NOT tell you, and where to
/// change that.
///
/// **Both states say something.** "Breach checking is on" is not permission
/// for this screen to have used it -- see the module docs -- so the on-state
/// wording says where breach status actually appears instead of leaving its
/// absence from here looking like an oversight.
pub fn breach_note(breach_checking_on: bool) -> String {
    if breach_checking_on {
        format!(
            "Breach data is not part of this report. Breach status is checked one password at \
             a time, on the item's own pane. The setting is {BREACH_SETTING_LOCATION}."
        )
    } else {
        format!(
            "Breach data is unavailable: checking passwords against known breaches is off, so \
             nothing here has been compared against a breach list. Turn it on in \
             {BREACH_SETTING_LOCATION}."
        )
    }
}

/// Vertical gap between finding rows, matching the item list's own `gap: 6px`.
const ROW_GAP: f32 = 6.0;
/// The pane's padding, matching the item list's `padding: 10px`.
const PANE_PADDING: f32 = 10.0;
/// Height of one finding row: two text lines plus the design's row padding.
const ROW_HEIGHT: f32 = 46.0;
/// How far a finding row's text is inset from the tile's edge, on BOTH sides.
/// The right-hand one is what stops a long item name being painted out over
/// the pane behind the tile; it is the same value as the left so a truncated
/// name's ellipsis is not hung flush against the rounded corner.
const ROW_TEXT_INSET: f32 = 12.0;

/// The Password health screen, drawn in the item-list column.
///
/// Writes the clicked finding's id straight into `selected_id`, which is the
/// same selection the detail pane on the right reads -- so a click here fills
/// that pane in without this screen going anywhere. That is the whole reason
/// the report replaces the item list rather than the whole window body: a
/// finding you cannot click through to the item is a dead end.
///
/// Takes the report rather than the items, so every decision on screen has
/// already been made by [`report_for`] and this function only paints.
pub fn draw_password_health(
    ui: &mut egui::Ui,
    report: &HealthReport,
    selected_id: &mut Option<String>,
    breach_checking_on: bool,
) {
    // Zeroed before the strip, for the reason `draw_item_list` spells out:
    // egui commits `item_spacing` as each widget is allocated, so a gap left
    // at the ambient 8 here cannot be retracted afterwards.
    ui.spacing_mut().item_spacing.y = 0.0;

    // The same white header strip the item list carries, so the two columns
    // read as one pane that changed its contents rather than two apps.
    egui::Frame::new()
        .fill(theme::CARD)
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::semibold(HEALTH_PANE_TITLE, 13.0).color(theme::INK));
        });
    let strip_bottom = ui.min_rect().bottom();
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            egui::Pos2::new(ui.min_rect().left(), strip_bottom - 1.0),
            egui::Pos2::new(ui.min_rect().right(), strip_bottom),
        ),
        CornerRadius::ZERO,
        theme::HAIRLINE,
    );

    egui::Frame::new()
        .inner_margin(Margin {
            left: PANE_PADDING as i8,
            right: 0,
            top: PANE_PADDING as i8,
            bottom: PANE_PADDING as i8,
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = ROW_GAP;
            theme::scrollbar_in_gutter(ui, PANE_PADDING);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.set_width((ui.available_width() - PANE_PADDING).max(0.0));
                draw_summary(ui, report);
                if !report.reused.is_empty() {
                    section_heading(ui, REUSED_HEADING);
                    for group in &report.reused {
                        group_heading(ui, &reuse_group_heading(group));
                        for item in &group.items {
                            let selected = selected_id.as_deref() == Some(item.id.as_str());
                            if finding_row(ui, &item.name, None, selected) {
                                *selected_id = Some(item.id.clone());
                            }
                        }
                    }
                }
                if !report.weak.is_empty() {
                    section_heading(ui, WEAK_HEADING);
                    for weak in &report.weak {
                        let detail = weak_detail(weak);
                        let selected = selected_id.as_deref() == Some(weak.item.id.as_str());
                        if finding_row(ui, &weak.item.name, Some(&detail), selected) {
                            *selected_id = Some(weak.item.id.clone());
                        }
                    }
                }
                ui.add_space(6.0);
                footer_note(ui, &breach_note(breach_checking_on));
            });
        });
}

/// The two section bands, spelled once so the tests that look for them and
/// the pane that paints them cannot drift.
pub const REUSED_HEADING: &str = "REUSED";
pub const WEAK_HEADING: &str = "WEAK";

/// The headline block. Drawn on a card in ordinary ink, exactly as a block
/// with findings is -- an all-clear vault gets a result, not a grey
/// placeholder that reads like a load which never finished.
fn draw_summary(ui: &mut egui::Ui, report: &HealthReport) {
    let summary = summary_of(report);
    egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.label(theme::semibold(summary.headline(), 14.0).color(match summary {
                // `theme::ERROR` is what this window uses for "something is
                // wrong" everywhere else. The other two states are ordinary
                // ink: there is no success colour in this design, and
                // inventing a green one here would be one app's worth of
                // visual language in a window that has another.
                Summary::Findings(_) => theme::ERROR,
                Summary::AllClear | Summary::NothingToCheck => theme::INK,
            }));
            ui.label(
                egui::RichText::new(summary.detail(report))
                    .size(12.0)
                    .color(theme::TEXT_SECONDARY),
            );
        });
}

/// A "REUSED"/"WEAK" band, in the sidebar's own section-label idiom.
fn section_heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(theme::letterspaced(text, 11.0, theme::BOLD, 1.2, theme::TEXT_GHOST));
}

/// "One password, 3 items", over the rows it covers.
fn group_heading(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(11.0).color(theme::TEXT_MUTED));
}

/// Where to paint `galley` so its left edge is at `left` and it is centred on
/// `centre_y` -- i.e. what `Align2::LEFT_CENTER` did for the `Painter::text`
/// calls this row used before it had to lay its own galleys out. `Painter::
/// galley` positions by the galley's top-left corner and offers no alignment,
/// so the half-height comes off here and the two baselines stay exactly where
/// the design put them.
fn centred_left(left: f32, centre_y: f32, galley: &egui::Galley) -> egui::Pos2 {
    egui::Pos2::new(left, centre_y - galley.size().y / 2.0)
}

/// One clickable finding. Returns whether it was clicked.
fn finding_row(ui: &mut egui::Ui, name: &str, detail: Option<&str>, selected: bool) -> bool {
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(8),
        if selected {
            theme::BLUE_WASH
        } else if response.hovered() {
            theme::CARD_TINT
        } else {
            theme::CARD
        },
    );
    // Both runs are laid into the room the tile actually has BEFORE anything
    // is painted, because `Painter::text` takes no width and would otherwise
    // draw a long name straight out past the tile's right edge and over the
    // pane behind it -- which is exactly the defect this row was reported
    // for. Nothing at all sits at the right of a finding row (no chevron, no
    // badge, no count), so the room is the tile less its inset on each side:
    // the ellipsis lands the same 12pt in from the right edge that the name
    // starts in from the left, rather than flush against the corner radius.
    let room = rect.width() - ROW_TEXT_INSET * 2.0;
    let name_galley = theme::truncated_galley(
        ui,
        theme::semibold(name, 13.0)
            .color(if selected { theme::BLUE_DEEP } else { theme::INK }),
        room,
        egui::TextStyle::Body,
    );
    // The detail line is ours, not the user's ("9 characters, lowercase
    // letters and digits"), but it is still longer than a narrow pane at a
    // long character-class list, so it is bounded by the same room.
    let detail_galley = detail.map(|detail| {
        theme::truncated_galley(
            ui,
            egui::RichText::new(detail).size(11.0).color(theme::TEXT_SECONDARY),
            room,
            egui::TextStyle::Body,
        )
    });
    let left = rect.left() + ROW_TEXT_INSET;
    // With a detail line the name sits on the upper of two baselines; alone,
    // it is centred in the row.
    let name_y = match detail {
        Some(_) => rect.top() + 15.0,
        None => rect.center().y,
    };
    ui.painter().galley(
        centred_left(left, name_y, &name_galley),
        name_galley,
        if selected { theme::BLUE_DEEP } else { theme::INK },
    );
    if let Some(galley) = detail_galley {
        let y = rect.bottom() - 15.0;
        ui.painter().galley(centred_left(left, y, &galley), galley, theme::TEXT_SECONDARY);
    }
    response.clicked()
}

/// The breach footer. Wrapped, muted, and always present in both states.
fn footer_note(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .fill(theme::CANVAS)
        .inner_margin(Margin::symmetric(2, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(text).size(11.0).color(theme::TEXT_MUTED));
        });
}

/// The rail row this screen must leave selected behind it.
///
/// **This is not decoration.** The detail pane resolves `selected_id` against
/// `list_for(filter.source(), ..)`, so with Trash or Archive selected behind
/// this screen a clicked finding -- which is always a LIVE vault item --
/// would be looked up in a list that by construction does not hold it, and
/// the pane would sit on "Select an item." for a row the user just clicked.
/// `vault_window::mod` calls this when the row is pressed; here is where
/// "a live-vault row" is decided, once.
///
/// A live-vault row is kept as it is: the user's own scope is theirs, and
/// leaving this screen should put them back where they were.
pub fn opening_filter(current: &sidebar::SidebarFilter) -> sidebar::SidebarFilter {
    match current.source() {
        sidebar::FilterSource::LiveVault => current.clone(),
        // All items, and not the row that was selected: every finding on this
        // screen is a live item, so the scope behind the report is the whole
        // live vault.
        sidebar::FilterSource::Trash | sidebar::FilterSource::Archive => {
            sidebar::SidebarFilter::All
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_bridge::LoginData;

    /// A login carrying `password`, or -- for `None` -- a login object with
    /// no password field at all.
    fn login(id: &str, name: &str, password: Option<&str>) -> VaultItem {
        let mut item = bare(id, name);
        item.item_type = Some(1);
        item.login = Some(LoginData {
            username: Some("user".into()),
            password: password.map(|p| Zeroizing::new(p.to_string())),
            totp: None,
            uris: vec![],
            other: serde_json::Map::new(),
        });
        item
    }

    /// An item with no `login` object whatsoever -- what a card, a secure
    /// note, an identity or an SSH key looks like to this module.
    fn bare(id: &str, name: &str) -> VaultItem {
        VaultItem {
            id: id.into(),
            name: name.into(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: Some(3),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    /// A password `rate` calls `Strength::Strong`, so a vault built out of
    /// these has no weak finding to confuse a reuse assertion with.
    const STRONG_A: &str = "Tr0ub4dor&3xtraLong!";
    const STRONG_B: &str = "Qu1etHarbour#Lantern9";
    const STRONG_C: &str = "V3lvet*Meridian~Fold4";

    /// The names of the items in each reuse group, in order -- what the pane
    /// actually lists.
    fn group_names(report: &HealthReport) -> Vec<Vec<&str>> {
        report
            .reused
            .iter()
            .map(|group| group.items.iter().map(|i| i.name.as_str()).collect())
            .collect()
    }

    fn weak_names(report: &HealthReport) -> Vec<&str> {
        report.weak.iter().map(|w| w.item.name.as_str()).collect()
    }

    // ==================================================================
    // Reuse: found where it is there, and absent where it is not
    // ==================================================================

    /// The headline claim, and its counterpart directly below it. **Both go
    /// through `report_for`**, so neither is a test of a fixture.
    #[test]
    fn two_items_sharing_a_password_are_reported_as_one_group() {
        let items = vec![
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_B)),
            login("c", "Forum", Some(STRONG_A)),
        ];
        let report = report_for(&items);
        assert_eq!(group_names(&report), vec![vec!["Bank", "Forum"]]);
        assert_eq!(report.with_password, 3);
        assert!(report.weak.is_empty(), "these three are all strong: {:?}", weak_names(&report));
    }

    /// The negative half of the pair, from the same function: change one
    /// password and the group must disappear entirely, not merely shrink.
    #[test]
    fn a_vault_with_no_reuse_yields_no_groups_at_all() {
        let items = vec![
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_B)),
            login("c", "Forum", Some(STRONG_C)),
        ];
        let report = report_for(&items);
        assert!(
            report.reused.is_empty(),
            "three different passwords produced reuse groups: {:?}",
            group_names(&report)
        );
        assert_eq!(report.with_password, 3, "the control: all three were actually looked at");
        assert_eq!(summary_of(&report), Summary::AllClear);
    }

    #[test]
    fn a_password_used_once_is_not_a_group() {
        let report = report_for(&[login("a", "Bank", Some(STRONG_A))]);
        assert!(report.reused.is_empty());
        assert_eq!(report.with_password, 1);
    }

    #[test]
    fn three_items_on_one_password_are_one_group_of_three_and_not_three_pairs() {
        let items = vec![
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_A)),
            login("c", "Forum", Some(STRONG_A)),
        ];
        let report = report_for(&items);
        assert_eq!(report.reused.len(), 1, "{:?}", group_names(&report));
        assert_eq!(group_names(&report), vec![vec!["Bank", "Shop", "Forum"]]);
        assert_eq!(reuse_group_heading(&report.reused[0]), "One password, 3 items");
    }

    /// Two independent groups, and the exact count -- an implementation that
    /// merged every reused password into one bucket, or that emitted a group
    /// per pair, both fail here and neither fails an `is_empty()` check.
    #[test]
    fn two_separate_reuses_are_two_groups_in_vault_order() {
        let items = vec![
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_B)),
            login("c", "Forum", Some(STRONG_A)),
            login("d", "Mail", Some(STRONG_B)),
        ];
        let report = report_for(&items);
        assert_eq!(
            group_names(&report),
            vec![vec!["Bank", "Forum"], vec!["Shop", "Mail"]],
            "groups must come out ordered by their first member's position in the vault, so \
             the screen reads the way the vault does"
        );
    }

    /// The grouping is byte-exact, not a similarity heuristic: two passwords
    /// differing in one character's case are two passwords.
    #[test]
    fn a_one_character_difference_is_not_reuse() {
        let items = vec![
            login("a", "Bank", Some("Tr0ub4dor&3xtraLong!")),
            login("b", "Shop", Some("Tr0ub4dor&3xtraLong?")),
        ];
        assert!(report_for(&items).reused.is_empty());
    }

    // ==================================================================
    // Items with no password: excluded, not counted as weak
    // ==================================================================

    /// **A card reported as "weak password" is a bug.** Asserted with a real
    /// weak login sitting in the same vault, so the test cannot pass by the
    /// weak list being empty for some unrelated reason.
    #[test]
    fn items_without_a_password_are_excluded_rather_than_counted_as_weak() {
        let items = vec![
            bare("card", "Visa"),
            bare("note", "Recovery codes"),
            login("empty", "Half-filled login", Some("")),
            login("absent", "Passwordless login", None),
            login("weak", "Old forum", Some("abc")),
        ];
        let report = report_for(&items);
        assert_eq!(
            weak_names(&report),
            vec!["Old forum"],
            "only the item that actually has a password may be reported"
        );
        assert_eq!(
            report.with_password, 1,
            "four of these five have nothing to check, so the denominator is 1"
        );
        assert!(report.reused.is_empty(), "two empty passwords are not a shared password");
    }

    /// The specific trap: two items with an EMPTY password would group
    /// together on any implementation that hashed before it filtered, and
    /// would be reported as a shared password the user cannot find.
    #[test]
    fn two_empty_passwords_are_not_a_reuse_group() {
        let items = vec![
            login("a", "One", Some("")),
            login("b", "Two", Some("")),
            login("c", "Three", None),
        ];
        let report = report_for(&items);
        assert!(report.reused.is_empty(), "{:?}", group_names(&report));
        assert_eq!(report.with_password, 0);
        assert_eq!(summary_of(&report), Summary::NothingToCheck);
    }

    // ==================================================================
    // Weakness: exactly what `rate` says, and nothing of its own
    // ==================================================================

    /// **One rule, two surfaces.** Walked over a table that contains both
    /// verdicts, so a `report_for` that flagged everything and one that
    /// flagged nothing both fail.
    #[test]
    fn the_weak_list_is_exactly_the_passwords_rate_calls_weak() {
        let passwords = [
            "abc",                    // < 8: Weak
            "abcdefgh",               // 8, one class: Weak
            "Ab1!efgh",               // 8, four classes: Fair
            "horsehorsehorse",        // 15, one class: Fair
            "Tr0ub4dor&3xtraLong!",   // Strong
        ];
        let items: Vec<VaultItem> = passwords
            .iter()
            .enumerate()
            .map(|(n, p)| login(&n.to_string(), &n.to_string(), Some(p)))
            .collect();
        let report = report_for(&items);

        let expected: Vec<String> = passwords
            .iter()
            .enumerate()
            .filter(|(_, p)| rate(p) == Strength::Weak)
            .map(|(n, _)| n.to_string())
            .collect();
        assert_eq!(expected.len(), 2, "control: the table must contain both verdicts");
        assert_eq!(weak_names(&report), expected.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(
            report.with_password,
            passwords.len(),
            "control: every password in the table was actually reached"
        );
    }

    /// The two facts carried with the finding are the password's own, not a
    /// default: a wrong-item lookup would give the wrong length here.
    #[test]
    fn a_weak_finding_carries_that_password_s_own_length_and_classes() {
        let items = vec![
            login("a", "Strong", Some("Tr0ub4dor&3xtraLong!")),
            login("b", "Old forum", Some("hunter22")),
        ];
        let report = report_for(&items);
        assert_eq!(report.weak.len(), 1);
        let weak = &report.weak[0];
        assert_eq!(weak.item.name, "Old forum");
        assert_eq!(weak.chars, 8);
        assert_eq!(weak.classes, CharClasses::of("hunter22"));
        assert_eq!(weak_detail(weak), "8 characters, lowercase letters and digits");
    }

    /// Characters, not bytes. `"pa\u{df}wort"` is 7 characters and 8 bytes;
    /// telling the user it is 8 would be telling them something they cannot
    /// count on screen.
    #[test]
    fn the_length_reported_is_characters_and_not_bytes() {
        let password = "pa\u{df}wort";
        assert_eq!(password.len(), 8, "control: this string really is 8 bytes");
        let report = report_for(&[login("a", "German", Some(password))]);
        assert_eq!(report.weak.len(), 1, "7 characters is under 8, so this is weak");
        assert_eq!(report.weak[0].chars, 7);
    }

    /// Every branch of the sentence, including the one-class case the "12
    /// characters, letters only" example is about.
    #[test]
    fn the_weak_sentence_names_the_classes_actually_present() {
        let one = WeakItem {
            item: FoundItem { id: "1".into(), name: "n".into() },
            chars: 12,
            classes: CharClasses::of("abcdefghijkl"),
        };
        assert_eq!(weak_detail(&one), "12 characters, lowercase letters only");

        let three = WeakItem {
            item: FoundItem { id: "1".into(), name: "n".into() },
            chars: 9,
            classes: CharClasses::of("aB1"),
        };
        assert_eq!(
            weak_detail(&three),
            "9 characters, lowercase letters, uppercase letters and digits"
        );

        let single_char = WeakItem {
            item: FoundItem { id: "1".into(), name: "n".into() },
            chars: 1,
            classes: CharClasses::of("7"),
        };
        assert_eq!(weak_detail(&single_char), "1 character, digits only");
    }

    /// **No score, stated as a test.** A "43/100" would be a number the user
    /// cannot act on, and this is the assertion that keeps one from being
    /// added to the row's own sentence later.
    #[test]
    fn the_weak_sentence_carries_no_score() {
        let weak = WeakItem {
            item: FoundItem { id: "1".into(), name: "n".into() },
            chars: 6,
            classes: CharClasses::of("abc123"),
        };
        let sentence = weak_detail(&weak);
        assert!(sentence.contains("6 characters"), "the actionable fact is missing: {sentence}");
        for banned in ["/100", "score", "Score", "%"] {
            assert!(!sentence.contains(banned), "{sentence:?} carries a score");
        }
    }

    // ==================================================================
    // The badge, and the three summary states
    // ==================================================================

    /// An item that is both reused and weak is ONE item needing attention.
    /// The counter-assertion is the test: the two lists really do both hold
    /// it, so a naive `reused + weak` would say 3 here.
    #[test]
    fn an_item_that_is_both_reused_and_weak_is_counted_once() {
        let items = vec![
            login("a", "One", Some("abc")),
            login("b", "Two", Some("abc")),
        ];
        let report = report_for(&items);
        assert_eq!(report.reused.len(), 1);
        assert_eq!(report.reused[0].items.len(), 2);
        assert_eq!(report.weak.len(), 2, "the premise: both are also weak");
        assert_eq!(report.flagged_items(), 2);
        assert_eq!(summary_of(&report), Summary::Findings(2));
        assert_eq!(Summary::Findings(2).headline(), "2 items need attention");
    }

    /// The three states, each from a vault that really produces it, and each
    /// with a headline distinct from the other two -- so a `summary_of` wired
    /// to a constant fails.
    #[test]
    fn the_three_summary_states_come_from_three_different_vaults() {
        let nothing = report_for(&[bare("card", "Visa")]);
        let clear = report_for(&[
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_B)),
        ]);
        let findings = report_for(&[
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_A)),
        ]);

        assert_eq!(summary_of(&nothing), Summary::NothingToCheck);
        assert_eq!(summary_of(&clear), Summary::AllClear);
        assert_eq!(summary_of(&findings), Summary::Findings(2));

        let headlines = [
            summary_of(&nothing).headline(),
            summary_of(&clear).headline(),
            summary_of(&findings).headline(),
        ];
        let mut distinct = headlines.to_vec();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), 3, "two states say the same thing: {headlines:?}");
    }

    /// **The good empty state is a result, not an absence.** It says what was
    /// checked, and it does not use the item list's grey "nothing here"
    /// wording -- which is the shape of a load that failed.
    #[test]
    fn the_all_clear_state_reports_what_it_checked() {
        let report = report_for(&[
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_B)),
        ]);
        let summary = summary_of(&report);
        assert_eq!(summary.headline(), "No reused or weak passwords");
        assert_eq!(summary.detail(&report), "Checked 2 passwords.");
        for banned in ["Nothing here yet", "Loading", "No items"] {
            assert!(!summary.headline().contains(banned));
            assert!(!summary.detail(&report).contains(banned));
        }
    }

    /// "No reused passwords" over a vault with no passwords in it is a
    /// non-answer wearing a result's clothes. The two must not say the same
    /// thing, which is why `NothingToCheck` exists at all.
    #[test]
    fn an_unanswerable_vault_does_not_claim_a_clean_bill_of_health() {
        let nothing = report_for(&[bare("card", "Visa")]);
        let clear = report_for(&[login("a", "Bank", Some(STRONG_A))]);
        assert_ne!(summary_of(&nothing).headline(), summary_of(&clear).headline());
        assert_eq!(summary_of(&nothing).headline(), "No saved passwords to check");
        assert!(summary_of(&nothing).detail(&nothing).contains("Cards"));
    }

    #[test]
    fn the_singular_wordings_are_singular() {
        let one_finding = report_for(&[
            login("a", "One", Some("abc")),
            login("b", "Two", Some(STRONG_A)),
        ]);
        assert_eq!(summary_of(&one_finding), Summary::Findings(1));
        assert_eq!(summary_of(&one_finding).headline(), "1 item needs attention");

        let one_password = report_for(&[login("a", "Bank", Some(STRONG_A))]);
        assert_eq!(summary_of(&one_password).detail(&one_password), "Checked 1 password.");
    }

    // ==================================================================
    // Secrets: what the report holds, and what it does not
    // ==================================================================

    /// **The report is `Debug`-safe because it holds nothing derived from a
    /// password.** Constructed with a needle nothing else in this crate could
    /// produce, and asserted against the whole formatted report -- which is
    /// the check `debug_leak_guard`'s source scan cannot make.
    #[test]
    fn the_report_cannot_print_a_password_or_its_digest() {
        const NEEDLE: &str = "correct-horse-battery-staple-NEEDLE";
        let items = vec![
            login("a", "Bank", Some(NEEDLE)),
            login("b", "Shop", Some(NEEDLE)),
        ];
        let report = report_for(&items);
        assert_eq!(report.reused.len(), 1, "control: the needle really was reused");

        let printed = format!("{report:?}");
        assert!(
            printed.contains("Bank"),
            "control: the report prints something at all, so the assertion below is not \
             passing over an empty string: {printed}"
        );
        assert!(!printed.contains(NEEDLE), "the password reached a formatter: {printed}");
        // The digest, in the two spellings a leak would take.
        let digest = Sha256::digest(NEEDLE.as_bytes());
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert!(!printed.to_lowercase().contains(&hex), "the digest reached a formatter");
        assert!(
            !printed.contains(&format!("{}", digest[0])),
            "a byte of the digest reached a formatter -- the report is carrying the key"
        );
    }

    /// **No plaintext-keyed index, and no grouping key at all, survives the
    /// call.** A source pin, because the thing being asserted is structural:
    /// the digests must live in `report_for` and nowhere else.
    #[test]
    fn no_type_in_this_module_carries_a_password_or_a_digest() {
        let source = own_production();
        // The four declared types are the report's whole surface.
        for (name, body) in declared_type_bodies(&source) {
            // `" password:"` with the leading space, so the honest
            // `with_password` COUNT on the report is not mistaken for a
            // field holding one. A field literally named `password`
            // still fails.
            for banned in [
                concat!("Zeroiz", "ing"),
                "[u8; 32]",
                " password:",
                "digest",
                "Sha256",
            ] {
                assert!(
                    !body.contains(banned),
                    "`{name}` mentions {banned:?} in its body. Nothing the report hands back \
                     may carry a password or a value derived from one: the grouping key lives \
                     inside `report_for` and dies there. See the module docs"
                );
            }
        }
        assert!(
            source.contains(concat!("Zeroiz", "ing<[u8; 32]>")),
            "control: the digest key is not in this file at all, so the scan above is vacuous"
        );
        assert!(
            declared_type_bodies(&source).iter().any(|(name, _)| name == "WeakItem"),
            "control: the parser missed a type, so the loop above ran over the wrong set"
        );
    }

    /// **Nothing in this file logs, and nothing in it reaches the network.**
    /// File-scoped, exactly as `breach::the_breach_module_never_logs` is, and
    /// worth stating for the same reason: this is the one module that holds
    /// every password in the vault in its hands at once.
    ///
    /// The needles are the bare macro tokens rather than fully-qualified
    /// paths, because `use log::warn;` is the ordinary spelling and a
    /// path-only guard never sees it.
    #[test]
    fn this_module_neither_logs_nor_opens_anything() {
        let source = own_production();
        for banned in [
            concat!("log", "::"),
            concat!("warn", "!"),
            concat!("info", "!"),
            concat!("debug", "!"),
            concat!("error", "!"),
            concat!("trace", "!"),
            concat!("println", "!"),
            concat!("eprintln", "!"),
            concat!("dbg", "!"),
            concat!("http", "_agent"),
            concat!("brea", "ch::"),
            concat!("ureq", ""),
            concat!("Command", "::new"),
            concat!("std::fs", ""),
        ] {
            assert!(
                !source.contains(banned),
                "`password_health.rs` spells {banned:?} in its production half. This module \
                 sees every password in the vault; it does not log, it does not spawn, and it \
                 does not make a request -- see the module docs on breach data"
            );
        }
        assert!(
            source.contains("fn report_for"),
            "control: the production half was not read, so every needle above matched nothing"
        );
    }

    /// This file's production half -- everything above the `#[cfg(test)]`
    /// module this test is in -- **with its comments removed**.
    ///
    /// The strip is not tidiness. This module documents heavily and by name:
    /// its own docs discuss `crate::breach`, say the word "password" in
    /// almost every paragraph, and quote the very needles the two guards
    /// below search for. Over raw text those guards would fire on their own
    /// prose, and the obvious fix -- dropping the needles that collide -- is
    /// the fix that guts them. `a_needle_in_prose_is_not_code_here` is the
    /// control that this really strips.
    fn own_production() -> String {
        code_without_comments(&own_source())
    }

    /// The same production half with its comments left in, for the one test
    /// that wants to look at the file as written.
    fn own_source() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/vault_window/password_health.rs");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {path:?}: {e}"));
        let marker = "#[cfg(test)]";
        let cut = text
            .find(marker)
            .unwrap_or_else(|| panic!("no {marker} in {path:?}, so the split found nothing"));
        text[..cut].to_string()
    }

    /// `text` with everything from each `//` to end of line removed. Catches
    /// `//`, `///` and `//!` in one rule, which is every comment form this
    /// file uses.
    fn code_without_comments(text: &str) -> String {
        text.lines()
            .map(|line| match line.find("//") {
                Some(at) => &line[..at],
                None => line,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The control for [`code_without_comments`]: without it both guards
    /// below pass on their own doc comments and assert nothing about the
    /// code at all.
    #[test]
    fn a_needle_in_prose_is_not_code_here() {
        let stripped = code_without_comments(concat!(
            "//! see `crate::breach` and log::warn!\n",
            "/// a password\n",
            "let x = 1; // brea", "ch::check_prefix\n",
        ));
        assert!(!stripped.contains("breach"), "prose survived the strip: {stripped:?}");
        assert!(!stripped.contains("password"), "prose survived the strip: {stripped:?}");
        assert!(stripped.contains("let x = 1;"), "the strip ate real code: {stripped:?}");
        assert!(
            own_source().contains("crate::breach"),
            "control: this module's own docs no longer mention `crate::breach`, so the \
             strip above is not being exercised by the guards that use it"
        );
    }

    /// Each `pub struct` in `source`, as (name, the text between its braces).
    fn declared_type_bodies(source: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let head = "\npub struct ";
        let mut from = 0usize;
        while let Some(at) = source[from..].find(head) {
            let start = from + at + head.len();
            let name: String = source[start..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let Some(open) = source[start..].find('{').map(|o| start + o) else {
                break;
            };
            let close = crate::below_cut::match_brace(source, open);
            out.push((name, source[open..=close].to_string()));
            from = close;
        }
        assert!(
            out.len() >= 3,
            "control: only {} `pub struct` declarations were parsed out of this module, which \
             is not its surface: {:?}",
            out.len(),
            out.iter().map(|(n, _)| n).collect::<Vec<_>>()
        );
        out
    }

    // ==================================================================
    // Breach data being off
    // ==================================================================

    /// Both states, and both differ -- a `breach_note` that returned one
    /// string whatever the setting said would pass any single-arm check.
    #[test]
    fn the_breach_note_says_which_state_the_setting_is_in() {
        let off = breach_note(false);
        let on = breach_note(true);
        assert_ne!(off, on, "the note says the same thing whether or not the setting is on");
        assert!(off.contains("unavailable"), "{off}");
        assert!(!on.contains("unavailable"), "{on}");
        for note in [&off, &on] {
            assert!(
                note.contains(BREACH_SETTING_LOCATION),
                "the note does not say where the setting is: {note}"
            );
        }
    }

    /// **The literal is pinned against the real preference row.** This module
    /// cannot reference `prefs_ui::BREACH_LABEL` -- it is private there -- so
    /// the sentence spells the label out, and this reads that file off disk
    /// to make renaming it a red suite rather than a sentence pointing at a
    /// row that no longer exists.
    #[test]
    fn the_breach_setting_is_named_where_it_actually_is() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/prefs_ui.rs");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {path:?}: {e}"));
        let label = BREACH_SETTING_LOCATION
            .rsplit_once('"')
            .and_then(|(head, _)| head.rsplit_once('"').map(|(_, label)| label.to_string()))
            .expect("the location string quotes the row's label");
        assert_eq!(label, "Check passwords against known breaches", "control: the parse");
        assert!(
            text.contains(&format!("{label:?}")),
            "`prefs_ui.rs` has no row labelled {label:?} any more, so \
             `BREACH_SETTING_LOCATION` sends the user to a preference that is not there"
        );
    }

    /// **This screen makes no lookup in EITHER state**, which is the claim
    /// the note's on-state wording rests on. Driven, not merely read: a full
    /// report over a vault of real passwords, with the setting on.
    #[test]
    fn a_report_performs_no_breach_lookup_even_when_the_setting_is_on() {
        let items = vec![
            login("a", "Bank", Some("password")),
            login("b", "Shop", Some("password")),
        ];
        let report = report_for(&items);
        assert_eq!(report.reused.len(), 1, "control: the report really did run");
        // `report_for` takes no network handle, no `BreachCache` and no
        // setting, so there is no argument through which a lookup could be
        // made. The source pin above is the other half of this claim.
        assert!(breach_note(true).contains("not part of this report"));
    }

    // ==================================================================
    // The rail row it is entered from
    // ==================================================================

    #[test]
    fn opening_from_a_live_row_keeps_that_row_and_from_an_out_of_vault_row_does_not() {
        assert_eq!(
            opening_filter(&sidebar::SidebarFilter::Logins),
            sidebar::SidebarFilter::Logins,
            "a live-vault row is the user's own scope and is left alone"
        );
        assert_eq!(opening_filter(&sidebar::SidebarFilter::Trash), sidebar::SidebarFilter::All);
        assert_eq!(opening_filter(&sidebar::SidebarFilter::Archive), sidebar::SidebarFilter::All);
        assert_eq!(
            opening_filter(&sidebar::SidebarFilter::Folder("f1".into())),
            sidebar::SidebarFilter::Folder("f1".into())
        );
    }

    // ==================================================================
    // The pane, driven
    // ==================================================================

    fn styled_context() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw_input(None), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(raw_input(None), |_ui| {});
        ctx
    }

    fn raw_input(click_at: Option<egui::Pos2>) -> egui::RawInput {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(320.0, 700.0),
            )),
            ..Default::default()
        };
        if let Some(at) = click_at {
            input.events.push(egui::Event::PointerMoved(at));
            input.events.push(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            });
            input.events.push(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            });
        }
        input
    }

    /// Every string the pane painted, with the rect it was painted in.
    fn painted(
        ctx: &egui::Context,
        report: &HealthReport,
        selected_id: &mut Option<String>,
        breach_on: bool,
        click_at: Option<egui::Pos2>,
    ) -> Vec<(String, egui::Rect)> {
        let output = ctx.run_ui(raw_input(click_at), |ui| {
            draw_password_health(ui, report, selected_id, breach_on);
        });
        let mut out = Vec::new();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut out);
        }
        out
    }

    fn collect_text(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => {
                out.push((text.galley.text().to_string(), text.galley.rect.translate(text.pos.to_vec2())))
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, out);
                }
            }
            _ => {}
        }
    }

    fn locate(painted: &[(String, egui::Rect)], needle: &str) -> egui::Pos2 {
        painted
            .iter()
            .find(|(text, _)| text == needle)
            .map(|(_, rect)| rect.center())
            .unwrap_or_else(|| panic!("the pane painted no {needle:?}: {painted:?}"))
    }

    fn texts(painted: &[(String, egui::Rect)]) -> Vec<&str> {
        painted.iter().map(|(text, _)| text.as_str()).collect()
    }

    /// **The click-through, driven end to end.** A finding you cannot click
    /// through to the item is a dead end, and this is the assertion that the
    /// row is not decoration -- with the counter-assertion that the selection
    /// really did start out somewhere else.
    #[test]
    fn clicking_a_finding_selects_that_item() {
        let ctx = styled_context();
        let items = vec![
            login("id-bank", "Bank", Some(STRONG_A)),
            login("id-forum", "Forum", Some(STRONG_A)),
        ];
        let report = report_for(&items);
        let mut selected: Option<String> = None;

        let first = painted(&ctx, &report, &mut selected, false, None);
        assert!(selected.is_none(), "the premise: nothing is selected before the click");
        let forum_at = locate(&first, "Forum");

        let _ = painted(&ctx, &report, &mut selected, false, Some(forum_at));
        assert_eq!(
            selected.as_deref(),
            Some("id-forum"),
            "the finding row is painted but clicking it selects nothing, so the report is a \
             dead end"
        );
    }

    /// The counter-assertion for the one above: a click that lands on the
    /// summary card, not on a row, must select nothing. Without this, a row
    /// wired to "any click anywhere" passes the test above.
    #[test]
    fn clicking_away_from_a_finding_selects_nothing() {
        let ctx = styled_context();
        let items = vec![
            login("id-bank", "Bank", Some(STRONG_A)),
            login("id-forum", "Forum", Some(STRONG_A)),
        ];
        let report = report_for(&items);
        let mut selected: Option<String> = None;

        let first = painted(&ctx, &report, &mut selected, false, None);
        let headline_at = locate(&first, &summary_of(&report).headline());

        let _ = painted(&ctx, &report, &mut selected, false, Some(headline_at));
        assert_eq!(selected, None, "a click on the headline selected an item");
    }

    /// The pane lists the findings it was given: both group members, under
    /// the group's own heading, under the section band.
    #[test]
    fn the_pane_paints_the_groups_and_the_weak_rows_it_was_given() {
        let ctx = styled_context();
        let items = vec![
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Forum", Some(STRONG_A)),
            login("c", "Old wiki", Some("abc")),
        ];
        let report = report_for(&items);
        let mut selected = None;
        let shown = painted(&ctx, &report, &mut selected, false, None);
        let shown = texts(&shown);

        for expected in [
            HEALTH_PANE_TITLE,
            REUSED_HEADING,
            "One password, 2 items",
            "Bank",
            "Forum",
            WEAK_HEADING,
            "Old wiki",
            "3 characters, lowercase letters only",
        ] {
            assert!(shown.contains(&expected), "the pane never painted {expected:?}: {shown:?}");
        }
    }

    /// **The empty state looks like success and not like a failed load.** It
    /// paints its result, it paints what it checked, and it paints neither
    /// section band -- because there is nothing under either.
    #[test]
    fn the_empty_state_paints_a_result_and_no_empty_sections() {
        let ctx = styled_context();
        let items = vec![
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_B)),
        ];
        let report = report_for(&items);
        let mut selected = None;
        let shown = painted(&ctx, &report, &mut selected, false, None);
        let shown = texts(&shown);

        assert!(shown.contains(&"No reused or weak passwords"), "{shown:?}");
        assert!(shown.contains(&"Checked 2 passwords."), "{shown:?}");
        assert!(!shown.contains(&REUSED_HEADING), "an empty REUSED band was painted: {shown:?}");
        assert!(!shown.contains(&WEAK_HEADING), "an empty WEAK band was painted: {shown:?}");
        for banned in ["Nothing here yet", "Loading\u{2026}", "No items match your search"] {
            assert!(!shown.contains(&banned), "the item list's placeholder wording leaked in");
        }
    }

    /// The breach footer is on screen in both states, not only when the
    /// setting is off -- asserted from the painted frame, because a note that
    /// is computed and never drawn tells the user nothing.
    #[test]
    fn the_breach_footer_is_painted_in_both_states() {
        let ctx = styled_context();
        let report = report_for(&[login("a", "Bank", Some(STRONG_A))]);
        for on in [false, true] {
            let mut selected = None;
            let shown = painted(&ctx, &report, &mut selected, on, None);
            let joined = texts(&shown).join(" | ");
            assert!(
                joined.contains("Check passwords against known breaches"),
                "with the setting {on}, the pane never said where the breach setting is: \
                 {joined}"
            );
        }
    }

    // ==================================================================
    // Nothing is painted outside its tile
    // ==================================================================

    /// A name far longer than any pane this window can be dragged to -- the
    /// one from the user's screenshot, which ran straight off the right of
    /// its tile and over the pane behind it.
    const LONG_NAME: &str =
        "Visual Studio App Center | iOS, Android, Xamarin & React Native App Development";

    /// A name that comfortably fits even the narrowest pane tested here, so
    /// the counter-assertion has something real to be about.
    const SHORT_NAME: &str = "Bank";

    /// One run the pane painted: the string it was HANDED, the characters it
    /// actually LAID OUT, and where those characters landed.
    ///
    /// The first two differ exactly when something was truncated --
    /// `Galley::text()` reports the source string whatever the wrap mode did
    /// to it, so a test that only read `text()` could never tell an
    /// ellipsised run from an intact one, and would pass while the ink still
    /// hung outside the tile.
    struct Run {
        source: String,
        drawn: String,
        rect: egui::Rect,
    }

    /// One frame of the real pane at a chosen pane width, with the finding
    /// tiles located as well as the text.
    struct Painted {
        runs: Vec<Run>,
        /// The rounded white rectangles `finding_row` allocates, found by
        /// their exact `ROW_HEIGHT` -- so the summary card and the header
        /// strip, which are neither this height, are not mistaken for rows.
        tiles: Vec<egui::Rect>,
    }

    impl Painted {
        /// The single run laid out from `source`, and the tile it sits in.
        ///
        /// Both halves panic rather than return an `Option`: this crate's
        /// standing defect is a test that passes because it never reached the
        /// thing it names, and "no such run" or "no tile around it" must be a
        /// red test, not a silently skipped assertion.
        fn run_and_tile(&self, source: &str) -> (&Run, egui::Rect) {
            let found: Vec<&Run> = self.runs.iter().filter(|r| r.source == source).collect();
            assert_eq!(
                found.len(),
                1,
                "expected exactly one run laid out from {source:?}, found {}; painted: {:?}",
                found.len(),
                self.runs.iter().map(|r| r.source.as_str()).collect::<Vec<_>>()
            );
            let run = found[0];
            let tile = self
                .tiles
                .iter()
                .find(|tile| tile.contains(run.rect.left_center()))
                .copied()
                .unwrap_or_else(|| {
                    panic!(
                        "the run from {source:?} at {:?} sits in none of the {} finding tiles \
                         {:?} -- it was not painted on a finding row at all",
                        run.rect,
                        self.tiles.len(),
                        self.tiles
                    )
                });
            (run, tile)
        }
    }

    /// Draws the real pane at `pane_width` and reports every run and tile.
    ///
    /// The width is a parameter because the defect is width-dependent: the
    /// pane is the resizable item-list column, and a name that fits at 420
    /// overflows at 220.
    fn painted_at(report: &HealthReport, pane_width: f32) -> Painted {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(pane_width, 700.0),
            )),
            ..Default::default()
        };
        // The two throwaway frames every harness in this crate runs: a font
        // set registered during a frame is only usable from the next one.
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});
        let mut selected = None;
        let output = ctx.run_ui(input(), |ui| {
            draw_password_health(ui, report, &mut selected, false);
        });
        let mut painted = Painted { runs: Vec::new(), tiles: Vec::new() };
        for clipped in &output.shapes {
            collect_runs(&clipped.shape, &mut painted);
        }
        assert!(
            !painted.tiles.is_empty(),
            "the pane painted no finding tiles at all at width {pane_width}, so nothing below \
             is about a finding row"
        );
        painted
    }

    fn collect_runs(shape: &egui::Shape, out: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => out.runs.push(Run {
                source: text.galley.text().to_string(),
                drawn: text
                    .galley
                    .rows
                    .iter()
                    .flat_map(|row| row.glyphs.iter().map(|glyph| glyph.chr))
                    .collect(),
                rect: text.galley.rect.translate(text.pos.to_vec2()),
            }),
            egui::Shape::Rect(rect) if rect.rect.height() == ROW_HEIGHT => {
                out.tiles.push(rect.rect)
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_runs(shape, out);
                }
            }
            _ => {}
        }
    }

    /// The three pane widths every case below is checked at, including one
    /// narrower than the window's own default -- the column is resizable and
    /// the defect only appears once the name is wider than the tile.
    ///
    /// Deliberately NOT derived from anything under test, and deliberately
    /// nothing to do with the clock: a fixture that measured a rendered width
    /// against a real date is how `main` went red at UTC midnight once
    /// already.
    const PANE_WIDTHS: [f32; 3] = [200.0, 320.0, 520.0];

    /// **The reported defect: a long name's ink stays inside its tile.**
    ///
    /// Measured as ink against the tile's rect, not as "the string changed",
    /// because the report is about paint crossing an edge. Both bands are
    /// covered: a reused finding (name only) and a weak one (name over a
    /// detail line), which are the two different vertical layouts the row
    /// has.
    #[test]
    fn a_long_finding_name_is_ellipsised_inside_its_tile() {
        for width in PANE_WIDTHS {
            for report in [
                report_for(&[
                    login("a", LONG_NAME, Some(STRONG_A)),
                    login("b", "Other", Some(STRONG_A)),
                ]),
                report_for(&[login("a", LONG_NAME, Some("abc12345"))]),
            ] {
                let painted = painted_at(&report, width);
                let (run, tile) = painted.run_and_tile(LONG_NAME);
                assert!(
                    run.drawn != run.source,
                    "at width {width} the name was laid out whole ({:?}) -- nothing truncated \
                     it",
                    run.drawn
                );
                assert!(
                    run.drawn.ends_with('\u{2026}'),
                    "at width {width} the name was cut off with no ellipsis: {:?}",
                    run.drawn
                );
                assert!(
                    run.rect.right() <= tile.right() - ROW_TEXT_INSET + 0.5,
                    "at width {width} the name's ink reaches {} but its tile ends at {} \
                     (inset {ROW_TEXT_INSET}); drawn: {:?}",
                    run.rect.right(),
                    tile.right(),
                    run.drawn
                );
                assert!(
                    run.rect.left() >= tile.left(),
                    "at width {width} the name's ink starts left of its own tile"
                );
            }
        }
    }

    /// **The counter-assertion: a name that fits is left completely alone.**
    ///
    /// Without this, ellipsising every row unconditionally would satisfy the
    /// test above.
    #[test]
    fn a_short_finding_name_is_not_touched() {
        for width in PANE_WIDTHS {
            let report = report_for(&[
                login("a", SHORT_NAME, Some(STRONG_A)),
                login("b", "Other", Some(STRONG_A)),
            ]);
            let painted = painted_at(&report, width);
            let (run, tile) = painted.run_and_tile(SHORT_NAME);
            assert_eq!(
                run.drawn, SHORT_NAME,
                "at width {width} a name that fits was altered anyway"
            );
            assert!(
                !run.drawn.contains('\u{2026}'),
                "at width {width} a short name was ellipsised: {:?}",
                run.drawn
            );
            assert!(
                run.rect.right() <= tile.right() - ROW_TEXT_INSET + 0.5,
                "at width {width} even the short name left its tile"
            );
        }
    }

    /// **The weak band's detail line is bounded by the same tile.**
    ///
    /// It is our own wording rather than the user's, but "9 characters,
    /// lowercase letters and digits" is longer than a narrow pane, and it was
    /// painted by the very same unbounded `Painter::text` call the name was.
    #[test]
    fn the_weak_detail_line_stays_inside_its_tile() {
        let report = report_for(&[login("a", "A", Some("abc12345"))]);
        let detail = weak_detail(&report.weak[0]);
        // The fixture is only worth anything if this line really is longer
        // than the narrowest tile; assert that before asserting about it.
        let narrow = painted_at(&report, PANE_WIDTHS[0]);
        let (run, tile) = narrow.run_and_tile(&detail);
        assert!(
            run.drawn != detail,
            "the detail line {detail:?} fits the {}pt pane whole, so this test would pass \
             without any truncation at all -- it needs a longer fixture",
            PANE_WIDTHS[0]
        );
        assert!(run.drawn.ends_with('\u{2026}'), "the detail line was cut with no ellipsis");
        assert!(
            run.rect.right() <= tile.right() - ROW_TEXT_INSET + 0.5,
            "the detail line's ink reaches {} past a tile ending at {}",
            run.rect.right(),
            tile.right()
        );
        // ...and at a wide pane the same line is untouched.
        let wide = painted_at(&report, PANE_WIDTHS[2]);
        let (run, _) = wide.run_and_tile(&detail);
        assert_eq!(run.drawn, detail, "a detail line that fits was ellipsised anyway");
    }

    /// **The group caption cannot overflow, and this is why.**
    ///
    /// "One password, 2 items" is drawn with `ui.label`, not `Painter::text`
    /// -- an egui `Label` in a bounded `Ui` wraps at the available width, so
    /// it grows downwards and never sideways. Same for the section bands and
    /// the footer note. Asserted rather than merely reasoned about, because
    /// "it's a Label" is exactly the kind of claim that stops being true when
    /// someone adds `.truncate(false)` or an `Extend` wrap mode.
    #[test]
    fn every_run_the_pane_paints_stays_within_the_pane() {
        for width in PANE_WIDTHS {
            let report = report_for(&[
                login("a", LONG_NAME, Some(STRONG_A)),
                login("b", LONG_NAME, Some(STRONG_A)),
                login("c", "A", Some("abc12345")),
            ]);
            let painted = painted_at(&report, width);
            // The caption really is drawn, so the sweep below is about it.
            let caption = reuse_group_heading(&report.reused[0]);
            assert!(
                painted.runs.iter().any(|r| r.source == caption),
                "the pane painted no {caption:?} at width {width}: {:?}",
                painted.runs.iter().map(|r| r.source.as_str()).collect::<Vec<_>>()
            );
            for run in &painted.runs {
                assert!(
                    run.rect.right() <= width + 0.5,
                    "at width {width} the run {:?} was painted out to {}, past the pane's own \
                     right edge",
                    run.drawn,
                    run.rect.right()
                );
            }
        }
    }
}

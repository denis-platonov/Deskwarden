# Browser URL Matching Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When the user presses `CTRL+ALT+B` in a browser, the account picker reads the address bar and puts the logins saved for that website on top. The first row is the one the card already focuses, so Enter takes it. Nothing is typed until the user presses a key.

**Architecture:** There are two new pure modules and one observation function.
- `uri_match` holds Bitwarden's six URI match rules as one function: a saved website, its match type, and the page URL in; a yes or no out.
- `browser_url` turns address-bar text into a `url::Url`. It decides the browser family from the window class, says which UI Automation properties find the address field, and decides whether a found field is the browser's own or a web page's.
- `browser_url::read` does the observation. It looks up the window class, then calls `injector::ui_automation::address_field_text`.
- `app::handle_no_match` calls the read once per press, and only in a browser. It hands the page to `app_candidates::candidates`, which gains a new top rank, `RANK_URL`.

`match_engine` and `picker_prompt` are not changed.

**Tech Stack:** Rust 2021, the `windows` 0.58 UI Automation bindings (already a dependency), `url` 2 (already in the tree, made direct), `psl` =2.1.238 (new), and `regex` 1 (already in the lock; it becomes a runtime dependency).

**Spec:** `docs/superpowers/specs/2026-10-01-browser-url-matching-design.md`. Where the code disagrees with the spec, see "Decisions this plan takes" below.

## Global Constraints

These apply to every task and every step. A task's requirements include this section.

- **Repository and branch.** `E:\Personal\node-bitwarden\deskwarden`, a Rust/egui 0.35 Windows Bitwarden client. Work on `main`, starting at HEAD `3fc985e`. Run every command below from `E:\Personal\node-bitwarden\deskwarden` in Git Bash, unless a step says otherwise.
- **Finding each edit.** Line numbers in this plan are as of `3fc985e`. They shift as earlier tasks land.
  - Every replacement quotes its exact old text, and that text occurs exactly once in its file. Find each edit by that text, not by line number.
  - The Edit tool's `old_string` is the quoted block with the code-fence lines removed.
- **CRLF line endings.**
  - Every `.rs` file in `deskwarden/` is CRLF (`.gitattributes`: `text eol=crlf`).
  - Edit `.rs` files only with the Edit tool, or with Python scripts written through the Write tool that read and write bytes and keep `\r\n`.
  - **Never** use bash heredocs or `sed -i` on a `.rs` file.
  - A new `.rs` file is written with the Write tool and then normalised with `--fix` (below).
  - After every step that touches `.rs` files, the checker must print `0` for each file touched.
  - Save this checker once, as `$TEMP/dw_crlf.py`. Use the Write tool, because it is a Python file and not a `.rs`:
    ```python
    # dw_crlf.py -- report lone LF bytes per file; with --fix, normalise to CRLF first.
    import sys
    fix = "--fix" in sys.argv
    for path in [a for a in sys.argv[1:] if a != "--fix"]:
        data = open(path, "rb").read()
        if fix:
            data = data.replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
            open(path, "wb").write(data)
        print(path, data.count(b"\n") - data.count(b"\r\n"))
    ```
    Usage: `python "$TEMP/dw_crlf.py" src/app.rs` checks. `python "$TEMP/dw_crlf.py" --fix src/uri_match.rs` normalises a new file.
  - `Cargo.toml` is **LF**, not CRLF (`git ls-files --eol` reports `w/lf`). Edit it with the Edit tool and keep it LF.
- **Source pins are heavy. When one goes red, read its message.** Then either fix the code, or update the pin while keeping its intent. Never delete a pin, and never widen one past what the change needs. The pins this plan moves on purpose:
  - **The manifest pin.** `job_object::tests::the_two_job_bearing_modules_can_start_a_child_only_through_this_one` pins `Cargo.toml` as `(length, FNV-1a/64)` with CRLF normalised to LF. It moves in Task 1. Recompute the new pair with the separate Python implementation in Task 1, Step 4. First check that the script reproduces the old pair, `(28452, 0x278a_e1ce_2972_4f1c)`. **Never copy the new pair out of the failure message.** Write a ledger hop comment in the style of the hops above it.
  - **`OPENS_NO_WINDOW`** in `src/foreground.rs` (`foreground::tests::every_module_in_this_crate_is_classified_as_opening_windows_or_not`). Each new `lib.rs` module gets an entry with a comment, and the array length in the type goes up by one. It goes 87 → 88 in Task 1 and 88 → 89 in Task 2.
  - **The Rust files outside `src/`.** The `root_names` list is in the same `job_object` test as the manifest pin. Task 4 adds `examples/address_bar_probe.rs` to it.
  - **Pins this plan must keep green without editing them:**
    - the thread census (no new `std::thread::spawn`);
    - the frame census;
    - `debug_leak_guard`: no new type carries `Zeroizing`;
    - the README badge, `job_object::tests::the_readme_test_count_is_not_a_boast`: 5900+, with `claimed <= actual < claimed + 200`. There were 6022 tests at `3fc985e`. This plan adds 40, which gives 6062, so the badge does not move. Task 4, Step 12 checks this.
    - the browser-list single-source pins: `app::disposition_tests::the_browser_list_is_the_only_place_a_browser_is_named` and `match_engine::tests::the_browser_rule_asks_app_rather_than_keeping_a_second_list`. **No browser executable name is written in `app.rs`'s production half, `main.rs`, or `match_engine.rs`.** `browser_url` tells browsers apart by window class for exactly this reason.
    - `injector::ui_automation::bstr_wipe_tests::both_credential_setters_go_through_the_wiping_helper`, which counts `.SetValue(` in that file's production half. It must stay at 2.
    - `app::picker_wiring_tests::the_vault_snapshot_does_not_outlive_the_scan_that_needs_it` and `the_empty_card_is_told_the_window_label_and_not_the_exe_name`, which read `handle_no_match`'s body.
    - `main`'s `the_remembered_record_is_offered_ahead_of_the_account_picker`. This is the pin that keeps the five-minute recall ahead of the picker.
- **Verify per task.** The last step of every task runs all four commands. All four must pass before the commit:
  1. `cargo test --lib -j 2`
  2. `cargo test --bins -j 2`. `--lib` never runs `main.rs`'s tests.
  3. `cargo check --all-targets`, with **0 warnings**.
  4. `RUSTFLAGS="-D warnings" cargo test --no-run`.
  - Use a private target directory, because the shared one can serve stale test binaries: `export CARGO_TARGET_DIR="$TEMP/dw-urlmatch-target"` in the shell you run cargo from. A cold `cargo check` takes about two minutes. A cold test build takes several, and the lib suite runs for about two minutes once it is built.
- **Commits.**
  - One commit per task, on `main`, naming the paths explicitly (`git add <paths>`). Never use `git add -A`.
  - The subject is one plain sentence in the repo's style, for example "The account picker puts the login saved for the page on top". The body says what changed and why. For Task 1 the body also names the dependency that moved and where it comes from, as the manifest pin's message asks.
  - The message ends with:
    ```
    Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
    ```
  - **Don't push. No release build. No version change, no CHANGELOG change.**
- **Never log the URL text.** That covers the address-bar text, the parsed page URL, and any part of them (host, path). No `log::` line in `browser_url`, `uri_match` or `handle_no_match` may carry them. Tests in Tasks 1, 2 and 4 hold this.
- **House rules the crate enforces by test.**
  - No `#[cfg(not(test))]` and no `cfg!`.
  - Below a file's first `#[cfg(test)]` there are only `#[cfg(test)] mod … { }` blocks.
  - Every Cargo feature `cfg` is the additive `#[cfg(any(test, feature = "test-support"))]`.
  - Comments follow the codebase's dense, explanatory voice. They say *why*, name the alternative that was rejected, and point at the pinning test.

## Decisions this plan takes (where the spec is silent, contradicts itself, or meets the code)

Each one is implemented as stated, and each is repeated in the final report.

1. **The picker's "pre-selection" is today's first-row focus. `picker_prompt` is unchanged.**
   - `picker_prompt::win32::apply_mode` already focuses the first row whenever the card shows candidates. It calls `SetFocus(ID_ROW)`, and `paint_control` draws the focused row as selected.
   - So a URL-ranked login sorted first *is* pre-selected. Enter chooses it. The second step then focuses *Username + Tab + Password*, and a second Enter fills.
   - The spec's Testing line, "pre-selects a URL-ranked first row **and not otherwise**", would mean taking that focus away from every card without a URL match. That includes every native app's card, and it would be a keyboard regression. **This plan keeps today's focus for every card.**
   - "Enter fills" is two Enters (account, then field), as it is today.
   - Owner to confirm.
2. **`regex` was not a runtime dependency.** The spec says it is "already a dependency". It was `optional = true`, turned on only by `test-support`, and so not linked into the shipped binary.
   - Task 1 makes it a plain dependency. `test-support` becomes `[]`, and the redundant `[dev-dependencies]` copy of `regex` is removed.
   - `Cargo.lock` already holds `regex 1.13.1` and its three dependencies, so no crate is added. The shipped binary now links them.
3. **`url` was only transitive.** It comes in at 2.5.8 through `ureq` and `webbrowser`. Task 1 names it directly as `url = "2"`. No crate is added.
4. **`psl` is pinned exactly, at `=2.1.238`.**
   - Verified on crates.io with `cargo search psl` and `cargo info psl` on 2026-10-01. The licence is `MIT/Apache-2.0`, and the crate is `no_std` with `forbid(unsafe_code)`.
   - It brings one more crate, `psl-types 2.0.11`, which is also `MIT/Apache-2.0`. `cargo deny check licenses` accepts both.
   - The list is data that decides which login is pre-picked, so it is pinned rather than left to `cargo update`.
   - Its compiled list does include the private section (`github.io`, `blogspot.com`), matching Bitwarden's `allowPrivateDomains: true`.
5. **Base domain for IP addresses and `localhost` compares host *and port*, as the spec says.**
   - Bitwarden's own `Utils.getDomain` returns only the hostname for those, so Bitwarden ignores the port there. The spec's "(and port, as Bitwarden does)" does not match Bitwarden's code.
   - The plan follows the spec. That is the stricter direction: it can cost a missed pre-pick, never a wrong one. Owner to confirm.
   - Ports are compared as the URL spells them, so a scheme's default port is "no port". That makes `https://x` and `https://x:443` one place, which is how JavaScript's `URL.host` behaves.
6. **Two rules follow Bitwarden where the spec is silent.**
   - The regular expression is compiled **case-insensitively**, as Bitwarden's `new RegExp(uri, "i")` is.
   - The host rule ignores the scheme, as Bitwarden's `getHost` does.
   - JavaScript-only regex syntax, such as look-behind or backreferences, does not compile in the `regex` crate, so it matches nothing.
7. **A `match` value this build cannot name is `Never`.** The spec is silent. A rule written by a later Bitwarden must not pre-pick on a guess.
   - The six wire numbers are read through the edit form's existing `vault_window::detail_edit::UriMatchChoice::from_wire`, so they are still spelled in one place.
8. **The browser family is decided by the top-level window class, not by the executable.**
   - Chromium browsers use `Chrome_WidgetWin_*` and Gecko browsers use `MozillaWindowClass`.
   - A second table of executable names would break the single-source browser-list pins in `app.rs` and `match_engine.rs`.
   - The caller has already asked `app::browser_window` whether the window is a browser at all.
9. **UI Automation properties, measured on this machine on 2026-10-01**, against Chrome, Edge, Brave and Firefox (scratch probe, not committed):
   - **Chromium.** An **Edit** control.
     - The key is its `ClassName`: `OmniboxViewViews` in Chrome and Edge, `BraveOmniboxViewViews` in Brave.
     - The fallback is the English `Name` "Address and search bar", which all three use.
     - **`AutomationId` is not used.** It was `view_1012` in Chrome and Brave and `view_1017` in Edge on the same day: it is a views counter, not an id. The `view_1020` in the brief is a value of that same kind.
   - **Firefox.** `AutomationId` `urlbar-input`. **The control type is ComboBox, not Edit**, so the spec's "the URL bar's Edit control" would have found nothing. The plan accepts ComboBox and Edit.
   - Opera, Vivaldi, Thorium, LibreWolf, Floorp, Waterfox and Zen were not installed, so they were not measured. Where the keys miss, the picker ranks as it does today.
   - Internet Explorer's `IEFrame` class has no family, so it is never read.
10. **A web page must not be able to pose as the address bar.** This is a security gap in the spec.
    - UI Automation puts a page's own elements in the same tree. A page's `id` becomes the `AutomationId`, and its `aria-label` becomes the `Name`.
    - In Firefox the page comes *before* the toolbar in tree order. A planted `<input id="urlbar-input" value="https://bank.example">` would therefore be found first. The bank's login would be pre-picked on the attacker's page, and two Enters would type it.
    - The spec's "a spoofed reading costs a key press, never a leak" does not hold without a guard.
    - **The guard:** `address_field_text` uses `FindAll`, not `FindFirst`, and accepts a match only if `browser_url::is_browser_chrome` passes. That requires no `Document` ancestor, and at most 10 steps up to the window.
    - Measured: Firefox's URL bar is 4 steps from the window, and the Chromium omnibox is 8 in Chrome, Edge and Brave. Every page field was 13 or more steps from the window, and all but one sat inside a `Document`.
11. **The address bar shows an elided URL.**
    - Measured: Chrome, Brave and Firefox showed no `://`. Edge showed the scheme.
    - `parse` puts `https://` in front, as the spec says. On an `http://` page, an Exact, Starts-with or Regular-expression rule written with `http://` will not match.
    - Base domain (the default) and Host do not look at the scheme, so they are unaffected.
    - Chrome may also hide `www.`. That was not measured, and it has the same effect on the same three rules.
12. **The executable-name rank is dropped for every browser window, whether or not an address was read.** The spec says "in a browser" the stem rank is not applied.
    - "No URL = today's behaviour exactly" therefore holds for native apps, which never have a URL.
    - A browser whose address could not be read differs from today only by the dropped stem rank (`chrome` against item names and domains).
    - Owner to confirm.
13. **`parse` checks "has a dot or is localhost" on the parsed host, not on the raw text.**
    - This makes `intranet/login` a search, and lets an IPv6 literal (`[::1]:8080`), which has no dot, be an address.
    - Text over 8192 bytes is not parsed.
14. **UI Automation timeouts.** The read sets connection 500 ms and transaction 1000 ms (the defaults are 2 s and 20 s). A hung browser therefore delays the card by about a second at most. Measured: the whole query took 50–95 ms.
15. **`examples/address_bar_probe.rs`** is added as the hand check the spec asks for: "checked by hand in Chrome, Edge and Firefox". It prints to the console of the person running it. It is not a log.
16. **`ItemFacts.uris` changes type**, from `Vec<String>` to `Vec<uri_match::SavedUri>`.
    - `vault_cache` serialises `ItemFacts` into the encrypted disk cache's "facts" section, so that JSON shape changes.
    - Nothing in the crate reads the facts section back yet, so no migration is needed.
17. **`detail_edit.rs`'s doc claim "nothing in Deskwarden reads `match`" becomes false.** Task 3 corrects that paragraph.

## File Structure

| File | New / changed | Responsibility |
|---|---|---|
| `deskwarden/Cargo.toml` | changed (T1) | `url`, `psl` added; `regex` made non-optional; `test-support = []`; dev-dependency `regex` removed |
| `deskwarden/Cargo.lock` | changed (T1, by cargo) | `psl`, `psl-types` entries |
| `deskwarden/src/uri_match.rs` | **new** (T1) | `MatchType`, `SavedUri`, `matches`: Bitwarden's six rules, pure |
| `deskwarden/src/browser_url.rs` | **new** (T2), extended (T4) | `Family`, `FieldKind`, `AddressField`, `family_of_class`, `is_browser_chrome`, `parse` (pure, T2); `read` (observation, T4) |
| `deskwarden/src/lib.rs` | changed (T1, T2) | `pub mod uri_match;`, `pub mod browser_url;` |
| `deskwarden/src/foreground.rs` | changed (T1, T2) | `OPENS_NO_WINDOW` entries |
| `deskwarden/src/job_object.rs` | changed (T1, T4) | manifest re-pin and its hop; the new example in `root_names` |
| `deskwarden/src/app.rs` | changed (T3, T4) | `ItemFacts.uris` carries match types; `page_address`/`page_address_with`; `picker_offers_for` takes the page; `handle_no_match` reads it; wiring tests |
| `deskwarden/src/app_candidates.rs` | changed (T3) | `candidates(.., page)`, `RANK_URL`, no stem rank in a browser |
| `deskwarden/src/vault_window/detail_edit.rs` | changed (T3) | doc paragraph about who reads `match` |
| `deskwarden/src/injector/ui_automation.rs` | changed (T4) | `address_field_text` and its `ancestry` walk |
| `deskwarden/src/injector/target.rs` | changed (T4) | `class_name_of` becomes `pub(crate)` |
| `deskwarden/examples/address_bar_probe.rs` | **new** (T4) | the hand check |

---

### Task 1: The public-suffix list, and Bitwarden's URI match rules as a pure function

**Files:**
- Modify: `deskwarden/Cargo.toml`: the `regex` entry at the end of `[dependencies]` (lines 445-452), the `[features]` line `test-support = ["dep:regex"]` (line 466), and the `regex` entry at the end of `[dev-dependencies]` (lines 481-488).
- Modify: `deskwarden/Cargo.lock`. Cargo does this; do not hand-edit it.
- Modify: `deskwarden/src/job_object.rs`: the manifest pin pair at line 3025.
- Create: `deskwarden/src/uri_match.rs`
- Modify: `deskwarden/src/lib.rs`: after `pub mod ui_process;` (line 211).
- Modify: `deskwarden/src/foreground.rs`: `OPENS_NO_WINDOW` (line 1423) and its `"ui_show",` entry (line 1687).

**Interfaces:**
- Consumes:
  - `crate::vault_window::detail_edit::UriMatchChoice::from_wire(Option<&serde_json::Value>) -> UriMatchChoice` (existing);
  - `crate::vault_bridge::UriEntry { uri: Option<String>, other: serde_json::Map<String, serde_json::Value> }` (existing).
- Produces (in `crate::uri_match`):
  - `pub enum MatchType { BaseDomain, Host, StartsWith, Exact, RegularExpression, Never }`. It derives `Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize`.
  - `impl MatchType { pub fn from_wire(value: Option<&serde_json::Value>) -> Option<MatchType> }`
  - `pub struct SavedUri { pub uri: String, pub match_type: Option<MatchType> }`. It derives `Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize`.
  - `impl SavedUri { pub fn of(entry: &crate::vault_bridge::UriEntry) -> Option<SavedUri> }`
  - `pub fn matches(saved: &str, match_type: Option<MatchType>, page: &url::Url) -> bool`

- [ ] **Step 1: Add the three dependencies to `Cargo.toml`**

With the Edit tool, replace this block at the end of `[dependencies]`:

```toml
# OPTIONAL, AND OFF IN EVERY BUILD THAT SHIPS. The engine behind
# `test_http::Matcher::Regex`, which five call sites use. It is named here as
# well as in `[dev-dependencies]` because `test_http` has to compile in two
# configurations: the library's own test target, where the dev-dependency
# supplies it, and the library built for `main.rs`'s test target, where
# `test-support` does. `cargo build` turns on neither, so `regex` is not
# linked into the shipped binary.
regex = { version = "1", optional = true }
```

with:

```toml
# The address bar's text as a URL (src/browser_url.rs), and a login's saved
# websites parsed into the same type (src/uri_match.rs), so the page and the
# saved website are compared as hosts and ports rather than as strings.
# ALREADY in this tree at this version, through `ureq` and `webbrowser`:
# naming it makes it direct rather than transitive and adds no crate.
url = "2"
# Mozilla's public-suffix list, compiled in (src/uri_match.rs). Bitwarden's
# default URI match rule, "base domain", compares REGISTRABLE domains, so
# `accounts.google.com` matches a login saved for `google.com` while
# `bbc.co.uk` does not match `other.co.uk`. That cannot be answered without
# the list. `no_std`, `#![forbid(unsafe_code)]`, no build script and no I/O:
# the list is generated Rust source. It adds two crates, `psl` and its
# `psl-types`, both `MIT/Apache-2.0`, already on `deny.toml`'s allow-list.
#
# **Pinned to one exact version**, unlike every other entry here. The list is
# DATA that changes with almost every release, and which login the account
# picker pre-picks is decided by it; a `cargo update` that quietly moved that
# decision is a change somebody should make on purpose, with this line.
psl = "=2.1.238"
# Bitwarden's "regular expression" URI match rule (src/uri_match.rs), and the
# engine behind `test_http::Matcher::Regex`.
#
# **This used to be OPTIONAL and off in every build that ships**, named only
# so `test_http` could compile under `test-support`. The URI matcher made it
# a runtime dependency, so it is now linked into the shipped binary. Same
# crate and same version: `Cargo.lock` held `regex 1.13.1` before this line
# changed and holds it after. A saved pattern is compiled with a size limit,
# and one that does not compile matches nothing -- see `uri_match`.
regex = "1"
```

Replace the feature line `test-support = ["dep:regex"]` with:

```toml
test-support = []
```

In `[dev-dependencies]`, replace:

```toml
deskwarden = { path = ".", features = ["test-support"] }
# The same crate as the optional `regex` above, named again here because the
# library's OWN test target compiles `test_http` under `cfg(test)` with no
# feature turned on. `mockito` -- which brought `regex` and its three
# dependencies into the tree in the first place, at this version, for exactly
# the same matcher -- is GONE from this manifest: `main.rs` was its last user
# and `test_http::no_module_in_this_crate_builds_a_mockito_server` is what
# keeps it gone.
regex = "1"
```

with:

```toml
deskwarden = { path = ".", features = ["test-support"] }
```

Check that the file is still LF:

Run: `python -c "b=open('Cargo.toml','rb').read(); print(b.count(b'\r\n'))"`
Expected: `0`

- [ ] **Step 2: Resolve the lock and see the manifest pin go red**

Run: `cargo check --all-targets`
Expected: `Locking 2 packages`, `Adding psl v2.1.238`, `Adding psl-types v2.0.11`, then `Finished` with 0 warnings.

Run: `cargo test --lib -j 2 the_two_job_bearing_modules_can_start_a_child_only_through_this_one`
Expected: FAIL. The panic says "`Cargo.toml` is not the file this module pinned" and shows the old pair `(28452, ...)` against a new one. **Do not copy the new numbers from this message.**

- [ ] **Step 3: Check the licences**

Run: `cargo deny check licenses`
Expected: `licenses ok`. The two new crates are `MIT/Apache-2.0`, and `deny.toml` is unchanged. If `cargo-deny` is not installed, skip this step and record that in the commit body. CI runs it.

- [ ] **Step 4: Recompute the pair independently, after reproducing the old one**

First, reproduce the PREVIOUS pinned pair from the committed manifest:

Run:
```bash
git show HEAD:deskwarden/Cargo.toml > "$TEMP/Cargo.prev.toml"
python - "$TEMP/Cargo.prev.toml" Cargo.toml <<'EOF'
import sys
def fnv1a64(b):
    h = 0xcbf29ce484222325
    for ch in b:
        h ^= ch
        h = (h * 0x100000001b3) & 0xFFFFFFFFFFFFFFFF
    return h
for path in sys.argv[1:]:
    raw = open(path, 'rb').read().replace(b'\r\n', b'\n')
    print(path, len(raw), hex(fnv1a64(raw)))
EOF
```
Expected: two lines.
- The first, for the previous manifest, must read `28452 0x278ae1ce29724f1c`. That is the pair currently pinned, so the script measures what the test measures. If it does not match, stop and find out why.
- The second, for the edited manifest, should read `29248 0x52ff8223157c4464`. If it differs, the Step 1 edit differs from this plan's text. Compare the edit against Step 1 before pinning anything.

(This heredoc feeds Python on stdin. It writes no `.rs` file.)

- [ ] **Step 5: Re-pin, with a hop in the ledger**

In `src/job_object.rs`, replace the pinned pair line:

```rust
            (28452, 0x278a_e1ce_2972_4f1c_u64),
```

with the hop comment and the new pair:

```rust
            // **29248 bytes, new hash: TWO DEPENDENCIES ADDED AND ONE MADE
            // NON-OPTIONAL, and this is the hop this pin exists for.** All three
            // from crates.io, none with a path, a git source or a `[patch]`, for
            // browser URL matching (`src/browser_url.rs`, `src/uri_match.rs`):
            //
            //  * `url = "2"` -- ADDED. Already in the tree at 2.5.8 through
            //    `ureq` and `webbrowser`, so naming it adds no crate.
            //  * `psl = "=2.1.238"` -- ADDED, pinned to one exact version because
            //    the public-suffix list is data, and it decides which login the
            //    account picker puts on top. It brings `psl` and `psl-types`, both
            //    `MIT/Apache-2.0`; `cargo deny check licenses` passes with no
            //    change to `deny.toml`.
            //  * `regex` -- was `{ version = "1", optional = true }`, turned on
            //    only by `test-support`; now a plain `"1"`, because
            //    `uri_match`'s regular-expression rule runs in the shipped
            //    binary. Same crate, still 1.13.1 in `Cargo.lock`.
            //    `test-support` is now `[]`, and the `[dev-dependencies]` copy of
            //    `regex` is REMOVED: a normal dependency already reaches every
            //    test target.
            //
            // Nothing else moved: no existing dependency was re-pointed or
            // re-featured, no `[patch]`/`[replace]`/`[workspace.dependencies]`
            // table appeared, no path or fork appeared, and `[build-dependencies]`
            // still reads exactly `winresource = "0.1"`. 28452 -> 29248 bytes: the
            // three entries and the comments above them, less the removed
            // dev-dependency and its comment.
            //
            // Recomputed the way every hop above records -- FNV-1a/64 over the file
            // with CRLF normalised to LF, in a separate implementation outside this
            // crate -- rather than copied out of the failure message. That
            // implementation was first run against the PREVIOUS pinned pair and
            // reproduced (28452, 0x278a_e1ce_2972_4f1c) exactly, so it is measuring
            // what this test measures.
            (29248, 0x52ff_8223_157c_4464_u64),
```

Run: `python "$TEMP/dw_crlf.py" src/job_object.rs`
Expected: `src/job_object.rs 0`

Run: `cargo test --lib -j 2 the_two_job_bearing_modules_can_start_a_child_only_through_this_one`
Expected: PASS.

Run: `cargo test --lib -j 2 nothing_in_this_crate_is_compiled_differently_when_it_is_tested`
Expected: PASS. The declared features are still exactly `["test-support"]`, and none is default.

- [ ] **Step 6: Write the failing tests**

Create `src/uri_match.rs` with the Write tool, containing **only** this test module for now:

```rust
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
    fn base_domain_compares_ip_addresses_by_address_and_port() {
        assert!(base("192.168.1.10", "https://192.168.1.10/login"));
        assert!(!base("192.168.1.10", "https://192.168.1.11/"));
        assert!(
            !base("10.0.1.10", "https://192.168.1.10/"),
            "the suffix list reads both of these as `1.10`; an address is not a domain"
        );
        assert!(base("192.168.1.10:8443", "https://192.168.1.10:8443/"));
        assert!(!base("192.168.1.10:8443", "https://192.168.1.10:9443/"));
        assert!(base("https://[::1]:8080", "http://[::1]:8080/"));
    }

    #[test]
    fn base_domain_compares_localhost_by_port() {
        assert!(base("http://localhost:3000", "http://localhost:3000/login"));
        assert!(!base("http://localhost:3000", "http://localhost:8080/"));
        assert!(base("localhost", "https://localhost/"));
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
```

Then normalise it: `python "$TEMP/dw_crlf.py" --fix src/uri_match.rs`. Expected: `src/uri_match.rs 0`.

In `src/lib.rs`, replace:

```rust
pub mod ui_process;
/// One account's master key
```

with:

```rust
pub mod ui_process;
/// Bitwarden's URI match rules -- base domain, host, starts with, exact,
/// regular expression, never -- as one pure function over a saved website and
/// the page in a browser's address bar. It only ever orders the account picker.
pub mod uri_match;
/// One account's master key
```

- [ ] **Step 7: Run the tests to verify they fail**

Run: `cargo test --lib -j 2 uri_match`
Expected: compile errors, among them `cannot find function 'matches' in this scope`, `use of undeclared type 'Url'` and `use of undeclared type 'MatchType'`. Nothing above the test module exists yet.

- [ ] **Step 8: Write the matcher**

Insert this production half at the top of `src/uri_match.rs`, above the `#[cfg(test)]` line:

```rust
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
//!   addresses have no registrable domain, so they compare by host and port.
//!   Bitwarden asks the list with private suffixes ON (`allowPrivateDomains`),
//!   which is what keeps `alice.github.io` and `bob.github.io` apart, and the
//!   `psl` crate's compiled list carries that section.
//! * **Host** (`1`): host and port equal. The scheme is not compared, exactly
//!   as Bitwarden's `getHost` does not: `http://example.com` saved matches
//!   `https://example.com` in the address bar.
//! * **Starts with** (`2`) and **Exact** (`3`): the page URL as a string,
//!   against the saved string as typed. Case-sensitive, as Bitwarden is.
//! * **Regular expression** (`4`): the saved pattern, case-insensitive (the
//!   `i` flag Bitwarden compiles it with), against the page URL. A pattern this
//!   crate's `regex` cannot compile -- including JavaScript-only syntax such as
//!   look-behind -- matches nothing.
//! * **Never** (`5`): nothing.
//!
//! A saved website without `://` is read as `http://` for the two host rules,
//! which is Bitwarden's `getUrl` rule.
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
            (Some(saved), Some(page)) => base_domain(&saved) == Some(page),
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

/// A saved website as a URL, reading one without `://` as `http://`.
fn saved_url(saved: &str) -> Option<Url> {
    let saved = saved.trim();
    if saved.contains("://") {
        Url::parse(saved).ok()
    } else {
        Url::parse(&format!("http://{saved}")).ok()
    }
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
/// and an IP address, which have none -- the host and port.
///
/// **The IP arm is not an optimisation.** The public-suffix list knows nothing
/// about addresses: asked about `192.168.1.10` it answers `1.10`, so every
/// address ending in `.1.10` would be one "domain".
fn base_domain(url: &Url) -> Option<(String, Option<u16>)> {
    match url.host()? {
        Host::Ipv4(_) | Host::Ipv6(_) => host_and_port(url),
        Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.');
            if domain.eq_ignore_ascii_case("localhost") {
                return host_and_port(url);
            }
            psl::domain_str(domain).map(|registrable| (registrable.to_ascii_lowercase(), None))
        }
    }
}
```

Run: `python "$TEMP/dw_crlf.py" --fix src/uri_match.rs`
Expected: `src/uri_match.rs 0`

- [ ] **Step 9: Run the tests to verify they pass**

Run: `cargo test --lib -j 2 uri_match`
Expected: `test result: ok. 19 passed`.

- [ ] **Step 10: Classify the new module, after seeing the census go red**

Run: `cargo test --lib -j 2 every_module_in_this_crate_is_classified_as_opening_windows_or_not`
Expected: FAIL. "`uri_match` is in neither list (or in both)".

In `src/foreground.rs`, replace:

```rust
        const OPENS_NO_WINDOW: [&str; 87] = [
```

with:

```rust
        const OPENS_NO_WINDOW: [&str; 88] = [
```

and replace:

```rust
            "ui_show",
```

with:

```rust
            "ui_show",
            // Bitwarden's six URI match rules as one pure function: a saved
            // website and the page in the address bar in, a yes or no out.
            // No I/O and no window -- the card whose row order it decides is
            // `picker_prompt`'s.
            "uri_match",
```

Run: `python "$TEMP/dw_crlf.py" src/foreground.rs src/lib.rs`
Expected: both `0`.

Run: `cargo test --lib -j 2 every_module_in_this_crate_is_classified_as_opening_windows_or_not`
Expected: PASS.

- [ ] **Step 11: Verify the whole task**

Run:
```bash
cargo test --lib -j 2
cargo test --bins -j 2
cargo check --all-targets
RUSTFLAGS="-D warnings" cargo test --no-run
```
Expected:
- both test runs report `test result: ok.` with 0 failed;
- `check` prints no `warning`;
- the `-D warnings` build finishes.

- [ ] **Step 12: Commit**

```bash
git add Cargo.toml Cargo.lock src/job_object.rs src/uri_match.rs src/lib.rs src/foreground.rs
git commit -m "A login's saved websites can be matched against a page by Bitwarden's own rules

uri_match is the six URI match rules -- base domain (by the public-suffix
list), host, starts with, exact, regular expression, never -- as one pure
function over a saved website and a page URL. Nothing calls it yet.

Dependencies moved, all from crates.io: url = \"2\" is now direct (already
in the tree via ureq and webbrowser); psl = \"=2.1.238\" is added, bringing
psl and psl-types (MIT/Apache-2.0); regex is no longer optional behind
test-support but a runtime dependency (same 1.13.1), and its redundant
dev-dependency copy is removed. The Cargo.toml pin is re-pinned at
(29248, 0x52ff_8223_157c_4464), recomputed independently after reproducing
the previous pair.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The address-bar text as a URL, and how the browser's own field is told from a page's

**Files:**
- Create: `deskwarden/src/browser_url.rs`
- Modify: `deskwarden/src/lib.rs`: after `pub mod breach_scan;` (line 47).
- Modify: `deskwarden/src/foreground.rs`: `OPENS_NO_WINDOW` (length 88 → 89) and its `"breach_scan",` entry.

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces (in `crate::browser_url`; Task 4 consumes all of them):
  - `pub enum Family { Chromium, Firefox }`
  - `pub fn family_of_class(class_name: &str) -> Option<Family>`
  - `pub enum FieldKind { Edit, ComboBox }`
  - `pub struct AddressField { pub kinds: &'static [FieldKind], pub class_names: &'static [&'static str], pub automation_ids: &'static [&'static str], pub names: &'static [&'static str] }`. It derives `Clone, Copy, Debug, PartialEq, Eq`.
  - `impl Family { pub fn address_field(self) -> AddressField }`
  - `pub const MAX_FIELD_DEPTH: usize = 10;`
  - `pub fn is_browser_chrome(depth: Option<usize>, crosses_a_document: bool) -> bool`
  - `pub const MAX_ADDRESS_LEN: usize = 8192;`
  - `pub fn parse(text: &str) -> Option<url::Url>`

- [ ] **Step 1: Write the failing tests**

Create `src/browser_url.rs` with the Write tool, containing only this test module:

```rust
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
```

Run: `python "$TEMP/dw_crlf.py" --fix src/browser_url.rs`
Expected: `src/browser_url.rs 0`

In `src/lib.rs`, replace:

```rust
pub mod breach_scan;
```

with:

```rust
pub mod breach_scan;
/// The page a browser window is showing, read from its address bar through UI
/// Automation when the account picker is asked for -- and never logged.
pub mod browser_url;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib -j 2 browser_url`
Expected: a compile error, `cannot find function 'parse' in this scope` (and `family_of_class`, `Family`, `FieldKind`, `is_browser_chrome`, `MAX_ADDRESS_LEN`, `MAX_FIELD_DEPTH`).

- [ ] **Step 3: Write the pure half**

Insert this at the top of `src/browser_url.rs`, above `#[cfg(test)]`:

```rust
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
```

Run: `python "$TEMP/dw_crlf.py" --fix src/browser_url.rs`
Expected: `src/browser_url.rs 0`

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib -j 2 browser_url`
Expected: `test result: ok. 14 passed`.

- [ ] **Step 5: Classify the new module**

Run: `cargo test --lib -j 2 every_module_in_this_crate_is_classified_as_opening_windows_or_not`
Expected: FAIL. "`browser_url` is in neither list (or in both)".

In `src/foreground.rs`, replace:

```rust
        const OPENS_NO_WINDOW: [&str; 88] = [
```

with:

```rust
        const OPENS_NO_WINDOW: [&str; 89] = [
```

and replace:

```rust
            "breach_scan",
```

with:

```rust
            "breach_scan",
            // Reads the foreground browser's address field through UI
            // Automation and parses it. It asks another process's window a
            // question and draws nothing -- `injector`, where its UI
            // Automation half lives, is on this list for the same reason.
            "browser_url",
```

Run: `python "$TEMP/dw_crlf.py" src/foreground.rs src/lib.rs`
Expected: both `0`.

Run: `cargo test --lib -j 2 every_module_in_this_crate_is_classified_as_opening_windows_or_not`
Expected: PASS.

- [ ] **Step 6: Verify the whole task**

Run:
```bash
cargo test --lib -j 2
cargo test --bins -j 2
cargo check --all-targets
RUSTFLAGS="-D warnings" cargo test --no-run
```
Expected:
- both test runs pass;
- `check` prints no warning;
- the `-D warnings` build finishes.

- [ ] **Step 7: Commit**

```bash
git add src/browser_url.rs src/lib.rs src/foreground.rs
git commit -m "Address-bar text becomes a page URL, and only the browser's own field counts as the address bar

browser_url::parse turns what an address bar holds into a URL: https:// is
assumed, only http and https are kept, a search is not an address. The
family of a browser is told by its window class, never by a second list of
executable names, and each family's address field is described by the UI
Automation properties measured in Chrome, Edge, Brave and Firefox.
is_browser_chrome refuses a candidate inside a Document or more than ten
steps below the window, so a web page cannot pose as the address bar.
Nothing reads an address yet.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The picker ranks the logins saved for the page first, and drops the executable name in a browser

**Files:**
- Modify: `deskwarden/src/app.rs`:
  - the `ItemFacts.uris` field (lines 3084-3085);
  - `ItemFacts::of` (line 3123);
  - `picker_offers_for` (line 3308).
- Modify: `deskwarden/src/app_candidates.rs`: the whole file. Production half lines 1-100, test module lines 102-198.
- Modify: `deskwarden/src/vault_window/detail_edit.rs`: the doc paragraph at lines 376-381.

**Interfaces:**
- Consumes:
  - `crate::uri_match::{SavedUri, MatchType, matches}` (Task 1);
  - `crate::app::{browser_window, BrowserWindow}` (existing).
- Produces:
  - `crate::app::ItemFacts.uris: Vec<crate::uri_match::SavedUri>`, replacing `Vec<String>`;
  - `pub fn crate::app_candidates::candidates(exe_name: &str, title: &str, items: &[crate::app::ItemFacts], page: Option<&url::Url>) -> Vec<Candidate>`. Task 4 passes the page. This task passes `None`.

- [ ] **Step 1: Write the failing tests**

In `src/app_candidates.rs`, replace the whole test module, from `#[cfg(test)]` to the end of the file, with:

```rust
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
```

Run: `python "$TEMP/dw_crlf.py" src/app_candidates.rs`
Expected: `src/app_candidates.rs 0`

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib -j 2 app_candidates`
Expected: a compile error, `this function takes 3 arguments but 4 arguments were supplied` at the `candidates(` calls.

- [ ] **Step 3: Make `ItemFacts` carry each website's match type**

In `src/app.rs`, replace:

```rust
    /// Every URI, for `app_candidates`' host matching.
    pub uris: Vec<String>,
```

with:

```rust
    /// Every saved website, with the match rule Bitwarden saved beside it:
    /// `app_candidates` ranks on these -- by host against the executable's
    /// name, and by [`crate::uri_match::matches`] against a browser's
    /// address bar. A website is not a secret; it is what the card's icon
    /// is already keyed by.
    pub uris: Vec<crate::uri_match::SavedUri>,
```

and in `ItemFacts::of`, replace:

```rust
                .map(|l| l.uris.iter().filter_map(|u| u.uri.clone()).collect())
```

with:

```rust
                .map(|l| l.uris.iter().filter_map(crate::uri_match::SavedUri::of).collect())
```

- [ ] **Step 4: Rank by the page, and not by the executable in a browser**

Replace the production half of `src/app_candidates.rs`, everything above `#[cfg(test)]`, with:

```rust
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
```

In `src/app.rs`'s `picker_offers_for`, replace:

```rust
    let candidates = crate::app_candidates::candidates(&window.exe_name, &window.title, &facts);
```

with:

```rust
    let candidates =
        crate::app_candidates::candidates(&window.exe_name, &window.title, &facts, None);
```

Run: `python "$TEMP/dw_crlf.py" src/app_candidates.rs src/app.rs`
Expected: both `0`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib -j 2 app_candidates`
Expected: `test result: ok. 10 passed`.

Run: `cargo test --lib -j 2 picker_wiring_tests`
Expected: PASS. The existing wiring tests build `ItemFacts` through `facts_of`, which uses the new mapping.

- [ ] **Step 6: Correct the edit form's claim that nothing reads `match`**

In `src/vault_window/detail_edit.rs`, replace:

```rust
/// The sound half: **nothing in Deskwarden reads `match`.** That is still
/// true. The autofill path matches on the foreground *window*
/// (`match_engine`, [`AppMatchDraft`]); the icon loader
/// (`favicon::icon_domain_for`) and the read pane's AUTOFILL TARGETS card
/// both take `login.uris.first()` and nothing else. No expression in this
/// crate names `match`, and none is added by this pass.
```

with:

```rust
/// The sound half, when it was written: **nothing in Deskwarden read
/// `match`.** Browser URL matching changed that, and only for the account
/// picker: `uri_match::MatchType::from_wire` reads the value through
/// [`UriMatchChoice::from_wire`], so the six numbers are still spelled once,
/// and the picker uses the answer to choose which login it puts on top in a
/// browser. Nothing is filled by it: the autofill path still matches on the
/// foreground *window* (`match_engine`, [`AppMatchDraft`]), and the icon
/// loader (`favicon::icon_domain_for`) and the read pane's AUTOFILL TARGETS
/// card both still take `login.uris.first()` and nothing else.
```

Run: `python "$TEMP/dw_crlf.py" src/vault_window/detail_edit.rs`
Expected: `src/vault_window/detail_edit.rs 0`

- [ ] **Step 7: Verify the whole task**

Run:
```bash
cargo test --lib -j 2
cargo test --bins -j 2
cargo check --all-targets
RUSTFLAGS="-D warnings" cargo test --no-run
```
Expected:
- both test runs pass;
- `check` prints no warning;
- the `-D warnings` build finishes.

- [ ] **Step 8: Commit**

```bash
git add src/app.rs src/app_candidates.rs src/vault_window/detail_edit.rs
git commit -m "The account picker can rank the logins saved for a page above every guess

app_candidates gains RANK_URL above the URI-host rank: an item with any
saved website that uri_match accepts for the page sorts first. ItemFacts
carries each website with its Bitwarden match type to make that possible,
and no secret. In a browser the executable's name is no longer a signal:
\"chrome\" against item names and domains surfaced the same logins on every
site. No address is read yet, so the picker is handed none.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: `CTRL+ALT+B` reads the address bar, and the login saved for it is the first row

**Files:**
- Modify: `deskwarden/src/injector/ui_automation.rs`:
  - the `Accessibility` import (lines 6-10);
  - the new constants and functions after `is_real_rect` (line 288).
- Modify: `deskwarden/src/injector/target.rs`: `class_name_of` (lines 79-80).
- Modify: `deskwarden/src/browser_url.rs`: append `read` after `parse`.
- Modify: `deskwarden/src/app.rs`:
  - `picker_offers_for` (lines 3300-3310) and the new `page_address`/`page_address_with` after it;
  - `handle_no_match` (line 3442);
  - the new tests at the end of `mod picker_wiring_tests` (line 10733).
- Create: `deskwarden/examples/address_bar_probe.rs`
- Modify: `deskwarden/src/job_object.rs`: `root_names` (line 3398).

**Interfaces:**
- Consumes:
  - `crate::browser_url::{AddressField, FieldKind, MAX_FIELD_DEPTH, is_browser_chrome, family_of_class, parse}` (Task 2);
  - `crate::app_candidates::candidates(.., page)` (Task 3).
- Produces:
  - `pub fn crate::injector::ui_automation::address_field_text(hwnd: isize, field: crate::browser_url::AddressField) -> windows::core::Result<Option<String>>`
  - `pub(crate) fn crate::injector::target::class_name_of(hwnd: isize) -> Option<String>`
  - `pub fn crate::browser_url::read(hwnd: isize) -> Option<url::Url>`
  - `fn crate::app::page_address(window: &ForegroundEvent) -> Option<url::Url>`
  - `fn crate::app::page_address_with(window: &ForegroundEvent, read: fn(isize) -> Option<url::Url>) -> Option<url::Url>`
  - `fn crate::app::picker_offers_for(cache: &VaultCache, window: &ForegroundEvent, page: Option<&url::Url>) -> PickerCard`

- [ ] **Step 1: Write the failing wiring tests**

In `src/app.rs`, at the end of `mod picker_wiring_tests`, replace:

```rust
             looks at it. Take it inside `picker_offers_for`, which returns offers and no items \
             and therefore drops it before the card is raised"
        );
    }
}
```

with:

```rust
             looks at it. Take it inside `picker_offers_for`, which returns offers and no items \
             and therefore drops it before the card is raised"
        );
    }

    fn at_window(exe: &str, title: &str) -> crate::window_watch::ForegroundEvent {
        crate::window_watch::ForegroundEvent {
            hwnd: 0x5150,
            pid: 77,
            exe_name: exe.to_string(),
            title: title.to_string(),
        }
    }

    fn never_asked(_hwnd: isize) -> Option<url::Url> {
        panic!("a native app's window was asked for a browser address")
    }

    fn reads_example(hwnd: isize) -> Option<url::Url> {
        assert_eq!(hwnd, 0x5150, "the address was read from a different window");
        url::Url::parse("https://example.com/login").ok()
    }

    /// **Only a browser is asked for an address.** A native app's window is
    /// not read at all -- not read and ignored -- so the cost and the reach
    /// of the UI Automation query are confined to the windows it is for.
    #[test]
    fn only_a_browser_window_is_asked_for_its_address() {
        assert_eq!(page_address_with(&at_window("Slack.exe", "Slack"), never_asked), None);
        let read = page_address_with(&at_window(BROWSER_IMAGE_NAMES[0], ""), reads_example);
        assert_eq!(read.map(|url| url.to_string()), Some("https://example.com/login".to_string()));
    }

    /// **The wiring the hotkey runs, minus the window.** `picker_offers_for`
    /// is the function `handle_no_match` hands the address to; given one, the
    /// login saved for that website is the first offer -- the row the card
    /// focuses when it opens -- and without one the card is what it was.
    #[test]
    fn the_login_saved_for_the_page_is_the_first_offer() {
        let item = |id: &str, name: &str, uri: &str| -> VaultItem {
            serde_json::from_str(&format!(
                r#"{{"id":"{id}","name":"{name}","type":1,"login":{{"username":"ada","password":"pw","uris":[{{"uri":"{uri}","match":null}}]}}}}"#
            ))
            .expect("the fixture item is valid")
        };
        let cache = crate::test_vault::cache_with_items(vec![
            item("title", "Example portal", "https://portal.example.org"),
            item("url", "Work login", "https://example.com"),
        ]);
        let window = at_window(BROWSER_IMAGE_NAMES[0], "Example portal");
        let page = url::Url::parse("https://accounts.example.com/").unwrap();

        let ranked = picker_offers_for(&cache, &window, Some(&page));
        let order: Vec<&str> = ranked.offers.iter().map(|o| o.candidate.id.as_str()).collect();
        assert_eq!(
            order,
            vec!["url", "title"],
            "the login saved for the page in the address bar is not the first row, so Enter on \
             the card that just opened takes the title's guess instead"
        );

        let unranked = picker_offers_for(&cache, &window, None);
        let order: Vec<&str> = unranked.offers.iter().map(|o| o.candidate.id.as_str()).collect();
        assert_eq!(order, vec!["title"], "with no address the card must be today's card");
    }

    /// **Read once, on the picker's path, and never spelled.** The read is
    /// a cross-process query against the window the user is typing into, and
    /// its answer is the website they are on: one call site, in
    /// `handle_no_match`, and no log line that could carry it.
    #[test]
    fn the_address_is_read_once_on_the_pickers_path_and_never_logged() {
        let source = include_str!("app.rs");
        let boundary = source
            .find(concat!("mod ", "tests {"))
            .expect("app.rs's first test module is gone");
        let production = &source[..boundary];
        assert_eq!(
            production.matches(concat!("crate::browser_url::", "read")).count(),
            1,
            "the address bar is read from somewhere other than `page_address`"
        );
        assert_eq!(
            production.matches(concat!("page_address(", "window)")).count(),
            1,
            "`page_address` is called from somewhere other than `handle_no_match`"
        );

        let start = source
            .find(concat!("pub fn handle_no", "_match("))
            .expect("`handle_no_match` is in this file");
        let body: String = source[start..]
            .lines()
            .take_while(|line| !line.trim_end().eq("}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            body.contains(concat!("let page = page_address(", "window);")),
            "control: `handle_no_match` no longer reads the address, so the checks below are vacuous"
        );
        assert!(
            body.contains(concat!("picker_offers_for(cache, window, page.", "as_ref())")),
            "`handle_no_match` reads the address and does not rank by it"
        );
        for spelling in ["{page", "page:?", "page.as_str()", "page.to_string()", "page.host"] {
            assert!(
                !body.contains(spelling),
                "`handle_no_match` spells the address ({spelling}); it is the website the user \
                 is on and must not reach a log line"
            );
        }
        assert!(
            !include_str!("main.rs").contains(concat!("browser_", "url")),
            "main.rs reaches the address bar itself, around the one read this file pins"
        );
    }
}
```

Run: `python "$TEMP/dw_crlf.py" src/app.rs`
Expected: `src/app.rs 0`

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib -j 2 picker_wiring_tests`
Expected: compile errors: `cannot find function 'page_address_with' in this scope`, and `this function takes 2 arguments but 3 arguments were supplied` for `picker_offers_for`.

- [ ] **Step 3: The UI Automation read**

In `src/injector/ui_automation.rs`, replace the `Accessibility` import:

```rust
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationValuePattern, TreeScope_Descendants,
    UIA_ControlTypePropertyId, UIA_EditControlTypeId, UIA_IsPasswordPropertyId,
    UIA_ValuePatternId,
};
```

with:

```rust
use windows::Win32::UI::Accessibility::{
    CUIAutomation, CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationCondition,
    IUIAutomationElement, IUIAutomationTreeWalker, IUIAutomationValuePattern,
    TreeScope_Descendants, UIA_AutomationIdPropertyId, UIA_ClassNamePropertyId,
    UIA_ComboBoxControlTypeId, UIA_ControlTypePropertyId, UIA_DocumentControlTypeId,
    UIA_EditControlTypeId, UIA_IsPasswordPropertyId, UIA_NamePropertyId, UIA_ValuePatternId,
};
```

and replace:

```rust
fn is_real_rect(r: RECT) -> bool {
    r.right > r.left && r.bottom > r.top
}
```

with:

```rust
fn is_real_rect(r: RECT) -> bool {
    r.right > r.left && r.bottom > r.top
}

/// How long UI Automation may wait for a browser to accept the connection,
/// and to answer one request, before [`address_field_text`] gives up.
///
/// The defaults are 2 s and 20 s, and this runs between a key press and the
/// card it opens: a hung browser must cost the user a short pause and a
/// picker ranked as before, never a frozen hotkey. Measured on 2026-10-01 the
/// whole query took 50-95 ms in Chrome, Edge, Brave and Firefox.
const ADDRESS_FIELD_CONNECTION_TIMEOUT_MS: u32 = 500;
const ADDRESS_FIELD_TRANSACTION_TIMEOUT_MS: u32 = 1000;

/// **The text of a browser's address field**, found by the properties
/// `field` names, or `Ok(None)` when there is no such field that belongs to
/// the browser's own chrome.
///
/// The same apartment handling as every function above. Created through
/// `CUIAutomation8` rather than `CUIAutomation` because only that object
/// answers for `IUIAutomation2`, which is where the two timeouts are set.
///
/// **`FindAll` and not `FindFirst`, on purpose.** A web page's own elements
/// are in the same tree, with the page's `id` as their `AutomationId`, and in
/// Firefox the page comes BEFORE the toolbar in tree order -- so the first
/// match could be a page's `<input id="urlbar-input">`. Every match is held to
/// [`crate::browser_url::is_browser_chrome`] instead, and the first that
/// passes is read.
///
/// The text is returned to the caller and to nobody else: it is not logged
/// here, and `browser_url::read` does not log it either.
pub fn address_field_text(
    hwnd: isize,
    field: crate::browser_url::AddressField,
) -> Result<Option<String>> {
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            log::warn!("CoInitializeEx failed unexpectedly: {hr:?}");
        }

        let automation: IUIAutomation2 =
            CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)?;
        // A refusal leaves the defaults in place, which is slower and still
        // correct, so neither answer is worth an error.
        let _ = automation.SetConnectionTimeout(ADDRESS_FIELD_CONNECTION_TIMEOUT_MS);
        let _ = automation.SetTransactionTimeout(ADDRESS_FIELD_TRANSACTION_TIMEOUT_MS);

        let root = automation.ElementFromHandle(HWND(hwnd as *mut core::ffi::c_void))?;

        // (control type is one of `field.kinds`) AND (any listed property).
        let mut kind: Option<IUIAutomationCondition> = None;
        for wanted in field.kinds {
            let id = match wanted {
                crate::browser_url::FieldKind::Edit => UIA_EditControlTypeId,
                crate::browser_url::FieldKind::ComboBox => UIA_ComboBoxControlTypeId,
            };
            let one =
                automation.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(id.0))?;
            kind = Some(match kind {
                None => one,
                Some(earlier) => automation.CreateOrCondition(&earlier, &one)?,
            });
        }
        let mut key: Option<IUIAutomationCondition> = None;
        let wanted = field
            .class_names
            .iter()
            .map(|value| (UIA_ClassNamePropertyId, *value))
            .chain(field.automation_ids.iter().map(|value| (UIA_AutomationIdPropertyId, *value)))
            .chain(field.names.iter().map(|value| (UIA_NamePropertyId, *value)));
        for (property, value) in wanted {
            let one = automation.CreatePropertyCondition(property, &VARIANT::from(value))?;
            key = Some(match key {
                None => one,
                Some(earlier) => automation.CreateOrCondition(&earlier, &one)?,
            });
        }
        let (Some(kind), Some(key)) = (kind, key) else {
            return Ok(None);
        };
        let address = automation.CreateAndCondition(&kind, &key)?;

        let found = root.FindAll(TreeScope_Descendants, &address)?;
        let walker = automation.RawViewWalker()?;
        for index in 0..found.Length()? {
            let element = found.GetElement(index)?;
            let (depth, crosses_a_document) = ancestry(&automation, &walker, &root, &element);
            if !crate::browser_url::is_browser_chrome(depth, crosses_a_document) {
                continue;
            }
            let value: IUIAutomationValuePattern =
                element.GetCurrentPatternAs(UIA_ValuePatternId)?;
            let text = value.CurrentValue()?.to_string();
            return Ok((!text.is_empty()).then_some(text));
        }
        Ok(None)
    }
}

/// The walk from `element` up to `root`: how many steps it took (the root
/// included), or `None` if it did not arrive within
/// [`crate::browser_url::MAX_FIELD_DEPTH`]; and whether any ancestor on the
/// way was a `Document`, which is what a web page is exposed as.
///
/// A failed step is "did not arrive", which [`crate::browser_url::is_browser_chrome`]
/// refuses -- an element whose ancestry cannot be shown is not the address bar.
unsafe fn ancestry(
    automation: &IUIAutomation2,
    walker: &IUIAutomationTreeWalker,
    root: &IUIAutomationElement,
    element: &IUIAutomationElement,
) -> (Option<usize>, bool) {
    let mut crosses_a_document = false;
    let mut current = element.clone();
    for depth in 1..=crate::browser_url::MAX_FIELD_DEPTH {
        let Ok(parent) = walker.GetParentElement(&current) else {
            return (None, crosses_a_document);
        };
        if automation.CompareElements(&parent, root).is_ok_and(|same| same.as_bool()) {
            return (Some(depth), crosses_a_document);
        }
        if parent.CurrentControlType().is_ok_and(|kind| kind == UIA_DocumentControlTypeId) {
            crosses_a_document = true;
        }
        current = parent;
    }
    (None, crosses_a_document)
}
```

In `src/injector/target.rs`, replace:

```rust
/// The window class of `hwnd`, or `None` if Win32 declined to say.
fn class_name_of(hwnd: isize) -> Option<String> {
```

with:

```rust
/// The window class of `hwnd`, or `None` if Win32 declined to say.
///
/// `pub(crate)` for `browser_url::read`, which tells a browser's engine by
/// its top-level window class -- the same observation, asked once, here.
pub(crate) fn class_name_of(hwnd: isize) -> Option<String> {
```

In `src/browser_url.rs`, replace the end of `parse`:

```rust
    looks_like_a_site.then_some(url)
}
```

with:

```rust
    looks_like_a_site.then_some(url)
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
pub fn read(hwnd: isize) -> Option<url::Url> {
    let class = crate::injector::target::class_name_of(hwnd)?;
    let family = family_of_class(&class)?;
    let text = crate::injector::ui_automation::address_field_text(hwnd, family.address_field())
        .ok()
        .flatten()?;
    parse(&text)
}
```

Run: `python "$TEMP/dw_crlf.py" src/injector/ui_automation.rs src/injector/target.rs src/browser_url.rs`
Expected: all three `0`.

- [ ] **Step 4: Wire the read into `handle_no_match`**

In `src/app.rs`, replace `picker_offers_for` with the version that takes the page, followed by the two new functions:

```rust
fn picker_offers_for(
    cache: &VaultCache,
    window: &crate::window_watch::ForegroundEvent,
) -> PickerCard {
    // **`project`, not `items()`.** `items()` deep-clones the whole vault --
    // every password included -- and the picker needs an id, a name, a
    // username and some URIs. See `ItemFacts`.
    let facts = cache.project(ItemFacts::of);
    let candidates =
        crate::app_candidates::candidates(&window.exe_name, &window.title, &facts, None);
    PickerCard { offers: picker_offers(&candidates, &facts), corpus: picker_corpus(&facts) }
}
```

with:

```rust
fn picker_offers_for(
    cache: &VaultCache,
    window: &crate::window_watch::ForegroundEvent,
    page: Option<&url::Url>,
) -> PickerCard {
    // **`project`, not `items()`.** `items()` deep-clones the whole vault --
    // every password included -- and the picker needs an id, a name, a
    // username and some URIs. See `ItemFacts`.
    let facts = cache.project(ItemFacts::of);
    let candidates =
        crate::app_candidates::candidates(&window.exe_name, &window.title, &facts, page);
    PickerCard { offers: picker_offers(&candidates, &facts), corpus: picker_corpus(&facts) }
}

/// **The page the browser in front of the user is showing**, for the account
/// picker's ranking -- or `None`, which ranks exactly as before.
///
/// Read once per `CTRL+ALT+B` press, from [`handle_no_match`] and nowhere
/// else, and that placement is the whole of two promises:
///
/// * **Never on a focus change.** In a browser the automatic foreground path
///   never reaches the unmatched card: [`disposition`] answers
///   [`Open::Nothing`] for a browser on [`Trigger::Foreground`]
///   (`a_browser_gets_neither_unmatched_card_even_with_the_prompt_on`), so
///   only the hotkey's [`Trigger::Hotkey`] gets here.
/// * **The five-minute recall still wins.** `main` offers the remembered
///   record first and reaches this card only when that offer answers
///   `Offered::No` (`main`'s
///   `the_remembered_record_is_offered_ahead_of_the_account_picker`), so a
///   multi-page sign-in keeps its record and costs no read.
fn page_address(window: &crate::window_watch::ForegroundEvent) -> Option<url::Url> {
    page_address_with(window, crate::browser_url::read)
}

/// [`page_address`] with the reader passed in -- this crate's `fn`-pointer
/// seam, so a test can prove a native app's window is never asked at all.
fn page_address_with(
    window: &crate::window_watch::ForegroundEvent,
    read: fn(isize) -> Option<url::Url>,
) -> Option<url::Url> {
    match browser_window(&window.exe_name) {
        BrowserWindow::Yes => read(window.hwnd),
        BrowserWindow::No => None,
    }
}
```

In `handle_no_match`, replace:

```rust
    // this one doing exactly what it did before.
    let PickerCard { offers, corpus } = picker_offers_for(cache, window);
```

with:

```rust
    // this one doing exactly what it did before.
    //
    // **The address bar is read first, once, and only in a browser** -- see
    // `page_address`. It decides which row is on top and nothing else, and it
    // is never logged: it is the website the user is on.
    let page = page_address(window);
    let PickerCard { offers, corpus } = picker_offers_for(cache, window, page.as_ref());
```

Run: `python "$TEMP/dw_crlf.py" src/app.rs`
Expected: `src/app.rs 0`

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib -j 2 picker_wiring_tests`
Expected: PASS. That includes `only_a_browser_window_is_asked_for_its_address`, `the_login_saved_for_the_page_is_the_first_offer`, `the_address_is_read_once_on_the_pickers_path_and_never_logged`, `the_vault_snapshot_does_not_outlive_the_scan_that_needs_it` and `the_empty_card_is_told_the_window_label_and_not_the_exe_name`.

Run: `cargo test --lib -j 2 browser_url`
Expected: `test result: ok. 14 passed`. That includes `the_address_is_never_logged`, now that `read` is in the production half.

Run: `cargo test --lib -j 2 both_credential_setters_go_through_the_wiping_helper`
Expected: PASS. The new code calls `CurrentValue`, never `.SetValue(`.

Run: `cargo test --lib -j 2 the_browser_list_is_the_only_place_a_browser_is_named`
Expected: PASS. No browser executable name was written in `app.rs`'s production half.

- [ ] **Step 6: The recall still wins, and a browser still opens nothing on its own**

These pins already hold both promises. Run them so this task's evidence includes them:

Run: `cargo test --bins -j 2 the_remembered_record_is_offered_ahead_of_the_account_picker`
Expected: PASS. `main` offers the remembered record first, and reaches `handle_no_match`, the only caller of `page_address`, only on `Offered::No`.

Run: `cargo test --lib -j 2 a_browser_gets_neither_unmatched_card_even_with_the_prompt_on`
Expected: PASS. A browser never reaches `handle_no_match` on a focus change, so the address is never read on one.

- [ ] **Step 7: The hand-check example, after seeing the file census go red**

Create `examples/address_bar_probe.rs` with the Write tool:

```rust
//! **The hand check for `browser_url::read`**, which no test can run.
//!
//! Run it, switch to a browser window within five seconds, and it prints what
//! Deskwarden would rank the account picker by: the address it read, or
//! "nothing". It prints to this console only -- the app itself never logs the
//! address.
//!
//! ```text
//! cargo run --example address_bar_probe
//! ```

fn main() {
    println!("Switch to a browser window. Reading its address bar in 5 seconds...");
    std::thread::sleep(std::time::Duration::from_secs(5));
    let Some(window) = deskwarden::window_watch::current_foreground_event() else {
        println!("There is no foreground window.");
        return;
    };
    println!(
        "Foreground: {} ({:?})",
        window.exe_name,
        deskwarden::app::browser_window(&window.exe_name)
    );
    match deskwarden::browser_url::read(window.hwnd) {
        Some(url) => println!("Address bar: {url}"),
        None => println!(
            "Address bar: nothing (not a supported browser, no address field found, or not a \
             web address)"
        ),
    }
}
```

Run: `python "$TEMP/dw_crlf.py" --fix examples/address_bar_probe.rs`
Expected: `examples/address_bar_probe.rs 0`

Run: `cargo test --lib -j 2 the_two_job_bearing_modules_can_start_a_child_only_through_this_one`
Expected: FAIL. "the set of Rust source files outside `src/` changed".

In `src/job_object.rs`, replace:

```rust
                "build.rs",
                "examples/field_locator_probe.rs",
```

with:

```rust
                "build.rs",
                // Prints what `browser_url::read` reads from the foreground
                // browser's address bar: the hand check for a UI Automation
                // query no test process can make.
                "examples/address_bar_probe.rs",
                "examples/field_locator_probe.rs",
```

Run: `python "$TEMP/dw_crlf.py" src/job_object.rs`
Expected: `src/job_object.rs 0`

Run: `cargo test --lib -j 2 the_two_job_bearing_modules_can_start_a_child_only_through_this_one`
Expected: PASS.

- [ ] **Step 8: Check by hand in Chrome, Edge and Firefox**

For each of Chrome, Edge and Firefox, with a sign-in page open (any site with a saved login):

Run: `cargo run --example address_bar_probe`, then click into the browser within five seconds.

Expected:
- `Foreground: chrome.exe (Yes)` (or `msedge.exe`, `firefox.exe`);
- `Address bar: https://<the site>/<path>`.

Then click into the page's own login form instead of the address bar and run it again. Expected: the same address. The field's text does not depend on focus.

With a non-browser (for example Notepad) in front, the expected output is `Foreground: Notepad.exe (No)` and `Address bar: nothing (...)`. A failure here is not a test failure. If a browser prints `nothing`, run its window through Accessibility Insights or `inspect.exe`, compare the address field's `ClassName` / `AutomationId` / `Name` / `ControlType` against `browser_url::Family::address_field`, and report it. Do not widen the keys without re-measuring.

- [ ] **Step 9: Check the feature end to end by hand**

Run the app from this tree: `cargo run`, with the vault unlocked. Then:
1. In Chrome, open a site with a saved login whose website is that site's domain, and press `CTRL+ALT+B`. Expected: the card's first row, already highlighted, is that login. Enter, then Enter again, fills it.
2. On a site with no saved login, press `CTRL+ALT+B`. Expected: the card ranks by the title, as before.
3. Fill a login, then press `CTRL+ALT+B` again at the same window within five minutes. Expected: the remembered-record card appears, not the picker.
4. Check `%APPDATA%\Deskwarden`'s log for the session. Expected: no line contains the site's host.

- [ ] **Step 10: Verify the whole task**

Run:
```bash
cargo test --lib -j 2
cargo test --bins -j 2
cargo check --all-targets
RUSTFLAGS="-D warnings" cargo test --no-run
```
Expected:
- both test runs pass;
- `check` prints no warning, examples included;
- the `-D warnings` build finishes.

- [ ] **Step 11: CRLF over every file this plan touched**

Run: `python "$TEMP/dw_crlf.py" src/uri_match.rs src/browser_url.rs src/lib.rs src/foreground.rs src/job_object.rs src/app.rs src/app_candidates.rs src/vault_window/detail_edit.rs src/injector/ui_automation.rs src/injector/target.rs examples/address_bar_probe.rs`
Expected: every line ends in ` 0`.

- [ ] **Step 12: The README test badge**

`job_object::tests::the_readme_test_count_is_not_a_boast` counts every literal `#[test]` under `deskwarden/src`, and requires `claimed <= actual < claimed + 200`.

Run (from `E:\Personal\node-bitwarden`):
```bash
python - <<'EOF'
import pathlib, re
n = sum(p.read_text(encoding='utf-8', errors='replace').count('#[test]') for p in pathlib.Path('deskwarden/src').rglob('*.rs'))
badge = re.search(r'/badge/tests-(\d+)%2B', open('README.md', encoding='utf-8').read()).group(1)
print('actual', n, 'claimed', badge)
EOF
```
Expected: `actual 6062 claimed 5900`, which is inside the window, so `README.md` is not touched. If `actual` is 6100 or more, change only the number in `tests-5900%2B` to `actual` rounded down to the hundred, and add `../README.md` to this task's commit.

- [ ] **Step 13: Commit**

```bash
git add src/injector/ui_automation.rs src/injector/target.rs src/browser_url.rs src/app.rs examples/address_bar_probe.rs src/job_object.rs
git commit -m "The account picker puts the login saved for the page in the address bar on top

CTRL+ALT+B in a browser reads the address bar once, through UI Automation,
and hands the page to the picker's ranking, so the login saved for that
website is the first row -- the row the card already focuses. Nothing is
typed until the user presses a key, nothing is read on a focus change, the
five-minute recall still comes first, and the address is never logged.

The read finds every candidate field and keeps only one that is the
browser's own: not inside a Document and at most ten steps below the
window, so a page cannot plant a field that poses as the address bar.
examples/address_bar_probe.rs is the hand check.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Self-Review

**1. Spec coverage**

| Spec requirement | Where |
|---|---|
| Owner decision 1: a URL match only pre-picks; nothing typed without a key; no unattended fill, no fill card, no matching outside the hotkey | T3 (ranking only), T4 (`page_address` only from `handle_no_match`; `match_engine` untouched; Decision 1) |
| Owner decision 2: Bitwarden's rules per URI, base domain by the PSL (`accounts.google.com`, `bbc.co.uk`) | T1 (`uri_match`, tests `base_domain_*`) |
| Owner decision 3: read the address bar through UI Automation, no extension | T4 (`address_field_text`) |
| `browser_url`: only on `CTRL+ALT+B`, only for `BROWSER_IMAGE_NAMES`, never on focus change or in the background | T4 (`page_address_with`, test `only_a_browser_window_is_asked_for_its_address`; Step 6) |
| Chromium "Address and search bar" / Firefox `urlbar-input` / IE not supported; stable properties first, name as fallback; documented per family | T2 (`Family::address_field` doc and test), Decision 9 |
| Returns the text or nothing; never a user-visible error | T4 (`read` maps every failure to `None`) |
| Beside `injector::ui_automation`, same apartment handling | T4 (`address_field_text` in that file, the same `CoInitializeEx` block) |
| `parse`: trim, add `https://`, http/https only, reject spaces, reject no dot / not localhost, `url` crate | T2 (`parse` and its 9 tests), Decision 13 |
| `uri_match::matches(saved, Option<MatchType>, &Url)`, six types, `None` = base domain, IP/localhost by host and port | T1, Decision 5 |
| Saved URIs without a scheme read as `http://` | T1 (`saved_url`, test `a_saved_website_without_a_scheme_is_read_as_http`) |
| Match type from `UriEntry::other["match"]` | T1 (`SavedUri::of`, `MatchType::from_wire`) |
| `psl` crate, manifest re-pin | T1, Steps 1-5 |
| `candidates` gains `Option<&Url>` and `RANK_URL` above `RANK_URI_HOST` | T3 |
| `ItemFacts` carries URIs with match types, no secret | T3 Step 3 |
| No exe-stem rank in a browser | T3 (`in_a_browser_the_executable_name_is_not_a_signal`), Decision 12 |
| Pre-select the first row when URL-ranked; recall still wins | T4 (`the_login_saved_for_the_page_is_the_first_offer`, Step 6), Decision 1 |
| Safety: `match_engine` unchanged; URL not logged | no task touches `match_engine.rs`; logging tests in T1, T2 and T4 |
| Testing: `parse` cases (scheme, http/https, search, localhost, IPs, ports, IDN) | T2 tests |
| Testing: `uri_match` cases (every type, `co.uk`, `github.io`, subdomains, IPs, localhost, ports, invalid regex, `None`, `Never`) | T1 tests |
| Testing: `app_candidates` cases | T3 tests |
| Testing: UIA read checked by hand in Chrome, Edge, Firefox | T4 Steps 8-9 |

Gaps found while writing are listed under "Decisions this plan takes": 1, 2, 5, 9, 10, 11 and 12 need the owner's eye. Nothing in the spec is left without a task.

**2. Placeholder scan.** Every code step shows the full code. Every command has its expected output. The only conditional step is the README badge, and its rule and its expected numbers are stated.

**3. Type consistency.**
- `MatchType`, `SavedUri { uri, match_type }` and `matches(&str, Option<MatchType>, &Url)` are used with the same names in T1, T3 (`app_candidates::ranked`, `ItemFacts.uris`) and T3's tests.
- `AddressField { kinds, class_names, automation_ids, names }`, `FieldKind::{Edit, ComboBox}`, `MAX_FIELD_DEPTH` and `is_browser_chrome(Option<usize>, bool)` are defined in T2 and used with the same names in T4's `address_field_text` and `ancestry`.
- `candidates(.., page: Option<&url::Url>)` is defined in T3 and called with `None` in T3 and with `page` in T4.
- `picker_offers_for(cache, window, page)` is defined in T4 and pinned by the T4 test's needle `picker_offers_for(cache, window, page.as_ref())`.

**Verification of this plan.** On 2026-10-01 every task's code was applied, in order, to a scratch copy of `3fc985e`, and these passed:
- `cargo test --lib`;
- `cargo test --bins`;
- `cargo check --all-targets` (0 warnings);
- `RUSTFLAGS="-D warnings" cargo test --no-run`.

The UI Automation query was run against live Chrome, Edge, Brave and Firefox windows, and returned an address in each.

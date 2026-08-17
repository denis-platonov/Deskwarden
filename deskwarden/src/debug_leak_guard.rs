//! The crate-wide guard that makes the next `Debug`-prints-a-secret bug fail
//! the suite instead of an audit.
//!
//! # Why this file exists
//!
//! Six types in this crate have been found able to print a secret through
//! `Debug`, and every one of them was found **by accident, one at a time**:
//!
//! * `CreatedSend` and `SendSummary` carried `access_url`, whose fragment is
//!   the Send's decryption key.
//! * `RawOutput`'s `stdout` is `bw send create`'s response body -- the access
//!   URL *and* the secret.
//! * `SendInvocation` printed `args` in full, justified on the premise that
//!   "arguments never carry a secret", which died the day a command took an
//!   access URL positionally.
//! * `NewItem` derived `Debug` over an SSH private key and, later, an
//!   imported record's password and TOTP seed.
//! * `LoginData`, `CardData`, `SshKeyData`, `VaultField` and `VaultItem` all
//!   derived `Debug` over `Zeroizing` fields. Those five were found by the
//!   first run of the scan in this file.
//!
//! Each was fixed on its own. The recurring shape is not any one of them: it
//! is that **a type gains a secret-bearing field later than it gained its
//! `Debug`, and nothing notices.** A seventh one-off is not the answer; a
//! test that fails on the shape is.
//!
//! # What this guard actually asserts
//!
//! A type that can reach a secret must not get its `Debug` from `#[derive]`.
//! It must either hand-write one (which is a decision someone made, in a
//! diff a reviewer sees) or appear in [`EXEMPT`] with a written reason.
//!
//! "Can reach a secret" is decided **structurally, from the source**, in two
//! steps:
//!
//! 1. **Seed.** A type whose own body mentions [`Zeroizing`] holds a secret.
//!    `Zeroizing` is this crate's own marker for "this value must be wiped",
//!    so it is the crate's existing, load-bearing statement of what a secret
//!    is -- not a new list invented here. And `Zeroizing<T>` **derives
//!    `Debug` and prints the inner value**; it is not a redacting wrapper, so
//!    a derived `Debug` over one really does print the secret.
//!
//! 2. **Propagate, with hand-written `Debug` as a barrier.** If type `A`
//!    mentions a secret-bearing type `B`, then `A` can reach `B`'s secret --
//!    *but only if `B`'s own `Debug` is derived*, because a derived `Debug`
//!    is what forwards the field verbatim. If `B` hand-writes its `Debug`,
//!    `B` has already refused, and `A` printing a `B` prints `B`'s refusal.
//!    So propagation stops at every hand-written impl.
//!
//! Step 2's barrier is what keeps this from flagging half the crate: fixing
//! the type that owns the secret un-flags everything downstream of it, which
//! is also the correct place to fix it.
//!
//! # This is not a list of type names
//!
//! [`EXEMPT`] is a list, but it is not the failure mode this crate keeps
//! losing to ("two enumerations that must agree", where forgetting one side
//! passes silently). It runs the other way: **the default is failure.** A new
//! type carrying a secret is caught with no edit to this file at all; the
//! list only lets a human record why a specific flagged type is a false
//! positive. Forgetting to update it reds the suite. Nothing is silently
//! allowed.
//!
//! # What this guard CANNOT see -- read this before trusting it
//!
//! It reads text. It is not a type checker, and it has real holes:
//!
//! * **A secret that is not a `Zeroizing`.** A plain `String` field holding a
//!   password seeds nothing. This is the biggest hole: four of the six
//!   historical leaks (`access_url`, `stdout`, `args`) were plain `String`s
//!   and **this guard would not have caught any of them.** It catches the
//!   `NewItem` class and would have caught the five it did catch. It is not a
//!   general secret-flow analysis and must not be described as one.
//! * **A type alias.** `type Secret = Zeroizing<String>;` used as a field
//!   type hides the marker from the seed step.
//! * **Indirection through a generic or a trait object** -- `Box<dyn Any>`, a
//!   `HashMap<String, T>` filled elsewhere -- carries no name to match on.
//! * **Name collisions across files.** Types are matched by bare name, so two
//!   types called `Refusal` in different modules are conflated. This
//!   over-approximates (both get flagged), which fails safe.
//! * **`Debug` obtained any way other than `#[derive]` or an inline
//!   `impl fmt::Debug for`** -- through a macro, or a blanket impl.
//!
//! It was chosen over the alternative -- construct every secret-bearing value
//! with a known needle and assert the needle never appears in `{:?}` -- for
//! one reason. That check only sees types **somebody remembered to
//! construct**, and "nobody remembered" is precisely how all six leaks
//! happened. A source scan sees every type in the crate whether or not anyone
//! thought about it, which is the failure being defended against. The runtime
//! check is stronger where it looks and blind where it does not; this one is
//! weaker per type and blind nowhere. The honest summary is that it closes
//! the `Zeroizing` class completely and leaves the plain-`String` class open.

use std::collections::{BTreeMap, BTreeSet};

/// A `struct` or `enum` declaration found in the crate's source.
#[derive(Debug)]
struct Item {
    /// Path relative to `src/`, for the failure message.
    file: String,
    name: String,
    /// Whether `Debug` appears in a `#[derive(..)]` on this item.
    derives_debug: bool,
    /// The text between the item's braces, or `""` for a unit/tuple item.
    body: String,
}

/// Types whose flag is a false positive, each with the reason it is one.
///
/// **Adding an entry is a claim that the type cannot print a secret**, and
/// the reason is the argument for that claim. "Nothing logs it today" is not
/// a reason -- that is a fact about call sites, and it is the exact argument
/// that was written on `NewItem` before `NewItem` was fixed.
const EXEMPT: &[(&str, &str)] = &[];

/// Every `.rs` file under `src/`, as (path relative to `src/`, contents).
///
/// Walks the directory rather than reading a list, so a file added to the
/// crate is scanned without this file changing.
fn crate_sources() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("cannot read a directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
                let rel = path
                    .strip_prefix(&root)
                    .expect("every walked path is under src/")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, text));
            }
        }
    }
    out.sort();
    out
}

/// Advances `i` past a comment or a string/char literal starting at `i`, or
/// returns `None` if nothing of the sort starts there.
///
/// The literal cases exist so a `"struct Foo {"` inside a string -- this
/// crate's test modules are full of source-shaped strings -- is not read as a
/// declaration.
fn skip_trivia(b: &[u8], i: usize) -> Option<usize> {
    match b[i] {
        b'/' if b.get(i + 1) == Some(&b'/') => {
            let mut j = i;
            while j < b.len() && b[j] != b'\n' {
                j += 1;
            }
            Some(j)
        }
        b'/' if b.get(i + 1) == Some(&b'*') => {
            let mut j = i + 2;
            let mut nest = 1usize;
            while j < b.len() && nest > 0 {
                if b[j] == b'/' && b.get(j + 1) == Some(&b'*') {
                    nest += 1;
                    j += 2;
                } else if b[j] == b'*' && b.get(j + 1) == Some(&b'/') {
                    nest -= 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            Some(j)
        }
        b'"' => {
            let mut j = i + 1;
            while j < b.len() && b[j] != b'"' {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            Some(j + 1)
        }
        // A raw string, but only when the letter starts a token, so `for` and
        // `breach` are not read as string openers.
        b'r' | b'b'
            if (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) =>
        {
            let mut j = i;
            if b[j] == b'b' {
                j += 1;
            }
            if b.get(j) != Some(&b'r') {
                return None;
            }
            j += 1;
            let from = j;
            while b.get(j) == Some(&b'#') {
                j += 1;
            }
            let hashes = j - from;
            if b.get(j) != Some(&b'"') {
                return None;
            }
            j += 1;
            // Scan for the closing quote followed by `hashes` hashes.
            while j < b.len() {
                if b[j] == b'"' && b[j + 1..].iter().take(hashes).all(|c| *c == b'#') {
                    return Some(j + 1 + hashes);
                }
                j += 1;
            }
            Some(b.len())
        }
        _ => None,
    }
}

/// `text` with every comment and literal replaced by a space.
///
/// **The body is matched against type names, so its comments must go.** A
/// doc comment that merely *mentions* `LoginData` is not a field of type
/// `LoginData`, and reading it as one flagged a third of the crate's UI enums
/// on the first run of this guard -- `SendError` among them, whose body holds
/// nothing but `String`s and whose prose happens to name types that do carry
/// secrets. Prose is where this crate explains its secrets; it is the last
/// place a scanner should take as evidence.
fn code_only(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < b.len() {
        if let Some(next) = skip_trivia(b, i) {
            if next > i {
                out.push(' ');
                i = next;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// Reads the identifier starting at `i`, or `None` if one does not.
fn ident_at(b: &[u8], i: usize) -> Option<(String, usize)> {
    if !(b[i].is_ascii_alphabetic() || b[i] == b'_') {
        return None;
    }
    let mut j = i;
    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
        j += 1;
    }
    Some((String::from_utf8_lossy(&b[i..j]).into_owned(), j))
}

/// Every `struct`/`enum` in `src`, plus the names every inline
/// `impl fmt::Debug for _` in `src` covers.
///
/// One linear pass that skips comments and literals, so a declaration quoted
/// inside a test's source-shaped string is not mistaken for a real one.
fn scan(file: &str, src: &str) -> (Vec<Item>, BTreeSet<String>) {
    let b = src.as_bytes();
    let mut items = Vec::new();
    let mut handwritten = BTreeSet::new();
    // The `#[derive(..)]` contents seen since the last item keyword.
    let mut pending_derive = String::new();
    let mut i = 0usize;
    while i < b.len() {
        if let Some(next) = skip_trivia(b, i) {
            // A raw-string probe can decline; only a real skip advances.
            if next > i {
                i = next;
                continue;
            }
        }
        if b[i] == b'#' && b.get(i + 1) == Some(&b'[') {
            // Capture the attribute by matching its brackets.
            let mut j = i + 1;
            let mut depth = 0i32;
            while j < b.len() {
                if let Some(next) = skip_trivia(b, j) {
                    if next > j {
                        j = next;
                        continue;
                    }
                }
                match b[j] {
                    b'[' => depth += 1,
                    b']' => {
                        depth -= 1;
                        if depth == 0 {
                            j += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            let attr = &src[i..j.min(src.len())];
            if let Some(rest) = attr.find("derive(") {
                pending_derive.push_str(&attr[rest..]);
            }
            i = j;
            continue;
        }
        let Some((word, after)) = ident_at(b, i) else {
            i += 1;
            continue;
        };
        match word.as_str() {
            // Attributes and visibility sit between a derive and its item, so
            // they must not clear what is pending.
            "pub" | "crate" | "self" | "super" | "in" => {}
            "struct" | "enum" => {
                let derives_debug = pending_derive
                    .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .any(|t| t == "Debug");
                pending_derive.clear();
                let Some((name, mut k)) = next_ident(b, after) else {
                    i = after;
                    continue;
                };
                // Find the item's `{`, or a `;` for a unit/tuple item.
                let mut body = String::new();
                while k < b.len() {
                    if let Some(next) = skip_trivia(b, k) {
                        if next > k {
                            k = next;
                            continue;
                        }
                    }
                    if b[k] == b';' {
                        break;
                    }
                    if b[k] == b'{' {
                        let close = crate::below_cut::match_brace(src, k);
                        body = code_only(&src[k..close]);
                        k = close;
                        break;
                    }
                    k += 1;
                }
                items.push(Item {
                    file: file.to_string(),
                    name,
                    derives_debug,
                    body,
                });
                i = k.max(after);
                continue;
            }
            "impl" => {
                pending_derive.clear();
                // Read the header up to the opening brace and see whether it
                // is a `Debug` impl, and for what.
                let mut k = after;
                while k < b.len() && b[k] != b'{' {
                    if let Some(next) = skip_trivia(b, k) {
                        if next > k {
                            k = next;
                            continue;
                        }
                    }
                    k += 1;
                }
                let header = &src[after..k.min(src.len())];
                if header.contains("Debug") {
                    if let Some(pos) = header.rfind(" for ") {
                        let target = header[pos + 5..]
                            .trim()
                            .trim_start_matches("crate::")
                            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                            .find(|s| !s.is_empty())
                            .unwrap_or("")
                            .to_string();
                        if !target.is_empty() {
                            handwritten.insert(target);
                        }
                    }
                }
                i = k.max(after);
                continue;
            }
            _ => pending_derive.clear(),
        }
        i = after;
    }
    (items, handwritten)
}

/// The next identifier at or after `i`, skipping whitespace and trivia.
fn next_ident(b: &[u8], mut i: usize) -> Option<(String, usize)> {
    while i < b.len() {
        if let Some(next) = skip_trivia(b, i) {
            if next > i {
                i = next;
                continue;
            }
        }
        if let Some(found) = ident_at(b, i) {
            return Some(found);
        }
        i += 1;
    }
    None
}

/// Whether `body` mentions `name` as a whole token.
fn mentions(body: &str, name: &str) -> bool {
    let b = body.as_bytes();
    let n = name.as_bytes();
    if n.is_empty() || b.len() < n.len() {
        return false;
    }
    (0..=b.len() - n.len()).any(|i| {
        &b[i..i + n.len()] == n
            && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_'))
            && b
                .get(i + n.len())
                .is_none_or(|c| !(c.is_ascii_alphanumeric() || *c == b'_'))
    })
}

/// Every `struct`/`enum` in the crate, and every type with a hand-written
/// `Debug`.
fn scan_crate() -> (Vec<Item>, BTreeSet<String>) {
    let mut items = Vec::new();
    let mut handwritten = BTreeSet::new();
    for (file, src) in crate_sources() {
        let (found, hw) = scan(&file, &src);
        items.extend(found);
        handwritten.extend(hw);
    }
    (items, handwritten)
}

/// The names of every type that can reach a secret, by the two-step rule in
/// this module's header.
fn secret_bearing(items: &[Item], handwritten: &BTreeSet<String>) -> BTreeSet<String> {
    let mut secret: BTreeSet<String> = items
        .iter()
        .filter(|i| mentions(&i.body, "Zeroizing"))
        .map(|i| i.name.clone())
        .collect();
    loop {
        // Only a type whose own `Debug` is derived forwards its fields
        // verbatim; a hand-written one is a barrier and stops propagation.
        let propagating: Vec<String> = secret
            .iter()
            .filter(|n| !handwritten.contains(*n))
            .cloned()
            .collect();
        let mut grew = false;
        for item in items {
            if secret.contains(&item.name) {
                continue;
            }
            if propagating.iter().any(|t| mentions(&item.body, t)) {
                secret.insert(item.name.clone());
                grew = true;
            }
        }
        if !grew {
            return secret;
        }
    }
}

/// Every type that reaches a secret and takes its `Debug` from a `#[derive]`,
/// as `name -> file`.
fn offenders() -> BTreeMap<String, String> {
    let (items, handwritten) = scan_crate();
    let secret = secret_bearing(&items, &handwritten);
    items
        .iter()
        .filter(|i| i.derives_debug && secret.contains(&i.name) && !handwritten.contains(&i.name))
        .filter(|i| !EXEMPT.iter().any(|(name, _)| *name == i.name))
        .map(|i| (i.name.clone(), i.file.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard itself.
    #[test]
    fn no_type_that_can_reach_a_secret_derives_debug() {
        let found = offenders();
        assert!(
            found.is_empty(),
            "these types can reach a secret and take their `Debug` from a `#[derive]`, so \
             `{{:?}}` on one prints the secret into whatever log or panic message asked \
             for it:\n{}\n\nFix it by hand-writing `impl std::fmt::Debug` for the type and \
             eliding the secret whole (see `send.rs`'s `SendPlan`/`CreatedSend` and \
             `vault_bridge.rs`'s `NewItem` for the house style). If the flag is a false \
             positive, add the type to `EXEMPT` in `debug_leak_guard.rs` WITH THE REASON \
             it cannot print a secret -- \"nothing logs it today\" is not a reason, it is \
             the argument that was written on `NewItem` before `NewItem` leaked.",
            found
                .iter()
                .map(|(name, file)| format!("  {file}: {name}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    /// Positive control for the scanner, so the guard above cannot pass by
    /// finding nothing at all.
    ///
    /// A scan that returned an empty item list would make
    /// `no_type_that_can_reach_a_secret_derives_debug` vacuously true, which
    /// is the failure this crate has shipped at least twice. These assertions
    /// name things known to be in the tree.
    #[test]
    fn the_scan_really_reads_this_crate() {
        let (items, handwritten) = scan_crate();
        assert!(
            items.len() > 300,
            "only {} types parsed out of the crate -- the scan is broken and the guard is \
             vacuous",
            items.len()
        );
        for expected in ["NewItem", "VaultItem", "LoginData", "SendPlan", "RawOutput"] {
            assert!(
                items.iter().any(|i| i.name == expected),
                "the scan did not find `{expected}`, which is known to be declared in this \
                 crate: the parse is wrong"
            );
        }
        // The barrier half has to work too, or every fixed type would still
        // be flagged and the guard would be permanently red.
        for expected in ["SendPlan", "CreatedSend", "RawOutput", "NewItem", "VaultItem"] {
            assert!(
                handwritten.contains(expected),
                "the scan did not see the hand-written `Debug` for `{expected}`"
            );
        }
        // And the seed step has to find secrets, or the propagation is over
        // an empty set.
        let secret = secret_bearing(&items, &handwritten);
        for expected in ["NewItem", "LoginData", "CardData", "SshKeyData", "VaultItem"] {
            assert!(
                secret.contains(expected),
                "`{expected}` holds a `Zeroizing` field but was not classified as \
                 secret-bearing"
            );
        }
    }

    /// **The guard reds on a real reintroduction.**
    ///
    /// The test above asserts the tree is clean, which a guard that can never
    /// fail would also do. This one feeds the scanner a type of exactly the
    /// shape that has leaked six times -- a `#[derive(Debug)]` over a
    /// `Zeroizing` field -- and asserts it is caught, so "the suite is green"
    /// means the guard looked and found nothing rather than that it cannot
    /// look.
    #[test]
    fn a_reintroduced_derive_over_a_secret_is_caught() {
        let source = r#"
            #[derive(Debug, Clone)]
            pub struct ReintroducedLeak {
                pub name: String,
                pub password: Zeroizing<String>,
            }
        "#;
        let (items, handwritten) = scan(" synthetic.rs", source);
        assert_eq!(items.len(), 1, "the scanner did not find the planted type");
        assert!(items[0].derives_debug, "the planted derive was not seen");
        let secret = secret_bearing(&items, &handwritten);
        assert!(
            secret.contains("ReintroducedLeak"),
            "a `#[derive(Debug)]` over a `Zeroizing` field was NOT classified as \
             secret-bearing -- the guard is decorative"
        );
    }

    /// The barrier really is a barrier: the same type with a hand-written
    /// `Debug` is still secret-bearing, but no longer propagates its secret
    /// to a container that merely holds one.
    ///
    /// Without this, "fixing" a type by hand-writing its `Debug` might leave
    /// every container flagged forever, and the pressure would be to widen
    /// `EXEMPT` instead of to fix anything.
    #[test]
    fn a_hand_written_debug_stops_the_secret_propagating_to_its_container() {
        let leaky = r#"
            #[derive(Debug, Clone)]
            pub struct Inner { pub password: Zeroizing<String> }
            #[derive(Debug, Clone)]
            pub struct Outer { pub inner: Inner }
        "#;
        let (items, hw) = scan("a.rs", leaky);
        let secret = secret_bearing(&items, &hw);
        assert!(
            secret.contains("Outer"),
            "a container of a derived-Debug secret must itself be flagged"
        );

        let fixed = r#"
            #[derive(Clone)]
            pub struct Inner { pub password: Zeroizing<String> }
            impl std::fmt::Debug for Inner {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str("Inner")
                }
            }
            #[derive(Debug, Clone)]
            pub struct Outer { pub inner: Inner }
        "#;
        let (items, hw) = scan("a.rs", fixed);
        assert!(hw.contains("Inner"), "the hand-written impl was not seen");
        let secret = secret_bearing(&items, &hw);
        assert!(
            secret.contains("Inner"),
            "`Inner` still HOLDS the secret and must stay classified as secret-bearing"
        );
        assert!(
            !secret.contains("Outer"),
            "`Inner` refuses in its own `Debug`, so `Outer` printing an `Inner` prints \
             that refusal and must not be flagged"
        );
    }

    /// A declaration quoted inside a string is not a declaration.
    ///
    /// This crate's test modules are full of source-shaped strings (the
    /// below-the-cut walks feed synthetic Rust to their matchers). Without
    /// the literal skipping, those would be scanned as real types and the
    /// guard would fail on text that compiles to nothing.
    #[test]
    fn a_type_quoted_inside_a_string_is_not_read_as_a_declaration() {
        let source = r##"
            const SAMPLE: &str = "#[derive(Debug)] struct NotReal { p: Zeroizing<String> }";
            const RAW: &str = r#"#[derive(Debug)] struct AlsoNotReal { p: Zeroizing<String> }"#;
        "##;
        let (items, _) = scan("a.rs", source);
        assert!(
            !items.iter().any(|i| i.name == "NotReal" || i.name == "AlsoNotReal"),
            "a type quoted inside a string literal was read as a real declaration: {:?}",
            items.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
    }

    /// Every entry in [`EXEMPT`] still names a type this crate declares, and
    /// still carries a reason.
    ///
    /// An exemption for a type that has been renamed or deleted is a silent
    /// hole: it would keep excusing whatever type takes that name next.
    #[test]
    fn every_exemption_names_a_real_type_and_gives_a_reason() {
        let (items, _) = scan_crate();
        for (name, reason) in EXEMPT {
            assert!(
                items.iter().any(|i| i.name == *name),
                "`{name}` is exempted in `debug_leak_guard.rs` but is not declared in this \
                 crate any more -- delete the entry rather than leaving it to excuse the \
                 next type that takes the name"
            );
            assert!(
                reason.len() > 30,
                "the exemption for `{name}` has no real reason written on it: {reason:?}"
            );
        }
    }
}

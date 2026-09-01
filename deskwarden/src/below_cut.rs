//! The ONE brace matcher every below-the-cut walk in this crate uses.
//!
//! # Why this file exists
//!
//! Six files (`send.rs`, `updater.rs`, `vault_export.rs`, `breach.rs`,
//! `job_object.rs`) each carry a two-state walk of the region
//! below their first `cfg(test)` module. Each walk's model of "the module
//! ended" is the line `}` at column 0 -- and a module close written INDENTED
//! is legal Rust, so a payload can close the module early, plant a file-scope
//! `pub fn` in the half no guard reads, and rebalance the count with a
//! column-0 `}` further down. The only thing that catches that is a real
//! brace count that says where the module ACTUALLY ends, so the walk can
//! refuse any other line as its close.
//!
//! That matcher was copy-pasted into three of the six and had to reach all
//! six. A sixfold copy is how the next divergence happens: the copies that
//! existed already carried an identical off-by-one in the escaped-quote
//! branch, and fixing three of them would have left three holes. So there is
//! exactly one implementation, here, and all six call it.
//!
//! # The defect this file was created to fix
//!
//! The old branch read:
//!
//! ```text
//! if i + 2 < b.len() && b[i + 1] == b'\\' {
//!     let mut j = i + 2;
//!     while j < b.len() && b[j] != b'\'' { j += 1; }
//!     i = j + 1;
//! }
//! ```
//!
//! For `'\''` the bytes are `'`, `\`, `'`, `'`. The scan for the terminator
//! started ON the escaped quote, stopped there immediately, and left `i` on
//! the literal's REAL closing tick. That phantom tick re-entered the branch,
//! took the `b[i + 2] == b'\''` arm, skipped three more bytes, and landed the
//! scanner on the content byte of the next char literal. If that byte was a
//! `"`, the matcher opened a phantom string and swallowed source -- including
//! a module's closing brace -- until the next `"`, and the offset assertion
//! then computed exactly the close the payload wanted it to.
//!
//! Measured on the unfixed tree: a payload whose decoy was `['\'','"']`
//! SURVIVED the full suite at 2202 passed / 0 failed / 0 warnings and shipped
//! three times over in the lib's DEBUG LLVM IR, in all three files that carried
//! matcher. The byte-identical payload with the decoy written `['a','"']` was
//! KILLED by the offset assertion. The off-by-one was the whole hole.
//!
//! The fix is to skip the escaped character BEFORE scanning for the
//! terminator.
//!
//! This module is `#[cfg(test)]` at its declaration in `lib.rs`, so nothing in
//! it can ship.

/// The byte offset, within `region`, of the `}` that matches the `{` at
/// `open`.
///
/// Comments and literals are skipped, because the region below the cut is
/// test code full of braces inside format strings and prose. A shape this
/// scanner cannot read makes the caller's assertion FAIL rather than pass
/// blindly: it panics rather than guessing.
pub fn match_brace(region: &str, open: usize) -> usize {
    let b = region.as_bytes();
    assert_eq!(
        b[open], b'{',
        "the caller pointed the brace matcher at something other than a brace"
    );
    let mut i = open;
    let mut depth = 0i32;
    while i < b.len() {
        match b[i] {
            b'/' if i + 1 < b.len() && b[i + 1] == b'/' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < b.len() && b[i + 1] == b'*' => {
                i += 2;
                let mut nest = 1usize;
                while i < b.len() && nest > 0 {
                    if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
                        nest += 1;
                        i += 2;
                    } else if b[i] == b'*' && i + 1 < b.len() && b[i + 1] == b'/' {
                        nest -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            b'r' | b'b' => {
                // A raw string -- `r".."`, `r#".."#`, `br#".."#` -- but only
                // when the letter STARTS a token, so `for` and `breach` are
                // not read as string openers.
                let starts = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
                let mut j = i;
                if b[j] == b'b' {
                    j += 1;
                }
                if starts && j < b.len() && b[j] == b'r' {
                    j += 1;
                    let from = j;
                    while j < b.len() && b[j] == b'#' {
                        j += 1;
                    }
                    let hashes = j - from;
                    if j < b.len() && b[j] == b'"' {
                        i = end_of_raw(b, j + 1, hashes);
                        continue;
                    }
                }
                i += 1;
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'\'' => {
                // `'x'` and `'\x'` are char literals; anything else with a
                // leading tick is a lifetime and carries no braces.
                if i + 2 < b.len() && b[i + 1] == b'\\' {
                    // Start PAST the escaped character. Starting on it made
                    // `'\''` end one byte early, and the tick left over
                    // opened a phantom token that swallowed the rest of the
                    // module -- see this file's header for the measurement.
                    // Starting at `i + 3` is also right for the long escapes,
                    // `'\u{7f}'`: the scan runs to the real terminator and
                    // the braces inside the escape are never counted.
                    let mut j = i + 3;
                    while j < b.len() && b[j] != b'\'' {
                        j += 1;
                    }
                    i = j + 1;
                } else if i + 2 < b.len() && b[i + 2] == b'\'' {
                    i += 3;
                } else {
                    i += 1;
                }
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    panic!("the block opened at byte {open} below the cut is never closed");
}

/// The byte offset just past the terminator of a raw string whose body starts
/// at `from` and which was opened with `hashes` hash marks.
fn end_of_raw(b: &[u8], from: usize, hashes: usize) -> usize {
    let mut i = from;
    while i < b.len() {
        if b[i] == b'"' {
            let mut k = 0usize;
            while k < hashes && i + 1 + k < b.len() && b[i + 1 + k] == b'#' {
                k += 1;
            }
            if k == hashes {
                return i + 1 + hashes;
            }
        }
        i += 1;
    }
    panic!("unterminated raw string below the cut");
}

/// `true` for `mod NAME {`, `pub mod NAME {` and `pub(crate) mod NAME {`, and
/// for nothing else. Deliberately exact rather than a `starts_with`: a whole
/// module written on one line is not a module opener as far as these walks
/// are concerned, and must fail them.
pub fn is_module_opener(line: &str) -> bool {
    let t = line.strip_prefix("pub(crate) ").unwrap_or(line);
    let t = t.strip_prefix("pub ").unwrap_or(t);
    let Some(rest) = t.strip_prefix("mod ") else {
        return false;
    };
    let Some(name) = rest.strip_suffix(" {") else {
        return false;
    };
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// How many gated, column-0 module openers a below-the-cut region contains.
///
/// The `modules == N` control every caller carries used to be a bare literal,
/// which meant that adding a second gated module below the cut and editing one
/// digit were two coordinated edits that between them widened the walk's
/// non-vacuity control without touching a word of its prose. Deriving the
/// count from the source removes that shape.
///
/// This is a NON-VACUITY control and nothing more, and it shares
/// [`is_module_opener`] with the walk it controls, so it cannot catch a bug in
/// that predicate. What it does catch is a walk that visited lines and opened
/// nothing.
pub fn column_zero_module_openers(region: &str) -> usize {
    region
        .lines()
        .filter(|line| !line.starts_with(char::is_whitespace) && is_module_opener(line.trim()))
        .count()
}

/// The parts of a below-the-cut walk that genuinely differ from file to file.
///
/// The walk itself does not differ, and used to be pasted fifteen times. That
/// is the shape this struct exists to remove: the escaped-quote off-by-one in
/// [`match_brace`] reached three copies at once precisely because the walk was
/// copied rather than called, and every fix since has had to be applied N
/// times or silently fail to propagate. What the copies really disagreed
/// about is small and is enumerated here; everything else is now one body.
///
/// Note what is NOT in this struct: the module-opener predicate is a function
/// POINTER, not a shared item. Each caller keeps its own
/// `below_cut_is_module_opener` and passes it in. That is deliberate. The
/// `modules == column_zero_module_openers(region)` control every caller
/// carries compares the walk's opener count against
/// [`column_zero_module_openers`], which uses [`is_module_opener`] -- a
/// DIFFERENT instance. A one-edit widening of either predicate desynchronizes
/// the two and reds the suite. Pointing the walk at [`is_module_opener`]
/// would have made both sides move together and thrown that away.
pub struct WalkRules {
    /// The test gate line, e.g. `#[cfg(test)]`, spelled by the caller so it
    /// is not a literal occurrence of itself in a file that counts them.
    pub gate: &'static str,
    /// `true` when the gate for the FIRST module sits above the region handed
    /// in, so the walk must start already armed. `false` when the region
    /// begins with the gate itself and nothing outside it is taken on trust.
    pub gated_at_start: bool,
    /// `true` requires the gate at column 0; `false` compares the trimmed
    /// line. Column 0 is the stronger rule -- an indented `#[cfg(test)]`
    /// inside a function body cannot arm the walk for the next module along
    /// -- but the files whose region starts mid-indentation need the looser
    /// one, so it is a knob and not a decree.
    pub gate_at_column_zero: bool,
    /// The caller's own copy of the opener predicate. See the note above for
    /// why this is not simply [`is_module_opener`].
    pub is_module_opener: fn(&str) -> bool,
    /// Column-0 lines that are the CONTENTS of a string literal rather than
    /// source. Each caller controls its own list for staleness.
    pub string_lines: &'static [&'static str],
    /// What an item down here would escape, in this particular file. Spliced
    /// into the refusal so the message still names the concrete damage.
    pub top_level_item_note: &'static str,
    /// The same, for a module below the cut that carries no gate.
    pub ungated_module_note: &'static str,
}

/// `(visited, modules, closes, depth)` -- the four numbers every caller's
/// non-vacuity controls are written against.
pub type WalkCounts = (usize, usize, usize, usize);

/// Walk `region` -- the whole of a file from its cut to EOF -- and require it
/// to be a sequence of gated, column-0 module blocks and nothing else.
///
/// Returns `Err` rather than panicking so a caller can drive the REAL walk
/// over the mutants its controls exist to catch, without a `catch_unwind`
/// that prints a panic into a green run's output. [`walk`] is the panicking
/// wrapper for the callers that prefer `catch_unwind`.
///
/// # What this closes that a line walk cannot
///
/// The two-state line walk's model of "the module ended" is the line `}` at
/// column 0. A module close written INDENTED is legal Rust, so a payload can
/// close the module early, plant a file-scope `pub fn` in the half no guard
/// reads, and rebalance with a column-0 `}` further down -- perfectly
/// balanced, no lexer trick. Every line of the payload is indented, so the
/// `depth == 1` branch skips it, and the trailing column-0 `}` restores
/// `depth == 0` with `closes == modules`. Measured with eight of those
/// planted at once: 2211 lib / 217 bin / 0 failed / 0 warnings in both
/// profiles, and seven `pub fn`s shipping in the lib's DEBUG LLVM IR.
///
/// So each module opener is brace-matched by [`match_brace`] and the byte
/// offset of its REAL close is recorded. Only that line may be accepted as
/// the close.
pub fn try_walk(region: &str, rules: &WalkRules) -> Result<WalkCounts, String> {
    let mut depth = 0usize;
    let mut gated = rules.gated_at_start;
    let (mut modules, mut closes, mut visited) = (0usize, 0usize, 0usize);
    // Byte offsets are carried alongside each line so a module opener can be
    // brace-matched and its real close pinned.
    let mut expected_close: Option<usize> = None;
    let mut at = 0usize;
    let mut numbered: Vec<(usize, &str)> = Vec::new();
    for raw in region.split_inclusive('\n') {
        numbered.push((at, raw.trim_end_matches('\n').trim_end_matches('\r')));
        at += raw.len();
    }
    for &(offset, line) in &numbered {
        visited += 1;
        if depth == 0 {
            // Between modules NOTHING is allowed but blanks, comments, the
            // gate and a module opener -- at ANY indentation, because an
            // indented `fn` at file scope is still a top-level item and a
            // column-0-only filter would walk straight past it.
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with("//") {
                continue;
            }
            let is_gate = if rules.gate_at_column_zero {
                line == rules.gate
            } else {
                trimmed == rules.gate
            };
            if is_gate {
                gated = true;
                continue;
            }
            if line.starts_with(char::is_whitespace) || !(rules.is_module_opener)(trimmed) {
                return Err(format!(
                    "top-level source below the cut: {line:?}. {} Move it above the test \
                     modules.",
                    rules.top_level_item_note
                ));
            }
            if !gated {
                return Err(format!(
                    "the module {line:?} below the cut is not test-gated, so it SHIPS -- and \
                     it ships in the half of the file no guard here reads. {}",
                    rules.ungated_module_note
                ));
            }
            gated = false;
            depth = 1;
            modules += 1;
            // Where this module REALLY ends, by brace count. Only that line
            // may be accepted as its close.
            let Some(rel) = line.rfind('{') else {
                return Err(format!(
                    "the module opener {line:?} below the cut does not end in an opening \
                     brace, so its real close cannot be computed"
                ));
            };
            expected_close = Some(match_brace(region, offset + rel));
        } else if !line.is_empty() && !line.starts_with(char::is_whitespace) {
            // Inside a test module every item is indented, so the only
            // column-0 line is the module's own closing brace.
            if line == "}" {
                if Some(offset) != expected_close {
                    return Err(format!(
                        "the column-0 `}}` at byte {offset} below the cut is not the brace \
                         that closes the module it appears to close ({expected_close:?}). The \
                         module was closed EARLIER, by an indented brace the line rule cannot \
                         see, and everything between the two was walked as if it were still \
                         module contents -- top-level items at file scope, in the half of \
                         this file no guard reads. Measured surviving the whole suite at 2211 \
                         lib / 217 bin / 0 failed / 0 warnings in both profiles, and shipping \
                         in the lib's DEBUG LLVM IR."
                    ));
                }
                expected_close = None;
                depth = 0;
                closes += 1;
                continue;
            }
            if !rules.string_lines.contains(&line) {
                return Err(format!(
                    "a column-0 line inside a test module below the cut: {line:?}. Either a \
                     top-level item escaped the brace count, or this is the contents of a \
                     string literal and belongs in this file's BELOW_CUT_STRING_LINES"
                ));
            }
        }
    }
    Ok((visited, modules, closes, depth))
}

/// [`try_walk`], panicking on refusal.
pub fn walk(region: &str, rules: &WalkRules) -> WalkCounts {
    match try_walk(region, rules) {
        Ok(counts) => counts,
        Err(why) => panic!("{why}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The offset of the `}` that closes the `{` at the first `{` in `s`.
    fn close_of_first_brace(s: &str) -> usize {
        let open = s.find('{').expect("a brace to match");
        match_brace(s, open)
    }

    /// The regression this file was created for, in its smallest form: the
    /// escaped-quote literal must not blind the matcher to the `"` after it.
    #[test]
    fn an_escaped_quote_literal_does_not_swallow_the_next_string() {
        // Before the fix the scanner landed inside `'"'`, opened a phantom
        // string at that quote, and ran past the closing brace to the next
        // one -- reporting the LATER brace as the close.
        let s = "{ ['\\'','\"'] } }";
        assert_eq!(
            close_of_first_brace(s),
            s.find("} }").expect("the real close is the first of the two"),
            "the matcher ran past the brace that really closes the block: the escaped-quote \
             literal left the scanner inside the string that follows it"
        );
    }

    /// A `'\''` immediately before a raw string: the raw string's hashes and
    /// its `}` must not be mistaken for source.
    #[test]
    fn an_escaped_quote_before_a_raw_string_leaves_the_raw_string_intact() {
        let s = "{ let _ = '\\''; let _ = r#\"}\"#; }";
        assert_eq!(
            close_of_first_brace(s),
            s.len() - 1,
            "the `}}` inside the raw string was counted as the block's close"
        );
    }

    /// The same literal inside a macro body, where the braces around it are
    /// real and must still balance.
    #[test]
    fn an_escaped_quote_inside_a_macro_body_keeps_the_braces_balanced() {
        let s = "{ macro_rules! m { () => { '\\'' } } let _ = \"}\"; }";
        assert_eq!(
            close_of_first_brace(s),
            s.len() - 1,
            "the escaped quote inside the macro body unbalanced the brace count"
        );
    }

    /// Braces that live inside a format string are text, not structure.
    #[test]
    fn braces_inside_a_format_string_are_not_counted() {
        let s = "{ format!(\"{{}}\", '{'); }";
        assert_eq!(close_of_first_brace(s), s.len() - 1);
    }

    /// Every escape shape the region below the cut can legally contain, each
    /// followed by a `"` that a mis-scan would turn into a phantom string.
    #[test]
    fn every_escape_shape_terminates_where_it_really_ends() {
        for lit in [
            "'\\''", "'\\\\'", "'\\n'", "'\\r'", "'\\t'", "'\\0'", "'\\\"'", "'\\x7f'",
            "'\\u{7f}'", "'\\u{1F600}'",
        ] {
            let s = format!("{{ let _ = [{lit},'\"']; }}");
            assert_eq!(
                close_of_first_brace(&s),
                s.len() - 1,
                "the escape {lit} left the scanner somewhere other than just past itself, so \
                 the `\"` after it opened a phantom string"
            );
        }
    }

    /// The shapes that were already correct, kept as a control: a rewrite of
    /// this lexer that fixed the escape and broke one of these would be
    /// caught here rather than by a mutant six files away.
    #[test]
    fn the_shapes_that_were_already_correct_stay_correct() {
        for body in [
            "/* /* */ */ \"}\"",
            "r##\"}\"## ",
            "// \"\n",
            "\"//}\"",
            "b'}'",
            "b'\"'",
            "'\"'",
            "'a'",
            "for x in 0..1 { let _ = x; }",
            "let _: &'static str = \"}\";",
            "fn f<'a>(_: &'a u8) {}",
            "let _ = breach_rate;",
        ] {
            let s = format!("{{ {body} }}");
            assert_eq!(
                close_of_first_brace(&s),
                s.len() - 1,
                "the matcher lost track inside {body:?}"
            );
        }
    }

    /// Non-vacuity: the matcher really can be wrong, so the tests above are
    /// not all trivially true.
    #[test]
    fn the_matcher_distinguishes_nested_braces_from_the_outer_one() {
        let s = "{ { } }";
        assert_eq!(close_of_first_brace(s), 6);
        assert_ne!(close_of_first_brace(s), 4, "control: it returned the INNER close");
    }

    #[test]
    fn the_module_opener_predicate_is_exact() {
        assert!(is_module_opener("mod tests {"));
        assert!(is_module_opener("pub mod a {"));
        assert!(is_module_opener("pub(crate) mod spawn_probe {"));
        assert!(!is_module_opener("mod tests { }"));
        assert!(!is_module_opener("impl JobCommand {"));
        assert!(!is_module_opener("pub mod a::b {"));
        assert!(!is_module_opener("mod  {"));
    }

    #[test]
    fn the_opener_count_ignores_indented_openers_and_counts_column_zero_ones() {
        let region = "#[cfg(test)]\nmod a {\n    mod inner {\n    }\n}\n#[cfg(test)]\nmod b {\n}\n";
        assert_eq!(column_zero_module_openers(region), 2);
        assert_eq!(
            column_zero_module_openers("    mod a {\n"),
            0,
            "control: an indented opener is not a column-0 one"
        );
    }

    // -- the shared walk ----------------------------------------------------
    //
    // These tests are the price of consolidation. Fifteen copies of a walk
    // meant fifteen chances for a fix to fail to propagate; ONE copy means one
    // edit can blind every caller of it at once. What makes that trade
    // acceptable is that the one copy is exercised here directly, over
    // synthetic regions built to be exactly the shapes the callers cannot
    // build for themselves -- including the payload the callers exist to
    // refuse, which no caller can plant in its own source without going red.

    /// A region the walk must accept, so the refusals below are not vacuous.
    const CLEAN: &str =
        "#[cfg(test)]\nmod a {\n    fn f() {}\n}\n#[cfg(test)]\nmod b {\n}\n";

    fn rules() -> WalkRules {
        WalkRules {
            gate: "#[cfg(test)]",
            gated_at_start: false,
            gate_at_column_zero: false,
            is_module_opener,
            string_lines: &[],
            top_level_item_note: "NOTE-TOP.",
            ungated_module_note: "NOTE-UNGATED.",
        }
    }

    #[test]
    fn the_shared_walk_accepts_a_region_of_gated_modules_and_nothing_else() {
        assert_eq!(
            try_walk(CLEAN, &rules()),
            Ok((7, 2, 2, 0)),
            "the walk refused a region that is exactly what it exists to allow, so every \
             refusal below proves nothing"
        );
    }

    /// **The defect the offset check exists for.** A module closed by an
    /// INDENTED brace, a `pub fn` at file scope after it, and a column-0 `}`
    /// further down to rebalance. Perfectly balanced source, no lexer trick:
    /// the line walk sees `modules == closes` and `depth == 0` and the `pub
    /// fn` ships. Measured surviving at 2211 lib / 217 bin / 0 failed / 0
    /// warnings with eight planted at once, in both profiles.
    #[test]
    fn the_shared_walk_refuses_a_module_closed_by_an_indented_brace() {
        let region = "#[cfg(test)]\nmod a {\n    fn f() {}\n    }\n    \
                      pub fn planted(x: u64) -> u64 { x }\n    #[allow(dead_code)]\n    \
                      mod filler {\n}\n";
        let why = try_walk(region, &rules())
            .expect_err("the walk accepted a module whose real close is an indented brace");
        assert!(
            why.contains("is not the brace that closes the module it appears to close"),
            "the walk refused this region, but for the wrong reason -- the offset check is \
             not what fired: {why}"
        );
    }

    /// Liveness control for the test above, at the IDENTICAL site: the same
    /// payload written at column 0 must also be refused, and by the OTHER
    /// rule. A walk that refused everything would pass the test above without
    /// the offset check existing at all.
    #[test]
    fn the_column_zero_form_of_the_same_payload_is_refused_by_the_line_rule() {
        let region =
            "#[cfg(test)]\nmod a {\n    fn f() {}\n}\npub fn planted(x: u64) -> u64 { x }\n";
        let why = try_walk(region, &rules())
            .expect_err("the walk accepted a column-0 `pub fn` below the cut");
        assert!(
            why.contains("top-level source below the cut") && why.contains("NOTE-TOP."),
            "the column-0 payload was refused by something other than the line rule, and the \
             caller's own note was not spliced into the message: {why}"
        );
    }

    #[test]
    fn the_shared_walk_refuses_an_indented_item_between_modules() {
        let region = format!("{CLEAN}    struct Sneaked(u8);\n");
        assert!(
            try_walk(&region, &rules()).is_err(),
            "the walk accepted an INDENTED top-level item, which a column-0-only filter misses"
        );
    }

    /// The indentation test is a rule of its own and not a restatement of the
    /// opener predicate. The test above is refused whether or not the
    /// indentation is checked, because `struct Sneaked(u8);` is not a module
    /// opener either way -- so on its own it leaves the indentation rule
    /// unmeasured. Measured: deleting `line.starts_with(char::is_whitespace)`
    /// from the shared walk SURVIVED the whole suite at 2222 passed / 0
    /// failed / 0 warnings. This is the shape that kills it -- an INDENTED,
    /// gated module opener, which the predicate accepts and only the
    /// indentation rule refuses.
    #[test]
    fn the_shared_walk_refuses_an_indented_module_opener() {
        let region = format!("{CLEAN}#[cfg(test)]\n    mod sneaky {{\n    }}\n");
        let why = try_walk(&region, &rules())
            .expect_err("the walk accepted an INDENTED module opener at file scope");
        assert!(
            why.contains("top-level source below the cut"),
            "the indented opener was refused by something other than the indentation rule: {why}"
        );
        // Control: the identical module at column 0 is accepted, so the
        // refusal above is about the indentation and nothing else.
        let at_column_zero = format!("{CLEAN}#[cfg(test)]\nmod sneaky {{\n}}\n");
        assert!(
            try_walk(&at_column_zero, &rules()).is_ok(),
            "control: the walk refuses the same module written at column 0, so the test above \
             is not measuring indentation"
        );
    }

    #[test]
    fn the_shared_walk_refuses_an_ungated_module_which_ships() {
        let region = format!("{CLEAN}mod shipped {{\n}}\n");
        let why = try_walk(&region, &rules()).expect_err("an ungated module below the cut ships");
        assert!(
            why.contains("not test-gated") && why.contains("NOTE-UNGATED."),
            "the ungated module was refused by the wrong rule, or the caller's note was not \
             spliced in: {why}"
        );
    }

    /// The `gate_at_column_zero` knob is real in both directions. An indented
    /// gate arms the walk under the loose rule and does not under the strict
    /// one -- which is what stops a `#[cfg(test)]` written inside a function
    /// body from arming the walk for the module after it.
    #[test]
    fn the_column_zero_gate_rule_refuses_an_indented_gate_the_loose_one_accepts() {
        let region = "    #[cfg(test)]\nmod a {\n}\n";
        assert!(
            try_walk(region, &rules()).is_ok(),
            "control: the loose rule is supposed to accept an indented gate"
        );
        let strict = WalkRules { gate_at_column_zero: true, ..rules() };
        let why = try_walk(region, &strict)
            .expect_err("the strict rule accepted an indented gate as arming the next module");
        // Under the strict rule the indented gate is not a gate at all, so it
        // falls through to the line rule and is refused as what it now is: an
        // indented top-level line below the cut.
        assert!(why.contains("top-level source below the cut"), "{why}");
    }

    /// The `gated_at_start` knob is real: a region that begins at the module
    /// opener, with its gate above the cut, is only walkable when the caller
    /// says so.
    #[test]
    fn the_gated_at_start_knob_decides_whether_the_first_module_needs_a_gate_inside_the_region() {
        let region = "mod a {\n}\n";
        assert!(
            try_walk(region, &rules()).is_err(),
            "control: with `gated_at_start: false` an ungated first module must be refused"
        );
        let armed = WalkRules { gated_at_start: true, ..rules() };
        assert_eq!(try_walk(region, &armed), Ok((2, 1, 1, 0)));
    }

    /// The `string_lines` allowance is real, and is an allowance and not a
    /// hole: it admits the exact line and nothing else.
    #[test]
    fn the_string_line_allowance_admits_exactly_the_lines_it_names() {
        let region = "#[cfg(test)]\nmod a {\n    let _ = \"\nnot source\n\";\n}\n";
        assert!(
            try_walk(region, &rules()).is_err(),
            "control: an unlisted column-0 line inside a module must be refused"
        );
        let allowed = WalkRules { string_lines: &["not source", "\";"], ..rules() };
        assert!(try_walk(region, &allowed).is_ok());
        let wrong = WalkRules { string_lines: &["not sourc", "\";"], ..rules() };
        assert!(
            try_walk(region, &wrong).is_err(),
            "the allowance matched a line it does not name, so it is a prefix test and not an \
             exact one"
        );
    }

    /// The opener predicate really is the CALLER's, not this module's. Each
    /// caller keeps its own copy so that widening it desynchronizes the walk
    /// from [`column_zero_module_openers`]; if the walk quietly used
    /// [`is_module_opener`] regardless, that property would be gone and this
    /// test would fail.
    #[test]
    fn the_walk_uses_the_predicate_the_caller_handed_it() {
        fn nothing_is_an_opener(_: &str) -> bool {
            false
        }
        let refuses = WalkRules { is_module_opener: nothing_is_an_opener, ..rules() };
        assert!(
            try_walk(CLEAN, &refuses).is_err(),
            "the walk accepted a module opener although the predicate it was handed rejects \
             every line -- it is using its own predicate and the callers' copies are decoration"
        );
        fn everything_is_an_opener(_: &str) -> bool {
            true
        }
        let permits = WalkRules { is_module_opener: everything_is_an_opener, ..rules() };
        assert!(
            try_walk("#[cfg(test)]\nnot a module at all {\n}\n", &permits).is_ok(),
            "control: the walk did not consult the handed-in predicate in the accepting \
             direction either"
        );
    }

    /// The panicking wrapper really panics, and carries the same message.
    #[test]
    #[should_panic(expected = "top-level source below the cut")]
    fn the_panicking_wrapper_panics_with_the_refusal() {
        walk("#[cfg(test)]\nmod a {\n}\npub fn planted() {}\n", &rules());
    }

    // -- `main.rs`, walked from here ----------------------------------------

    /// **The same walk over `main.rs`, driven from the library.**
    ///
    /// `main.rs` carries its own below-the-cut guard and that guard really
    /// runs: measured, the column-0 payload fails
    /// `startup_shape_tests::nothing_but_gated_test_modules_lives_below_the_guards_cut`
    /// in the binary's test target. But its walk is a LINE walk, and the
    /// payload that closes its last module with an INDENTED brace and
    /// rebalances further down is invisible to a line walk -- measured
    /// SURVIVING at 2211 lib / 217 bin / 0 failed / 0 warnings in both
    /// profiles. Only a real brace count kills it, and `main.rs` cannot reach
    /// one:
    ///
    /// * `deskwarden::below_cut` does not exist in the binary's universe. The
    ///   library is compiled WITHOUT `cfg(test)` when the binary links it,
    ///   and this module is `#[cfg(test)]` at its declaration in `lib.rs`.
    ///   Ungating it there would ship [`match_brace`] in production, which is
    ///   the property `lib.rs` states and `job_object` re-checks.
    /// * Re-declaring it in `main.rs` as `#[path = "below_cut.rs"] mod
    ///   below_cut;` is refused by
    ///   `job_object::the_two_job_bearing_modules_can_start_a_child_only_through_this_one`,
    ///   which bans `#[path]` and `include!` crate-wide because between them
    ///   they are the one ingredient every module-smuggling mutant that
    ///   module has measured requires. Measured: it fails. And the
    ///   `#[cfg(test)]` such a declaration carries sits near the TOP of
    ///   `main.rs`, which moves the cut every
    ///   `production_only(include_str!("main.rs"))` slice in the crate takes
    ///   -- two further tests fail on it. Both refusals are correct and
    ///   neither was weakened to make room for this.
    /// * Copying [`match_brace`] into `main.rs` is precisely the defect this
    ///   file was created to remove.
    ///
    /// So the walk reaches into `main.rs` from here instead. This runs in the
    /// LIBRARY's test target rather than the binary's, and `main.rs` is now
    /// covered by two independent instances: its own line walk, which still
    /// catches the column-0 form, and this one, which catches the indented
    /// close as well.
    #[test]
    fn main_rs_carries_nothing_but_gated_test_modules_below_its_cut() {
        // Split for the same reason `main.rs` splits its own copy: an unsplit
        // literal here is harmless, but keeping the spelling identical means
        // the two constants can be compared by eye.
        const MARKER: &str = concat!("mod te", "sts {");
        const GATE: &str = concat!("#[cfg(", "test)]");
        const RULES: WalkRules = WalkRules {
            gate: GATE,
            // The cut lands ON the module opener, so the gate for the first
            // module is above the region and is asserted separately below.
            gated_at_start: true,
            gate_at_column_zero: false,
            is_module_opener,
            string_lines: &[],
            top_level_item_note:
                "Every source guard in `main.rs` slices at the test-module marker and reads only \
                 what is above it, and the library's guards that read `main.rs` as source do the \
                 same, so an item down here is read by nothing.",
            ungated_module_note:
                "A `pub(crate) mod ext { .. }` written down there is the same escape, one `mod` \
                 deep.",
        };

        let source = include_str!("main.rs");

        // The cut is where `main.rs`'s own guards believe it is: exactly one
        // occurrence, at the start of a line, gated immediately above.
        assert_eq!(
            source.matches(MARKER).count(),
            1,
            "control: {MARKER:?} occurs {} times in `main.rs`. Every guard there takes the \
             FIRST one, so a second copy above the real test modules moves the cut upwards \
             and empties them",
            source.matches(MARKER).count()
        );
        let cut = source.find(MARKER).expect("counted just above");
        assert!(
            cut > 0 && source.as_bytes()[cut - 1] == b'\n',
            "the cut landed in the MIDDLE of a line, so the marker was matched inside a \
             comment or a string rather than at a real declaration"
        );
        assert!(
            source[..cut].trim_end().ends_with(GATE),
            "the module the cut lands on is not preceded by {GATE:?}, so the region below the \
             cut is not test-only to begin with"
        );

        let region = &source[cut..];
        let (visited, modules, closes, depth) = walk(region, &RULES);
        assert!(
            visited > 100,
            "control: the walk visited only {visited} lines below `main.rs`'s cut, so the \
             slice is empty and this test proves nothing"
        );
        assert_eq!(
            (modules, closes, depth),
            // Six since the direct-REST branch met the process split, and
            // each side of that merge had four: the two that were always
            // here, `bw_serve_gate` (the rule that no path reaches
            // `bw serve` without the backend policy having answered) and
            // `vault_backend_choice_tests` (the startup decision that picks a
            // backend) from one side, `no_door_assigns_the_pending_search_pin`
            // and `the_daemon_never_blocks_on_a_ui_process_pin` from the
            // other. All are `#[cfg(test)] mod` at column 0 below the cut,
            // which is exactly what this walk counts.
            //
            // **EIGHT since the live settings channel.** The window that can
            // hand the daemon a preferences edit without exiting added
            // `the_live_settings_channel` (the loop's ordering and the
            // doorbell's ownership) and `the_one_settings_write_back`
            // (`apply_edited_settings`, driven directly over a real disk
            // cache). Raised deliberately, and it asserts MORE than six did:
            // two further modules must now be clean, gated, column-0 blocks.
            //
            // **NINE since a missing `bw.exe` stopped ending the launch.**
            // `a_missing_cli_is_never_fatal` holds the recovery driven over
            // its `fn`-pointer seam, and the rule that no backend failure
            // reaches `fatal_startup_error` without first asking whether the
            // binary is merely absent. Raised for the same reason again: one
            // further module must now be a clean, gated, column-0 block.
            (9, 9, 0),
            "below `main.rs`'s cut there are no longer exactly nine opened-and-closed test \
             modules: {modules} opened, {closes} closed, ending at depth {depth}. THIS IS A \
             REAL, NON-ENVIRONMENTAL FAILURE -- this test does no I/O and is not among the \
             mockito-port-collision failures documented for this machine. Do not step over \
             it: either a module below `main.rs`'s cut was added, removed, or stopped being a \
             clean gated block, or verify every module down there is genuinely \
             `#[cfg(test)]`-gated and test-only before touching this number."
        );
        assert_eq!(
            modules,
            column_zero_module_openers(region),
            "the walk opened {modules} modules but there are {} column-0 gated module openers \
             below `main.rs`'s cut",
            column_zero_module_openers(region)
        );

        // The payload `main.rs`'s own line walk cannot see: its last module
        // closed by an INDENTED brace, a `pub fn` at file scope after it, and
        // a column-0 `}` further down to rebalance the count.
        let lf = source.replace("\r\n", "\n");
        let balanced = format!(
            "{}    }}\n    pub fn sneaked(x: u64) -> u64 {{ x }}\n    \
             #[allow(dead_code)]\n    mod filler {{\n}}\n",
            lf.strip_suffix("}\n")
                .expect("`main.rs` ends with a column-0 closing brace")
        );
        let cut_of = balanced.find(MARKER).expect("the marker survives the append");
        assert!(
            try_walk(&balanced[cut_of..], &RULES).is_err(),
            "the walk accepted `main.rs`'s last test module closed by an INDENTED brace with a \
             `pub fn` at file scope after it -- the payload measured surviving the whole suite \
             green"
        );
        // Liveness control at the identical site: the column-0 form of the
        // same payload, which `main.rs`'s own guard also catches.
        //
        // The cut is recomputed IN the string being sliced. `cut` above is a
        // byte offset into `source`, which `include_str!` reads with the
        // working tree's line endings; `column_zero` is built from `lf`, which
        // is two bytes shorter per line. Measured on a CRLF checkout: `cut` is
        // 395685 and the marker in the LF copy is at 388330 -- 7355 bytes off,
        // landing the slice in the middle of a test function body, so the walk
        // returned `Err` for garbage rather than for the appended `pub fn`. On
        // a pure-LF tree the delta is zero and it passed, so the control could
        // never go red on either configuration for the reason it names.
        let column_zero = format!("{lf}pub fn sneaked(x: u64) -> u64 {{ x }}\n");
        let cut_of_column_zero =
            column_zero.find(MARKER).expect("the marker survives the append");
        assert!(
            column_zero.as_bytes()[cut_of_column_zero - 1] == b'\n',
            "control: the recomputed cut landed mid-line, so the slice below is not a \
             region boundary and its refusal would be about garbage"
        );
        let why = try_walk(&column_zero[cut_of_column_zero..], &RULES)
            .expect_err("control: the walk accepted a column-0 `pub fn` below `main.rs`'s cut");
        assert!(
            why.contains("top-level source below the cut"),
            "control: the column-0 `pub fn` was refused, but not by the top-level line rule \
             this control names: {why}"
        );
        // And the unmutated file must still pass, or both refusals above are
        // a walk that refuses everything.
        assert!(
            try_walk(&lf[lf.find(MARKER).expect("the marker is in the LF copy")..], &RULES).is_ok(),
            "control: the walk refuses `main.rs` as it actually is"
        );
    }
}

/// **Every Rust source file in this crate, walked, in one test.**
///
/// # Why this module exists
///
/// Fifteen per-file guards were written, converted and repaired over roughly
/// ten rounds, and a census at `0097883` found that **35 of the crate's 49
/// then-source files carried no below-the-cut guard at all**. The measured
/// consequence, in `vault_window/item_list.rs` -- 5,200 lines, no guard, and a
/// comment pointing at `vault_window/mod.rs`'s guard, which reads
/// `include_str!("mod.rs")` and never looks at `item_list.rs`:
///
/// ```text
/// pub fn shipped_il(x: u64) -> u64 { x.wrapping_mul(97) }
/// ```
///
/// appended at column 0 to EOF SURVIVED the whole suite in debug AND release
/// at 2225 lib / 217 bin / 6 ignored / 0 failed / 0 warnings, and appeared
/// three times in the lib's DEBUG LLVM IR. It shipped.
///
/// The per-file approach lost that race because it is an ENUMERATION: a guard
/// exists for a file only if somebody wrote one, and the files nobody thought
/// of are exactly the files nobody read. So the file list here is DERIVED from
/// the tree -- every `.rs` under the crate directory, cross-checked against a
/// `git ls-files` oracle -- and the walk is the same
/// [`crate::below_cut::match_brace`] the per-file guards use.
///
/// # What is asserted, and for which files
///
/// * A file whose cut exists is walked from that cut: below it there may be
///   NOTHING but gated, column-0 module blocks, blank lines and comments.
/// * A file that INTERLEAVES -- production code below its first test module,
///   which three files in this crate legitimately do -- cannot satisfy that,
///   so it is walked from its LAST gated module instead: after that module's
///   real closing brace only blanks and comments may follow. The set of
///   interleaving files is not a knob: it is compared against a written list
///   and a file that joins it reds the suite.
/// * A file with no cut at all is NOT skipped. A silent skip is how 35 files
///   were missed. It is asserted to carry no test at all, which is the only
///   reading under which "nothing lives below its cut" is true of a file that
///   has no cut.
#[cfg(test)]
pub(crate) mod every_file_in_the_crate {
    use super::{is_module_opener, match_brace};

    /// The test gate, assembled so this module does not contain a column-0
    /// occurrence of the string it searches for.
    const GATE: &str = concat!("#[cfg", "(test)]");

    /// The opening words of the refusal that says an item sits at file scope
    /// below a cut. [`verdict`] re-words that one refusal into the "this file
    /// interleaves" message, so it is spelled once rather than at every
    /// comparison.
    const TOP_LEVEL: &str = "top-level source below the cut";

    /// The opening words of the refusal that says something shares a line with
    /// a test module's closing brace. A DIFFERENT prefix from [`TOP_LEVEL`]
    /// because the two are re-worded differently -- not because either is
    /// tolerated. Neither is, and there is no longer any tolerance to grant.
    /// See [`walk`].
    const CLOSING_LINE: &str = "source on the line that closes a test module below the cut";

    /// Every line of `source` as `(byte offset, line without its terminator)`.
    ///
    /// Offsets are into `source` itself and are used to slice `source`
    /// itself. An offset taken in one copy of a file and used in another --
    /// CRLF working-tree bytes against an LF `replace` -- was measured 7355
    /// bytes off in this crate, and passed silently forever.
    fn numbered(source: &str) -> Vec<(usize, &str)> {
        let mut out = Vec::new();
        let mut at = 0usize;
        for raw in source.split_inclusive('\n') {
            out.push((at, raw.trim_end_matches('\n').trim_end_matches('\r')));
            at += raw.len();
        }
        out
    }

    /// `true` for a line that is exactly one outer attribute and nothing
    /// else.
    ///
    /// **Not `starts_with("#[")`.** That form skips the whole line, and
    /// `#[allow(dead_code)] pub fn shipped() -> u64 { 7 }` is one line
    /// beginning with `#[`. Requiring the line to END at the attribute's
    /// bracket refuses that shape as top-level source, which is what it is.
    fn is_lone_attribute(trimmed: &str) -> bool {
        trimmed.starts_with("#[") && trimmed.ends_with(']')
    }

    /// The byte offset of the gate line of the FIRST gated, column-0 module
    /// block in `source`, or `None` when the file has none.
    ///
    /// **Not the first gate in the file, and not the first gate that begins a
    /// line either.** `own_cut_index` in `send_ui.rs` established that the
    /// gate must at least begin a line, because that file spells it inside a
    /// doc comment 490 lines above its first test module; that rule is kept
    /// here and is the `line == GATE` comparison below. But a line-start gate
    /// is still not always a cut: `lib.rs` gates a `pub mod` DECLARATION,
    /// `accounts.rs` and `vault_bridge.rs` gate test-only helper `fn`s, and
    /// `detail.rs` gates a `use`. Cutting at those put the walk in the middle
    /// of production code, where it refused the next production line and
    /// reported a violation that was really its own cut being wrong -- eleven
    /// files did that when this rule was first measured. The cut is where the
    /// file's test HALF begins, so it is the gate of the first gated module.
    fn cut_index(source: &str) -> Option<usize> {
        module_blocks(source).first().map(|b| b.gate_at)
    }

    /// One gated, column-0 module block: where its gate line starts, and where
    /// its `mod NAME {` line starts, with that line's text.
    struct Block<'a> {
        gate_at: usize,
        opener_at: usize,
        opener: &'a str,
    }

    /// Every gate line in `source` that opens a column-0 module block, in
    /// order, with the opener it gates.
    fn module_blocks(source: &str) -> Vec<Block<'_>> {
        let lines = numbered(source);
        let mut out = Vec::new();
        for (i, &(offset, line)) in lines.iter().enumerate() {
            if line != GATE {
                continue;
            }
            // Between the gate and the module it gates there may be further
            // attributes and comments, and nothing else.
            let opener = lines[i + 1..].iter().find(|(_, l)| {
                let t = l.trim();
                !(t.is_empty() || t.starts_with("//") || is_lone_attribute(t))
            });
            if let Some(&(opener_at, l)) = opener {
                if !l.starts_with(char::is_whitespace) && is_module_opener(l.trim()) {
                    out.push(Block { gate_at: offset, opener_at, opener: l });
                }
            }
        }
        out
    }

    /// `true` when what follows a test module's closing brace ON ITS OWN LINE
    /// cannot be source: nothing, a `//` comment, or one complete `/* .. */`
    /// block comment.
    ///
    /// The block-comment arm is not a widening. A payload written inside a
    /// block comment does not compile into anything, so it is not a payload;
    /// what the arm buys is that `} /* inert */` -- legal, inert code -- no
    /// longer reds. The check is deliberately not `starts_with("/*") &&
    /// ends_with("*/")`, which would tolerate
    /// `/* */ pub fn shipped() {} /* */`: the FIRST `*/` must be the end of
    /// the line, so nothing outside a comment can follow.
    fn after_close_is_inert(after: &str) -> bool {
        after.is_empty()
            || after.starts_with("//")
            || (after.starts_with("/*")
                && after.match_indices("*/").next().is_some_and(|(i, _)| i + 2 == after.len()))
    }

    /// **The closing LINE of every gated module in the file, read.**
    ///
    /// Returns how many closes were checked, or the reason one of them carries
    /// source.
    ///
    /// # The hole this exists for
    ///
    /// [`walk`] reads the remainder of a module's closing brace line, and that
    /// rule is sound -- where it runs. It did not run. In an INTERLEAVED file
    /// (`app.rs`, `theme.rs`, `injector/sequence.rs`) the walk from the cut
    /// aborts at the first [`TOP_LEVEL`] production line, so it never reaches
    /// the closing-brace line of any module after that one, and [`verdict`]
    /// then re-walks only from the LAST gate. Every non-last module close in
    /// those three files -- nine of the crate's one hundred and sixty-one --
    /// was read by nothing. Measured: `src/app.rs:1980`, the close of the
    /// second of that file's five gated modules,
    ///
    /// ```text
    /// }    ->    } pub fn shipped_eol_mid(x: u64) -> u64 { x.wrapping_mul(97) }
    /// ```
    ///
    /// one line changed, no line added, brace balance unchanged, no edit to
    /// this file at all: SURVIVED the whole suite in debug and release at 2247
    /// lib / 217 bin / 6 ignored / 0 failed / 0 warnings, and appeared three
    /// times in the lib's DEBUG LLVM IR. The byte-identical payload on the
    /// LAST module's close was refused by [`CLOSING_LINE`], which is what a
    /// rule that is right and unreached looks like.
    ///
    /// So this walk does not depend on where any other walk stops. It goes
    /// straight to every gated module block in the file, brace-matches it, and
    /// reads the tail of the line its real close sits on -- in EVERY file,
    /// interleaved or not, for EVERY module rather than the last one.
    fn closing_brace_lines_carry_no_source(
        source: &str,
        blocks: &[Block<'_>],
    ) -> Result<usize, String> {
        // A REAL per-close counter, incremented once per close actually read.
        //
        // This used to end `Ok(blocks.len())`, which is the length of the very
        // vector the caller compares the result against: `verdict` sets
        // `gates = blocks.len()` and `closes_read =
        // closing_brace_lines_carry_no_source(source, &blocks)?`, so both sides
        // of the crate-wide `assert_eq!(closes_read, gates_seen)` were the same
        // `Vec::len()` and the equality could not fail. Measured: mutating the
        // loop header to `for block in blocks.iter().take(1)` -- which then
        // reads one of `app.rs`'s five closes -- STILL reported equality. That
        // mutant was killed, but by the per-module liveness payload, never by
        // the control written to catch exactly it.
        let mut checked = 0usize;
        for block in blocks {
            let Some(rel) = block.opener.rfind('{') else {
                return Err(format!(
                    "the module opener {:?} does not end in an opening brace",
                    block.opener
                ));
            };
            let after_at = match_brace(source, block.opener_at + rel) + 1;
            let line_end = source[after_at..].find('\n').map_or(source.len(), |n| after_at + n);
            let after = source[after_at..line_end].trim();
            if !after_close_is_inert(after) {
                return Err(format!(
                    "{CLOSING_LINE}: {after:?} at byte {after_at}, sharing a line with the \
                     `}}` that really closes the module opened at {:?}. Everything to the \
                     right of a test module's closing brace is compiled into the shipped \
                     binary and is the one place below a cut that no line walk looks at: \
                     measured SURVIVING the whole suite in both profiles, on the close of a \
                     MID-FILE module of an interleaved file, with three occurrences in the \
                     lib's DEBUG LLVM IR. Move it above the test modules.",
                    block.opener
                ));
            }
            checked += 1;
        }
        Ok(checked)
    }

    /// The closing-line count is a COUNT and not a restatement of its input.
    ///
    /// The crate-wide `assert_eq!(closes_read, gates_seen)` compares a sum of
    /// this function's return values against a sum of `module_blocks(..).len()`
    /// values. While this function ended `Ok(blocks.len())` those were the same
    /// number by construction and the assertion could not fail for any mutation
    /// whatsoever. This test is local, synthetic and cheap, so the property is
    /// measured here rather than only at the end of a thirteen-second sweep:
    /// a walk that reads fewer closes than the file has modules must return a
    /// smaller number.
    #[test]
    fn the_closing_line_walk_returns_how_many_closes_it_actually_read() {
        let source = concat!(
            "#[cfg", "(test)]\nmod a {\n}\n",
            "#[cfg", "(test)]\nmod b {\n    fn f() {}\n}\n",
            "#[cfg", "(test)]\nmod c {\n}\n",
        );
        let blocks = module_blocks(source);
        assert_eq!(blocks.len(), 3, "control: the fixture does not carry three gated modules");
        assert_eq!(
            closing_brace_lines_carry_no_source(source, &blocks),
            Ok(3),
            "the closing-line walk did not report one close per module"
        );
        // The mutation the crate-wide equality was blind to, written out: a
        // walk that reads only the FIRST block must report 1, not 3. If this
        // function ever returns `blocks.len()` again, these two disagree.
        assert_eq!(
            closing_brace_lines_carry_no_source(source, &blocks[..1]),
            Ok(1),
            "the closing-line walk reported the length of the whole file's block list rather \
             than the number of closes it read, so the crate-wide equality it feeds is an \
             identity and cannot fail"
        );
        assert_eq!(closing_brace_lines_carry_no_source(source, &[]), Ok(0));
    }

    /// **A file that interleaves is REFUSED, not counted.**
    ///
    /// This is the property that replaced the interleaving tolerance, and it
    /// is measured here rather than only at the end of the crate-wide sweep,
    /// because the sweep can only see it if some real file is shaped that way
    /// -- and after this round none is. A guard whose refusal branch no
    /// tree ever reaches is a guard that can be deleted in one edit with the
    /// suite still green, which is exactly how the previous rule in this area
    /// was lost.
    ///
    /// Synthetic, local and cheap, and driven through the SAME [`verdict`]
    /// the sweep drives, so weakening the rule to bring back a tolerance --
    /// swallowing the [`TOP_LEVEL`] refusal, walking only the tail, returning
    /// `Ok` for the interleaving shape -- reds here immediately.
    #[test]
    fn an_interleaving_file_is_refused_rather_than_tolerated() {
        // Production BETWEEN two gated modules: the exact shape the three
        // exempted files had, and the region a measured `pub fn` shipped out
        // of this crate from.
        let interleaving = concat!(
            "fn above() {}\n",
            "#[cfg", "(test)]\nmod a {\n    #[test]\n    fn t() {}\n}\n",
            "pub fn shipped_between(x: u64) -> u64 { x.wrapping_mul(97) }\n",
            "#[cfg", "(test)]\nmod b {\n    #[test]\n    fn t() {}\n}\n",
        );
        let Err(why) = verdict(interleaving) else {
            panic!(
                "an interleaving file was ACCEPTED, so the tolerance this round deleted is \
                 back and a `pub fn` between two gated modules ships again"
            );
        };
        assert!(
            why.contains("shipped_between"),
            "the refusal does not name the item that ships, so it is refusing something \
             else: {why}"
        );
        assert!(
            why.contains("INTERLEAVES"),
            "the refusal does not tell the reader what is wrong with the file's SHAPE, which \
             is the whole remedy: {why}"
        );

        // Positive control at the identical site: the same bytes with the
        // first module moved below the production line -- the pure relocation
        // this round applied to `app.rs`, `theme.rs` and
        // `injector/sequence.rs` -- is ACCEPTED. Without this, a `verdict`
        // that refused every file would pass the assertion above.
        let reordered = concat!(
            "fn above() {}\n",
            "pub fn shipped_between(x: u64) -> u64 { x.wrapping_mul(97) }\n",
            "#[cfg", "(test)]\nmod a {\n    #[test]\n    fn t() {}\n}\n",
            "#[cfg", "(test)]\nmod b {\n    #[test]\n    fn t() {}\n}\n",
        );
        let found = verdict(reordered)
            .expect("the reordered file is the shape every file in this crate now has")
            .expect("the reordered fixture has a cut");
        assert_eq!(found.gates, 2, "control: the fixture does not carry two gated modules");
        assert_eq!(found.closes_read, 2, "control: both closing lines must still be read");
    }

    /// Walk `region` -- a file from one of its cuts to EOF -- and require it
    /// to be a sequence of gated, column-0 module blocks and nothing else.
    ///
    /// Returns the number of module blocks, or the reason the region is not
    /// that shape.
    ///
    /// # Why the module's interior is not walked at all
    ///
    /// Every per-file walk in this crate reads the lines INSIDE a test module
    /// looking for the column-0 line that closes it, which forces each of
    /// them to carry a list of the column-0 lines that are really the
    /// contents of a string literal -- an enumeration, per file, that goes
    /// stale. It is unnecessary. The interior of a gated module cannot ship
    /// whatever is written in it, and [`match_brace`] already says exactly
    /// where the module ends, so this walk JUMPS from a module opener to the
    /// byte past its real close and looks at nothing in between. A module
    /// closed early by an INDENTED brace -- the balanced payload that beat
    /// the line walks -- therefore lands its `pub fn` in the region this walk
    /// is looking at, not in the region it is skipping.
    ///
    /// # Why the jump is not allowed to overshoot to end-of-line
    ///
    /// The jump resumes at the next LINE whose start offset is at or past the
    /// byte after the close. The closing brace's OWN line starts before that
    /// byte, so resuming line-wise skips the whole of it -- and everything to
    /// the right of a test module's `}` is then read by nothing at all. One
    /// line changed, no line added, brace balance provably unchanged:
    ///
    /// ```text
    /// }    ->    } pub fn shipped_eol(x: u64) -> u64 { x.wrapping_mul(97) }
    /// ```
    ///
    /// Measured on the tree before this rule existed: SURVIVED the full suite
    /// in `vault_window/item_list.rs` in DEBUG and RELEASE at 2243 lib / 217
    /// bin / 6 ignored / 0 failed / 0 warnings, and SURVIVED in `accounts.rs`,
    /// with three occurrences of the symbol in the lib's DEBUG LLVM IR. It
    /// shipped. So the remainder of the closing brace's line is read, and
    /// refused unless it is blank or a comment -- the same rule
    /// [`is_lone_attribute`] already applies to `#[allow(dead_code)] pub fn`
    /// written on one line, for the same reason.
    ///
    /// The refusal carries [`CLOSING_LINE`] and not [`TOP_LEVEL`] on purpose.
    /// A [`TOP_LEVEL`] refusal is how an interleaving file is recognised, and
    /// is turned by [`verdict`] into a refusal that says so and names the
    /// item that ships. The two are distinct because the remedy is distinct:
    /// an interleaving file is fixed by MOVING a test module, this shape by
    /// deleting what shares the brace's line. Neither is tolerated anywhere
    /// -- there was once a three-file tolerance for the interleaving shape,
    /// and it is gone.
    ///
    /// **This check here is the second reader of that line, not the only one.**
    /// It runs only for the modules this walk REACHES, and the walk from a cut
    /// stops at the first [`TOP_LEVEL`] line, so in an interleaved file it
    /// reaches almost none of them -- which is how a payload on a mid-file
    /// module's close shipped. Every module's closing line, in every file, is
    /// read by [`closing_brace_lines_carry_no_source`] before this walk is
    /// entered at all.
    fn walk(region: &str) -> Result<usize, String> {
        let mut gated = false;
        let mut modules = 0usize;
        let mut resume = 0usize;
        for (offset, line) in numbered(region) {
            if offset < resume {
                continue;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with("//") {
                continue;
            }
            if line == GATE {
                gated = true;
                continue;
            }
            if is_lone_attribute(trimmed) {
                continue;
            }
            if !line.starts_with(char::is_whitespace) && is_module_opener(trimmed) {
                if !gated {
                    return Err(format!(
                        "the module {line:?} below the cut is not test-gated, so it SHIPS, \
                         and it ships in the half of the file no reviewer reads"
                    ));
                }
                let Some(rel) = line.rfind('{') else {
                    return Err(format!(
                        "the module opener {line:?} does not end in an opening brace"
                    ));
                };
                resume = match_brace(region, offset + rel) + 1;
                // The jump lands one byte past the real close. Read the REST
                // of that byte's line before resuming line-wise, because the
                // line-wise resume is about to skip it. See the note above.
                let line_end =
                    region[resume..].find('\n').map_or(region.len(), |n| resume + n);
                let after = region[resume..line_end].trim();
                if !after_close_is_inert(after) {
                    return Err(format!(
                        "{CLOSING_LINE}: {after:?} at byte {resume} of the region, sharing a \
                         line with the `}}` that really closes the module opened at {line:?}. \
                         The walk jumps from a module opener to the byte past its real close \
                         and then resumes at the next LINE, so the remainder of the closing \
                         brace's own line is the one place below a cut that nothing reads. An \
                         item written there is compiled into the shipped binary: measured \
                         SURVIVING the whole suite in both profiles and appearing three times \
                         in the lib's DEBUG LLVM IR. Move it above the test modules."
                    ));
                }
                modules += 1;
                gated = false;
                continue;
            }
            return Err(format!(
                "{TOP_LEVEL}: {line:?} at byte {offset} of the region. An \
                 item down here is compiled into the shipped binary and sits in the half of \
                 the file every guard in this crate stops reading at. Move it above the test \
                 modules."
            ));
        }
        Ok(modules)
    }

    /// Every `.rs` file under the crate directory, as absolute paths, sorted.
    ///
    /// `target/` is pruned by name at every depth: the user runs the built
    /// application out of `deskwarden/target`, it holds no source of ours,
    /// and reading it is minutes of I/O.
    fn rust_sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                rust_sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// The `.rs` files git says this crate has, or `None` where git cannot
    /// be run -- a release tarball, a `git archive` export.
    ///
    /// Tracked UNION untracked-and-not-ignored, because this repository is
    /// developed write-then-compile-before-`add` and a file that is not yet
    /// added is still a file that compiles into the binary.
    fn git_listed(dir: &std::path::Path) -> Option<Vec<std::path::PathBuf>> {
        fn list(dir: &std::path::Path, extra: &[&str]) -> Option<Vec<String>> {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["ls-files", "-z"])
                .args(extra)
                .output()
                .ok()?;
            out.status.success().then(|| {
                out.stdout
                    .split(|b| *b == 0)
                    .filter(|s| !s.is_empty())
                    .map(|s| String::from_utf8_lossy(s).into_owned())
                    .collect()
            })
        }
        let mut all = list(dir, &[])?;
        all.extend(list(dir, &["--others", "--exclude-standard"]).unwrap_or_default());
        let mut out: Vec<std::path::PathBuf> = all
            .iter()
            .filter(|rel| rel.ends_with(".rs") && !rel.starts_with("target/"))
            .map(|rel| dir.join(rel))
            .filter(|p| p.is_file())
            .collect();
        out.sort();
        out.dedup();
        Some(out)
    }

    /// `path` relative to the crate directory, with forward slashes.
    fn relative(crate_dir: &std::path::Path, path: &std::path::Path) -> String {
        path.strip_prefix(crate_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// What the property says about one file's TEXT: how many gated module
    /// blocks it carries, and how many of their closing lines were read.
    ///
    /// **There is no `interleaved` flag any more.** A file that carries
    /// production between two of its gated test modules is now a REFUSAL, not
    /// a second kind of verdict -- see [`verdict`].
    struct Verdict {
        gates: usize,
        /// How many module closing LINES were read in this file. Carried out
        /// so the sweep can require it to equal `gates`: a closing-line check
        /// that quietly read fewer closes than the file has modules is
        /// exactly the defect this round was lost to.
        closes_read: usize,
    }

    /// **The whole property, applied to one file's text.**
    ///
    /// `Ok(None)` for a file with no cut -- the caller still has an assertion
    /// to make about those and must not treat them as a skip. `Err` is the
    /// property being violated, in the words the violation deserves.
    ///
    /// This is the one place the rule lives, so the liveness controls below
    /// drive exactly the code the crate-wide sweep drives, over exactly the
    /// same files. The previous version of this test wrote the sweep inline
    /// and then proved liveness against ONE hardcoded victim path -- an
    /// enumeration of a single file, protecting a single file, in a test whose
    /// entire purpose is that enumerations lose. There is no victim name here;
    /// the controls run over the derived set.
    fn verdict(source: &str) -> Result<Option<Verdict>, String> {
        // The blocks are computed ONCE and handed to everything below. They
        // used to be recomputed by `cut_index` and again per walk, per payload,
        // per file.
        let blocks = module_blocks(source);
        let Some(cut) = blocks.first().map(|b| b.gate_at) else {
            return Ok(None);
        };
        if !(cut == 0 || source.as_bytes()[cut - 1] == b'\n') {
            return Err("the cut landed in the middle of a line, so the gate was matched \
                        inside a comment or a string literal rather than at a real attribute"
                .to_string());
        }
        let gates = blocks.len();
        // FIRST, and for every module in the file rather than for whichever
        // ones a walk happens to reach: nothing shares a line with a gated
        // module's real closing brace.
        let closes_read = closing_brace_lines_carry_no_source(source, &blocks)?;
        match walk(&source[cut..]) {
            Ok(0) => Err("the walk below the cut opened no module".to_string()),
            Ok(_) => Ok(Some(Verdict { gates, closes_read })),
            // **The strict walk is the whole rule now.** There is no
            // interleaving tolerance: every file's production lives above its
            // first gated test module and everything from that module to EOF
            // is gated test modules and nothing else. A file that violates
            // that reds here, naming itself, rather than being counted into a
            // set that merely priced the exemption.
            Err(why) if why.starts_with(TOP_LEVEL) => Err(format!(
                "{why} -- so this file INTERLEAVES: it carries production code BETWEEN two of \
                 its gated test modules. That region is read by nothing, and a `pub fn` \
                 written in it was measured shipping in this crate's debug LLVM IR. Move ALL \
                 of this file's production ABOVE its first `#[cfg(test)]` module; relocating \
                 a test module is a pure move and changes no behaviour. There is no longer a \
                 set of tolerated files to join, and adding one is not the fix"
            )),
            Err(why) => Err(why),
        }
    }

    /// The three payloads measured SHIPPING out of this crate, each written
    /// into `source` at the site it was measured at, with the name of the
    /// shape for the failure message.
    ///
    /// Every one of them is applied to every file that has a cut. A payload
    /// that a file's real bytes happen to defeat is a payload that file was
    /// never protected from.
    fn shipping_payloads(source: &str) -> Vec<(&'static str, String)> {
        let mut out = vec![
            // 1. The census payload: appended at column 0 to EOF. Measured
            //    surviving the whole suite in both profiles with three
            //    occurrences in the debug LLVM IR.
            (
                "a `pub fn` appended at column 0 below the cut",
                format!("{source}pub fn shipped_census(x: u64) -> u64 {{ x.wrapping_mul(97) }}\n"),
            ),
            // 2. The same, hidden behind an attribute on the SAME line, which
            //    a `starts_with(\"#[\")` skip would walk straight past. See
            //    `is_lone_attribute`.
            (
                "a `pub fn` sharing a line with an outer attribute",
                format!(
                    "{source}#[allow(dead_code)] pub fn shipped_attr(x: u64) -> u64 {{ x }}\n"
                ),
            ),
        ];
        if let Some(last_close) = source.rfind("\n}") {
            // 3. The END-OF-LINE payload, written on the closing brace's own
            //    line. One line changed, no line added, brace balance
            //    unchanged. This is what the closing-brace-line rule in
            //    `walk` exists for, and before that rule it survived the full
            //    suite in debug and release and shipped.
            let mut eol = source.to_string();
            eol.insert_str(
                last_close + 2,
                " pub fn shipped_eol(x: u64) -> u64 { x.wrapping_mul(97) }",
            );
            out.push(("a `pub fn` sharing a line with a module's closing brace", eol));
            // 4. The BALANCED payload, which beat every line walk in this
            //    crate: the last module is closed by an INDENTED brace, the
            //    `pub fn` sits at file scope after it, and the module's own
            //    column-0 `}` is STOLEN to close it, so a brace count over the
            //    whole file cannot tell the mutant from the original.
            let mut balanced = source.to_string();
            balanced.insert_str(
                last_close + 1,
                "    }\npub fn shipped_bal(x: u64) -> u64 {\n    x.wrapping_mul(97)\n",
            );
            let balance = |s: &str| {
                s.matches('{').count() as isize - s.matches('}').count() as isize
            };
            assert_eq!(
                balance(&balanced),
                balance(source),
                "control: the balanced payload changed the file's brace balance, so it is the \
                 column-0 payload again under another name and its refusal proves nothing \
                 about the indented close"
            );
            out.push(("a file-scope `pub fn` behind an indented module close", balanced));
        }
        // 5. The end-of-line payload on EVERY gated module's closing line, one
        //    mutant per module, rather than only on the last one shape 3
        //    reaches.
        //
        //    WHICH module the payload sits on decides whether anything reads
        //    it, and neither the first nor the last is the dangerous site: the
        //    walk from the cut reaches the FIRST module's close and the tail
        //    walk reaches the LAST one, while a module in the MIDDLE of an
        //    interleaved file is reached by neither -- the walk from the cut
        //    aborts at the first production line above it and the tail walk
        //    starts below it. Measured at `src/app.rs:1980`, the close of the
        //    second of that file's five gated modules: SURVIVED the whole
        //    suite in both profiles and shipped three times over in the debug
        //    LLVM IR.
        //
        //    Planting on every module rather than on a computed "middle" one
        //    is the point, and is measured: the first version of this shape
        //    picked the FIRST module, and with it in place the check this
        //    control exists to protect could be deleted in one edit and the
        //    mid-file payload still SURVIVED at 2247 passed. A control that
        //    picks a site is an enumeration of one site.
        for block in module_blocks(source) {
            let Some(rel) = block.opener.rfind('{') else {
                continue;
            };
            let close = match_brace(source, block.opener_at + rel);
            let mut mid = source.to_string();
            mid.insert_str(
                close + 1,
                " pub fn shipped_eol_mid(x: u64) -> u64 { x.wrapping_mul(97) }",
            );
            out.push(("a `pub fn` sharing a line with a module's closing brace", mid));
        }
        out
    }

    /// **Nothing but gated test modules lives below the cut of ANY file in
    /// this crate.**
    ///
    /// See this module's documentation for what shipped through the hole
    /// this closes and why the file list is derived rather than written.
    #[test]
    fn nothing_but_gated_test_modules_lives_below_any_files_cut() {
        let crate_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        rust_sources(crate_dir, &mut files);
        files.sort();

        // ---- Non-vacuity of the LIST itself. A derived list that misses a
        // file is the defect this test exists to fix, so the list is measured
        // before it is used.
        assert!(
            files.len() >= 55,
            "control: only {} `.rs` files were derived from {crate_dir:?}. This crate has had \
             sixty-one since the census; a list this short means the walk below is a walk over \
             a fraction of the crate, which is precisely the failure being fixed",
            files.len()
        );
        for known in [
            "src/below_cut.rs",
            "src/lib.rs",
            "src/login_ui.rs",
            "src/vault_window/mod.rs",
            "src/vault_window/send_ui.rs",
            "src/vault_window/item_list.rs",
            "src/vault_window/folder_modal.rs",
            "src/injector/sequence.rs",
            "src/hotkey.rs",
            "build.rs",
        ] {
            assert!(
                files.iter().any(|p| relative(crate_dir, p) == known),
                "control: the derived list does not contain {known:?}, so whatever is below \
                 that file's cut is not being read by this test"
            );
        }
        // ---- And against an independent oracle, where one can be had. The
        // walk and `git ls-files` disagreeing means one of them is wrong
        // about what this crate is made of.
        {
            // Loud, not optional. `if let Some(listed) = ..` meant that an
            // environment where git cannot be run silently dropped the entire
            // oracle and left the floors as the only control on the file list.
            let listed = git_listed(crate_dir).expect(
                "`git ls-files` could not be run in the crate directory, so the independent \
                 oracle for the file list is gone and the directory walk is checking itself. \
                 This test requires git, the way `login_ui`'s probe scan does: run it in a \
                 checkout rather than an unpacked archive.",
            );
            let derived: Vec<String> = files.iter().map(|p| relative(crate_dir, p)).collect();
            let oracle: Vec<String> = listed.iter().map(|p| relative(crate_dir, p)).collect();
            assert!(
                oracle.len() >= 55,
                "control: git listed only {} `.rs` files, so it is not an oracle for anything",
                oracle.len()
            );
            for rel in &oracle {
                assert!(
                    derived.contains(rel),
                    "git says this crate owns {rel:?} and the directory walk did not reach \
                     it, so it would never have been walked"
                );
            }
            for rel in &derived {
                assert!(
                    oracle.contains(rel),
                    "the directory walk reached {rel:?} and git does not list it: it is \
                     ignored, or it is build output this walk should not be reading"
                );
            }
        }

        // ---- The property, file by file. Every file lands in exactly one of
        // the two buckets -- a cut, or no cut -- and NEITHER of them is a
        // skip. There is no third bucket any more: the interleaving one was
        // deleted along with the three files that were in it. A file that
        // carries production between two of its gated modules is refused by
        // `verdict` and panics here, naming itself.
        let mut with_cut = 0usize;
        let mut gates_seen = 0usize;
        let mut closes_read = 0usize;
        for path in &files {
            let rel = relative(crate_dir, path);
            let source = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("{rel} is readable to be walked: {e}"));

            let Some(found) = verdict(&source).unwrap_or_else(|why| panic!("{rel}: {why}"))
            else {
                // **A file with no cut is not skipped.** A silent skip is
                // how thirty-five files went unguarded. "No gated test
                // module" is only the same as "nothing lives below a cut"
                // if the file really has no tests, so that is asserted
                // rather than assumed: a `#[test]` here would be a test
                // outside any gated module, which SHIPS.
                let ungated_test = source
                    .lines()
                    .find(|l| l.trim().starts_with(concat!("#[", "test]")));
                assert!(
                    ungated_test.is_none(),
                    "{rel} has no gated test module, so this test read it as production from \
                     the first byte to the last -- and it contains {ungated_test:?}, a test \
                     attribute outside any `cfg(test)` module. That test, and everything it \
                     touches, is compiled into the shipped binary."
                );
                continue;
            };
            with_cut += 1;
            gates_seen += found.gates;
            closes_read += found.closes_read;
        }

        // ---- Non-vacuity of the WALK.
        assert!(
            with_cut >= 44,
            "control: only {with_cut} of {} files had a cut at all, so the property was \
             asserted about almost nothing and the rest of this test is the no-cut branch",
            files.len()
        );
        assert!(
            gates_seen >= 130,
            "control: only {gates_seen} gated test modules were walked across the crate, \
             so the cut-finding rule has stopped finding cuts"
        );
        // EVERY module's closing line was read, not the ones some walk
        // happened to reach. The round this rule was added to lost nine of the
        // crate's one hundred and sixty-one closing lines to a walk that
        // aborted before them, so the count is compared rather than assumed.
        //
        // This is now a real comparison. It was written as one and was not:
        // `closing_brace_lines_carry_no_source` returned `Ok(blocks.len())`
        // and `gates` was set to `blocks.len()` of the SAME vector, so both
        // sides were one `Vec::len()` and no mutation could separate them --
        // `for block in blocks.iter().take(1)` reported equality while reading
        // one of `app.rs`'s five closes. The function now counts what it reads,
        // and `the_closing_line_walk_returns_how_many_closes_it_actually_read`
        // measures that directly.
        assert_eq!(
            closes_read, gates_seen,
            "control: {gates_seen} gated test modules were found across the crate but only \
             {closes_read} of their closing LINES were read. The remainder are the one place \
             below a cut that nothing looks at, and a `pub fn` written there was measured \
             surviving the whole suite and shipping in the lib's DEBUG LLVM IR"
        );

        // ---- **AND NO FILE INTERLEAVES.**
        //
        // There is no number to compare here, and that is the point. A file
        // that carries production BETWEEN two of its gated test modules is
        // refused by `verdict`, so the loop above has already panicked naming
        // that file. The rule is now the same one for every file in the
        // crate: production above the first `#[cfg(test)]` module, gated test
        // modules from there to EOF, nothing else.
        //
        // What this replaced: a derived `INTERLEAVED_FILES` set of three, a
        // second holder of that number in `job_object.rs`, and a special case
        // in this walk -- machinery whose entire product was a PRICE for an
        // exemption. `app.rs`, `theme.rs` and `injector/sequence.rs` were the
        // set; each had its offending test modules relocated below its
        // production (a pure move -- no production line changed), so the set
        // is empty and the exemption is gone. With no tolerance left to buy,
        // the three-edit escape that machinery funded -- payload, plus a
        // decoy gated module, plus both numbers -- has nothing to pay for: a
        // decoy gated module no longer makes a file exempt, it makes it RED.
        //
        // **WHO HOLDS THIS, AND WHAT DEFEATING IT COSTS.** The commit that
        // deleted the tolerance said the rule lived in two places and that
        // both of `job_object.rs`'s needles had to be edited to weaken it.
        // Measured, that accounting was wrong: the needles as written were
        // satisfied by text already in this file, and the holder that
        // actually killed the weakening mutants was not claimed at all. The
        // real list, in order of how much weight each carries:
        //
        // 1. **THE LOOP ABOVE -- the load-bearing holder, and it is not
        //    text.** `shipping_payloads` drives this same `verdict` over
        //    EVERY file's real bytes and requires a refusal for every
        //    payload. A `verdict` whose `TOP_LEVEL` arm returns `Ok` stops
        //    refusing the appended-`pub fn` payload as well, so it reds here
        //    naming a real file -- with the fixture below deleted, with both
        //    needles green, with no help from either. Measured: neutering
        //    that arm reds at this loop in `src/accounts.rs` whether or not
        //    the fixture still exists.
        // 2. `an_interleaving_file_is_refused_rather_than_tolerated`, which
        //    drives this same `verdict` over a synthetic file that
        //    interleaves and over the same file with its test module moved.
        //    It is what keeps the refusal BRANCH reachable and its wording
        //    honest -- no real file is shaped that way -- and what makes the
        //    accept path a control rather than an assumption. It is not what
        //    makes the rule survive: hollow it and the loop above still reds.
        // 3. Two needles in `job_object.rs`, which quote a clause of this
        //    file's refusal `format!` and a clause of the fixture's panic
        //    message rather than the bare token `INTERLEAVES` -- which occurs
        //    four times here, three of them in prose, so it stayed green over
        //    a deleted branch. What they buy is VISIBILITY, not a second
        //    file's worth of work. The claim that they "cost an edit in a
        //    SECOND file" was measured false: that test reads this file with
        //    a raw `read_to_string`, so with the `TOP_LEVEL` arm neutered and
        //    the fixture hollowed, TWO `//` COMMENT LINES added here carrying
        //    the clauses verbatim turn it green with no edit to
        //    `job_object.rs` at all. They are a tripwire -- the cheapest
        //    defeat now has to write the refusal's own words back as dead
        //    comment text beside the branch it deleted, which no diff reads
        //    as innocent -- not a barrier. Holder 1 is the barrier.
        //
        // So the honest price of bringing the tolerance back is: rewrite this
        // `verdict` arm AND defeat the payload loop above (whose floors,
        // `shapes == 4 * live + gates_seen` and `closes_read == gates_seen`
        // all bind), and delete or hollow the fixture. Satisfying the two
        // needles is a formality by comparison -- but a conspicuous one.
        // Only the first of those is a rule; the rest are what make it
        // awkward to remove quietly.

        // ---- Liveness, over the DERIVED set, on every file's real bytes.
        //
        // A guard that no longer refuses anything reports `ok` forever. Every
        // payload below is one measured SHIPPING out of this crate, every one
        // is written into a real file's real text, and every one is applied to
        // EVERY file that has a cut. The previous version of this control
        // named one victim path and so protected one file; a mutation planted
        // anywhere else was measured surviving with the control still green.
        let mut live = 0usize;
        let mut shapes = 0usize;
        for path in &files {
            let rel = relative(crate_dir, path);
            let source = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("{rel} is readable: {e}"));
            if cut_index(&source).is_none() {
                continue;
            }
            // The control on the controls: the file as it ACTUALLY is passes,
            // so the refusals below are not a walk that refuses everything.
            assert!(
                matches!(verdict(&source), Ok(Some(_))),
                "control: the property refuses {rel} as it actually is, so every refusal \
                 measured against it below proves nothing"
            );
            for (shape, mutant) in shipping_payloads(&source) {
                // Each payload is re-cut IN the string being sliced, inside
                // `verdict`. An offset taken in one copy of a file and used to
                // slice another -- CRLF working-tree bytes against an LF
                // `replace` -- was measured 7355 bytes off in this crate and
                // passed silently forever.
                let why = verdict(&mutant).err().unwrap_or_else(|| {
                    panic!(
                        "{rel}: the property ACCEPTED {shape}. That item is at file scope \
                         below this file's cut and is compiled into the shipped binary."
                    )
                });
                assert!(
                    why.contains("shipped_"),
                    "{rel}: {shape} was refused, but for something other than the item it \
                     plants, so this control is measuring the wrong defect: {why}"
                );
                shapes += 1;
            }
            live += 1;
        }
        assert!(
            live >= 44,
            "control: the liveness payloads were driven over only {live} files, so most of \
             the crate has no measured proof that this guard refuses anything in it"
        );
        assert!(
            shapes >= 4 * 44 + 130,
            "control: only {shapes} payload-shape pairs were measured across {live} files, so \
             some file silently offered fewer shapes than the four this crate has measured \
             shipping, or the closing-line payload was not planted on every module"
        );
        // EXACTLY four payloads per file with a cut, plus one per gated module
        // in the crate -- the closing-line payload is planted on EVERY module,
        // not on a chosen one. A shape that quietly stopped being generated
        // for some file would otherwise leave that file's refusal unmeasured
        // while the floor above stayed satisfied.
        assert_eq!(
            shapes,
            4 * live + gates_seen,
            "control: {shapes} payload-shape pairs were measured over {live} files carrying \
             {gates_seen} gated modules between them, and the four whole-file shapes plus one \
             closing-line payload per module come to {}. Some file offered fewer",
            4 * live + gates_seen
        );
    }

    /// **What still gets a `pub fn` below a cut, where, and at what price.**
    ///
    /// Written down rather than implied, because every round in this area has
    /// been lost to a cost that was claimed and not measured.
    ///
    /// * **The decoy-module escape is GONE, at any price.** It used to cost
    ///   three edits: `pub fn shipped(..) {..}` plus a decoy
    ///   `#[cfg(test)] mod d {}` appended to a file (edit one), then
    ///   `INTERLEAVED_FILES` here from 3 to 4 (edit two) and `job_object`'s own
    ///   copy from 3 to 4 (edit three, second file). The decoy worked because
    ///   it made the file INTERLEAVE, and interleaving was tolerated at a
    ///   price. It is no longer tolerated at any price: the tolerance, both
    ///   holders of the number, and the special case in the walk are deleted,
    ///   so the decoy now just makes the file red. There is nothing left to
    ///   pay, and so nothing left to buy.
    ///
    /// * **No file is exempt.** `app.rs`, `theme.rs` and
    ///   `injector/sequence.rs` were the three that interleaved, and a
    ///   one-edit column-0 `pub fn` planted in their mid-file regions --
    ///   roughly 4,343 lines between them, `app.rs` 664..2557, `theme.rs`
    ///   1707..3005, `injector/sequence.rs` 839..1991 -- was measured
    ///   SURVIVING the whole suite in both profiles and shipping three times
    ///   over in the lib's debug LLVM IR. Each file's offending test modules
    ///   were relocated below its production, verbatim and with no production
    ///   line changed; those same plants are now KILLED. Every file in the
    ///   crate is held from its first gate to EOF by the strict rule.
    ///
    /// * **Nothing below one edit.** Every whole-file shape measured shipping
    ///   out of this crate -- column-0 append, attribute-shared line,
    ///   closing-brace line on any module, and the balanced indented close --
    ///   is refused in every file that has a cut, and the refusals are driven
    ///   over every such file's real bytes above.
    #[allow(dead_code)]
    const HONEST_BOUNDARY: () = ();
}

//! The ONE brace matcher every below-the-cut walk in this crate uses.
//!
//! # Why this file exists
//!
//! Six files (`send.rs`, `updater.rs`, `vault_export.rs`, `breach.rs`,
//! `overlay_ui.rs`, `job_object.rs`) each carry a two-state walk of the region
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
            (2, 2, 0),
            "below `main.rs`'s cut there are no longer exactly two opened-and-closed test \
             modules: {modules} opened, {closes} closed, ending at depth {depth}"
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

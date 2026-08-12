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
//! three times over in the lib's LLVM IR, in all three files that carried the
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
}

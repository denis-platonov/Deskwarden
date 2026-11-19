//! The catalog file format, parsed.
//!
//! See [`crate::i18n`] for the format's documentation; this file is the
//! reader and nothing else. It takes the text of one `locales/<tag>.txt` and
//! answers a map from key to template, plus every line it could not read.
//!
//! **Parsing never fails and never panics.** A line it cannot read is skipped
//! and reported in [`Parsed::problems`], so a typo in a translator's file
//! costs that one string (which falls back to English) and not the screen.
//! `every_catalog_parses_without_a_problem` in `i18n`'s tests is what turns
//! those reports into a red build.

use std::collections::{BTreeSet, HashMap};

use super::plural::Category;

/// One key as the file spells it: the bare key and, for a plural form, its
/// category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntryKey {
    pub key: &'static str,
    pub plural: Option<Category>,
}

/// A parsed catalog.
#[derive(Debug, Default)]
pub struct Catalog {
    entries: HashMap<(&'static str, Option<Category>), &'static str>,
}

/// What a parse produced: the catalog, and a message per unreadable line.
#[derive(Debug, Default)]
pub struct Parsed {
    pub catalog: Catalog,
    pub problems: Vec<String>,
}

impl Catalog {
    /// The template for `key` in `plural` form (`None` for a key that is not
    /// plural).
    pub fn get(&self, key: &str, plural: Option<Category>) -> Option<&'static str> {
        self.entries.get(&(key, plural)).copied()
    }

    /// Every entry, in no particular order.
    pub fn entries(&self) -> impl Iterator<Item = (EntryKey, &'static str)> + '_ {
        self.entries
            .iter()
            .map(|((key, plural), value)| (EntryKey { key, plural: *plural }, *value))
    }

    /// How many entries (a plural key counts once per form).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Parses one catalog file.
///
/// `source` is `'static` because every catalog is an `include_str!`: a value
/// with no escape in it is handed out as a slice of the file itself, and only
/// a value that needed rewriting (a continuation, an escape) is built once
/// and leaked -- once per process per locale, bounded by the size of the
/// file, which is the price of `t` returning `&'static str`.
pub fn parse(source: &'static str) -> Parsed {
    let mut out = Parsed::default();
    let mut lines = source.lines().enumerate();
    while let Some((number, line)) = lines.next() {
        let number = number + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((raw_key, rest)) = trimmed.split_once('=') else {
            out.problems.push(format!("line {number}: no `=` in `{trimmed}`"));
            continue;
        };
        let raw_key = raw_key.trim();
        let Some((key, plural)) = split_key(raw_key) else {
            out.problems.push(format!("line {number}: `{raw_key}` is not a key"));
            continue;
        };
        // The value: everything after the first `=`, with a trailing `\`
        // joining the next line on (its leading space dropped). `lines()` of
        // a 'static source yields 'static slices, so a value that needs no
        // rewriting keeps pointing into the file.
        let first: &'static str = rest.trim_start().trim_end_matches([' ', '\t', '\r']);
        let stored: &'static str = if !ends_with_continuation(first) && !first.contains('\\') {
            first
        } else {
            let mut value = first.to_owned();
            while ends_with_continuation(&value) {
                value.pop();
                match lines.next() {
                    Some((_, next)) => value.push_str(next.trim_start()),
                    None => break,
                }
            }
            let value = value.trim_end_matches([' ', '\t', '\r']);
            match decode(value) {
                Ok(decoded) => Box::leak(decoded.into_boxed_str()),
                Err(why) => {
                    out.problems.push(format!("line {number}: `{raw_key}`: {why}"));
                    continue;
                }
            }
        };
        if let Err(why) = check_braces(stored) {
            out.problems.push(format!("line {number}: `{raw_key}`: {why}"));
            continue;
        }
        // The first definition stands; a second is a mistake worth a red build.
        if out.catalog.entries.contains_key(&(key, plural)) {
            out.problems.push(format!("line {number}: `{raw_key}` is defined twice"));
            continue;
        }
        out.catalog.entries.insert((key, plural), stored);
    }
    out
}

/// `key` or `key[category]`; `None` when it is neither.
fn split_key(raw: &str) -> Option<(&str, Option<Category>)> {
    let (key, plural) = match raw.split_once('[') {
        Some((key, tail)) => {
            let name = tail.strip_suffix(']')?;
            (key, Some(Category::from_name(name)?))
        }
        None => (raw, None),
    };
    let ok = !key.is_empty()
        && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
        && key.starts_with(|c: char| c.is_ascii_lowercase());
    ok.then_some((key, plural))
}

/// Whether the value ends in an unescaped backslash: an odd run of them.
fn ends_with_continuation(value: &str) -> bool {
    let run = value.chars().rev().take_while(|c| *c == '\\').count();
    run % 2 == 1
}

/// Rewrites the escapes: `\\`, `\n`, `\t`, `\s` (a space that survives the
/// trim) and `\u{XXXX}`.
fn decode(value: &str) -> Result<String, String> {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('s') => out.push(' '),
            Some('u') => {
                if chars.next() != Some('{') {
                    return Err("`\\u` must be followed by `{`".into());
                }
                let mut hex = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(h) if h.is_ascii_hexdigit() && hex.len() < 6 => hex.push(h),
                        _ => return Err("unterminated or malformed `\\u{..}`".into()),
                    }
                }
                let code = u32::from_str_radix(&hex, 16).map_err(|_| "bad `\\u{..}`".to_owned())?;
                out.push(char::from_u32(code).ok_or_else(|| "`\\u{..}` is not a character".to_owned())?);
            }
            Some(other) => return Err(format!("unknown escape `\\{other}`")),
            None => return Err("a lone `\\` at the end".into()),
        }
    }
    Ok(out)
}

/// A `{` opens a placeholder `{name}` or is doubled `{{`; a `}` closes one or
/// is doubled `}}`. Anything else is a typo that would print a brace.
fn check_braces(template: &str) -> Result<(), String> {
    for piece in pieces(template) {
        if let Piece::Bad(why) = piece {
            return Err(why);
        }
    }
    Ok(())
}

/// One piece of a template.
#[derive(Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    Text(&'a str),
    /// A literal `{` or `}` written doubled.
    Brace(char),
    Placeholder(&'a str),
    Bad(String),
}

/// Splits a template into text, literal braces and placeholders.
pub fn pieces(template: &str) -> Vec<Piece<'_>> {
    let bytes = template.as_bytes();
    let mut out = Vec::new();
    let mut text_from = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                if i > text_from {
                    out.push(Piece::Text(&template[text_from..i]));
                }
                if bytes.get(i + 1) == Some(&b'{') {
                    out.push(Piece::Brace('{'));
                    i += 2;
                } else if let Some(close) = template[i + 1..].find('}') {
                    let name = &template[i + 1..i + 1 + close];
                    let valid = !name.is_empty()
                        && name.starts_with(|c: char| c.is_ascii_lowercase())
                        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
                    if valid {
                        out.push(Piece::Placeholder(name));
                    } else {
                        out.push(Piece::Bad(format!("`{{{name}}}` is not a placeholder name")));
                    }
                    i += close + 2;
                } else {
                    out.push(Piece::Bad("a `{` with no closing `}`".into()));
                    i += 1;
                }
                text_from = i;
            }
            b'}' => {
                if i > text_from {
                    out.push(Piece::Text(&template[text_from..i]));
                }
                if bytes.get(i + 1) == Some(&b'}') {
                    out.push(Piece::Brace('}'));
                    i += 2;
                } else {
                    out.push(Piece::Bad("a lone `}`; write `}}` for a literal one".into()));
                    i += 1;
                }
                text_from = i;
            }
            _ => i += 1,
        }
    }
    if text_from < bytes.len() {
        out.push(Piece::Text(&template[text_from..]));
    }
    out
}

/// The placeholder names a template uses.
pub fn placeholders(template: &str) -> BTreeSet<String> {
    pieces(template)
        .into_iter()
        .filter_map(|p| match p {
            Piece::Placeholder(name) => Some(name.to_owned()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &'static str) -> Parsed {
        parse(text)
    }

    #[test]
    fn a_plain_line_is_a_key_and_its_value() {
        let p = parsed("# a comment\n\nsave = Save\nsave.all = Save all  \n");
        assert!(p.problems.is_empty(), "{:?}", p.problems);
        assert_eq!(p.catalog.get("save", None), Some("Save"));
        assert_eq!(p.catalog.get("save.all", None), Some("Save all"));
        assert_eq!(p.catalog.len(), 2);
    }

    #[test]
    fn crlf_files_read_the_same() {
        let p = parsed("a = One\r\nb = Two\r\n");
        assert_eq!(p.catalog.get("a", None), Some("One"));
        assert_eq!(p.catalog.get("b", None), Some("Two"));
    }

    #[test]
    fn an_equals_sign_in_the_value_stays_in_the_value() {
        let p = parsed("hint = Use a=b to match\n");
        assert_eq!(p.catalog.get("hint", None), Some("Use a=b to match"));
    }

    #[test]
    fn a_trailing_backslash_joins_the_next_line() {
        let p = parsed("long = First half, \\\n    second half.\nnext = Next\n");
        assert!(p.problems.is_empty(), "{:?}", p.problems);
        assert_eq!(p.catalog.get("long", None), Some("First half, second half."));
        assert_eq!(p.catalog.get("next", None), Some("Next"));
    }

    #[test]
    fn escapes_are_read() {
        let p = parsed("e = Line one\\nLine two\\ttab\\\\ \\u{2026}\\s\n");
        assert_eq!(p.catalog.get("e", None), Some("Line one\nLine two\ttab\\ \u{2026} "));
    }

    #[test]
    fn plural_forms_are_keyed_by_category() {
        let p = parsed("items[one] = {n} item\nitems[other] = {n} items\n");
        assert_eq!(p.catalog.get("items", Some(Category::One)), Some("{n} item"));
        assert_eq!(p.catalog.get("items", Some(Category::Other)), Some("{n} items"));
        assert_eq!(p.catalog.get("items", None), None);
    }

    #[test]
    fn bad_lines_are_skipped_and_reported_and_good_ones_survive() {
        let p = parsed("ok = fine\nno equals\nBad Key = x\nitems[several] = x\nesc = \\q\nbrace = {oops\nlone = a } b\nok = again\n");
        assert_eq!(p.catalog.get("ok", None), Some("fine"));
        // Six unreadable lines and the second `ok`, which loses to the first.
        assert_eq!(p.problems.len(), 7, "{:#?}", p.problems);
    }

    #[test]
    fn doubled_braces_are_literal_and_names_are_placeholders() {
        assert_eq!(
            placeholders("{{not}} {a} and {b_2}"),
            ["a".to_owned(), "b_2".to_owned()].into_iter().collect()
        );
        assert!(check_braces("{{x}}").is_ok());
        assert!(check_braces("{Bad}").is_err());
        assert!(check_braces("{}").is_err());
    }

    #[test]
    fn an_empty_value_is_a_value() {
        let p = parsed("blank =\n");
        assert_eq!(p.catalog.get("blank", None), Some(""));
    }
}
